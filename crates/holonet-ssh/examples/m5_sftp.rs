//! Milestone 5 phase 0, spike (a): `russh-sftp` 4.0 against the WSL test
//! server. Lists the home folder, times a large upload and download, resumes
//! both from the middle, sets permissions, follows a symbolic link, lists a
//! folder of 5,000 files, and reads a file name that is not UTF-8. Works in
//! `~/holonet-m5-spike` on the server and removes it at the end.
//!
//!   cargo run -p holonet-ssh --example m5_sftp --release -- <local scratch folder> [MB]

use std::time::Instant;

use anyhow::{Context, Result, ensure};
#[path = "../tests/support/mod.rs"]
mod support;

use holonet_ssh::Session;
use holonet_ssh::russh_sftp::client::SftpSession;
use holonet_ssh::russh_sftp::protocol::{FileAttributes, OpenFlags};
use support::{load_env, options_from};
use tokio::io::{AsyncReadExt, AsyncSeekExt, AsyncWriteExt};

const CHUNK: usize = 256 * 1024;
const DIR: &str = "holonet-m5-spike";

/// FNV-1a over a reader, for comparing files without holding them.
async fn hash<R: tokio::io::AsyncRead + Unpin>(mut r: R) -> Result<(u64, u64)> {
    let (mut h, mut n) = (0xcbf29ce484222325u64, 0u64);
    let mut buf = vec![0u8; CHUNK];
    loop {
        let k = r.read(&mut buf).await?;
        if k == 0 {
            return Ok((h, n));
        }
        for b in &buf[..k] {
            h = (h ^ *b as u64).wrapping_mul(0x100000001b3);
        }
        n += k as u64;
    }
}

fn mbps(bytes: u64, secs: f64) -> String {
    format!("{:.1} MB/s", bytes as f64 / 1_048_576.0 / secs)
}

/// Copy `len` bytes from `src` to `dst` in chunks.
async fn copy<R, W>(src: &mut R, dst: &mut W, len: u64) -> Result<u64>
where
    R: tokio::io::AsyncRead + Unpin,
    W: tokio::io::AsyncWrite + Unpin,
{
    let mut buf = vec![0u8; CHUNK];
    let mut done = 0u64;
    while done < len {
        let want = ((len - done) as usize).min(CHUNK);
        let k = src.read(&mut buf[..want]).await?;
        if k == 0 {
            break;
        }
        dst.write_all(&buf[..k]).await?;
        done += k as u64;
    }
    dst.flush().await?;
    Ok(done)
}

