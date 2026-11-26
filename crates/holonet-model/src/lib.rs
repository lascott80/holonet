//! Holonet's data: hosts, stored keys, known hosts and settings.
//!
//! Plain data with serde, no I/O. The vault (`holonet-vault`) encrypts and
//! stores a [`VaultData`]; everything else reads and edits it.
//!
//! Fields for later milestones (groups, snippets, logging) are added when
//! their milestone lands. `#[serde(default)]` on every
//! collection and optional field keeps older vault files readable.

use std::fmt;

use serde::{Deserialize, Serialize};
use uuid::Uuid;
use zeroize::Zeroizing;

/// Schema version of [`VaultData`]. Bump when a change needs a migration.
///
/// 2 (milestone 4): key passphrases are saved on the key, not on each host.
/// [`VaultData::migrate`] moves them.
pub const SCHEMA_VERSION: u32 = 2;

/// Seconds since the Unix epoch, UTC.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(transparent)]
pub struct Timestamp(pub i64);

impl Timestamp {
    pub fn now() -> Self {
        let secs = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_secs() as i64)
            .unwrap_or_default();
        Self(secs)
    }
}

/// A secret string (password or passphrase). Wiped from memory on drop and
/// never printed by `Debug`.
#[derive(Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(transparent)]
pub struct Secret(Zeroizing<String>);

impl Secret {
    pub fn new(value: impl Into<String>) -> Self {
        Self(Zeroizing::new(value.into()))
    }

    pub fn expose(&self) -> &str {
        &self.0
    }

    pub fn is_empty(&self) -> bool {
        self.0.is_empty()
    }
}

impl fmt::Debug for Secret {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("Secret(<redacted>)")
    }
}

macro_rules! id_type {
    ($name:ident) => {
        #[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize)]
        #[serde(transparent)]
        pub struct $name(pub Uuid);

        impl $name {
            pub fn new() -> Self {
                Self(Uuid::new_v4())
            }
        }

        impl Default for $name {
            fn default() -> Self {
                Self::new()
            }
        }
    };
}

id_type!(HostId);
id_type!(KeyId);
id_type!(IdentityId);

/// Icon shown for a host. Chosen by the person; defaults to a server.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum RoleIcon {
    #[default]
    Server,
    Database,
    Shield,
    Nas,
    Chip,
    Cloud,
}

/// How a host signs in.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "method", rename_all = "snake_case")]
pub enum Auth {
    /// Password sign-in. `None` means ask on every connect.
    Password {
        #[serde(default)]
        saved: Option<Secret>,
    },
    /// A key stored in the vault. If the key has a passphrase and none is
    /// saved on the key ([`StoredKey::saved_passphrase`]), it is asked for on
    /// connect.
    Key {
        key_id: KeyId,
        /// Where schema 1 saved the passphrase. Read so [`VaultData::migrate`]
        /// can move it to the key; never written.
        #[serde(default, rename = "passphrase", skip_serializing)]
        legacy_passphrase: Option<Secret>,
    },
}

impl Default for Auth {
    fn default() -> Self {
        Auth::Password { saved: None }
    }
}

impl Auth {
    pub fn key(key_id: KeyId) -> Self {
        Auth::Key { key_id, legacy_passphrase: None }
    }

    pub fn key_id(&self) -> Option<KeyId> {
        match self {
            Auth::Key { key_id, .. } => Some(*key_id),
            Auth::Password { .. } => None,
        }
    }
}

/// Which address a host connects to (`decisions.md`, NewHost.dc.html
/// "Connect using").
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Route {
    /// The primary address first, then Tailscale after the fallback wait.
    #[default]
    Automatic,
    Primary,
    Tailscale,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Host {
    pub id: HostId,
    /// Shown name; empty means "use the address".
    #[serde(default)]
    pub label: String,
    /// Hostname or IP address. Required.
    pub address: String,
    #[serde(default = "default_port")]
    pub port: u16,
    #[serde(default)]
    pub username: String,
    #[serde(default)]
    pub auth: Auth,
    /// A shared sign-in. When set, it replaces `username` and `auth`.
    #[serde(default)]
    pub identity: Option<IdentityId>,
    /// A 100.x address or tailnet name, if the host is on the tailnet.
    #[serde(default)]
    pub tailscale_address: Option<String>,
    #[serde(default)]
    pub route: Route,
    /// A saved host to connect through.
    #[serde(default)]
    pub jump_host: Option<HostId>,
    #[serde(default = "yes")]
    pub keep_alive: bool,
    #[serde(default)]
    pub tags: Vec<String>,
    #[serde(default)]
    pub icon: RoleIcon,
    pub created_at: Timestamp,
    #[serde(default)]
    pub last_connected_at: Option<Timestamp>,
}

fn default_port() -> u16 {
    22
}

fn yes() -> bool {
    true
}

/// A problem with a field on the host form.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum HostProblem {
    AddressMissing,
    AddressInvalid,
    PortZero,
    TailscaleAddressInvalid,
}

impl HostProblem {
    /// Interface text for the problem, in the design's voice.
    pub fn message(self) -> &'static str {
        match self {
            HostProblem::AddressMissing => "Add an address to test or save this host.",
            HostProblem::AddressInvalid => "Use a hostname or an IP address, without spaces or a user name.",
            HostProblem::PortZero => "Use a port from 1 to 65535.",
            HostProblem::TailscaleAddressInvalid => {
                "Use a 100.x address or a tailnet name, such as name.tailnet.ts.net."
            }
        }
    }
}

