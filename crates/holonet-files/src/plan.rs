//! Planning a transfer: the selected items expanded into the folders to
//! create and the files to copy, and the conflicts to ask about once for
//! the whole transfer (`decisions.md`). Conflicts are the selected items
//! whose names already exist at the destination; a folder that exists is
//! merged under Overwrite.
//!
//! Links: on this PC a link to a file is followed and a link to a folder is
//! skipped (no loops). On a host the same, since Windows cannot make the link.

use std::collections::HashSet;
use std::path::{Path, PathBuf};

use crate::remote::Remote;
use crate::{FileError, Kind, RemotePath, Result, local};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Direction {
    Upload,
    Download,
}

/// How to treat items that already exist (the conflict popup's choice, or
/// Settings' default).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Choice {
    Overwrite,
    KeepBoth,
    Skip,
}

/// A selected item.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct TopItem {
    pub name: String,
    pub raw: Vec<u8>,
    /// Its name already exists at the destination.
    pub conflict: bool,
    /// The name it gets at the destination (differs under Keep both).
    pub dest_raw: Vec<u8>,
}

/// Where something sits below its selected item.
#[derive(Clone, Debug, PartialEq, Eq)]
struct Place {
    top: usize,
    /// Names below the selected item; empty for the item itself.
    rel: Vec<Vec<u8>>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
struct PlannedFile {
    place: Place,
    local: Option<PathBuf>,
    remote: Option<RemotePath>,
    size: u64,
}

/// One file to copy.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct FileJob {
    pub direction: Direction,
    pub local: PathBuf,
    pub remote: RemotePath,
    /// The name shown in the queue.
    pub name: String,
    pub size: u64,
}

#[derive(Clone, Debug)]
pub struct Plan {
    pub direction: Direction,
    local_dir: PathBuf,
    remote_dir: RemotePath,
    pub items: Vec<TopItem>,
    folders: Vec<Place>,
    files: Vec<PlannedFile>,
    /// Items left out, and why ("Links to folders are not copied").
    pub skipped: Vec<(String, String)>,
    /// Names already in the destination folder (for Keep both).
    existing: HashSet<Vec<u8>>,
}

/// A name Windows cannot use: reserved characters, a trailing dot or
/// space, or a device name such as `CON` or `com1.txt`.
pub fn windows_unsafe(name: &str) -> bool {
    if name.is_empty() || name.ends_with(['.', ' ']) {
        return true;
    }
    if name.chars().any(|c| (c as u32) < 32 || r#"<>:"/\|?*"#.contains(c)) {
        return true;
    }
    let stem = name.split('.').next().unwrap_or("").to_ascii_uppercase();
    matches!(stem.as_str(), "CON" | "PRN" | "AUX" | "NUL")
        || ((stem.starts_with("COM") || stem.starts_with("LPT"))
            && stem.len() == 4
            && stem.as_bytes()[3].is_ascii_digit()
            && stem.as_bytes()[3] != b'0')
}

/// "name (1).ext", "name (2).ext", … the first that is free. A leading dot
/// is part of the name, not an extension (".bashrc (1)").
pub fn keep_both_name(raw: &[u8], taken: &HashSet<Vec<u8>>) -> Vec<u8> {
    let dot = raw.iter().rposition(|b| *b == b'.').filter(|i| *i > 0);
    let (stem, ext) = match dot {
        Some(i) => (&raw[..i], &raw[i..]),
        None => (raw, &b""[..]),
    };
    (1..)
        .map(|n| [stem, format!(" ({n})").as_bytes(), ext].concat())
        .find(|candidate| !taken.contains(candidate))
        .expect("a free name")
}

impl Plan {
    fn new(direction: Direction, local_dir: PathBuf, remote_dir: RemotePath, existing: HashSet<Vec<u8>>) -> Self {
        Self {
            direction,
            local_dir,
            remote_dir,
            items: Vec::new(),
            folders: Vec::new(),
            files: Vec::new(),
            skipped: Vec::new(),
            existing,
        }
    }

    fn add_item(&mut self, name: &str, raw: &[u8]) -> usize {
        self.items.push(TopItem {
            name: name.to_string(),
            raw: raw.to_vec(),
            conflict: self.existing.contains(raw),
            dest_raw: raw.to_vec(),
        });
        self.items.len() - 1
    }

    /// The names of the selected items that already exist there.
    pub fn conflicts(&self) -> Vec<&str> {
        self.items.iter().filter(|i| i.conflict).map(|i| i.name.as_str()).collect()
    }

    pub fn file_count(&self) -> usize {
        self.files.len()
    }

    pub fn total_bytes(&self) -> u64 {
        self.files.iter().map(|f| f.size).sum()
    }

