//! Groups (Groups.dc.html): a colour and defaults for their hosts. A host
//! takes a default only for the fields its [`FromGroup`] marks; everything
//! else is its own. `VaultData::resolved` applies them.

use serde::{Deserialize, Serialize};

use crate::{GroupId, HostId, KeyId, Route};

/// The six colours drawn on the Groups page.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum GroupColor {
    #[default]
    Cyan,
    Violet,
    Mint,
    Gray,
    Rose,
    Lime,
}

impl GroupColor {
    pub const ALL: [GroupColor; 6] =
        [GroupColor::Cyan, GroupColor::Violet, GroupColor::Mint, GroupColor::Gray, GroupColor::Rose, GroupColor::Lime];

    pub fn label(self) -> &'static str {
        match self {
            GroupColor::Cyan => "Cyan",
            GroupColor::Violet => "Violet",
            GroupColor::Mint => "Mint",
            GroupColor::Gray => "Gray",
            GroupColor::Rose => "Rose",
            GroupColor::Lime => "Lime",
        }
    }
}

/// A group's default sign-in key.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case", tag = "kind", content = "key_id")]
pub enum GroupKey {
    /// "Ask each time": a host taking the default signs in with a password
    /// asked on connect.
    #[default]
    AskEachTime,
    Key(KeyId),
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Group {
    pub id: GroupId,
    pub name: String,
    #[serde(default)]
    pub color: GroupColor,
    #[serde(default)]
    pub key: GroupKey,
    #[serde(default)]
    pub jump_host: Option<HostId>,
    #[serde(default)]
    pub route: Route,
    /// "Warn before broadcast or multi-host snippets" (`decisions.md`).
    #[serde(default)]
    pub warn: bool,
}

impl Group {
    pub fn new(name: impl Into<String>) -> Self {
        Self {
            id: GroupId::new(),
            name: name.into(),
            color: GroupColor::default(),
            key: GroupKey::default(),
            jump_host: None,
            route: Route::default(),
            warn: false,
        }
    }

    /// The name to show; "Untitled group" when empty.
    pub fn display_name(&self) -> &str {
        let name = self.name.trim();
        if name.is_empty() { "Untitled group" } else { name }
    }
}

/// Which of a host's fields come from its group ("from group" on the host
/// form). A host keeps its own value for each field as well, which comes
/// back when the field stops following the group.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct FromGroup {
    pub route: bool,
    pub jump_host: bool,
    /// The sign-in key. Off for new hosts, since a host without a group has
    /// no default key; an identity always takes precedence.
    pub key: bool,
}

impl Default for FromGroup {
    fn default() -> Self {
        Self { route: true, jump_host: true, key: false }
    }
}