impl Host {
    pub fn new(address: impl Into<String>) -> Self {
        Self {
            id: HostId::new(),
            label: String::new(),
            address: address.into(),
            port: 22,
            username: String::new(),
            auth: Auth::default(),
            identity: None,
            tailscale_address: None,
            route: Route::default(),
            jump_host: None,
            keep_alive: true,
            tags: Vec::new(),
            icon: RoleIcon::default(),
            created_at: Timestamp::now(),
            last_connected_at: None,
        }
    }

    /// Name to show: the label, or the address when there is none.
    pub fn display_name(&self) -> &str {
        let label = self.label.trim();
        if label.is_empty() { self.address.trim() } else { label }
    }

    /// `user@address:port` as shown on cards, with `:22` left out.
    pub fn target(&self) -> String {
        let user = if self.username.is_empty() { String::new() } else { format!("{}@", self.username) };
        let port = if self.port == 22 { String::new() } else { format!(":{}", self.port) };
        format!("{user}{}{port}", self.address)
    }

    /// The Tailscale address, trimmed, if one is set.
    pub fn tailscale(&self) -> Option<&str> {
        self.tailscale_address.as_deref().map(str::trim).filter(|a| !a.is_empty())
    }

    /// The route actually used: Primary when there is no Tailscale address
    /// (NewHost.dc.html forces it).
    pub fn effective_route(&self) -> Route {
        if self.tailscale().is_none() { Route::Primary } else { self.route }
    }

    /// The equivalent `ssh` command shown on the host form.
    pub fn ssh_command(&self) -> String {
        self.ssh_command_via(None)
    }

    /// The equivalent command through a jump host, named by its label as
    /// drawn (`ssh -J bastion-east deploy@10.30.0.9`). The target is the
    /// Tailscale address only when the route is Tailscale.
    pub fn ssh_command_via(&self, jump: Option<&str>) -> String {
        let address = match (self.effective_route(), self.tailscale()) {
            (Route::Tailscale, Some(t)) => t,
            _ if self.address.trim().is_empty() => "[address]",
            _ => self.address.trim(),
        };
        let jump = jump.map(|j| format!("-J {j} ")).unwrap_or_default();
        let user = if self.username.is_empty() { String::new() } else { format!("{}@", self.username) };
        let port = if self.port == 22 { String::new() } else { format!(" -p {}", self.port) };
        format!("ssh {jump}{user}{address}{port}")
    }

    /// Problems that stop the host from being saved, in form order.
    pub fn problems(&self) -> Vec<HostProblem> {
        let mut problems = Vec::new();
        let address = self.address.trim();
        if address.is_empty() {
            problems.push(HostProblem::AddressMissing);
        } else if !is_valid_address(address) {
            problems.push(HostProblem::AddressInvalid);
        }
        if self.port == 0 {
            problems.push(HostProblem::PortZero);
        }
        if let Some(t) = self.tailscale()
            && !is_valid_address(t)
        {
            problems.push(HostProblem::TailscaleAddressInvalid);
        }
        problems
    }

    /// Copy with a new id and "copy" label, for Duplicate.
    pub fn duplicate(&self) -> Self {
        let mut copy = self.clone();
        copy.id = HostId::new();
        copy.label = format!("{} copy", self.display_name());
        copy.created_at = Timestamp::now();
        copy.last_connected_at = None;
        copy
    }
}

/// Hostname, IPv4, or IPv6 (optionally in brackets). Deliberately loose: it
/// rejects obvious mistakes (spaces, `user@`, `:port`, URLs), not unusual names.
pub fn is_valid_address(address: &str) -> bool {
    let a = address.strip_prefix('[').and_then(|a| a.strip_suffix(']')).unwrap_or(address);
    if a.parse::<std::net::IpAddr>().is_ok() {
        return true;
    }
    !a.is_empty()
        && a.len() <= 253
        && a.split('.').all(|label| {
            !label.is_empty()
                && label.len() <= 63
                && label.chars().all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '_')
                && !label.starts_with('-')
                && !label.ends_with('-')
        })
}

/// A named sign-in shared by several hosts (Keychain.dc.html, Identities).
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Identity {
    pub id: IdentityId,
    /// Shown name, e.g. "deploy" or "root (client-acme)".
    pub name: String,
    pub username: String,
    pub auth: Auth,
}

/// A private key copied into the vault.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct StoredKey {
    pub id: KeyId,
    /// Shown name, e.g. the file name "id_ed25519".
    pub name: String,
    /// e.g. "ssh-ed25519", "rsa-sha2-512", "ecdsa-sha2-nistp256".
    pub algorithm: String,
    /// The OpenSSH private key exactly as imported. If it had a passphrase it
    /// is still encrypted with it (owner decision, milestone 2 plan).
    pub private_key: Secret,
    pub encrypted: bool,
    /// OpenSSH public key line, e.g. "ssh-ed25519 AAAA... comment".
    pub public_key: String,
    /// The key's comment, e.g. "holonet@work-laptop". Shown with the public key.
    #[serde(default)]
    pub comment: String,
    /// The passphrase, when the person chose to save it in the vault.
    #[serde(default)]
    pub saved_passphrase: Option<Secret>,
    /// Where it was imported from, for the person's reference.
    #[serde(default)]
    pub source: Option<String>,
    pub imported_at: Timestamp,
}

/// Where a trusted host key came from.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum KnownHostSource {
    FirstConnection,
    ImportedKnownHosts,
}

/// A different key a server presented, recorded when the connection was stopped.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Mismatch {
    pub presented_key: String,
    pub seen_at: Timestamp,
}

