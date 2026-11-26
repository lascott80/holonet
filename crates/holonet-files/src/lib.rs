//! The two sides of the SFTP screen, without a window (milestone 5): a host's
//! files over SFTP ([`remote`]) and this PC's ([`local`]), listed into the
//! same [`Entry`] so one file list can show either. The transfer engine
//! (phase 3) builds on both.

pub mod local;
pub mod remote;

use std::fmt;

/// What an entry is. A link records whether it leads to a folder (it then
/// opens like one) or nowhere.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Kind {
    Folder,
    File,
    Link {
        folder: bool,
        broken: bool,
    },
    /// A device, socket or pipe: listed, never transferred.
    Other,
}

impl Kind {
    /// Opens as a folder (a folder, or a link to one).
    pub fn opens(self) -> bool {
        matches!(self, Kind::Folder | Kind::Link { folder: true, .. })
    }
}

/// One row of a file list.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Entry {
    /// The name to show. A host's name that is not UTF-8 is shown with
    /// replacement characters; [`Entry::raw_name`] keeps the bytes.
    pub name: String,
    /// The name's bytes as the host sent them (this PC: the UTF-8 name).
    pub raw_name: Vec<u8>,
    pub kind: Kind,
    /// Bytes; 0 for folders.
    pub size: u64,
    /// Seconds since 1970, when known.
    pub modified: Option<i64>,
    /// Permission bits (`0o7777`) on a host; `None` on this PC.
    pub mode: Option<u32>,
    pub uid: Option<u32>,
    pub gid: Option<u32>,
    /// Hidden: the hidden or system attribute on this PC, a leading dot on
    /// a host (Settings, SFTP, "Show hidden files").
    pub hidden: bool,
}

impl Entry {
    /// `-rwxr-x---` style, for the Permissions column.
    pub fn symbolic_mode(&self) -> Option<String> {
        let mode = self.mode?;
        let kind = match self.kind {
            Kind::Folder => 'd',
            Kind::Link { .. } => 'l',
            Kind::File => '-',
            Kind::Other => '?',
        };
        let bits: String = (0..9).map(|i| if mode & (0o400 >> i) != 0 { b"rwx"[i % 3] as char } else { '-' }).collect();
        Some(format!("{kind}{bits}"))
    }
}

/// Folders (and links to folders) first, then names without regard to case.
pub fn sort(entries: &mut [Entry]) {
    entries.sort_by(|a, b| {
        b.kind.opens().cmp(&a.kind.opens()).then_with(|| a.name.to_lowercase().cmp(&b.name.to_lowercase()))
    });
}

/// A path on a host: bytes, `/`-separated (SFTP paths are POSIX paths, and
/// may not be UTF-8).
#[derive(Clone, PartialEq, Eq, Hash, Default)]
pub struct RemotePath(Vec<u8>);

impl RemotePath {
    pub fn new(bytes: impl Into<Vec<u8>>) -> Self {
        Self(bytes.into())
    }

    pub fn as_bytes(&self) -> &[u8] {
        &self.0
    }

    /// `self/name`.
    pub fn join(&self, name: &[u8]) -> Self {
        let mut out = self.0.clone();
        if !out.ends_with(b"/") {
            out.push(b'/');
        }
        out.extend_from_slice(name);
        Self(out)
    }

    /// The folder above, or `None` at `/`.
    pub fn parent(&self) -> Option<Self> {
        let trimmed = self.0.strip_suffix(b"/").unwrap_or(&self.0);
        if trimmed.is_empty() {
            return None;
        }
        match trimmed.iter().rposition(|b| *b == b'/') {
            Some(0) => Some(Self(b"/".to_vec())),
            Some(i) => Some(Self(trimmed[..i].to_vec())),
            None => None,
        }
    }

    /// The last part of the path.
    pub fn file_name(&self) -> &[u8] {
        let trimmed = self.0.strip_suffix(b"/").unwrap_or(&self.0);
        match trimmed.iter().rposition(|b| *b == b'/') {
            Some(i) => &trimmed[i + 1..],
            None => trimmed,
        }
    }

    /// The parts for breadcrumbs: `/` first, then each folder.
    pub fn components(&self) -> Vec<(String, RemotePath)> {
        let mut out = vec![("/".to_string(), RemotePath::new(b"/".to_vec()))];
        let mut path = RemotePath::new(b"/".to_vec());
        for part in self.0.split(|b| *b == b'/').filter(|p| !p.is_empty()) {
            path = path.join(part);
            out.push((String::from_utf8_lossy(part).into_owned(), path.clone()));
        }
        out
    }
}

impl fmt::Display for RemotePath {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&String::from_utf8_lossy(&self.0))
    }
}

