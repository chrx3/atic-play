//! The YouTube Music account: whether there is a session, who it is, and the login window.
//!
//! The session itself is never touched here — [`play_account`] keeps it in the system keyring and
//! signs in through a separate window (`atic-play-login`). This module only carries the state the
//! rail and the account sheet show, and swaps the app's [`MusicSource`](crate::source) between the
//! signed-in client and the anonymous one.
//!
//! Nothing here ever logs a cookie.

use gpui::{
    AnyElement, App, AppContext as _, Entity, FontWeight, ImageSource, InteractiveElement,
    IntoElement, ObjectFit, ParentElement, StatefulInteractiveElement, Styled, StyledImage, div,
    img, px, prelude::FluentBuilder as _,
};
use gpui_m3::{
    Button, Dialog, Icon, IconButton, LoadingIndicator, Scheme, Theme,
    theme::{
        type_scale::{BODY_MEDIUM, TITLE_MEDIUM, TypeScaleExt},
    },
};

use crate::covers::{ArtSize, CoverCache};
use crate::icons::icon;
use crate::model::{Load, Model, ViewKind};
use crate::source::{self, AccountInfo, is_signed_out};

/// The account side of the app.
pub struct AccountState {
    /// Whether the running source carries a session.
    pub signed_in: bool,
    /// Name, handle and photo, while they load.
    pub info: Load<AccountInfo>,
    /// The login window is open and the app is waiting for it.
    pub signing_in: bool,
    /// The session was there but YouTube rejected it: the UI offers the login again.
    pub expired: bool,
    /// The account sheet is open.
    pub menu_open: bool,
    generation: u64,
}

impl Default for AccountState {
    fn default() -> Self {
        Self {
            signed_in: false,
            info: Load::Idle,
            signing_in: false,
            expired: false,
            menu_open: false,
            generation: 0,
        }
    }
}

impl Model {
    /// Reads the session the source already started with. Called once, when the app opens.
    pub fn start_account(&mut self, cx: &mut gpui::Context<Self>) {
        self.account.signed_in = self.source.signed_in();
        self.account.expired = self.source.session_rejected();
        if self.account.signed_in {
            self.refresh_account(cx);
            self.library_load_all(cx);
        } else if self.account.expired {
            self.say(
                "Tu sesión de YouTube Music venció. Inicia sesión de nuevo.",
                cx,
            );
        }
    }

    /// Asks who the account is. Without a session it just clears what was there.
    pub fn refresh_account(&mut self, cx: &mut gpui::Context<Self>) {
        if !self.source.signed_in() {
            self.account.signed_in = false;
            self.account.info = Load::Idle;
            cx.notify();
            return;
        }
        self.account.generation += 1;
        let generation = self.account.generation;
        self.account.info = Load::Loading;
        cx.notify();
        let source = self.source.clone();
        cx.spawn(async move |this, cx| {
            let info = cx.background_spawn(async move { source.account() }).await;
            this.update(cx, |model, cx| {
                if model.account.generation != generation {
                    return;
                }
                model.account.info = match info {
                    Ok(info) => {
                        log::info!("account: signed in as «{}»", info.name);
                        Load::Ready(info)
                    }
                    Err(err) => {
                        model.session_error(&err, cx);
                        Load::Failed(err.into())
                    }
                };
                cx.notify();
            })
            .ok();
        })
        .detach();
    }

    /// Opens the login window (in the background) and, when it comes back with a session, swaps
    /// the app to the signed-in client and reloads everything that depends on the account.
    pub fn start_login(&mut self, cx: &mut gpui::Context<Self>) {
        if self.account.signing_in {
            return;
        }
        if self.source.name() != "YouTube Music" {
            self.say(
                "Iniciar sesión necesita la fuente real de YouTube Music",
                cx,
            );
            return;
        }
        self.account.signing_in = true;
        self.account.menu_open = false;
        cx.notify();
        self.say("Esperando la ventana de Google…", cx);
        cx.spawn(async move |this, cx| {
            let answer = cx
                .background_spawn(async move { play_account::run_login() })
                .await;
            this.update(cx, |model, cx| {
                model.account.signing_in = false;
                match answer {
                    Ok(Some(_session)) => {
                        // Only the fact that a session arrived is logged; the cookies never are.
                        log::info!("account: login finished with a session");
                        model.use_signed_source(cx);
                        model.say("Listo: ya estás dentro", cx);
                    }
                    Ok(None) => {
                        log::info!("account: the login window was closed");
                        model.say("No iniciaste sesión", cx);
                    }
                    Err(err) => {
                        log::warn!("account: the login window failed: {err}");
                        model.say(format!("No se pudo iniciar sesión: {err}"), cx);
                    }
                }
                cx.notify();
            })
            .ok();
        })
        .detach();
    }

