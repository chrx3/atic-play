//! The session in the keyring of the system.
//!
//! The session is a secret, so it goes to the keyring of the system (Windows Credential Manager,
//! macOS Keychain, Secret Service on Linux) and never to a file of ours.
//!
//! The keyring entry with the session is a small index that says how many pieces the secret has
//! and when it was saved; the secret itself goes in the pieces `youtube-music#1`,
//! `youtube-music#2`… because the Windows Credential Manager does not take more than about
//! 2.5 KB per secret.

use std::time::{Duration, SystemTime, UNIX_EPOCH};

use anyhow::{Context, anyhow, bail};
use keyring::v1::{Entry, Error as KeyringError};
use serde::{Deserialize, Serialize};

use crate::Session;
use crate::secret;

/// The name of the keyring service: every credential of Atic Play lives under it.
pub const SERVICE: &str = "atic-play";

/// The name of the credential with the session.
pub const USER: &str = "youtube-music";

/// How many pieces we look for when we clean the leftovers of a broken session.
const MAX_PARTS: usize = 32;

/// The version of the format of the index.
const FORMAT_VERSION: u32 = 1;

/// What the keyring entry with the session holds.
#[derive(Clone, Debug, Serialize, Deserialize)]
struct Index {
    version: u32,
    /// How many pieces the secret has.
    parts: usize,
    /// When the session was saved, in seconds since the epoch.
    saved_at: u64,
}

/// The session of an account, in one keyring service.
#[derive(Clone, Debug)]
pub struct Store {
    service: String,
    user: String,
}

impl Default for Store {
    fn default() -> Self {
        Self::new(SERVICE, USER)
    }
}

impl Store {
    /// A store with another service and user name (the tests use one of their own).
    pub fn new(service: impl Into<String>, user: impl Into<String>) -> Self {
        Self {
            service: service.into(),
            user: user.into(),
        }
    }

    /// The session saved in the keyring, if there is one.
    pub fn load(&self) -> anyhow::Result<Option<Session>> {
        let Some(index) = self.read_index()? else {
            return Ok(None);
        };

        let mut pieces = Vec::with_capacity(index.parts);
        for number in 1..=index.parts {
            let piece = self
                .entry(&self.part_user(number))?
                .get_password()
                .with_context(|| {
                    format!(
                        "the session in the keyring is not complete (piece {number} is missing)"
                    )
                })?;
            pieces.push(piece);
        }

        Ok(Some(Session {
            cookies: secret::join(&pieces),
            saved_at: UNIX_EPOCH + Duration::from_secs(index.saved_at),
        }))
    }

    /// Saves the session in the keyring.
    pub fn save(&self, session: &Session) -> anyhow::Result<()> {
        if session.cookies.trim().is_empty() {
            bail!("the session has no cookies");
        }

        let parts = secret::split(&session.cookies, secret::MAX_PART_LEN);
        if parts.is_empty() {
            bail!("the session has no cookies");
        }

        // How many pieces the old session had, to clean the leftovers later.
        let old_parts = self
            .read_index()
            .ok()
            .flatten()
            .map(|index| index.parts)
            .unwrap_or(0);

        // The pieces first and the index at the end: if a piece fails we keep the old session.
        for (number, part) in parts.iter().enumerate() {
            self.entry(&self.part_user(number + 1))?
                .set_password(part)
                .context("could not write the session in the keyring")?;
        }

        let index = Index {
            version: FORMAT_VERSION,
            parts: parts.len(),
            saved_at: to_unix_seconds(session.saved_at),
        };
        let record =
            serde_json::to_string(&index).context("could not write the session in the keyring")?;
        self.entry(&self.user)?
            .set_password(&record)
            .context("could not write the session in the keyring")?;

        // The pieces that the old session had and this one does not are not needed anymore.
        for number in parts.len() + 1..=old_parts.min(MAX_PARTS) {
            let _ = self.entry(&self.part_user(number))?.delete_credential();
        }

        Ok(())
    }

    /// Forgets the session.
    pub fn clear(&self) -> anyhow::Result<()> {
        let parts = self
            .read_index()
            .ok()
            .flatten()
            .map(|index| index.parts)
            .unwrap_or(0);

        match self.entry(&self.user)?.delete_credential() {
            Ok(()) => {}
            Err(KeyringError::NoEntry) => {}
            Err(err) => return Err(anyhow!("could not forget the session: {err}")),
        }

        // The pieces of the session, and the ones a broken session may have left behind.
        for number in 1..=parts.clamp(1, MAX_PARTS) {
            match self.entry(&self.part_user(number))?.delete_credential() {
                Ok(()) => {}
                Err(KeyringError::NoEntry) => {}
                Err(err) => return Err(anyhow!("could not forget the session: {err}")),
            }
        }

        Ok(())
    }

    /// The index of the session: how many pieces it has and when it was saved.
    fn read_index(&self) -> anyhow::Result<Option<Index>> {
        let raw = match self.entry(&self.user)?.get_password() {
            Ok(raw) => raw,
            Err(KeyringError::NoEntry) => return Ok(None),
            Err(err) => {
                return Err(anyhow!(
                    "could not read the session from the keyring: {err}"
                ));
            }
        };

        let index: Index =
            serde_json::from_str(&raw).context("the session in the keyring is damaged")?;
        Ok(Some(index))
    }

    /// The name of the credential with a piece of the secret.
    fn part_user(&self, number: usize) -> String {
        format!("{}#{number}", self.user)
    }

    /// One credential of the keyring.
    fn entry(&self, user: &str) -> anyhow::Result<Entry> {
        Entry::new(&self.service, user)
            .map_err(|err| anyhow!("the keyring of the system is not available: {err}"))
    }
}

/// Seconds since the epoch; times before it become zero.
fn to_unix_seconds(time: SystemTime) -> u64 {
    time.duration_since(UNIX_EPOCH)
        .map(|since| since.as_secs())
        .unwrap_or(0)
}
