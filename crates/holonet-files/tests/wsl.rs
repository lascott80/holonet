//! Integration tests for the host side against the WSL test server
//! (docs/milestone-1-plan.md). Skipped unless `HOLONET_TEST_SERVER=1`:
//!
//!   $env:HOLONET_TEST_SERVER = "1"; cargo test -p holonet-files --test wsl
//!
//! Each test works in its own `~/holonet-files-<name>` folder and removes it.

#[path = "../../holonet-ssh/tests/support/mod.rs"]
mod support;

use holonet_files::remote::Remote;
use holonet_files::{FileError, Kind, RemotePath, sort};
use holonet_ssh::Session;
use support::{load_env, options_from};

fn enabled() -> bool {
    let on = std::env::var("HOLONET_TEST_SERVER").is_ok_and(|v| v == "1");
    if !on {
        eprintln!("skipped: set HOLONET_TEST_SERVER=1 to run against the WSL test server");
    }
    on
}

/// A signed-in session, SFTP on it, and a fresh test folder.
async fn setup(name: &str) -> (Session, Remote, RemotePath) {
    let env = load_env().expect(".dev/test-server.env");
    let session = Session::connect(options_from("ed25519", &env).unwrap()).await.unwrap();
    let folder = format!("holonet-files-{name}");
    session.exec(&format!("rm -rf ~/{folder} && mkdir ~/{folder}")).await.unwrap();
    let remote = Remote::open(&session).await.unwrap();
    let dir = remote.home().join(folder.as_bytes());
    (session, remote, dir)
}

async fn teardown(session: Session, remote: Remote, dir: &RemotePath) {
    session.exec(&format!("rm -rf '{dir}'")).await.unwrap();
    remote.close().await.unwrap();
    session.close().await.unwrap();
}

#[tokio::test]
async fn lists_kinds_links_hidden_files_and_modes() {
    if !enabled() {
        return;
    }
    let (session, remote, dir) = setup("list").await;
    assert!(remote.home().as_bytes().starts_with(b"/home/"));
    session
        .exec(&format!(
            "cd '{dir}' && mkdir sub && printf 12345 > five.txt && chmod 750 five.txt && touch .hidden \
             && ln -s sub to-folder && ln -s five.txt to-file && ln -s nowhere broken"
        ))
        .await
        .unwrap();
    let mut list = remote.list(&dir).await.unwrap();
    sort(&mut list);
    let rows: Vec<(&str, Kind, bool)> = list.iter().map(|e| (e.name.as_str(), e.kind, e.hidden)).collect();
    assert_eq!(
        rows,
        [
            ("sub", Kind::Folder, false),
            ("to-folder", Kind::Link { folder: true, broken: false }, false),
            (".hidden", Kind::File, true),
            ("broken", Kind::Link { folder: false, broken: true }, false),
            ("five.txt", Kind::File, false),
            ("to-file", Kind::Link { folder: false, broken: false }, false),
        ]
    );
    let five = list.iter().find(|e| e.name == "five.txt").unwrap();
    assert_eq!((five.size, five.mode, five.symbolic_mode().as_deref()), (5, Some(0o750), Some("-rwxr-x---")));
    assert!(five.modified.is_some() && five.uid.is_some());
    // One entry, not following the link.
    let link = remote.entry(&dir.join(b"to-folder")).await.unwrap();
    assert_eq!(link.kind, Kind::Link { folder: true, broken: false });
    teardown(session, remote, &dir).await;
}

#[tokio::test]
async fn renames_creates_folders_and_removes_recursively() {
    if !enabled() {
        return;
    }
    let (session, remote, dir) = setup("edit").await;
    let a = dir.join(b"a");
    remote.create_folder(&a).await.unwrap();
    assert_eq!(remote.create_folder(&a).await, Err(FileError::AlreadyExists));
    session
        .exec(&format!("cd '{a}' && mkdir -p deep/er && touch deep/er/x deep/y z && ln -s /etc link"))
        .await
        .unwrap();
    let b = dir.join(b"b");
    remote.rename(&a, &b).await.unwrap();
    assert!(!remote.exists(&a).await.unwrap() && remote.exists(&b).await.unwrap());
    remote.create_folder(&a).await.unwrap();
    assert_eq!(remote.rename(&a, &b).await, Err(FileError::AlreadyExists));
    // b: deep, deep/er, deep/er/x, deep/y, z, link, and b itself. The link
    // is removed, never followed into /etc.
    assert_eq!(remote.remove(&b).await.unwrap(), 7);
    assert!(!remote.exists(&b).await.unwrap());
    assert_eq!(remote.remove(&b).await, Err(FileError::NotFound));
    assert!(session.exec("test -d /etc && echo yes").await.unwrap().stdout.contains("yes"));
    teardown(session, remote, &dir).await;
}

#[tokio::test]
async fn sets_modes_and_makes_links_in_the_servers_order() {
    if !enabled() {
        return;
    }
    let (session, remote, dir) = setup("mode").await;
    let file = dir.join(b"f.txt");
    session.exec(&format!("touch '{file}' && chmod 664 '{file}'")).await.unwrap();
    remote.set_mode(&file, 0o640).await.unwrap();
    assert_eq!(remote.entry(&file).await.unwrap().mode, Some(0o640));
    // A relative link inside the test folder; the home folder must not gain
    // a stray link (phase 0's first run made two there).
    let link = dir.join(b"l");
    remote.symlink(b"f.txt", &link).await.unwrap();
    assert_eq!(remote.read_link(&link).await.unwrap(), b"f.txt");
    let shell = session.exec(&format!("readlink '{link}'; ls -la ~ | grep -c -- '->' || true")).await.unwrap();
    let lines: Vec<&str> = shell.stdout.lines().collect();
    assert_eq!(lines, ["f.txt", "0"]);
    teardown(session, remote, &dir).await;
}

#[tokio::test]
async fn keeps_names_that_are_not_utf8() {
    if !enabled() {
        return;
    }
    let (session, remote, dir) = setup("bytes").await;
    session.exec(&format!("touch '{dir}'/$(printf 'caf\\351.txt')")).await.unwrap();
    let list = remote.list(&dir).await.unwrap();
    let entry = list.iter().find(|e| e.raw_name.starts_with(b"caf")).unwrap();
    assert_eq!(entry.raw_name, b"caf\xe9.txt");
    assert_eq!(entry.name, "caf\u{fffd}.txt");
    // Renamed by its bytes.
    remote.rename(&dir.join(&entry.raw_name), &dir.join(b"cafe.txt")).await.unwrap();
    assert!(remote.exists(&dir.join(b"cafe.txt")).await.unwrap());
    teardown(session, remote, &dir).await;
}

#[tokio::test]
async fn reports_missing_and_forbidden_folders() {
    if !enabled() {
        return;
    }
    let (session, remote, dir) = setup("errors").await;
    assert_eq!(remote.list(&dir.join(b"missing")).await.unwrap_err(), FileError::NotFound);
    assert_eq!(remote.list(&RemotePath::new(b"/root".to_vec())).await.unwrap_err(), FileError::PermissionDenied);
    // Relative paths start at home.
    let canon = remote.canonicalize(b"holonet-files-errors/../holonet-files-errors").await.unwrap();
    assert_eq!(canon, dir);
    teardown(session, remote, &dir).await;
}