/// A trusted server key.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct KnownHost {
    /// Address as connected to (hostname or IP), lowercased.
    pub host: String,
    pub port: u16,
    /// OpenSSH public key line without comment, e.g. "ssh-ed25519 AAAA...".
    pub public_key: String,
    pub trusted_at: Timestamp,
    #[serde(default)]
    pub last_matched_at: Option<Timestamp>,
    pub source: KnownHostSource,
    #[serde(default)]
    pub mismatch: Option<Mismatch>,
}

impl KnownHost {
    /// The key type token, e.g. "ssh-ed25519".
    pub fn key_type(&self) -> &str {
        self.public_key.split_whitespace().next().unwrap_or("")
    }

    /// Short label for the key type as the design shows it: ED25519, ECDSA, RSA.
    pub fn key_type_label(&self) -> &'static str {
        key_type_label(self.key_type())
    }

    pub fn matches(&self, host: &str, port: u16) -> bool {
        self.port == port && self.host.eq_ignore_ascii_case(host)
    }
}

pub fn key_type_label(key_type: &str) -> &'static str {
    match key_type {
        t if t.contains("ed25519") => "ED25519",
        t if t.contains("ecdsa") => "ECDSA",
        t if t.contains("rsa") => "RSA",
        t if t.contains("dss") => "DSA",
        _ => "Unknown",
    }
}

/// What to do when a server presents a key that is not in Known hosts.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum UnknownHostKeys {
    #[default]
    Ask,
    Refuse,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Settings {
    #[serde(default)]
    pub unknown_host_keys: UnknownHostKeys,
    /// Seconds before an unanswered connection attempt fails.
    #[serde(default = "default_connect_timeout")]
    pub connect_timeout_secs: u32,
    /// Minutes idle before the vault locks; `None` means never.
    /// Enforced from milestone 4.
    #[serde(default = "default_lock_after")]
    pub lock_after_idle_mins: Option<u32>,
    /// Lock when Windows locks (`decisions.md`).
    #[serde(default = "yes")]
    pub lock_when_windows_locks: bool,
    /// Unlock with Windows Hello. On only once a Hello key slot exists.
    #[serde(default)]
    pub hello_unlock: bool,
    #[serde(default)]
    pub terminal: TerminalSettings,
    #[serde(default)]
    pub connection: ConnectionSettings,
}

fn default_connect_timeout() -> u32 {
    10
}

fn default_lock_after() -> Option<u32> {
    Some(15)
}

impl Default for Settings {
    fn default() -> Self {
        Self {
            unknown_host_keys: UnknownHostKeys::Ask,
            connect_timeout_secs: default_connect_timeout(),
            lock_after_idle_mins: default_lock_after(),
            lock_when_windows_locks: true,
            hello_unlock: false,
            terminal: TerminalSettings::default(),
            connection: ConnectionSettings::default(),
        }
    }
}

/// A font build bundled with Holonet (JetBrainsMono Nerd Font v3.5.1).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum BundledFont {
    /// Ligatures and double-width icon glyphs (the design's default).
    #[default]
    NerdFont,
    /// Icons fit a single cell.
    NerdFontMono,
    /// Proportional icon widths.
    NerdFontPropo,
    /// Ligatures removed.
    NerdFontNl,
}

impl BundledFont {
    pub const ALL: [BundledFont; 4] = [Self::NerdFont, Self::NerdFontMono, Self::NerdFontPropo, Self::NerdFontNl];

    /// Family name the font files declare.
    pub fn family(self) -> &'static str {
        match self {
            Self::NerdFont => "JetBrainsMono Nerd Font",
            Self::NerdFontMono => "JetBrainsMono Nerd Font Mono",
            Self::NerdFontPropo => "JetBrainsMono Nerd Font Propo",
            Self::NerdFontNl => "JetBrainsMonoNL Nerd Font",
        }
    }

    pub fn has_ligatures(self) -> bool {
        self != Self::NerdFontNl
    }
}

/// The terminal font: a bundled build or an installed family.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case", tag = "kind", content = "name")]
pub enum TerminalFont {
    Bundled(BundledFont),
    Installed(String),
}

impl Default for TerminalFont {
    fn default() -> Self {
        Self::Bundled(BundledFont::default())
    }
}

impl TerminalFont {
    pub fn family(&self) -> &str {
        match self {
            Self::Bundled(b) => b.family(),
            Self::Installed(name) => name,
        }
    }

