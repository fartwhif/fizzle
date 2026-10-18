//! Server/account profile persistence.
//!
//! Stored as JSON at `$ACCLIENT_PROFILES` or `~/.acclient-rs/profiles.json`:
//! a list of known servers, a list of accounts, and the active pair.

use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct Server {
    pub name: String,
    pub host: String,
}

#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct Account {
    pub name: String,
    pub password: String,
    /// Character to auto-enter for this account (persisted on entry).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub character: Option<String>,
    /// Server name to connect to for this account (persisted on connect).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub server: Option<String>,
}

#[derive(Clone, Debug, Default, Serialize, Deserialize)]
pub struct Profiles {
    #[serde(default)]
    pub servers: Vec<Server>,
    #[serde(default)]
    pub accounts: Vec<Account>,
    /// Index into `servers`; `None` until a connection succeeds.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub active_server: Option<usize>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub active_account: Option<usize>,
    /// UI theme name: `custom` | `utf8` | `mono` | `green`.
    #[serde(default)]
    pub theme: String,
    /// In-world view: `3d` (software world renderer) or `tactical` (radar).
    #[serde(default)]
    pub world_view: String,
    /// Override the retail DAT directory (`$AC_DAT_DIR` / standard paths).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub dat_dir: Option<String>,
    /// Draw the 3D actor glow halos.
    #[serde(default)]
    pub glow_actors: bool,
    /// Animate the 3D target-reticle's breathing pulse.
    #[serde(default = "default_reticle_animated")]
    pub reticle_animation: bool,
}

/// Reticle animation is on by default.
fn default_reticle_animated() -> bool {
    true
}

impl Profiles {
    #[must_use]
    pub fn path() -> PathBuf {
        if let Ok(p) = std::env::var("ACCLIENT_PROFILES") {
            return PathBuf::from(p);
        }
        let home = std::env::var("HOME").unwrap_or_else(|_| ".".into());
        Path::new(&home).join(".acclient-rs").join("profiles.json")
    }

    pub fn load() -> Self {
        let p = Self::path();
        match std::fs::read_to_string(&p) {
            Ok(s) => serde_json::from_str(&s).unwrap_or_default(),
            Err(_) => Self::default(),
        }
    }

    pub fn save(&self) -> anyhow::Result<()> {
        let p = Self::path();
        if let Some(dir) = p.parent() {
            std::fs::create_dir_all(dir)?;
        }
        std::fs::write(&p, serde_json::to_string_pretty(self)?)?;
        Ok(())
    }

    /// Ensure a server is present; return its index.
    pub fn upsert_server(&mut self, name: &str, host: &str) -> usize {
        if let Some(i) = self.servers.iter().position(|s| s.name == name || s.host == host) {
            return i;
        }
        self.servers.push(Server { name: name.to_string(), host: host.to_string() });
        self.servers.len() - 1
    }

    /// Ensure an account is present; return its index.
    pub fn upsert_account(&mut self, name: &str, password: &str) -> usize {
        if let Some(i) = self.accounts.iter().position(|a| a.name == name) {
            return i;
        }
        self.accounts.push(Account { name: name.to_string(), password: password.to_string(), character: None, server: None });
        self.accounts.len() - 1
    }
}
