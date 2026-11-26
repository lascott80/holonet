//! Copying one file between this PC and a host, resumably.
//!
//! - Downloads write to `name.holonet-part` beside the target and are
//!   renamed over it when complete; a resumed download appends to the part.
//! - Uploads write to the target; a resumed upload continues from the
//!   host's copy's size.
//! - A resume happens only if the source still has the size and
//!   modification time recorded when the file was first started
//!   ([`Stamp`]); otherwise the copy fails with [`CopyError::SourceChanged`]
//!   and a retry starts again from nothing.
//! - Sizes must match at the end, and the modification time is carried over.

use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU8, Ordering};

use holonet_ssh::russh_sftp::protocol::{FileAttributes, OpenFlags};
use tokio::io::{AsyncReadExt, AsyncSeekExt, AsyncWriteExt};

use crate::remote::Remote;
use crate::{FileError, RemotePath};

const CHUNK: usize = 256 * 1024;

/// [`Control`] values.
pub const RUN: u8 = 0;
pub const PAUSE: u8 = 1;
pub const CANCEL: u8 = 2;

/// Set by the queue to stop a copy between chunks.
pub type Control = AtomicU8;

/// The source as it was when the file was first started.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Stamp {
    pub size: u64,
    pub modified: Option<i64>,
}