    /// Forgets the session (keyring and the login window's data) and goes back to anonymous.
    pub fn logout(&mut self, cx: &mut gpui::Context<Self>) {
        self.account.menu_open = false;
        self.say("Cerrando sesión…", cx);
        cx.spawn(async move |this, cx| {
            let answer = cx
                .background_spawn(async move { play_account::logout() })
                .await;
            this.update(cx, |model, cx| {
                match answer {
                    Ok(()) => log::info!("account: signed out"),
                    Err(err) => log::warn!("account: could not sign out: {err}"),
                }
                model.use_anonymous_source(cx);
                model.say("Cerraste sesión", cx);
                cx.notify();
            })
            .ok();
        })
        .detach();
    }

    /// Brings a fresh signed-in client in (after a login or a renewed session) and reloads the
    /// pages that were anonymous until now.
    pub fn use_signed_source(&mut self, cx: &mut gpui::Context<Self>) {
        self.source = source::signed_source("es", "CL");
        self.account.signed_in = self.source.signed_in();
        self.account.expired = false;
        self.account.info = Load::Idle;
        self.refresh_account(cx);
        self.library_load_all(cx);
        self.refresh_home(cx);
    }

    /// Drops the session from the app (the keyring is left alone) and goes back to the public
    /// catalog, so everything keeps working without an account.
    pub fn use_anonymous_source(&mut self, cx: &mut gpui::Context<Self>) {
        self.source = std::sync::Arc::new(source::YtMusicSource::new("es", "CL"));
        self.account.signed_in = false;
        self.account.info = Load::Idle;
        self.account.expired = false;
        self.library_clear();
        if self.view.kind() == ViewKind::Library {
            self.go_home(cx);
        }
        self.refresh_home(cx);
        cx.notify();
    }

    /// Watches for the session dying under us. YouTube answers
    /// [`SIGNED_OUT`](crate::source::SIGNED_OUT) when the cookies expired; the app then goes
    /// anonymous and says so, once.
    ///
    /// It runs on the tick, so every page — home, search, album, library — is covered in one place.
    pub fn check_session(&mut self, cx: &mut gpui::Context<Self>) {
        if !self.account.signed_in || self.account.expired {
            return;
        }
        if self.library_error().is_some_and(is_signed_out)
            || load_error(&self.home).is_some_and(is_signed_out)
            || load_error(&self.search).is_some_and(is_signed_out)
        {
            log::warn!("account: the session expired");
            self.session_error(source::SIGNED_OUT, cx);
            self.source = std::sync::Arc::new(source::YtMusicSource::new("es", "CL"));
            if self.view.kind() == ViewKind::Library {
                self.go_home(cx);
            }
            self.library_clear();
            self.refresh_home(cx);
            cx.notify();
        }
    }

    /// Turns an error from an account call into the expired-session state, when that is what it
    /// is. Answers whether it did.
    pub fn session_error(&mut self, error: &str, cx: &mut gpui::Context<Self>) -> bool {
        if !is_signed_out(error) {
            return false;
        }
        if !self.account.expired {
            self.account.expired = true;
            self.account.signed_in = false;
            self.account.info = Load::Idle;
            self.library_clear();
            self.say(
                "Tu sesión de YouTube Music venció. Inicia sesión de nuevo.",
                cx,
            );
        }
        true
    }
}

/// The message of a failed load, when it failed.
fn load_error(load: &Load<impl Sized>) -> Option<&str> {
    match load {
        Load::Failed(error) => Some(error.as_ref()),
        _ => None,
    }
}

// ---------------------------------------------------------------- the rail button