impl fmt::Debug for RemotePath {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "RemotePath({self})")
    }
}

/// Why a file operation failed, on either side.
#[derive(Clone, Debug, PartialEq, Eq, thiserror::Error)]
pub enum FileError {
    #[error("No such file or folder.")]
    NotFound,
    #[error("Permission denied.")]
    PermissionDenied,
    #[error("Something with this name already exists in this folder.")]
    AlreadyExists,
    #[error("The connection was lost.")]
    Lost,
    #[error("The host did not answer in time.")]
    TimedOut,
    #[error("{0}")]
    Other(String),
}

impl From<holonet_ssh::russh_sftp::client::error::Error> for FileError {
    fn from(e: holonet_ssh::russh_sftp::client::error::Error) -> Self {
        use holonet_ssh::russh_sftp::client::error::Error;
        use holonet_ssh::russh_sftp::protocol::StatusCode;
        match e {
            Error::Status(s) => match s.status_code {
                StatusCode::NoSuchFile => FileError::NotFound,
                StatusCode::PermissionDenied => FileError::PermissionDenied,
                StatusCode::NoConnection | StatusCode::ConnectionLost => FileError::Lost,
                _ if s.error_message.trim().is_empty() => FileError::Other(format!("The host said: {}", s.status_code)),
                _ => FileError::Other(format!("The host said: {}", s.error_message.trim())),
            },
            Error::Timeout => FileError::TimedOut,
            Error::IO(_) => FileError::Lost,
            other => FileError::Other(other.to_string()),
        }
    }
}

impl From<std::io::Error> for FileError {
    fn from(e: std::io::Error) -> Self {
        match e.kind() {
            std::io::ErrorKind::NotFound => FileError::NotFound,
            std::io::ErrorKind::PermissionDenied => FileError::PermissionDenied,
            std::io::ErrorKind::AlreadyExists => FileError::AlreadyExists,
            _ => FileError::Other(e.to_string()),
        }
    }
}

pub type Result<T> = std::result::Result<T, FileError>;

#[cfg(test)]
mod tests {
    use super::*;

    fn entry(name: &str, kind: Kind) -> Entry {
        Entry {
            name: name.into(),
            raw_name: name.as_bytes().to_vec(),
            kind,
            size: 0,
            modified: None,
            mode: None,
            uid: None,
            gid: None,
            hidden: false,
        }
    }

    #[test]
    fn folders_and_folder_links_sort_first_then_names_ignoring_case() {
        let mut list = vec![
            entry("zeta.txt", Kind::File),
            entry("Beta", Kind::Folder),
            entry("current", Kind::Link { folder: true, broken: false }),
            entry("alpha.txt", Kind::File),
            entry("dead", Kind::Link { folder: false, broken: true }),
            entry("alpha", Kind::Folder),
        ];
        sort(&mut list);
        let names: Vec<&str> = list.iter().map(|e| e.name.as_str()).collect();
        assert_eq!(names, ["alpha", "Beta", "current", "alpha.txt", "dead", "zeta.txt"]);
    }

    #[test]
    fn remote_paths_join_split_and_keep_bytes() {
        let home = RemotePath::new(b"/home/deploy".to_vec());
        let file = home.join(b"caf\xe9.txt");
        assert_eq!(file.as_bytes(), b"/home/deploy/caf\xe9.txt");
        assert_eq!(file.file_name(), b"caf\xe9.txt");
        assert_eq!(file.parent(), Some(home.clone()));
        assert_eq!(RemotePath::new(b"/".to_vec()).join(b"etc").as_bytes(), b"/etc");
        assert_eq!(RemotePath::new(b"/etc".to_vec()).parent(), Some(RemotePath::new(b"/".to_vec())));
        assert_eq!(RemotePath::new(b"/".to_vec()).parent(), None);
        assert_eq!(file.to_string(), "/home/deploy/caf\u{fffd}.txt");
        let crumbs: Vec<String> = home.components().into_iter().map(|(n, _)| n).collect();
        assert_eq!(crumbs, ["/", "home", "deploy"]);
        assert_eq!(home.components()[2].1, home);
    }

    #[test]
    fn symbolic_mode_reads_like_ls() {
        let mut e = entry("deploy.sh", Kind::File);
        e.mode = Some(0o750);
        assert_eq!(e.symbolic_mode().as_deref(), Some("-rwxr-x---"));
        e.kind = Kind::Folder;
        e.mode = Some(0o755);
        assert_eq!(e.symbolic_mode().as_deref(), Some("drwxr-xr-x"));
        e.mode = None;
        assert_eq!(e.symbolic_mode(), None);
    }
}
