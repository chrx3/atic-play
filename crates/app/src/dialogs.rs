//! The small dialogs of the app: the menu of a song, "agregar a lista" and the keyboard
//! shortcut reference.
//!
//! All of them are rendered from one place — the [`Dialogs`] state of the model — so a dialog is
//! never left open behind a page change and only one is up at a time.

use gpui::{
    AnyElement, AnyView, App, AppContext as _, ClickEvent, Context, Entity, InteractiveElement,
    IntoElement, ParentElement, SharedString, StatefulInteractiveElement, Styled, Window, div, px,
};
use gpui_m3::{
    Button, Dialog, Icon, ListItem, TextField, Theme,
    theme::{
        type_scale::{LABEL_MEDIUM, TypeScaleExt},
    },
};

use crate::icons::icon;
use crate::library::LibTab;
use crate::model::{Load, Model};
use crate::source::Track;

/// What is open, if anything.
pub struct Dialogs {
    /// The menu of one song.
    pub song: Option<Track>,
    /// "Agregar a lista" for one song.
    pub add: Option<Track>,
    /// The keyboard shortcut reference.
    pub shortcuts: bool,
    /// The name of the playlist being created.
    pub text: Entity<TextField>,
}

impl Dialogs {
    pub fn new(cx: &mut Context<Model>) -> Self {
        let text = cx.new(|cx| {
            TextField::new(cx)
                .placeholder("Nombre de la lista")
                .label("Nueva lista")
        });
        Self {
            song: None,
            add: None,
            shortcuts: false,
            text,
        }
    }

    pub fn close(&mut self, cx: &mut Context<Model>) {
        self.song = None;
        self.add = None;
        self.shortcuts = false;
        cx.notify();
    }
}

impl Model {
    /// Opens the menu of `track` (the "⋮" of a row).
    pub fn open_song_menu(&mut self, track: Track, cx: &mut Context<Model>) {
        self.dialogs.add = None;
        self.dialogs.shortcuts = false;
        self.dialogs.song = Some(track);
        cx.notify();
    }

    /// Opens "agregar a lista" for `track`, and makes sure the playlists are on their way.
    pub fn open_add_to_playlist(&mut self, track: Track, cx: &mut Context<Model>) {
        self.dialogs.song = None;
        self.dialogs.shortcuts = false;
        self.dialogs.add = Some(track);
        if self.source.signed_in() {
            if self.library.playlists.ready().is_none() {
                self.library_retry(LibTab::Playlists, cx);
            }
        } else {
            self.say("Inicia sesión para guardar canciones en tus listas", cx);
        }
        cx.notify();
    }
}

/// The "⋮" button of a queue row in the playing sheet.
pub fn song_menu_button(
    model: &Entity<Model>,
    track: Track,
    id: impl Into<SharedString>,
) -> AnyElement {
    let entity = model.clone();
    gpui_m3::IconButton::new(id.into(), icon("grip"))
        .size(px(32.))
        .tooltip("Más opciones")
        .on_click(move |_, _, cx| {
            cx.stop_propagation();
            entity.update(cx, |model, cx| model.open_song_menu(track.clone(), cx));
        })
        .into_any_element()
}

/// Every dialog that is open, as one element for the shell to place over the window.
pub fn render(model: &Entity<Model>, cx: &mut App) -> Option<AnyElement> {
    let (song, add, shortcuts) = {
        let model = model.read(cx);
        (
            model.dialogs.song.clone(),
            model.dialogs.add.clone(),
            model.dialogs.shortcuts,
        )
    };
    if shortcuts {
        return Some(shortcuts_dialog(model, cx));
    }
    if let Some(track) = add {
        return Some(add_dialog(model, track, cx));
    }
    if let Some(track) = song {
        return Some(song_dialog(model, track, cx));
    }
    None
}

fn dismiss(model: &Entity<Model>) -> impl Fn(&ClickEvent, &mut Window, &mut App) + 'static {
    let model = model.clone();
    move |_, _, cx| {
        model.update(cx, |model, cx| model.dialogs.close(cx));
    }
}

/// "Me gusta / Agregar a lista" for one song.
fn song_dialog(model: &Entity<Model>, track: Track, cx: &mut App) -> AnyElement {
    let s = *Theme::of(cx);
    let (liked, signed_in) = {
        let model = model.read(cx);
        (
            model.is_liked(&track.id),
            model.account.signed_in,
        )
    };
    let subtitle = track.subtitle();

    let like = {
        let entity = model.clone();
        let track2 = track.clone();
        ListItem::new("song-like", if liked { "Quitar de Me gusta" } else { "Me gusta" })
            .leading(
                Icon::new(icon(if liked { "heart-filled" } else { "heart" }))
                    .size(px(20.))
                    .color(if liked { s.primary } else { s.on_surface_variant }),
            )
            .subtitle(if signed_in {
                "Se guarda en tu cuenta"
            } else {
                "Inicia sesión para guardarlo"
            })
            .on_click(move |_, _, cx| {
                entity.update(cx, |model, cx| {
                    model.toggle_like(track2.clone(), cx);
                    model.dialogs.close(cx);
                });
            })
    };
    let add = {
        let entity = model.clone();
        let track2 = track.clone();
        ListItem::new("song-add", "Agregar a lista")
            .leading(
                Icon::new(icon("queue"))
                    .size(px(20.))
                    .color(s.on_surface_variant),
            )
            .subtitle(if signed_in {
                "Elige una lista tuya, o crea una"
            } else {
                "Inicia sesión para usar tus listas"
            })
            .on_click(move |_, _, cx| {
                entity.update(cx, |model, cx| {
                    model.open_add_to_playlist(track2.clone(), cx);
                });
            })
    };
    let play = {
        let entity = model.clone();
        let track2 = track.clone();
        ListItem::new("song-play", "Reproducir ahora")
            .leading(
                Icon::new(icon("play"))
                    .size(px(20.))
                    .color(s.on_surface_variant),
            )
            .on_click(move |_, _, cx| {
                let track2 = track2.clone();
                entity.update(cx, |model, cx| {
                    model.dialogs.close(cx);
                    model.play_now(track2, cx);
                });
            })
            .into_any_element()
    };

    Dialog::new("song-menu")
        .icon(icon("music"))
        .title(track.title.clone())
        .width(px(430.))
        .child(
            div()
                .text_size(px(13.))
                .text_color(s.on_surface_variant)
                .child(subtitle),
        )
        .child(
            div()
                .flex()
                .flex_col()
                .rounded(px(20.))
                .bg(s.surface_container)
                .p(px(4.))
                .child(like)
                .child(add)
                .child(play),
        )
        .action(
            Button::new("song-close", "Cerrar").on_click(dismiss(model)),
        )
        .into_any_element()
}