    /// Apply the choice to every conflicting item.
    pub fn resolve(mut self, choice: Choice) -> Plan {
        match choice {
            Choice::Overwrite => {}
            Choice::Skip => {
                let skipped: HashSet<usize> = (0..self.items.len()).filter(|i| self.items[*i].conflict).collect();
                self.folders.retain(|p| !skipped.contains(&p.top));
                self.files.retain(|f| !skipped.contains(&f.place.top));
                for i in &skipped {
                    let name = self.items[*i].name.clone();
                    self.skipped.push((name, "Already there; skipped".into()));
                }
            }
            Choice::KeepBoth => {
                let mut taken = self.existing.clone();
                taken.extend(self.items.iter().map(|i| i.raw.clone()));
                for item in self.items.iter_mut().filter(|i| i.conflict) {
                    item.dest_raw = keep_both_name(&item.raw, &taken);
                    taken.insert(item.dest_raw.clone());
                }
            }
        }
        for item in &mut self.items {
            item.conflict = false;
        }
        self
    }

    fn local_dest(&self, place: &Place) -> PathBuf {
        let mut path = self.local_dir.join(String::from_utf8_lossy(&self.items[place.top].dest_raw).as_ref());
        for part in &place.rel {
            path.push(String::from_utf8_lossy(part).as_ref());
        }
        path
    }

    fn remote_dest(&self, place: &Place) -> RemotePath {
        let mut path = self.remote_dir.join(&self.items[place.top].dest_raw);
        for part in &place.rel {
            path = path.join(part);
        }
        path
    }

    /// Create the folders, parents first. A folder already there is used
    /// (Overwrite merges folders).
    pub async fn create_folders(&self, remote: &Remote) -> Result<()> {
        for place in &self.folders {
            match self.direction {
                Direction::Upload => match remote.create_folder(&self.remote_dest(place)).await {
                    Ok(()) | Err(FileError::AlreadyExists) => {}
                    Err(e) => return Err(e),
                },
                Direction::Download => match local::create_folder(&self.local_dest(place)) {
                    Ok(()) | Err(FileError::AlreadyExists) => {}
                    Err(e) => return Err(e),
                },
            }
        }
        Ok(())
    }

    /// The files to copy, in order.
    pub fn jobs(&self) -> Vec<FileJob> {
        self.files
            .iter()
            .map(|f| {
                let (local, remote) = match self.direction {
                    Direction::Upload => (f.local.clone().expect("upload source"), self.remote_dest(&f.place)),
                    Direction::Download => (self.local_dest(&f.place), f.remote.clone().expect("download source")),
                };
                let name = match self.direction {
                    Direction::Upload => {
                        local.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default()
                    }
                    Direction::Download => String::from_utf8_lossy(remote.file_name()).into_owned(),
                };
                FileJob { direction: self.direction, local, remote, name, size: f.size }
            })
            .collect()
    }
}

/// Plan sending `items` (on this PC) into the host's folder `dest`.
pub async fn plan_upload(remote: &Remote, items: &[PathBuf], dest: &RemotePath) -> Result<Plan> {
    let existing: HashSet<Vec<u8>> = remote.list(dest).await?.into_iter().map(|e| e.raw_name).collect();
    let mut plan = Plan::new(Direction::Upload, PathBuf::new(), dest.clone(), existing);
    for item in items {
        let entry = local::entry(item)?;
        let top = plan.add_item(&entry.name, entry.name.as_bytes());
        walk_local(&mut plan, item, &entry.name, entry.kind, entry.size, Place { top, rel: Vec::new() })?;
    }
    Ok(plan)
}

fn walk_local(plan: &mut Plan, path: &Path, name: &str, kind: Kind, size: u64, place: Place) -> Result<()> {
    match kind {
        Kind::Folder => {
            plan.folders.push(place.clone());
            let mut children = local::list(path)?;
            crate::sort(&mut children);
            for child in children {
                let mut rel = place.rel.clone();
                rel.push(child.name.as_bytes().to_vec());
                let child_place = Place { top: place.top, rel };
                walk_local(plan, &path.join(&child.name), &child.name, child.kind, child.size, child_place)?;
            }
        }
        Kind::File => plan.files.push(PlannedFile { place, local: Some(path.to_path_buf()), remote: None, size }),
        Kind::Link { folder: false, broken: false } => {
            let size = std::fs::metadata(path)?.len();
            plan.files.push(PlannedFile { place, local: Some(path.to_path_buf()), remote: None, size });
        }
        Kind::Link { folder: true, .. } => plan.skipped.push((name.into(), "Links to folders are not copied".into())),
        Kind::Link { broken: true, .. } => plan.skipped.push((name.into(), "The link leads nowhere".into())),
        Kind::Other => plan.skipped.push((name.into(), "Not a file or folder".into())),
    }
    Ok(())
}

/// Plan fetching `items` (on the host) into the folder `dest` on this PC.
pub async fn plan_download(remote: &Remote, items: &[RemotePath], dest: &Path) -> Result<Plan> {
    let existing: HashSet<Vec<u8>> = local::list(dest)?.into_iter().map(|e| e.name.into_bytes()).collect();
    let mut plan = Plan::new(Direction::Download, dest.to_path_buf(), RemotePath::default(), existing);
    for item in items {
        let entry = remote.entry(item).await?;
        if windows_unsafe(&entry.name) {
            plan.skipped.push((entry.name.clone(), "This name cannot be used on Windows".into()));
            continue;
        }
        let top = plan.add_item(&entry.name, entry.name.as_bytes());
        Box::pin(walk_remote(
            remote,
            &mut plan,
            item,
            &entry.name,
            entry.kind,
            entry.size,
            Place { top, rel: Vec::new() },
        ))
        .await?;
    }
    Ok(plan)
}

