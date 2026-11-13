//! The Google account of Atic Play.
//!
//! The session of the account lives in the keyring of the system (Windows Credential Manager,
//! macOS Keychain, Secret Service on Linux) and never in a file of ours. The library reads and
//! writes it, and can open the login window that signs in with Google.
//!
//! It does not touch a GUI: [`run_login`] launches `atic-play-login`, a program that lives next to
//! the app, and waits for its answer.
//!
//! Everything is blocking: call it from a background thread, never from the UI thread.
//!
//! The app has to be built with the whole workspace (`cargo build --workspace`), so the login
//! window lands next to it.

mod login;
mod secret;
mod store;

pub mod cookies;

use std::path::PathBuf;
use std::time::SystemTime;

pub use cookies::Cookie;
pub use store::{SERVICE, Store, USER};

/// The session of an account: the cookies that say who the user is.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Session {
    /// The value of the `Cookie` header: `name=value; name=value`.
    pub cookies: String,
    /// When the session was saved.
    pub saved_at: SystemTime,
}

/// The session saved in the keyring, if there is one.
pub fn load() -> anyhow::Result<Option<Session>> {
    Store::default().load()
}

/// Saves the session in the keyring.
///
/// The secret is cut in pieces when it does not fit in one keyring entry.
pub fn save(session: &Session) -> anyhow::Result<()> {
    Store::default().save(session)
}

/// Forgets the session.
pub fn clear() -> anyhow::Result<()> {
    Store::default().clear()
}

/// Where the login window keeps the data of its web view (its cookies and its cache).
///
/// It is a directory of our own so the web view does not mix with the ones of other programs.
pub fn webview_data_dir() -> PathBuf {
    let base = if cfg!(windows) {
        std::env::var_os("LOCALAPPDATA").map(PathBuf::from)
    } else if cfg!(target_os = "macos") {
        std::env::var_os("HOME").map(|home| {
            PathBuf::from(home)
                .join("Library")
                .join("Application Support")
        })
    } else {
        std::env::var_os("XDG_DATA_HOME")
            .map(PathBuf::from)
            .or_else(|| {
                std::env::var_os("HOME")
                    .map(|home| PathBuf::from(home).join(".local").join("share"))
            })
    };

    base.unwrap_or_else(std::env::temp_dir)
        .join("atic-play")
        .join("webview")
}

/// Forgets the session and deletes the data of the login window.
///
/// On Windows the web view keeps its cookies in the folder of [`webview_data_dir`], so they go
/// away with it. On macOS the web view uses the store of the system, so its cookies stay there.
pub fn logout() -> anyhow::Result<()> {
    let dir = webview_data_dir();
    if dir.exists() {
        std::fs::remove_dir_all(&dir)
            .map_err(|err| anyhow::anyhow!("could not delete {}: {err}", dir.display()))?;
    }

    clear()
}

/// The path of the login window: it lives next to the app.
///
/// The app has to be built with the whole workspace, so this program is next to it.
pub fn login_binary_path() -> PathBuf {
    login::login_binary_path()
}

/// Opens the login window and waits for it.
///
/// `Ok(None)` when the user closed the window without signing in.
pub fn run_login() -> anyhow::Result<Option<Session>> {
    login::run_login()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_login_window_lives_next_to_the_app() {
        let path = login_binary_path();
        let name = path.file_name().unwrap().to_string_lossy().to_string();
        assert!(
            name.starts_with("atic-play-login"),
            "unexpected name: {name}"
        );
        if cfg!(windows) {
            assert!(name.ends_with(".exe"), "unexpected name: {name}");
        }
    }

    #[test]
    fn the_web_view_data_is_ours() {
        let dir = webview_data_dir();
        assert!(
            dir.ends_with("webview"),
            "unexpected dir: {}",
            dir.display()
        );
        assert!(
            dir.parent().unwrap().ends_with("atic-play"),
            "unexpected dir: {}",
            dir.display()
        );
    }
}
