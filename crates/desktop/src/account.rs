use std::future::Future;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::time::Duration;

use gpui::{Context, SharedString};
use ytm_core::{Client, Library, Track};

use crate::google_sign_in;

/// Runs this executable in sign-in mode and waits for the window to close. Returns the session
/// cookies, or `None` if the user closed the window first.
///
/// Blocks: call it on a background thread.
fn run_sign_in_window(cookies_path: &Path, profile_dir: &Path) -> anyhow::Result<Option<String>> {
    // A fresh browser profile every time, so no earlier session lingers in it.
    remove_sign_in_files(cookies_path, profile_dir);
    let status = Command::new(std::env::current_exe()?)
        .arg(google_sign_in::ARG)
        .arg(cookies_path)
        .arg(profile_dir)
        .status()?;
    let result = match status.code().and_then(|code| u8::try_from(code).ok()) {
        Some(google_sign_in::SIGNED_IN) => Ok(Some(std::fs::read_to_string(cookies_path)?)),
        Some(google_sign_in::CANCELLED) => Ok(None),
        _ => Err(anyhow::anyhow!(
            "la ventana de inicio de sesión falló ({status})"
        )),
    };
    // The session now lives only in Atic-play's own copy; WebView2 may still hold the profile
    // for a moment after the window closes, so retry briefly.
    for _ in 0..5 {
        if remove_sign_in_files(cookies_path, profile_dir) {
            break;
        }
        std::thread::sleep(Duration::from_secs(1));
    }
    result
}

/// Deletes the sign-in window's cookies and profile; true once both are gone.
fn remove_sign_in_files(cookies_path: &Path, profile_dir: &Path) -> bool {
    let _ = std::fs::remove_file(cookies_path);
    let _ = std::fs::remove_dir_all(profile_dir);
    !cookies_path.exists() && !profile_dir.exists()
}

pub enum AccountState {
    /// Checking the session saved last time.
    Restoring,
    SignedOut,
    SigningIn,
    SignedIn,
}

/// The YouTube Music session and what the user has saved. Views observe it.
pub struct Account {
    client: Client,
    data_dir: PathBuf,
    state: AccountState,
    library: Option<Library>,
    library_loading: bool,
    /// The last sign-in or library error, shown where it happened.
    error: Option<SharedString>,
}

impl Account {
    pub fn new(client: Client, data_dir: PathBuf, cx: &mut Context<Self>) -> Self {
        let restore = client.restore_session();
        cx.spawn(async move |this, cx| {
            let restored = restore.await;
            this.update(cx, |this, cx| {
                match restored {
                    Ok(true) => this.signed_in(cx),
                    Ok(false) => this.state = AccountState::SignedOut,
                    Err(err) => {
                        log::warn!("saved session: {err:#}");
                        this.state = AccountState::SignedOut;
                        this.error = Some(
                            "Tu sesión de YouTube Music expiró. Vuelve a iniciar sesión.".into(),
                        );
                    }
                }
                cx.notify();
            })
            .ok();
        })
        .detach();

        Account {
            client,
            data_dir,
            state: AccountState::Restoring,
            library: None,
            library_loading: false,
            error: None,
        }
    }

    pub fn state(&self) -> &AccountState {
        &self.state
    }

    pub fn is_signed_in(&self) -> bool {
        matches!(self.state, AccountState::SignedIn)
    }

    pub fn library(&self) -> Option<&Library> {
        self.library.as_ref()
    }

    pub fn is_library_loading(&self) -> bool {
        self.library_loading
    }

    pub fn error(&self) -> Option<&SharedString> {
        self.error.as_ref()
    }