async fn walk_remote(
    remote: &Remote,
    plan: &mut Plan,
    path: &RemotePath,
    name: &str,
    kind: Kind,
    size: u64,
    place: Place,
) -> Result<()> {
    match kind {
        Kind::Folder => {
            plan.folders.push(place.clone());
            let mut children = remote.list(path).await?;
            crate::sort(&mut children);
            for child in children {
                if windows_unsafe(&child.name) {
                    plan.skipped.push((child.name.clone(), "This name cannot be used on Windows".into()));
                    continue;
                }
                let mut rel = place.rel.clone();
                rel.push(child.name.as_bytes().to_vec());
                let child_place = Place { top: place.top, rel };
                let child_path = path.join(&child.raw_name);
                Box::pin(walk_remote(remote, plan, &child_path, &child.name, child.kind, child.size, child_place))
                    .await?;
            }
        }
        Kind::File => plan.files.push(PlannedFile { place, local: None, remote: Some(path.clone()), size }),
        Kind::Link { folder: false, broken: false } => {
            let size = remote.sftp().metadata(path.as_bytes().to_vec()).await?.size.unwrap_or(0);
            plan.files.push(PlannedFile { place, local: None, remote: Some(path.clone()), size });
        }
        Kind::Link { folder: true, .. } => plan.skipped.push((name.into(), "Links to folders are not copied".into())),
        Kind::Link { broken: true, .. } => plan.skipped.push((name.into(), "The link leads nowhere".into())),
        Kind::Other => plan.skipped.push((name.into(), "Not a file or folder".into())),
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn keep_both_finds_the_first_free_number() {
        let mut taken: HashSet<Vec<u8>> = [b"index.html".to_vec(), b"index (1).html".to_vec()].into();
        assert_eq!(keep_both_name(b"index.html", &taken), b"index (2).html");
        taken.clear();
        assert_eq!(keep_both_name(b"site", &taken), b"site (1)");
        assert_eq!(keep_both_name(b".bashrc", &taken), b".bashrc (1)");
        assert_eq!(keep_both_name(b"a.tar.gz", &taken), b"a.tar (1).gz");
    }

    #[test]
    fn windows_unsafe_names() {
        for bad in ["a:b", "what?", "CON", "con.txt", "com1", "LPT9.log", "trailing.", "trailing ", "tab\there"] {
            assert!(windows_unsafe(bad), "{bad}");
        }
        for ok in ["index.html", ".env", "com", "com0", "console.log", "caf\u{fffd}.txt", "a b"] {
            assert!(!windows_unsafe(ok), "{ok}");
        }
    }

    /// A download plan built by hand: a conflicting folder with a file in
    /// it, and a new file.
    fn sample() -> Plan {
        let existing: HashSet<Vec<u8>> = [b"site".to_vec()].into();
        let mut plan = Plan::new(Direction::Download, PathBuf::from("C:\\dl"), RemotePath::default(), existing);
        let site = plan.add_item("site", b"site");
        let notes = plan.add_item("notes.txt", b"notes.txt");
        plan.folders.push(Place { top: site, rel: vec![] });
        plan.files.push(PlannedFile {
            place: Place { top: site, rel: vec![b"index.html".to_vec()] },
            local: None,
            remote: Some(RemotePath::new(b"/srv/site/index.html".to_vec())),
            size: 10,
        });
        plan.files.push(PlannedFile {
            place: Place { top: notes, rel: vec![] },
            local: None,
            remote: Some(RemotePath::new(b"/srv/notes.txt".to_vec())),
            size: 5,
        });
        plan
    }

    #[test]
    fn resolving_conflicts_overwrites_renames_or_skips() {
        let plan = sample();
        assert_eq!(plan.conflicts(), ["site"]);
        assert_eq!((plan.file_count(), plan.total_bytes()), (2, 15));

        let over = plan.clone().resolve(Choice::Overwrite);
        let dests: Vec<PathBuf> = over.jobs().into_iter().map(|j| j.local).collect();
        assert_eq!(dests, [PathBuf::from("C:\\dl\\site\\index.html"), PathBuf::from("C:\\dl\\notes.txt")]);
        assert!(over.conflicts().is_empty());

        let both = plan.clone().resolve(Choice::KeepBoth);
        let jobs = both.jobs();
        assert_eq!(jobs[0].local, PathBuf::from("C:\\dl\\site (1)\\index.html"));
        assert_eq!(jobs[0].name, "index.html");
        assert_eq!(both.local_dest(&both.folders[0]), PathBuf::from("C:\\dl\\site (1)"));

        let skip = plan.resolve(Choice::Skip);
        let jobs = skip.jobs();
        assert_eq!(jobs.len(), 1);
        assert_eq!(jobs[0].remote.as_bytes(), b"/srv/notes.txt");
        assert!(skip.folders.is_empty());
        assert_eq!(skip.skipped, [("site".to_string(), "Already there; skipped".to_string())]);
    }
}