#[derive(Clone, Debug, PartialEq, Eq, thiserror::Error)]
pub enum CopyError {
    #[error("{0}")]
    File(#[from] FileError),
    /// An I/O error from an SFTP file handle, before [`classify`] decides
    /// whether the connection went.
    #[error("{0}")]
    Io(IoText),
    #[error("The source changed since the transfer started.")]
    SourceChanged,
    #[error("The copy ended at {got} bytes instead of {expected}.")]
    Short { got: u64, expected: u64 },
}

impl CopyError {
    /// The connection went: the copy can resume after reconnecting.
    pub fn is_lost(&self) -> bool {
        matches!(self, CopyError::File(FileError::Lost | FileError::TimedOut))
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Outcome {
    Done,
    /// Paused or cancelled through the [`Control`].
    Stopped,
}

/// The `.holonet-part` file a download writes to.
pub fn part_path(target: &Path) -> PathBuf {
    let mut name = target.file_name().unwrap_or_default().to_os_string();
    name.push(".holonet-part");
    target.with_file_name(name)
}

fn local_stamp(path: &Path) -> Result<Stamp, FileError> {
    let meta = std::fs::metadata(path)?;
    let modified =
        meta.modified().ok().and_then(|t| t.duration_since(std::time::UNIX_EPOCH).ok()).map(|d| d.as_secs() as i64);
    Ok(Stamp { size: meta.len(), modified })
}

/// Copy `from` (host) to `to` (this PC). `stamp` is `None` the first time
/// and is filled in; `progress` gets the bytes done so far.
pub async fn download(
    remote: &Remote,
    from: &RemotePath,
    to: &Path,
    stamp: &mut Option<Stamp>,
    control: &Control,
    mut progress: impl FnMut(u64),
) -> Result<Outcome, CopyError> {
    let attrs = remote.sftp().metadata(from.as_bytes().to_vec()).await.map_err(FileError::from)?;
    let now = Stamp { size: attrs.size.unwrap_or(0), modified: attrs.mtime.map(i64::from) };
    let part = part_path(to);
    let resume = match stamp {
        Some(s) if *s != now => return Err(CopyError::SourceChanged),
        Some(_) => true,
        None => {
            *stamp = Some(now);
            false
        }
    };
    let offset = if resume { std::fs::metadata(&part).map(|m| m.len().min(now.size)).unwrap_or(0) } else { 0 };
    let mut src = remote.sftp().open(from.as_bytes().to_vec()).await.map_err(FileError::from)?;
    if offset > 0 {
        src.seek(std::io::SeekFrom::Start(offset)).await.map_err(FileError::from)?;
    }
    let mut dst = if offset > 0 {
        let mut f = tokio::fs::OpenOptions::new().write(true).open(&part).await.map_err(FileError::from)?;
        f.set_len(offset).await.map_err(FileError::from)?;
        f.seek(std::io::SeekFrom::Start(offset)).await.map_err(FileError::from)?;
        f
    } else {
        tokio::fs::File::create(&part).await.map_err(FileError::from)?
    };
    let mut done = offset;
    progress(done);
    let mut buf = vec![0u8; CHUNK];
    while done < now.size {
        if control.load(Ordering::Relaxed) != RUN {
            dst.flush().await.map_err(FileError::from)?;
            return Ok(Outcome::Stopped);
        }
        let want = ((now.size - done) as usize).min(CHUNK);
        let n = src.read(&mut buf[..want]).await.map_err(|e| CopyError::Io(e.into()))?;
        if n == 0 {
            break;
        }
        dst.write_all(&buf[..n]).await.map_err(FileError::from)?;
        done += n as u64;
        progress(done);
    }
    dst.flush().await.map_err(FileError::from)?;
    let file = dst.into_std().await;
    if done != now.size {
        return Err(CopyError::Short { got: done, expected: now.size });
    }
    if let Some(m) = now.modified {
        let _ = file.set_modified(std::time::UNIX_EPOCH + std::time::Duration::from_secs(m.max(0) as u64));
    }
    drop(file);
    std::fs::rename(&part, to).map_err(FileError::from)?;
    Ok(Outcome::Done)
}

/// Copy `from` (this PC) to `to` (host).
pub async fn upload(
    remote: &Remote,
    from: &Path,
    to: &RemotePath,
    stamp: &mut Option<Stamp>,
    control: &Control,
    mut progress: impl FnMut(u64),
) -> Result<Outcome, CopyError> {
    let now = local_stamp(from)?;
    let resume = match stamp {
        Some(s) if *s != now => return Err(CopyError::SourceChanged),
        Some(_) => true,
        None => {
            *stamp = Some(now);
            false
        }
    };
    let offset = if resume {
        match remote.sftp().metadata(to.as_bytes().to_vec()).await {
            Ok(a) => a.size.unwrap_or(0).min(now.size),
            Err(_) => 0,
        }
    } else {
        0
    };
    let mut dst = if offset > 0 {
        let mut f =
            remote.sftp().open_with_flags(to.as_bytes().to_vec(), OpenFlags::WRITE).await.map_err(FileError::from)?;
        f.seek(std::io::SeekFrom::Start(offset)).await.map_err(FileError::from)?;
        f
    } else {
        remote.sftp().create(to.as_bytes().to_vec()).await.map_err(FileError::from)?
    };
    let mut src = tokio::fs::File::open(from).await.map_err(FileError::from)?;
    if offset > 0 {
        src.seek(std::io::SeekFrom::Start(offset)).await.map_err(FileError::from)?;
    }
    let mut done = offset;
    progress(done);
    let mut buf = vec![0u8; CHUNK];
    while done < now.size {
        if control.load(Ordering::Relaxed) != RUN {
            dst.shutdown().await.map_err(|e| CopyError::Io(e.into()))?;
            return Ok(Outcome::Stopped);
        }
        let want = ((now.size - done) as usize).min(CHUNK);
        let n = src.read(&mut buf[..want]).await.map_err(FileError::from)?;
        if n == 0 {
            break;
        }
        dst.write_all(&buf[..n]).await.map_err(|e| CopyError::Io(e.into()))?;
        done += n as u64;
        progress(done);
    }
    dst.shutdown().await.map_err(|e| CopyError::Io(e.into()))?;
    let got = remote.sftp().metadata(to.as_bytes().to_vec()).await.map_err(FileError::from)?.size.unwrap_or(0);
    if got != now.size {
        return Err(CopyError::Short { got, expected: now.size });
    }
    if let Some(m) = now.modified {
        let mut attrs = FileAttributes::empty();
        attrs.mtime = Some(m.clamp(0, u32::MAX as i64) as u32);
        attrs.atime = attrs.mtime;
        let _ = remote.sftp().set_metadata(to.as_bytes().to_vec(), attrs).await;
    }
    Ok(Outcome::Done)
}

/// The text of an I/O error (`std::io::Error` is not `Clone`).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct IoText(pub String);

impl std::fmt::Display for IoText {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.0)
    }
}

impl From<std::io::Error> for IoText {
    fn from(e: std::io::Error) -> Self {
        IoText(e.to_string())
    }
}

/// SFTP file handles report the server's refusals and a dropped
/// connection alike as I/O errors. Ask the server something cheap: no
/// answer means the connection went; an answer means the server refused,
/// and its message stands.
pub async fn classify(remote: &Remote, e: CopyError) -> CopyError {
    let CopyError::Io(text) = e else { return e };
    let probe = tokio::time::timeout(std::time::Duration::from_secs(5), remote.sftp().canonicalize("."));
    match probe.await {
        Ok(Ok(_)) => CopyError::File(FileError::Other(format!("The host said: {}", text.0))),
        _ => CopyError::File(FileError::Lost),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn part_files_sit_beside_the_target() {
        assert_eq!(part_path(Path::new("C:\\dl\\big.iso")), PathBuf::from("C:\\dl\\big.iso.holonet-part"));
    }
}