#[tokio::main]
async fn main() -> Result<()> {
    let mut args = std::env::args().skip(1);
    let scratch = std::path::PathBuf::from(args.next().context("local scratch folder")?);
    let mb: u64 = args.next().and_then(|a| a.parse().ok()).unwrap_or(500);
    let size = mb * 1_048_576;
    let env = load_env()?;
    let session = Session::connect(options_from("ed25519", &env)?).await?;
    let t = Instant::now();
    let sftp: SftpSession = session.open_sftp().await?;
    println!("sftp subsystem open in {:.0} ms", t.elapsed().as_secs_f64() * 1000.0);

    // Home and a listing.
    let home = sftp.canonicalize(".").await?;
    let t = Instant::now();
    let entries: Vec<_> = sftp.read_dir(home.to_string()).await?.collect();
    println!("home {home}: {} entries in {:.0} ms", entries.len(), t.elapsed().as_secs_f64() * 1000.0);
    for e in entries.iter().take(4) {
        let m = e.metadata();
        println!(
            "  {:<24} dir={} link={} size={:?} perm={:o} mtime={:?} user={:?}",
            e.file_name().to_string(),
            m.is_dir(),
            m.is_symlink(),
            m.size,
            m.permissions.unwrap_or(0),
            m.mtime,
            m.user
        );
    }

    let _ = session.exec(&format!("rm -rf ~/{DIR}")).await?;
    sftp.create_dir(format!("{home}/{DIR}")).await?;
    let remote = format!("{home}/{DIR}/big.bin");

    // A local file of pseudo-random bytes (no easy compression).
    let local = scratch.join("m5-big.bin");
    {
        let mut f = tokio::fs::File::create(&local).await?;
        let mut x = 0x9e3779b97f4a7c15u64;
        let mut buf = vec![0u8; CHUNK];
        for _ in 0..(size / CHUNK as u64) {
            for c in buf.chunks_mut(8) {
                x ^= x << 13;
                x ^= x >> 7;
                x ^= x << 17;
                c.copy_from_slice(&x.to_le_bytes());
            }
            f.write_all(&buf).await?;
        }
        f.flush().await?;
    }
    let (local_hash, _) = hash(tokio::fs::File::open(&local).await?).await?;

    // Upload, timed.
    let t = Instant::now();
    {
        let mut src = tokio::fs::File::open(&local).await?;
        let mut dst = sftp.create(&remote).await?;
        copy(&mut src, &mut dst, size).await?;
        dst.shutdown().await?;
    }
    let up = t.elapsed().as_secs_f64();
    println!("upload {mb} MB: {:.1} s, {}", up, mbps(size, up));

    // Download, timed.
    let down_path = scratch.join("m5-down.bin");
    let t = Instant::now();
    {
        let mut src = sftp.open(&remote).await?;
        let mut dst = tokio::fs::File::create(&down_path).await?;
        copy(&mut src, &mut dst, size).await?;
    }
    let down = t.elapsed().as_secs_f64();
    println!("download {mb} MB: {:.1} s, {}", down, mbps(size, down));
    let (h, n) = hash(tokio::fs::File::open(&down_path).await?).await?;
    ensure!(h == local_hash && n == size, "download differs");
    println!("download matches the local file");

    // Resumed upload: a third, close, then reopen without truncating, seek to
    // the remote size and send the rest.
    let resumed = format!("{home}/{DIR}/resumed.bin");
    {
        let mut src = tokio::fs::File::open(&local).await?;
        let mut dst = sftp.create(&resumed).await?;
        copy(&mut src, &mut dst, size / 3).await?;
        dst.shutdown().await?;
    }
    let have = sftp.metadata(&resumed).await?.size.unwrap_or(0);
    {
        let mut src = tokio::fs::File::open(&local).await?;
        src.seek(std::io::SeekFrom::Start(have)).await?;
        let mut dst = sftp.open_with_flags(&resumed, OpenFlags::WRITE).await?;
        dst.seek(std::io::SeekFrom::Start(have)).await?;
        copy(&mut src, &mut dst, size - have).await?;
        dst.shutdown().await?;
    }
    let (h, n) = hash(sftp.open(&resumed).await?).await?;
    ensure!(h == local_hash && n == size, "resumed upload differs");
    println!("resumed upload from {have} bytes: matches");

    // Resumed download: keep the first half, then seek both and finish.
    let part = scratch.join("m5-part.bin");
    {
        let mut src = sftp.open(&remote).await?;
        let mut dst = tokio::fs::File::create(&part).await?;
        copy(&mut src, &mut dst, size / 2).await?;
    }
    let have = tokio::fs::metadata(&part).await?.len();
    {
        let mut src = sftp.open(&remote).await?;
        src.seek(std::io::SeekFrom::Start(have)).await?;
        let mut dst = tokio::fs::OpenOptions::new().append(true).open(&part).await?;
        copy(&mut src, &mut dst, size - have).await?;
    }
    let (h, n) = hash(tokio::fs::File::open(&part).await?).await?;
    ensure!(h == local_hash && n == size, "resumed download differs");
    println!("resumed download from {have} bytes: matches");

    // Permissions.
    let small = format!("{home}/{DIR}/small.txt");
    sftp.write(&small, b"hello").await?;
    let before = sftp.metadata(&small).await?.permissions.unwrap_or(0);
    let mut attrs = FileAttributes::empty();
    attrs.permissions = Some(0o640);
    sftp.set_metadata(&small, attrs).await?;
    let after = sftp.metadata(&small).await?.permissions.unwrap_or(0);
    println!("chmod: {:o} -> {:o}", before, after);
    ensure!(after & 0o777 == 0o640, "chmod did not apply");

    // Symbolic links: to a file and to a folder.
    sftp.create_dir(format!("{home}/{DIR}/sub")).await?;
    // OpenSSH's server takes SSH_FXP_SYMLINK's two paths in reverse order
    // (target first), unlike draft-ietf-secsh-filexfer-02. The first run of
    // this spike, in spec order, made links in the home folder.
    sftp.symlink("small.txt", format!("{home}/{DIR}/link-file")).await?;
    sftp.symlink("sub", format!("{home}/{DIR}/link-dir")).await?;
    for name in ["link-file", "link-dir"] {
        let p = format!("{home}/{DIR}/{name}");
        let l = sftp.symlink_metadata(&p).await?;
        let m = sftp.metadata(&p).await?;
        let target = sftp.read_link(&p).await?;
        println!("{name}: lstat link={} -> {target}; stat dir={} file={}", l.is_symlink(), m.is_dir(), m.is_regular());
    }
    let listed: Vec<_> = sftp.read_dir(format!("{home}/{DIR}")).await?.collect();
    for e in &listed {
        if e.file_name().to_string().starts_with("link") {
            println!(
                "  read_dir reports {} as link={} dir={}",
                e.file_name(),
                e.metadata().is_symlink(),
                e.metadata().is_dir()
            );
        }
    }

    // A folder of 5,000 files.
    let _ = session.exec(&format!("mkdir -p ~/{DIR}/many && cd ~/{DIR}/many && seq -w 1 5000 | xargs touch")).await?;
    let t = Instant::now();
    let many = sftp.read_dir(format!("{home}/{DIR}/many")).await?.count();
    println!("read_dir of {many} files: {:.0} ms", t.elapsed().as_secs_f64() * 1000.0);

    // A name that is not UTF-8 (Latin-1 é), made by the shell.
    let _ = session.exec(&format!("touch ~/{DIR}/$(printf 'caf\\351.txt')")).await?;
    let names: Vec<_> = sftp.read_dir(format!("{home}/{DIR}")).await?.map(|e| e.file_name()).collect();
    if let Some(raw) = names.iter().find(|n| n.as_bytes().starts_with(b"caf")) {
        println!("non-UTF-8 name: bytes {:?}, shown as {:?}", raw.as_bytes(), raw.to_string());
        let path: Vec<u8> = [format!("{home}/{DIR}/").into_bytes(), raw.as_bytes().to_vec()].concat();
        let m = sftp.metadata(path).await?;
        println!("  stat by raw bytes works: size {:?}", m.size);
    } else {
        println!("non-UTF-8 name not listed");
    }

    let _ = session.exec(&format!("rm -rf ~/{DIR}")).await?;
    for p in [&local, &down_path, &part] {
        let _ = tokio::fs::remove_file(p).await;
    }
    sftp.close().await?;
    session.close().await?;
    println!("cleaned up");
    Ok(())
}