    /// Whether the ligature setting applies. Installed fonts are assumed to
    /// have them; a font without ligatures simply shows none.
    pub fn supports_ligatures(&self) -> bool {
        match self {
            Self::Bundled(b) => b.has_ligatures(),
            Self::Installed(_) => true,
        }
    }
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum CursorShape {
    #[default]
    Block,
    Bar,
    Underline,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ColorScheme {
    #[default]
    HolonetDark,
    HighContrast,
    Light,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Scrollback {
    #[default]
    Lines10k,
    Lines50k,
    Unlimited,
}

impl Scrollback {
    /// Lines of history; `None` for unlimited.
    pub fn lines(self) -> Option<usize> {
        match self {
            Self::Lines10k => Some(10_000),
            Self::Lines50k => Some(50_000),
            Self::Unlimited => None,
        }
    }
}

pub const FONT_SIZE_RANGE: std::ops::RangeInclusive<u8> = 10..=20;
/// Line height in tenths: 1.0 to 1.8.
pub const LINE_HEIGHT_RANGE: std::ops::RangeInclusive<u8> = 10..=18;

/// Settings, Terminal (Settings.dc.html).
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct TerminalSettings {
    pub font: TerminalFont,
    /// Pixels, 10 to 20.
    pub font_size: u8,
    /// Line height in tenths of the font size, 10 to 18 (1.6 is 16).
    pub line_height_tenths: u8,
    pub ligatures: bool,
    pub cursor: CursorShape,
    pub cursor_blink: bool,
    pub scheme: ColorScheme,
    pub scrollback: Scrollback,
    pub copy_on_select: bool,
    pub right_click_pastes: bool,
}

impl Default for TerminalSettings {
    fn default() -> Self {
        Self {
            font: TerminalFont::default(),
            font_size: 13,
            line_height_tenths: 16,
            ligatures: true,
            cursor: CursorShape::Block,
            cursor_blink: true,
            scheme: ColorScheme::HolonetDark,
            scrollback: Scrollback::Lines10k,
            copy_on_select: false,
            right_click_pastes: true,
        }
    }
}

impl TerminalSettings {
    /// Font size, clamped to the allowed range.
    pub fn font_size(&self) -> f32 {
        self.font_size.clamp(*FONT_SIZE_RANGE.start(), *FONT_SIZE_RANGE.end()) as f32
    }

    /// Line height as a multiple of the font size, clamped to 1.0..=1.8.
    pub fn line_height(&self) -> f32 {
        self.line_height_tenths.clamp(*LINE_HEIGHT_RANGE.start(), *LINE_HEIGHT_RANGE.end()) as f32 / 10.0
    }

    /// Whether ligatures are drawn: on, and the font has them.
    pub fn ligatures_on(&self) -> bool {
        self.ligatures && self.font.supports_ligatures()
    }
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum KeepAlive {
    #[default]
    Secs30,
    Secs60,
    Off,
}

impl KeepAlive {
    pub fn interval(self) -> Option<std::time::Duration> {
        match self {
            Self::Secs30 => Some(std::time::Duration::from_secs(30)),
            Self::Secs60 => Some(std::time::Duration::from_secs(60)),
            Self::Off => None,
        }
    }
}

/// How long Automatic waits on the primary address before trying
/// Tailscale (`decisions.md`: 3 seconds, adjustable).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum FallbackWait {
    Secs2,
    #[default]
    Secs3,
    Secs5,
    Secs10,
}

impl FallbackWait {
    pub fn duration(self) -> std::time::Duration {
        std::time::Duration::from_secs(match self {
            Self::Secs2 => 2,
            Self::Secs3 => 3,
            Self::Secs5 => 5,
            Self::Secs10 => 10,
        })
    }
}

/// Settings, Connection (Settings.dc.html).
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct ConnectionSettings {
    pub keep_alive: KeepAlive,
    /// Reconnect after sleep, a network change or a dropped connection
    /// (`decisions.md`: on, 5 attempts).
    pub reconnect: bool,
    pub fallback_wait: FallbackWait,
}

impl Default for ConnectionSettings {
    fn default() -> Self {
        Self { keep_alive: KeepAlive::Secs30, reconnect: true, fallback_wait: FallbackWait::default() }
    }
}

impl ConnectionSettings {
    /// Keep-alive interval for a host: the global setting, unless the host
    /// turned keep-alive off.
    pub fn keep_alive_for(&self, host: &Host) -> Option<std::time::Duration> {
        if host.keep_alive { self.keep_alive.interval() } else { None }
    }
}

/// Everything the vault stores.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct VaultData {
    pub schema: u32,
    #[serde(default)]
    pub hosts: Vec<Host>,
    #[serde(default)]
    pub keys: Vec<StoredKey>,
    #[serde(default)]
    pub identities: Vec<Identity>,
    #[serde(default)]
    pub known_hosts: Vec<KnownHost>,
    #[serde(default)]
    pub settings: Settings,
}

impl Default for VaultData {
    fn default() -> Self {
        Self {
            schema: SCHEMA_VERSION,
            hosts: Vec::new(),
            keys: Vec::new(),
            identities: Vec::new(),
            known_hosts: Vec::new(),
            settings: Settings::default(),
        }
    }
}

/// Why a host's jump host cannot be used.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum JumpProblem {
    /// The chain leads back to a host already in it.
    Loop,
    /// A jump host in the chain was deleted.
    Missing,
}

impl JumpProblem {
    pub fn message(self) -> &'static str {
        match self {
            JumpProblem::Loop => "This jump host leads back to this host. Choose another, or None.",
            JumpProblem::Missing => "A jump host on the way was deleted. Choose another, or None.",
        }
    }
}

impl VaultData {
    pub fn host(&self, id: HostId) -> Option<&Host> {
        self.hosts.iter().find(|h| h.id == id)
    }

    pub fn host_mut(&mut self, id: HostId) -> Option<&mut Host> {
        self.hosts.iter_mut().find(|h| h.id == id)
    }

    pub fn key(&self, id: KeyId) -> Option<&StoredKey> {
        self.keys.iter().find(|k| k.id == id)
    }

    pub fn key_mut(&mut self, id: KeyId) -> Option<&mut StoredKey> {
        self.keys.iter_mut().find(|k| k.id == id)
    }

    pub fn identity(&self, id: IdentityId) -> Option<&Identity> {
        self.identities.iter().find(|i| i.id == id)
    }

