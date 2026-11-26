//! Transfer engine tests against the WSL test server. Skipped unless
//! `HOLONET_TEST_SERVER=1`:
//!
//!   $env:HOLONET_TEST_SERVER = "1"; cargo test -p holonet-files --test transfer
//!
//! Each test uses `~/holonet-xfer-<name>` on the server and a folder under
//! the system temp folder here, and removes both.

#[path = "../../holonet-ssh/tests/support/mod.rs"]
mod support;

use std::path::PathBuf;
use std::sync::Arc;
use std::time::Duration;

use holonet_files::engine::{Engine, EngineEvent, Job, JobState};
use holonet_files::plan::{Choice, plan_download, plan_upload};
use holonet_files::remote::Remote;
use holonet_files::{RemotePath, transfer};
use holonet_ssh::Session;
use support::{load_env, options_from};
use tokio::sync::mpsc::UnboundedReceiver;

fn enabled() -> bool {
    let on = std::env::var("HOLONET_TEST_SERVER").is_ok_and(|v| v == "1");
    if !on {
        eprintln!("skipped: set HOLONET_TEST_SERVER=1 to run against the WSL test server");
    }
    on
}

async fn connect() -> (Session, Arc<Remote>) {
    let env = load_env().expect(".dev/test-server.env");
    let session = Session::connect(options_from("ed25519", &env).unwrap()).await.unwrap();
    let remote = Arc::new(Remote::open(&session).await.unwrap());
    (session, remote)
}