/// The avatar at the bottom of the rail: the account photo, the first letter of its name, or the
/// "Iniciar sesión" button when there is no session.
pub fn rail_button(model: &Entity<Model>, cx: &mut App) -> AnyElement {
    let (signed_in, signing_in, photo, name, dark, expired) = {
        let model = model.read(cx);
        (
            model.account.signed_in,
            model.account.signing_in,
            match &model.account.info {
                Load::Ready(info) => info.photo.clone(),
                _ => None,
            },
            match &model.account.info {
                Load::Ready(info) => Some(info.name.clone()),
                _ => None,
            },
            model.dark,
            model.account.expired,
        )
    };
    let s = *Theme::of(cx);

    if !signed_in {
        let login = model.clone();
        return div()
            .flex()
            .flex_col()
            .items_center()
            .pb(px(10.))
            .child(
                IconButton::new("rail-login", icon("account"))
                    .tonal()
                    .size(px(44.))
                    .tooltip(if expired {
                        "Iniciar sesión de nuevo"
                    } else {
                        "Iniciar sesión"
                    })
                    .on_click(move |_, _, cx| login.update(cx, |model, cx| model.start_login(cx))),
            )
            .child(
                div()
                    .pt(px(4.))
                    .text_size(px(10.))
                    .text_color(if expired { s.error } else { s.on_surface_variant })
                    .child(if expired { "Vencida" } else { "Entrar" }),
            )
            .into_any_element();
    }

    if signing_in {
        return div()
            .pb(px(10.))
            .child(
                div()
                    .size(px(44.))
                    .flex()
                    .items_center()
                    .justify_center()
                    .child(LoadingIndicator::new().size(px(22.))),
            )
            .into_any_element();
    }

    let image = cover_thumb(model, photo.as_deref(), cx);
    let initial = name
        .as_deref()
        .and_then(|name| name.trim().chars().next())
        .map(|first| first.to_uppercase().to_string())
        .unwrap_or_else(|| "?".to_string());
    let avatar = match image {
        Some(image) => img(ImageSource::Render(image))
            .size(px(36.))
            .rounded(px(18.))
            .object_fit(ObjectFit::Cover)
            .into_any_element(),
        None => div()
            .size(px(36.))
            .rounded(px(18.))
            .bg(if dark {
                s.secondary_container
            } else {
                s.primary_container
            })
            .flex()
            .items_center()
            .justify_center()
            .text_size(px(15.))
            .font_weight(FontWeight(600.))
            .text_color(if dark {
                s.on_secondary_container
            } else {
                s.on_primary_container
            })
            .child(initial)
            .into_any_element(),
    };

    let open = model.clone();
    div()
        .id("rail-account")
        .relative()
        .pb(px(10.))
        .cursor_pointer()
        .on_click(move |_, _, cx| {
            open.update(cx, |model, cx| {
                model.account.menu_open = true;
                cx.notify();
            })
        })
        .child(div().p(px(4.)).child(avatar))
        .when(expired, |d| {
            d.child(
                div()
                    .absolute()
                    .bottom(px(8.))
                    .right(px(2.))
                    .size(px(10.))
                    .rounded(px(5.))
                    .bg(s.error),
            )
        })
        .into_any_element()
}

/// The account photo through the cover cache, so it is downloaded and decoded like any other art.
fn cover_thumb(
    model: &Entity<Model>,
    url: Option<&str>,
    cx: &mut App,
) -> Option<std::sync::Arc<gpui::RenderImage>> {
    let covers: Entity<CoverCache> = model.read(cx).covers.clone();
    if let Some(image) = covers.read(cx).peek_size(url, ArtSize::Thumb) {
        return Some(image);
    }
    covers.update(cx, |covers, cx| {
        covers.image_size(url, ArtSize::Thumb, cx)
    })
}

// ---------------------------------------------------------------- the account sheet