    /// The username and sign-in a host uses: its identity's when it has one
    /// that still exists, otherwise its own.
    pub fn sign_in<'a>(&'a self, host: &'a Host) -> (&'a str, &'a Auth) {
        match host.identity.and_then(|id| self.identity(id)) {
            Some(identity) => (identity.username.as_str(), &identity.auth),
            None => (host.username.as_str(), &host.auth),
        }
    }

    /// A copy of `host` with its identity's username and sign-in in place
    /// of its own, for connecting and for showing. `identity` stays set, so
    /// callers can still say where the sign-in comes from.
    pub fn resolved(&self, host: &Host) -> Host {
        let mut resolved = host.clone();
        let (user, auth) = self.sign_in(host);
        resolved.username = user.to_string();
        resolved.auth = auth.clone();
        resolved
    }

    /// Where a host's sign-in is stored: its identity's when it has one that
    /// still exists, otherwise its own. For saving a password typed at a
    /// prompt.
    pub fn sign_in_auth_mut(&mut self, host: HostId) -> Option<&mut Auth> {
        let identity = self.host(host)?.identity.filter(|id| self.identity(*id).is_some());
        match identity {
            Some(id) => self.identities.iter_mut().find(|i| i.id == id).map(|i| &mut i.auth),
            None => self.host_mut(host).map(|h| &mut h.auth),
        }
    }

    /// Hosts that use an identity.
    pub fn identity_users(&self, identity: IdentityId) -> Vec<HostId> {
        self.hosts.iter().filter(|h| h.identity == Some(identity)).map(|h| h.id).collect()
    }

    /// Delete an identity. Hosts that used it keep a copy of its username
    /// and sign-in as their own, so they still connect.
    pub fn delete_identity(&mut self, identity: IdentityId) -> Option<Identity> {
        let index = self.identities.iter().position(|i| i.id == identity)?;
        let removed = self.identities.remove(index);
        for host in self.hosts.iter_mut().filter(|h| h.identity == Some(identity)) {
            host.identity = None;
            host.username = removed.username.clone();
            host.auth = removed.auth.clone();
        }
        Some(removed)
    }

    /// Hosts and identities that sign in with a key. A host counts when it
    /// uses the key itself or through its identity.
    pub fn key_users(&self, key_id: KeyId) -> (Vec<HostId>, Vec<IdentityId>) {
        let hosts = self.hosts.iter().filter(|h| self.sign_in(h).1.key_id() == Some(key_id)).map(|h| h.id).collect();
        let identities = self.identities.iter().filter(|i| i.auth.key_id() == Some(key_id)).map(|i| i.id).collect();
        (hosts, identities)
    }

    /// The hops to reach `host`, first hop first, not including `host`
    /// itself. `host` need not be saved yet (the host form checks before
    /// saving), so its own id counts as a loop.
    pub fn jump_chain(&self, host: &Host) -> Result<Vec<HostId>, JumpProblem> {
        let mut chain = Vec::new();
        let mut next = host.jump_host;
        while let Some(id) = next {
            if id == host.id || chain.contains(&id) {
                return Err(JumpProblem::Loop);
            }
            let hop = self.host(id).ok_or(JumpProblem::Missing)?;
            chain.push(id);
            next = hop.jump_host;
        }
        chain.reverse();
        Ok(chain)
    }

    /// Bring data from an older schema up to date. Idempotent.
    ///
    /// Schema 1 to 2: a key passphrase saved on a host (or identity) moves to
    /// the key, unless the key already has one.
    pub fn migrate(&mut self) {
        let mut moved: Vec<(KeyId, Secret)> = Vec::new();
        let auths = self.hosts.iter_mut().map(|h| &mut h.auth).chain(self.identities.iter_mut().map(|i| &mut i.auth));
        for auth in auths {
            if let Auth::Key { key_id, legacy_passphrase } = auth
                && let Some(secret) = legacy_passphrase.take()
            {
                moved.push((*key_id, secret));
            }
        }
        for (key_id, secret) in moved {
            if let Some(key) = self.key_mut(key_id)
                && key.saved_passphrase.is_none()
            {
                key.saved_passphrase = Some(secret);
            }
        }
        self.schema = SCHEMA_VERSION;
    }

    pub fn known_host(&self, host: &str, port: u16) -> Option<&KnownHost> {
        self.known_hosts.iter().find(|k| k.matches(host, port))
    }

    /// The trusted key for `host:port`, or else for one of the same saved
    /// host's other addresses on that port (owner decision, milestone 4
    /// phase 6: a host's primary and Tailscale addresses share trust when
    /// the key matches). The flag says it came from another address.
    pub fn known_host_or_alias(&self, host: &str, port: u16, aliases: &[String]) -> Option<(&KnownHost, bool)> {
        if let Some(known) = self.known_host(host, port) {
            return Some((known, false));
        }
        aliases.iter().find_map(|a| self.known_host(a.trim(), port)).map(|k| (k, true))
    }

    /// A host's addresses other than `connected`: its primary and
    /// Tailscale addresses, trimmed.
    pub fn other_addresses(host: &Host, connected: &str) -> Vec<String> {
        [Some(host.address.trim()), host.tailscale()]
            .into_iter()
            .flatten()
            .filter(|a| !a.is_empty() && !a.eq_ignore_ascii_case(connected.trim()))
            .map(str::to_string)
            .collect()
    }

    pub fn known_host_mut(&mut self, host: &str, port: u16) -> Option<&mut KnownHost> {
        self.known_hosts.iter_mut().find(|k| k.matches(host, port))
    }

    /// Saved hosts that connect to this address and port.
    pub fn hosts_for(&self, host: &str, port: u16) -> impl Iterator<Item = &Host> {
        self.hosts.iter().filter(move |h| h.port == port && h.address.trim().eq_ignore_ascii_case(host))
    }

    /// Trust a key for `host:port`, replacing any key trusted before.
    pub fn trust_host_key(&mut self, host: &str, port: u16, public_key: &str, source: KnownHostSource) {
        let entry = KnownHost {
            host: host.to_ascii_lowercase(),
            port,
            public_key: public_key.to_string(),
            trusted_at: Timestamp::now(),
            last_matched_at: Some(Timestamp::now()),
            source,
            mismatch: None,
        };
        match self.known_host_mut(host, port) {
            Some(existing) => *existing = entry,
            None => self.known_hosts.push(entry),
        }
    }

    /// Remove a trusted key. Returns whether one was removed.
    pub fn forget_host_key(&mut self, host: &str, port: u16) -> bool {
        let before = self.known_hosts.len();
        self.known_hosts.retain(|k| !k.matches(host, port));
        self.known_hosts.len() != before
    }

    /// Delete a host. Its server key is kept unless `forget_key` is set and
    /// no other saved host uses the same address and port (decisions.md).
    pub fn delete_host(&mut self, id: HostId, forget_key: bool) -> Option<Host> {
        let index = self.hosts.iter().position(|h| h.id == id)?;
        let host = self.hosts.remove(index);
        if forget_key && self.hosts_for(host.address.trim(), host.port).next().is_none() {
            self.forget_host_key(host.address.trim(), host.port);
        }
        Some(host)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn display_name_and_target() {
        let mut h = Host::new("10.0.0.5");
        assert_eq!(h.display_name(), "10.0.0.5");
        assert_eq!(h.target(), "10.0.0.5");
        h.label = "nas-01".into();
        h.username = "admin".into();
        h.port = 2222;
        assert_eq!(h.display_name(), "nas-01");
        assert_eq!(h.target(), "admin@10.0.0.5:2222");
        assert_eq!(h.ssh_command(), "ssh admin@10.0.0.5 -p 2222");
        assert_eq!(Host::new("").ssh_command(), "ssh [address]");
    }

    #[test]
    fn address_validation() {
        for ok in ["nas-01", "pi.local", "10.0.0.5", "::1", "[fe80::1]", "a_b.example.com"] {
            assert!(is_valid_address(ok), "{ok} should be valid");
        }
        for bad in ["root@host", "host:22", "has space", "http://x", "-bad", "a..b"] {
            assert!(!is_valid_address(bad), "{bad} should be invalid");
        }
        assert_eq!(Host::new(" ").problems(), vec![HostProblem::AddressMissing]);
        let mut h = Host::new("ok");
        h.port = 0;
        assert_eq!(h.problems(), vec![HostProblem::PortZero]);
    }

    #[test]
    fn secrets_are_redacted_in_debug() {
        let auth = Auth::Password { saved: Some(Secret::new("hunter2")) };
        assert!(!format!("{auth:?}").contains("hunter2"));
    }

    #[test]
    fn known_hosts_trust_replace_forget() {
        let mut d = VaultData::default();
        d.trust_host_key("Pi.Local", 22, "ssh-ed25519 AAAA1", KnownHostSource::FirstConnection);
        assert_eq!(d.known_host("pi.local", 22).unwrap().key_type_label(), "ED25519");
        d.trust_host_key("pi.local", 22, "ssh-ed25519 AAAA2", KnownHostSource::FirstConnection);
        assert_eq!(d.known_hosts.len(), 1);
        assert_eq!(d.known_host("pi.local", 22).unwrap().public_key, "ssh-ed25519 AAAA2");
        assert!(d.known_host("pi.local", 2222).is_none());
        assert!(d.forget_host_key("pi.local", 22));
        assert!(d.known_hosts.is_empty());
    }

    #[test]
    fn delete_host_keeps_key_unless_asked_and_unshared() {
        let mut d = VaultData::default();
        let a = Host::new("pi.local");
        let b = Host::new("pi.local");
        let (a_id, b_id) = (a.id, b.id);
        d.hosts.extend([a, b]);
        d.trust_host_key("pi.local", 22, "ssh-ed25519 K", KnownHostSource::FirstConnection);
        // Another host still uses the key, so forgetting is skipped.
        d.delete_host(a_id, true);
        assert!(d.known_host("pi.local", 22).is_some());
        // Default keeps the key.
        d.delete_host(b_id, false);
        assert!(d.known_host("pi.local", 22).is_some());
    }

    #[test]
    fn json_round_trip_and_old_files_load() {
        let mut d = VaultData::default();
        let mut h = Host::new("pi.local");
        h.auth = Auth::key(KeyId::new());
        h.tailscale_address = Some("100.64.0.21".into());
        h.route = Route::Tailscale;
        d.hosts.push(h);
        d.identities.push(Identity {
            id: IdentityId::new(),
            name: "deploy".into(),
            username: "deploy".into(),
            auth: Auth::Password { saved: Some(Secret::new("s")) },
        });
        let json = serde_json::to_string(&d).unwrap();
        assert_eq!(serde_json::from_str::<VaultData>(&json).unwrap(), d);
        // A minimal file with only required fields still loads with defaults.
        let old =
            r#"{"schema":1,"hosts":[{"id":"00000000-0000-0000-0000-000000000001","address":"x","created_at":0}]}"#;
        let d: VaultData = serde_json::from_str(old).unwrap();
        assert_eq!(d.hosts[0].port, 22);
        assert!(d.hosts[0].keep_alive);
        assert_eq!(d.settings.connect_timeout_secs, 10);
    }

    #[test]
    fn milestone_2_settings_load_with_terminal_defaults() {
        // Settings exactly as a milestone 2 vault wrote them.
        let old = r#"{"schema":1,"settings":{"unknown_host_keys":"refuse","connect_timeout_secs":7,"lock_after_idle_mins":15}}"#;
        let d: VaultData = serde_json::from_str(old).unwrap();
        assert_eq!(d.settings.unknown_host_keys, UnknownHostKeys::Refuse);
        assert_eq!(d.settings.connect_timeout_secs, 7);
        assert_eq!(d.settings.terminal, TerminalSettings::default());
        assert_eq!(d.settings.connection, ConnectionSettings::default());
        // Defaults follow Settings.dc.html and decisions.md.
        let t = &d.settings.terminal;
        assert_eq!(t.font.family(), "JetBrainsMono Nerd Font");
        assert_eq!((t.font_size(), t.line_height()), (13.0, 1.6));
        assert!(t.ligatures && t.cursor_blink && t.right_click_pastes && !t.copy_on_select);
        assert_eq!(t.scrollback.lines(), Some(10_000));
        assert_eq!(d.settings.connection.keep_alive.interval(), Some(std::time::Duration::from_secs(30)));
        assert!(d.settings.connection.reconnect);
    }

    #[test]
    fn terminal_settings_round_trip_partial_and_clamp() {
        let mut t = TerminalSettings {
            font: TerminalFont::Installed("Cascadia Mono".into()),
            scheme: ColorScheme::Light,
            ..Default::default()
        };
        let json = serde_json::to_string(&t).unwrap();
        assert_eq!(serde_json::from_str::<TerminalSettings>(&json).unwrap(), t);
        // Missing fields take their defaults; unknown enum spellings are rejected.
        let partial: TerminalSettings = serde_json::from_str(r#"{"font_size":16}"#).unwrap();
        assert_eq!(partial.font_size, 16);
        assert_eq!(partial.font, TerminalFont::Bundled(BundledFont::NerdFont));
        assert!(serde_json::from_str::<TerminalSettings>(r#"{"cursor":"beam"}"#).is_err());
        // Out-of-range values are clamped when read.
        t.font_size = 99;
        t.line_height_tenths = 2;
        assert_eq!((t.font_size(), t.line_height()), (20.0, 1.0));
        // The NL build has no ligatures whatever the switch says.
        t.font = TerminalFont::Bundled(BundledFont::NerdFontNl);
        assert!(t.ligatures && !t.ligatures_on());
    }

    #[test]
    fn keep_alive_respects_the_host_switch() {
        let c = ConnectionSettings { keep_alive: KeepAlive::Secs60, ..Default::default() };
        let mut h = Host::new("x");
        assert_eq!(c.keep_alive_for(&h), Some(std::time::Duration::from_secs(60)));
        h.keep_alive = false;
        assert_eq!(c.keep_alive_for(&h), None);
        let off = ConnectionSettings { keep_alive: KeepAlive::Off, ..c };
        h.keep_alive = true;
        assert_eq!(off.keep_alive_for(&h), None);
    }

    fn stored_key(name: &str) -> StoredKey {
        StoredKey {
            id: KeyId::new(),
            name: name.into(),
            algorithm: "ssh-ed25519".into(),
            private_key: Secret::new("-----BEGIN OPENSSH PRIVATE KEY-----"),
            encrypted: true,
            public_key: "ssh-ed25519 AAAA".into(),
            comment: String::new(),
            saved_passphrase: None,
            source: None,
            imported_at: Timestamp(0),
        }
    }

    #[test]
    fn milestone_3_vault_migrates_passphrases_to_keys() {
        // A host and key exactly as schema 1 wrote them, passphrase on the host.
        let key_id = "00000000-0000-0000-0000-0000000000aa";
        let old = format!(
            r#"{{"schema":1,
            "hosts":[{{"id":"00000000-0000-0000-0000-000000000001","address":"x","created_at":0,
                "auth":{{"method":"key","key_id":"{key_id}","passphrase":"hunter2"}}}},
                {{"id":"00000000-0000-0000-0000-000000000002","address":"y","created_at":0,
                "auth":{{"method":"key","key_id":"{key_id}","passphrase":"other"}}}}],
            "keys":[{{"id":"{key_id}","name":"id_ed25519","algorithm":"ssh-ed25519","private_key":"k",
                "encrypted":true,"public_key":"ssh-ed25519 AAAA","imported_at":0}}],
            "settings":{{"lock_after_idle_mins":15}}}}"#
        );
        let mut d: VaultData = serde_json::from_str(&old).unwrap();
        assert_eq!(d.schema, 1);
        d.migrate();
        assert_eq!(d.schema, SCHEMA_VERSION);
        // The first host's passphrase wins; neither host keeps one.
        assert_eq!(d.keys[0].saved_passphrase.as_ref().unwrap().expose(), "hunter2");
        assert!(d.hosts.iter().all(|h| matches!(h.auth, Auth::Key { legacy_passphrase: None, .. })));
        // New fields take their defaults.
        let h = &d.hosts[0];
        assert_eq!((h.route, h.tailscale(), h.jump_host, h.identity), (Route::Automatic, None, None, None));
        assert!(d.settings.lock_when_windows_locks && !d.settings.hello_unlock);
        assert_eq!(d.settings.connection.fallback_wait.duration(), std::time::Duration::from_secs(3));
        // Written back, the passphrase is only on the key.
        let json = serde_json::to_string(&d).unwrap();
        assert_eq!(json.matches("hunter2").count(), 1);
        assert!(!json.contains("other"));
        // Migrating again changes nothing.
        let before = d.clone();
        d.migrate();
        assert_eq!(d, before);
    }

    #[test]
    fn jump_chains_and_loops() {
        let mut d = VaultData::default();
        let mut bastion = Host::new("203.0.113.20");
        let mut inner = Host::new("10.30.0.2");
        inner.jump_host = Some(bastion.id);
        let mut app = Host::new("10.30.0.9");
        app.jump_host = Some(inner.id);
        assert_eq!(d.jump_chain(&bastion), Ok(vec![]));
        d.hosts.extend([bastion.clone(), inner.clone(), app.clone()]);
        assert_eq!(d.jump_chain(&app), Ok(vec![bastion.id, inner.id]));
        // Pointing the bastion at the app would close a loop.
        bastion.jump_host = Some(app.id);
        assert_eq!(d.jump_chain(&bastion), Err(JumpProblem::Loop));
        // A host as its own jump host.
        app.jump_host = Some(app.id);
        assert_eq!(d.jump_chain(&app), Err(JumpProblem::Loop));
        // A deleted hop.
        d.delete_host(inner.id, false);
        app.jump_host = Some(inner.id);
        assert_eq!(d.jump_chain(&app), Err(JumpProblem::Missing));
    }

    #[test]
    fn routes_and_equivalent_command() {
        let mut h = Host::new("10.30.0.9");
        h.username = "deploy".into();
        assert_eq!(h.effective_route(), Route::Primary, "no Tailscale address forces Primary");
        h.route = Route::Tailscale;
        assert_eq!(h.effective_route(), Route::Primary);
        h.tailscale_address = Some(" 100.64.0.21 ".into());
        assert_eq!(h.effective_route(), Route::Tailscale);
        assert_eq!(h.ssh_command(), "ssh deploy@100.64.0.21");
        h.route = Route::Automatic;
        assert_eq!(h.ssh_command_via(Some("bastion-east")), "ssh -J bastion-east deploy@10.30.0.9");
        h.tailscale_address = Some("not valid!".into());
        assert_eq!(h.problems(), vec![HostProblem::TailscaleAddressInvalid]);
        h.tailscale_address = Some("  ".into());
        assert!(h.problems().is_empty() && h.tailscale().is_none());
    }

    #[test]
    fn resolved_hosts_and_identity_deletion() {
        let mut d = VaultData::default();
        let identity = Identity {
            id: IdentityId::new(),
            name: "deploy".into(),
            username: "deploy".into(),
            auth: Auth::Password { saved: None },
        };
        let mut a = Host::new("a");
        a.identity = Some(identity.id);
        let mut b = Host::new("b");
        b.identity = Some(identity.id);
        let c = Host::new("c");
        d.identities.push(identity.clone());
        d.hosts.extend([a.clone(), b.clone(), c.clone()]);
        assert_eq!(d.resolved(&a).username, "deploy");
        assert_eq!(d.identity_users(identity.id), [a.id, b.id]);
        // A password typed for a host goes to its identity, shared by both.
        if let Some(Auth::Password { saved }) = d.sign_in_auth_mut(a.id) {
            *saved = Some(Secret::new("typed"));
        }
        assert!(matches!(d.resolved(&b).auth, Auth::Password { saved: Some(_) }));
        assert!(matches!(d.host(c.id).unwrap().auth, Auth::Password { saved: None }));
        // Deleting the identity leaves each host its own copy.
        d.delete_identity(identity.id).unwrap();
        let a_after = d.host(a.id).unwrap();
        assert_eq!((a_after.identity, a_after.username.as_str()), (None, "deploy"));
        assert!(matches!(a_after.auth, Auth::Password { saved: Some(_) }));
    }

    #[test]
    fn a_hosts_addresses_share_trust() {
        let mut d = VaultData::default();
        let mut h = Host::new("192.168.1.51");
        h.tailscale_address = Some("100.64.0.21".into());
        d.trust_host_key("192.168.1.51", 22, "ssh-ed25519 K", KnownHostSource::FirstConnection);
        let aliases = VaultData::other_addresses(&h, "100.64.0.21");
        assert_eq!(aliases, ["192.168.1.51"]);
        let (known, alias) = d.known_host_or_alias("100.64.0.21", 22, &aliases).unwrap();
        assert!(alias && known.public_key == "ssh-ed25519 K");
        // Its own entry wins once there is one.
        d.trust_host_key("100.64.0.21", 22, "ssh-ed25519 K", KnownHostSource::FirstConnection);
        assert!(!d.known_host_or_alias("100.64.0.21", 22, &aliases).unwrap().1);
        // Another port is another server.
        assert!(d.known_host_or_alias("100.64.0.21", 2222, &aliases).is_none());
    }

    #[test]
    fn identities_sign_in_and_key_users() {
        let mut d = VaultData::default();
        let key = stored_key("work-ed25519");
        let identity = Identity {
            id: IdentityId::new(),
            name: "deploy".into(),
            username: "deploy".into(),
            auth: Auth::key(key.id),
        };
        let mut a = Host::new("a");
        a.username = "root".into();
        a.identity = Some(identity.id);
        let mut b = Host::new("b");
        b.auth = Auth::key(key.id);
        let c = Host::new("c");
        d.keys.push(key.clone());
        d.identities.push(identity.clone());
        d.hosts.extend([a.clone(), b.clone(), c]);
        assert_eq!(d.sign_in(&a).0, "deploy");
        assert_eq!(d.sign_in(&b).0, "");
        assert_eq!(d.key_users(key.id), (vec![a.id, b.id], vec![identity.id]));
        // A host whose identity was deleted falls back to its own sign-in.
        d.identities.clear();
        assert_eq!(d.sign_in(&a).0, "root");
        assert_eq!(d.key_users(key.id), (vec![b.id], vec![]));
    }
}