    /// Whether the track is in the user's liked songs, as far as the loaded library knows (its
    /// first page, plus likes given in this session).
    #[expect(
        dead_code,
        reason = "used by the Sprint 3 UI on the sprint-3-ui branch"
    )]
    pub fn is_liked(&self, track_id: &str) -> bool {
        self.library
            .as_ref()
            .is_some_and(|library| library.liked.iter().any(|track| track.id == track_id))
    }

    /// Likes the track in YouTube Music, or takes the like back. The library changes at once and
    /// is put back if YouTube refuses.
    #[expect(
        dead_code,
        reason = "used by the Sprint 3 UI on the sprint-3-ui branch"
    )]
    pub fn toggle_like(&mut self, track: Track, cx: &mut Context<Self>) {
        if !self.is_signed_in() {
            return;
        }
        let Some(library) = self.library.as_mut() else {
            return;
        };
        let liked = !library.liked.iter().any(|liked| liked.id == track.id);
        if liked {
            library.liked.insert(0, track.clone());
        } else {
            library.liked.retain(|liked| liked.id != track.id);
        }
        cx.notify();

        let request = self.client.set_liked(track.id.clone(), liked);
        cx.spawn(async move |this, cx| {
            let Err(err) = request.await else {
                return;
            };
            log::warn!("like {}: {err:#}", track.id);
            this.update(cx, |this, cx| {
                if let Some(library) = &mut this.library {
                    if liked {
                        library.liked.retain(|liked| liked.id != track.id);
                    } else {
                        library.liked.insert(0, track);
                    }
                }
                this.error = Some(format!("No se pudo guardar el me gusta: {err:#}").into());
                cx.notify();
            })
            .ok();
        })
        .detach();
    }

    /// Opens the Google sign-in window and signs in with the session it returns.
    pub fn sign_in_with_google(&mut self, cx: &mut Context<Self>) {
        let cookies_path = self.data_dir.join("sign-in-session.txt");
        let profile_dir = self.data_dir.join("sign-in-browser");
        let window = cx
            .background_executor()
            .spawn(async move { run_sign_in_window(&cookies_path, &profile_dir) });
        self.finish_sign_in(window, cx);
    }

    /// Signs in with a `cookies.txt` the user exported from their browser.
    pub fn sign_in_with_file(&mut self, path: PathBuf, cx: &mut Context<Self>) {
        let cookies = cx.background_executor().spawn(async move {
            std::fs::read_to_string(&path)
                .map(Some)
                .map_err(|err| anyhow::anyhow!("no se pudo leer el archivo: {err}"))
        });
        self.finish_sign_in(cookies, cx);
    }

    /// Signs in with the cookies `cookies` yields; `None` means the user cancelled.
    fn finish_sign_in(
        &mut self,
        cookies: impl Future<Output = anyhow::Result<Option<String>>> + 'static,
        cx: &mut Context<Self>,
    ) {
        self.state = AccountState::SigningIn;
        self.error = None;
        cx.notify();

        let client = self.client.clone();
        cx.spawn(async move |this, cx| {
            let result = match cookies.await {
                Ok(Some(cookies)) => client.sign_in(cookies).await.map(Some),
                Ok(None) => Ok(None),
                Err(err) => Err(err),
            };
            this.update(cx, |this, cx| {
                match result {
                    Ok(Some(())) => this.signed_in(cx),
                    Ok(None) => this.state = AccountState::SignedOut,
                    Err(err) => {
                        this.state = AccountState::SignedOut;
                        this.error = Some(format!("No se pudo iniciar sesión: {err:#}").into());
                    }
                }
                cx.notify();
            })
            .ok();
        })
        .detach();
    }

    pub fn sign_out(&mut self, cx: &mut Context<Self>) {
        self.state = AccountState::SignedOut;
        self.library = None;
        self.error = None;
        cx.notify();
        let sign_out = self.client.sign_out();
        cx.background_executor()
            .spawn(async move {
                if let Err(err) = sign_out.await {
                    log::warn!("sign out: {err:#}");
                }
            })
            .detach();
    }

    pub fn reload_library(&mut self, cx: &mut Context<Self>) {
        if !self.is_signed_in() || self.library_loading {
            return;
        }
        self.library_loading = true;
        cx.notify();
        let library = self.client.library();
        cx.spawn(async move |this, cx| {
            let library = library.await;
            this.update(cx, |this, cx| {
                this.library_loading = false;
                match library {
                    Ok(library) => this.library = Some(library),
                    Err(err) => {
                        this.error =
                            Some(format!("No se pudo cargar tu biblioteca: {err:#}").into())
                    }
                }
                cx.notify();
            })
            .ok();
        })
        .detach();
    }

    fn signed_in(&mut self, cx: &mut Context<Self>) {
        self.state = AccountState::SignedIn;
        self.error = None;
        self.reload_library(cx);
    }
}