/// The small sheet behind the avatar: who is signed in, and the way out.
pub fn dialog(model: &Entity<Model>, cx: &mut App) -> Option<AnyElement> {
    let (open, signed_in, info, signing_in, expired) = {
        let model = model.read(cx);
        (
            model.account.menu_open,
            model.account.signed_in,
            model.account.info.clone(),
            model.account.signing_in,
            model.account.expired,
        )
    };
    if !open {
        return None;
    }
    let s = *Theme::of(cx);

    let header = match (&info, signed_in) {
        (Load::Ready(info), true) => {
            let image = cover_thumb(model, info.photo.as_deref(), cx);
            let initial = info
                .name
                .trim()
                .chars()
                .next()
                .map(|first| first.to_uppercase().to_string())
                .unwrap_or_else(|| "?".into());
            let art = match image {
                Some(image) => img(ImageSource::Render(image))
                    .size(px(56.))
                    .rounded(px(28.))
                    .object_fit(ObjectFit::Cover)
                    .into_any_element(),
                None => div()
                    .size(px(56.))
                    .rounded(px(28.))
                    .bg(s.primary_container)
                    .flex()
                    .items_center()
                    .justify_center()
                    .text_size(px(22.))
                    .text_color(s.on_primary_container)
                    .child(initial)
                    .into_any_element(),
            };
            div()
                .flex()
                .items_center()
                .gap(px(14.))
                .child(art)
                .child(
                    div()
                        .flex()
                        .flex_col()
                        .min_w_0()
                        .child(
                            div()
                                .type_emphasized(TITLE_MEDIUM)
                                .truncate()
                                .child(info.name.clone()),
                        )
                        .child(
                            div()
                                .type_style(BODY_MEDIUM)
                                .text_color(s.on_surface_variant)
                                .truncate()
                                .child(
                                    info.handle
                                        .clone()
                                        .unwrap_or_else(|| "YouTube Music".into()),
                                ),
                        ),
                )
                .into_any_element()
        }
        (Load::Loading, true) => div()
            .flex()
            .items_center()
            .gap(px(12.))
            .child(LoadingIndicator::new().size(px(20.)))
            .child(
                div()
                    .text_size(px(13.))
                    .text_color(s.on_surface_variant)
                    .child("Viendo tu cuenta…"),
            )
            .into_any_element(),
        (Load::Failed(error), true) if !is_signed_out(error) => div()
            .text_size(px(13.))
            .text_color(s.on_surface_variant)
            .child(format!("No se pudo leer tu cuenta: {error}"))
            .into_any_element(),
        _ => div()
            .flex()
            .flex_col()
            .gap(px(6.))
            .child(div().type_emphasized(TITLE_MEDIUM).child(if expired {
                "Tu sesión venció"
            } else {
                "Sin cuenta"
            }))
            .child(
                div()
                    .text_size(px(13.))
                    .text_color(s.on_surface_variant)
                    .child(if expired {
                        "Vuelve a iniciar sesión para tener tus Me gusta, tus listas y tu historial."
                    } else {
                        "Inicia sesión para tener tus Me gusta, tus listas y tu historial."
                    }),
            )
            .into_any_element(),
    };

    let login = {
        let model = model.clone();
        Button::new(
            "account-login",
            if expired {
                "Iniciar sesión de nuevo"
            } else {
                "Iniciar sesión"
            },
        )
        .filled()
        .when(signing_in, |button| button.disabled(true))
        .on_click(move |_, _, cx| {
            model.update(cx, |model, cx| model.start_login(cx));
        })
    };
    let logout = {
        let model = model.clone();
        Button::new("account-logout", "Cerrar sesión")
            .tonal()
            .on_click(move |_, _, cx| {
                model.update(cx, |model, cx| model.logout(cx));
            })
    };
    let shortcuts = {
        let model = model.clone();
        Button::new("account-shortcuts", "Atajos").text().on_click(move |_, _, cx| {
            model.update(cx, |model, cx| {
                model.account.menu_open = false;
                model.dialogs.shortcuts = true;
                cx.notify();
            });
        })
    };
    let close = {
        let model = model.clone();
        Button::new("account-close", "Listo").on_click(move |_, _, cx| {
            model.update(cx, |model, cx| {
                model.account.menu_open = false;
                cx.notify();
            });
        })
    };

    let mut dialog = Dialog::new("account-sheet")
        .icon(icon("account"))
        .title("Tu cuenta")
        .width(px(420.))
        .child(header)
        .child(
            div()
                .text_size(px(12.))
                .text_color(s.on_surface_variant)
                .child(if signed_in {
                    "Tu sesión vive en el llavero de Windows, no en un archivo de Atic Play."
                } else {
                    "Con una cuenta tienes Me gusta, tus listas y tu historial."
                }),
        )
        .action(shortcuts)
        .action(close);
    if signed_in {
        dialog = dialog.action(logout);
    }
    if !signed_in || expired {
        dialog = dialog.action(login);
    }
    Some(dialog.into_any_element())
}

// ---------------------------------------------------------------- the banner

/// The thin line under the top bar when the session died, with the way to fix it.
pub fn banner(model: &Entity<Model>, s: &Scheme, cx: &mut App) -> Option<AnyElement> {
    let visible = {
        let model = model.read(cx);
        model.account.expired && !model.account.signing_in
    };
    if !visible {
        return None;
    }
    let model2 = model.clone();
    Some(
        div()
            .flex()
            .items_center()
            .gap(px(12.))
            .mx(px(20.))
            .mt(px(8.))
            .px(px(16.))
            .py(px(10.))
            .rounded(px(16.))
            .bg(s.error_container)
            .text_color(s.on_error_container)
            .child(Icon::new("alert").size(px(18.)).color(s.on_error_container))
            .child(
                div()
                    .flex_1()
                    .text_size(px(13.))
                    .child("Tu sesión de YouTube Music venció. Sigues escuchando como invitado."),
            )
            .child(
                Button::new("session-login", "Iniciar sesión de nuevo")
                    .tonal()
                    .on_click(move |_, _, cx| {
                        model2.update(cx, |model, cx| model.start_login(cx));
                    }),
            )
            .into_any_element(),
    )
}