/// The account's playlists, plus a new one.
fn add_dialog(model: &Entity<Model>, track: Track, cx: &mut App) -> AnyElement {
    let s = *Theme::of(cx);
    let (state, field) = {
        let model = model.read(cx);
        (model.library.playlists.clone(), model.dialogs.text.clone())
    };

    let mut rows = div().flex().flex_col().gap(px(2.));
    match state {
        Load::Idle | Load::Loading => {
            rows = rows.child(
                div()
                    .py(px(18.))
                    .text_size(px(13.))
                    .text_color(s.on_surface_variant)
                    .child("Cargando tus listas…"),
            );
        }
        Load::Failed(error) => {
            rows = rows.child(
                div()
                    .py(px(18.))
                    .text_size(px(13.))
                    .text_color(s.error)
                    .child(format!("No se pudieron cargar tus listas: {error}")),
            );
        }
        Load::Ready(lists) if lists.is_empty() => {
            rows = rows.child(
                div()
                    .py(px(18.))
                    .text_size(px(13.))
                    .text_color(s.on_surface_variant)
                    .child("Todavía no tienes listas: crea una abajo."),
            );
        }
        Load::Ready(lists) => {
            for (index, playlist) in lists.iter().take(12).enumerate() {
                let entity = model.clone();
                let (playlist_id, title) = (playlist.id.clone(), playlist.title.clone());
                let track = track.clone();
                rows = rows.child(
                    ListItem::new(("add-to", index), title)
                        .subtitle(playlist.author.clone().unwrap_or_default())
                        .leading(
                            Icon::new(icon("queue"))
                                .size(px(18.))
                                .color(s.on_surface_variant),
                        )
                        .on_click(move |_, _, cx| {
                            let track = track.clone();
                            entity.update(cx, |model, cx| {
                                model.add_to_playlist(playlist_id.clone(), track, cx)
                            });
                        }),
                );
            }
        }
    }

    let create = {
        let entity = model.clone();
        let field = field.clone();
        let track2 = track.clone();
        Button::new("add-create", "Crear y agregar")
            .filled()
            .on_click(move |_, _, cx| {
                let title = field.read(cx).text().to_string();
                let track2 = track2.clone();
                entity.update(cx, |model, cx| {
                    model.create_playlist_with(title.clone(), track2.clone(), cx)
                });
            })
    };

    Dialog::new("add-to-playlist")
        .icon(icon("queue"))
        .title("Agregar a una lista")
        .width(px(460.))
        .child(
            div()
                .text_size(px(13.))
                .text_color(s.on_surface_variant)
                .child(format!("«{}» — {}", track.title, track.artist)),
        )
        .child(rows)
        .child(
            div()
                .flex()
                .flex_col()
                .gap(px(10.))
                .pt(px(6.))
                .child(
                    div()
                        .type_style(LABEL_MEDIUM)
                        .text_color(s.on_surface_variant)
                        .child("NUEVA LISTA"),
                )
                .child(AnyView::from(field)),
        )
        .action(Button::new("add-close", "Cancelar").on_click(dismiss(model)))
        .action(create)
        .into_any_element()
}

/// The shortcut reference: the same list the bindings are built from.
fn shortcuts_dialog(model: &Entity<Model>, cx: &mut App) -> AnyElement {
    let s = *Theme::of(cx);
    let mut rows = div().flex().flex_col().gap(px(2.));
    for (index, (keys, what)) in crate::shortcuts::BINDINGS.iter().enumerate() {
        rows = rows.child(
            ListItem::new(("shortcut", index), *what)
                .trailing(
                    div()
                        .flex()
                        .gap(px(6.))
                        .children(keys.split('+').map(|key| {
                            div()
                                .px(px(8.))
                                .py(px(3.))
                                .rounded(px(8.))
                                .bg(s.surface_container_high)
                                .text_size(px(11.5))
                                .text_color(s.on_surface)
                                .child(key.trim().to_string())
                        })),
                ),
        );
    }
    Dialog::new("shortcuts")
        .icon(icon("music"))
        .title("Atajos de teclado")
        .width(px(470.))
        .child(
            div()
                .text_size(px(12.5))
                .text_color(s.on_surface_variant)
                .child(crate::shortcuts::summary()),
        )
        .child(
            div()
                .id("shortcuts-scroll")
                .max_h(px(430.))
                .overflow_y_scroll()
                .child(rows),
        )
        .action(Button::new("shortcuts-close", "Listo").on_click(dismiss(model)))
        .into_any_element()
}

