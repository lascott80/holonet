//! A host's files over SFTP (`russh-sftp` 4.0 with `raw-string`).
//!
//! Findings from milestone 5 phase 0 that shape this:
//! - `read_dir` reports a link as a link, so a link is `stat`ed to learn
//!   whether it opens as a folder.
//! - OpenSSH's server takes `SSH_FXP_SYMLINK`'s two paths in reverse order
//!   (target first). [`Remote::symlink`] passes them the way the server
//!   expects; OpenSSH is recognised by its `statvfs@openssh.com` extension.
//! - SFTP v3 attributes carry numeric owner and group ids only.

use holonet_ssh::Session;
use holonet_ssh::russh_sftp::client::SftpSession;
use holonet_ssh::russh_sftp::protocol::FileAttributes;

use crate::{Entry, FileError, Kind, RemotePath, Result};

/// An SFTP session on a host.
pub struct Remote {
    sftp: SftpSession,
    home: RemotePath,
    /// The server takes symlink paths in OpenSSH's (reversed) order.
    openssh_symlink: bool,
}

fn to_entry(name: &[u8], attrs: &FileAttributes, kind: Kind) -> Entry {
    Entry {
        name: String::from_utf8_lossy(name).into_owned(),
        raw_name: name.to_vec(),
        kind,
        size: if kind == Kind::Folder { 0 } else { attrs.size.unwrap_or(0) },
        modified: attrs.mtime.map(i64::from),
        mode: attrs.permissions.map(|p| p & 0o7777),
        uid: attrs.uid,
        gid: attrs.gid,
        hidden: name.first() == Some(&b'.'),
    }
}

fn kind_of(attrs: &FileAttributes) -> Kind {
    if attrs.is_symlink() {
        Kind::Link { folder: false, broken: false }
    } else if attrs.is_dir() {
        Kind::Folder
    } else if attrs.is_regular() {
        Kind::File
    } else {
        Kind::Other
    }
}

impl Remote {
    /// Open SFTP on a signed-in session.
    pub async fn open(session: &Session) -> Result<Self> {
        let sftp = session.open_sftp().await.map_err(|e| FileError::Other(format!("SFTP did not start: {e:#}")))?;
        Self::from_sftp(sftp).await
    }

    pub async fn from_sftp(sftp: SftpSession) -> Result<Self> {
        let home = RemotePath::new(sftp.canonicalize(".").await?.as_bytes().to_vec());
        let openssh_symlink = matches!(sftp.fs_info(home.as_bytes().to_vec()).await, Ok(Some(_)));
        Ok(Self { sftp, home, openssh_symlink })
    }

    /// The sign-in's home folder.
    pub fn home(&self) -> &RemotePath {
        &self.home
    }

    /// The underlying session, for transfers.
    pub fn sftp(&self) -> &SftpSession {
        &self.sftp
    }

    /// The absolute form of `path` (resolving `~`-relative and `..` parts as
    /// the server does). A relative path is taken from the home folder.
    pub async fn canonicalize(&self, path: &[u8]) -> Result<RemotePath> {
        let full = if path.first() == Some(&b'/') { RemotePath::new(path.to_vec()) } else { self.home.join(path) };
        Ok(RemotePath::new(self.sftp.canonicalize(full.as_bytes().to_vec()).await?.as_bytes().to_vec()))
    }

    /// A folder's entries, links resolved, unsorted.
    pub async fn list(&self, dir: &RemotePath) -> Result<Vec<Entry>> {
        let mut out = Vec::new();
        for item in self.sftp.read_dir(dir.as_bytes().to_vec()).await? {
            let name = item.file_name();
            let attrs = item.metadata();
            let mut kind = kind_of(&attrs);
            if let Kind::Link { .. } = kind {
                kind = match self.sftp.metadata(dir.join(name.as_bytes()).as_bytes().to_vec()).await {
                    Ok(target) => Kind::Link { folder: target.is_dir(), broken: false },
                    Err(_) => Kind::Link { folder: false, broken: true },
                };
            }
            out.push(to_entry(name.as_bytes(), &attrs, kind));
        }
        Ok(out)
    }

    /// One entry, without following a link.
    pub async fn entry(&self, path: &RemotePath) -> Result<Entry> {
        let attrs = self.sftp.symlink_metadata(path.as_bytes().to_vec()).await?;
        let mut kind = kind_of(&attrs);
        if let Kind::Link { .. } = kind {
            kind = match self.sftp.metadata(path.as_bytes().to_vec()).await {
                Ok(target) => Kind::Link { folder: target.is_dir(), broken: false },
                Err(_) => Kind::Link { folder: false, broken: true },
            };
        }
        Ok(to_entry(path.file_name(), &attrs, kind))
    }

    pub async fn exists(&self, path: &RemotePath) -> Result<bool> {
        match self.sftp.symlink_metadata(path.as_bytes().to_vec()).await {
            Ok(_) => Ok(true),
            Err(e) => match FileError::from(e) {
                FileError::NotFound => Ok(false),
                other => Err(other),
            },
        }
    }

    /// Rename (or move) within the host. Refuses to replace something.
    pub async fn rename(&self, from: &RemotePath, to: &RemotePath) -> Result<()> {
        if self.exists(to).await? {
            return Err(FileError::AlreadyExists);
        }
        Ok(self.sftp.rename(from.as_bytes().to_vec(), to.as_bytes().to_vec()).await?)
    }

    pub async fn create_folder(&self, path: &RemotePath) -> Result<()> {
        if self.exists(path).await? {
            return Err(FileError::AlreadyExists);
        }
        Ok(self.sftp.create_dir(path.as_bytes().to_vec()).await?)
    }

    /// Delete permanently (`decisions.md`): a folder with everything in it.
    /// A link is removed, never followed. Returns how many items went.
    pub async fn remove(&self, path: &RemotePath) -> Result<usize> {
        let attrs = self.sftp.symlink_metadata(path.as_bytes().to_vec()).await?;
        if attrs.is_dir() && !attrs.is_symlink() {
            let mut count = 0;
            let children: Vec<_> = self.sftp.read_dir(path.as_bytes().to_vec()).await?.collect();
            for child in children {
                count += Box::pin(self.remove(&path.join(child.file_name().as_bytes()))).await?;
            }
            self.sftp.remove_dir(path.as_bytes().to_vec()).await?;
            Ok(count + 1)
        } else {
            self.sftp.remove_file(path.as_bytes().to_vec()).await?;
            Ok(1)
        }
    }

    /// Set permission bits (`0o7777`); owner and group are left alone.
    pub async fn set_mode(&self, path: &RemotePath, mode: u32) -> Result<()> {
        let mut attrs = FileAttributes::empty();
        attrs.permissions = Some(mode & 0o7777);
        Ok(self.sftp.set_metadata(path.as_bytes().to_vec(), attrs).await?)
    }

    /// Make `link` a symbolic link to `target` (as written, often relative).
    pub async fn symlink(&self, target: &[u8], link: &RemotePath) -> Result<()> {
        if self.openssh_symlink {
            Ok(self.sftp.symlink(target.to_vec(), link.as_bytes().to_vec()).await?)
        } else {
            Ok(self.sftp.symlink(link.as_bytes().to_vec(), target.to_vec()).await?)
        }
    }

    pub async fn read_link(&self, path: &RemotePath) -> Result<Vec<u8>> {
        Ok(self.sftp.read_link(path.as_bytes().to_vec()).await?.as_bytes().to_vec())
    }

    pub async fn close(&self) -> Result<()> {
        Ok(self.sftp.close().await?)
    }
}