fn local_dir(name: &str) -> PathBuf {
    let dir = std::env::temp_dir().join(format!("holonet-xfer-{name}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    dir
}

/// Pseudo-random bytes, so a misplaced chunk shows.
fn bytes(len: usize, seed: u64) -> Vec<u8> {
    let mut x = seed | 1;
    (0..len)
        .map(|_| {
            x ^= x << 13;
            x ^= x >> 7;
            x ^= x << 17;
            x as u8
        })
        .collect()
}

/// Wait for events until `done` holds for the snapshot, or fail after a minute.
async fn wait(engine: &Engine, rx: &mut UnboundedReceiver<EngineEvent>, done: impl Fn(&[Job]) -> bool) {
    let deadline = tokio::time::Instant::now() + Duration::from_secs(60);
    loop {
        if done(&engine.snapshot()) {
            return;
        }
        match tokio::time::timeout_at(deadline, rx.recv()).await {
            Ok(Some(_)) => {}
            _ => panic!("timed out; queue: {} {:?}", engine.summary(), engine.snapshot()),
        }
    }
}

fn all_done(jobs: &[Job]) -> bool {
    !jobs.is_empty() && jobs.iter().all(|j| j.state == JobState::Done)
}

async fn read_remote(remote: &Remote, path: &RemotePath) -> Vec<u8> {
    remote.sftp().read(path.as_bytes().to_vec()).await.unwrap()
}

#[tokio::test(flavor = "multi_thread")]
async fn folders_go_both_ways_with_each_conflict_choice() {
    if !enabled() {
        return;
    }
    let (session, remote) = connect().await;
    session.exec("rm -rf ~/holonet-xfer-tree && mkdir -p ~/holonet-xfer-tree/site && echo old > ~/holonet-xfer-tree/site/old.txt && echo old > ~/holonet-xfer-tree/notes.txt").await.unwrap();
    let dest = remote.home().join(b"holonet-xfer-tree");
    let here = local_dir("tree");
    let site = here.join("site");
    std::fs::create_dir_all(site.join("css")).unwrap();
    std::fs::create_dir_all(site.join("empty")).unwrap();
    let index = bytes(300_000, 1);
    std::fs::write(site.join("index.html"), &index).unwrap();
    std::fs::write(site.join("css").join("a.css"), b"body{}").unwrap();
    std::fs::write(here.join("notes.txt"), b"new notes").unwrap();
    let items = [site.clone(), here.join("notes.txt")];

    let (tx, mut rx) = tokio::sync::mpsc::unbounded_channel();
    let engine = Engine::new(tokio::runtime::Handle::current(), 2, tx);
    engine.set_remote(Some(remote.clone()));

    // Both selected items exist there.
    let plan = plan_upload(&remote, &items, &dest).await.unwrap();
    assert_eq!(plan.conflicts(), ["site", "notes.txt"]);
    assert_eq!((plan.file_count(), plan.total_bytes()), (3, 300_000 + 6 + 9));

    // Keep both: new names beside the old ones, empty folder included.
    let both = plan_upload(&remote, &items, &dest).await.unwrap().resolve(Choice::KeepBoth);
    both.create_folders(&remote).await.unwrap();
    engine.add(both.jobs());
    wait(&engine, &mut rx, all_done).await;
    assert_eq!(read_remote(&remote, &dest.join(b"site (1)").join(b"index.html")).await, index);
    assert_eq!(read_remote(&remote, &dest.join(b"notes (1).txt")).await, b"new notes");
    assert!(remote.exists(&dest.join(b"site (1)").join(b"empty")).await.unwrap());
    assert_eq!(read_remote(&remote, &dest.join(b"notes.txt")).await, b"old\n");
    engine.clear_finished();

    // Overwrite: the folder merges, the file is replaced.
    let over = plan_upload(&remote, &items, &dest).await.unwrap().resolve(Choice::Overwrite);
    over.create_folders(&remote).await.unwrap();
    engine.add(over.jobs());
    wait(&engine, &mut rx, all_done).await;
    assert_eq!(read_remote(&remote, &dest.join(b"notes.txt")).await, b"new notes");
    assert_eq!(read_remote(&remote, &dest.join(b"site").join(b"old.txt")).await, b"old\n");
    assert_eq!(read_remote(&remote, &dest.join(b"site").join(b"css").join(b"a.css")).await, b"body{}");
    engine.clear_finished();

    // Skip: nothing is sent.
    let skip = plan_upload(&remote, &items, &dest).await.unwrap().resolve(Choice::Skip);
    assert_eq!(skip.jobs().len(), 0);
    assert_eq!(skip.skipped.len(), 2);

    // And back down into an empty folder here.
    let back = local_dir("tree-back");
    let down = plan_download(&remote, &[dest.join(b"site")], &back).await.unwrap();
    assert!(down.conflicts().is_empty());
    down.create_folders(&remote).await.unwrap();
    engine.add(down.jobs());
    wait(&engine, &mut rx, all_done).await;
    assert_eq!(std::fs::read(back.join("site").join("index.html")).unwrap(), index);
    assert_eq!(std::fs::read(back.join("site").join("old.txt")).unwrap(), b"old\n");
    assert!(back.join("site").join("empty").is_dir());
    assert!(!transfer::part_path(&back.join("site").join("index.html")).exists());
    assert_eq!(engine.summary(), "Idle");

    session.exec("rm -rf ~/holonet-xfer-tree").await.unwrap();
    std::fs::remove_dir_all(&here).unwrap();
    std::fs::remove_dir_all(&back).unwrap();
    remote.close().await.unwrap();
    session.close().await.unwrap();
}

#[tokio::test(flavor = "multi_thread")]
async fn pauses_resumes_and_refuses_a_changed_source() {
    if !enabled() {
        return;
    }
    let (session, remote) = connect().await;
    session.exec("rm -rf ~/holonet-xfer-pause && mkdir ~/holonet-xfer-pause").await.unwrap();
    let dest = remote.home().join(b"holonet-xfer-pause");
    let here = local_dir("pause");
    // Large enough that the copy is still running when the pause lands.
    let big = bytes(192 * 1_048_576, 2);
    let file = here.join("big.bin");
    std::fs::write(&file, &big).unwrap();

    let (tx, mut rx) = tokio::sync::mpsc::unbounded_channel();
    let engine = Engine::new(tokio::runtime::Handle::current(), 1, tx);
    engine.set_remote(Some(remote.clone()));
    let plan = plan_upload(&remote, std::slice::from_ref(&file), &dest).await.unwrap();
    let id = engine.add(plan.jobs())[0];

    // Pause at the first progress report.
    wait(&engine, &mut rx, |j| j[0].done > 0).await;
    engine.pause(id);
    wait(&engine, &mut rx, |j| j[0].state == JobState::Paused).await;
    let paused_at = engine.snapshot()[0].done;
    assert!(paused_at > 0 && paused_at < big.len() as u64, "paused at {paused_at}");
    assert_eq!(engine.summary(), "1 paused");
    engine.resume(id);
    wait(&engine, &mut rx, all_done).await;
    assert_eq!(read_remote(&remote, &dest.join(b"big.bin")).await, big);
    engine.clear_finished();

    // Pause, change the source, resume: refused; retry starts over.
    let small = here.join("changes.bin");
    std::fs::write(&small, bytes(128 * 1_048_576, 3)).unwrap();
    let plan = plan_upload(&remote, std::slice::from_ref(&small), &dest).await.unwrap();
    let id = engine.add(plan.jobs())[0];
    wait(&engine, &mut rx, |j| j[0].done > 0).await;
    engine.pause(id);
    wait(&engine, &mut rx, |j| j[0].state == JobState::Paused).await;
    let changed = bytes(129 * 1_048_576, 4);
    std::fs::write(&small, &changed).unwrap();
    engine.resume(id);
    wait(&engine, &mut rx, |j| matches!(j[0].state, JobState::Failed(_))).await;
    assert_eq!(engine.snapshot()[0].state, JobState::Failed("The source changed since the transfer started.".into()));
    engine.retry(id);
    wait(&engine, &mut rx, all_done).await;
    assert_eq!(read_remote(&remote, &dest.join(b"changes.bin")).await, changed);

    session.exec("rm -rf ~/holonet-xfer-pause").await.unwrap();
    std::fs::remove_dir_all(&here).unwrap();
    remote.close().await.unwrap();
    session.close().await.unwrap();
}

/// The sshd process serving `session`: the parent of a command it runs.
async fn sshd_pid(session: &Session) -> String {
    session.exec("echo $PPID").await.unwrap().stdout.trim().to_string()
}

#[tokio::test(flavor = "multi_thread")]
async fn resumes_after_the_connection_is_killed_mid_file() {
    if !enabled() {
        return;
    }
    let (session, remote) = connect().await;
    session.exec("rm -rf ~/holonet-xfer-kill && mkdir ~/holonet-xfer-kill").await.unwrap();
    let here = local_dir("kill");
    let big = bytes(96 * 1_048_576, 5);
    std::fs::write(here.join("src.bin"), &big).unwrap();
    // Put the file on the server to download it back.
    let dest = remote.home().join(b"holonet-xfer-kill");
    remote.sftp().write(dest.join(b"big.bin").as_bytes().to_vec(), &big).await.unwrap();
    let pid = sshd_pid(&session).await;
    assert!(pid.parse::<u32>().is_ok(), "{pid}");

    let (tx, mut rx) = tokio::sync::mpsc::unbounded_channel();
    let engine = Engine::new(tokio::runtime::Handle::current(), 1, tx);
    engine.set_remote(Some(remote.clone()));
    let back = here.join("back");
    std::fs::create_dir_all(&back).unwrap();
    let plan = plan_download(&remote, &[dest.join(b"big.bin")], &back).await.unwrap();
    // The second connection is ready first, so the kill lands early.
    let (killer, killer_remote) = connect().await;
    engine.add(plan.jobs());

    // Kill the first connection once 8 MB are through.
    wait(&engine, &mut rx, |j| j[0].done > 8 * 1_048_576).await;
    killer.exec(&format!("kill -9 {pid}")).await.unwrap();
    let mut lost = false;
    let deadline = tokio::time::Instant::now() + Duration::from_secs(60);
    while !lost {
        match tokio::time::timeout_at(deadline, rx.recv()).await {
            Ok(Some(EngineEvent::ConnectionLost)) => lost = true,
            Ok(Some(_)) => {}
            _ => panic!("no ConnectionLost; queue: {} {:?}", engine.summary(), engine.snapshot()),
        }
    }
    let job = engine.snapshot()[0].clone();
    assert_eq!(job.state, JobState::Interrupted);
    assert_eq!(engine.summary(), "1 waiting to reconnect");
    let part = transfer::part_path(&back.join("big.bin"));
    let kept = std::fs::metadata(&part).unwrap().len();
    assert!(kept > 0 && kept < big.len() as u64, "part {kept}");

    // A new connection: the copy resumes from the part, not from zero.
    engine.set_remote(Some(killer_remote.clone()));
    wait(&engine, &mut rx, |j| j[0].state == JobState::Active).await;
    let resumed_from = engine.snapshot()[0].done;
    assert!(resumed_from >= kept.min(job.done) && resumed_from > 0, "resumed from {resumed_from}, kept {kept}");
    wait(&engine, &mut rx, all_done).await;
    assert_eq!(std::fs::read(back.join("big.bin")).unwrap(), big);
    assert!(!part.exists());
    eprintln!("killed at {} bytes, part kept {kept}, resumed from {resumed_from}", job.done);

    killer.exec("rm -rf ~/holonet-xfer-kill").await.unwrap();
    std::fs::remove_dir_all(&here).unwrap();
    killer_remote.close().await.unwrap();
    killer.close().await.unwrap();
}
