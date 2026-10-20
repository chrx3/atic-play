//! Dialogs that change the user's playlists: saving songs to one, creating, renaming and
//! deleting one, and taking a song out. Deleting and taking out ask first, since YouTube Music
//! has no undo.

use gpui::{ClickEvent, Context, Entity, EventEmitter, Subscription, Window, div, prelude::*, px};
use gpui_m3::{Button, Dialog, ListItem, TextField, TextFieldEvent, Theme};
use ytm_core::{Playlist, Track};

use crate::account::Account;
use crate::covers::Covers;
use crate::ui::{CARD_COVER_SIZE, thumbnail};

/// What the dialog is for.
#[derive(Clone)]
pub enum PlaylistDialogMode {
    /// Save these songs to a playlist the user picks, or to a new one.
    SaveTo(Vec<Track>),
    /// A new playlist with these songs (possibly none).
    Create(Vec<Track>),
    Rename(Playlist),
    Delete(Playlist),
    Remove {
        playlist: Playlist,
        track: Track,
    },
}

pub enum PlaylistDialogEvent {
    Close,
}

pub struct PlaylistDialog {
    account: Entity<Account>,
    covers: Entity<Covers>,
    mode: PlaylistDialogMode,
    /// The name, when creating or renaming.
    name: Entity<TextField>,
    /// The name field takes the keyboard on the first frame.
    focus_name: bool,
    _subscriptions: Vec<Subscription>,
}

impl EventEmitter<PlaylistDialogEvent> for PlaylistDialog {}

impl PlaylistDialog {
    pub fn new(
        account: Entity<Account>,
        covers: Entity<Covers>,
        mode: PlaylistDialogMode,
        cx: &mut Context<Self>,
    ) -> Self {
        let name = cx.new(|cx| TextField::new(cx).placeholder("Nombre de la playlist"));
        if let PlaylistDialogMode::Rename(playlist) = &mode {
            let title = playlist.title.clone();
            name.update(cx, |field, cx| field.set_text(title, cx));
        }
        let subscriptions =
            vec![
                cx.subscribe(&name, |this, _, event: &TextFieldEvent, cx| match event {
                    TextFieldEvent::Submitted(_) => this.confirm(cx),
                    TextFieldEvent::Cancelled => cx.emit(PlaylistDialogEvent::Close),
                    TextFieldEvent::Changed(_) => cx.notify(),
                }),
            ];
        // The list of playlists to save to may be stale or not loaded yet.
        if matches!(mode, PlaylistDialogMode::SaveTo(_)) {
            account.update(cx, |account, cx| account.reload_editable_playlists(cx));
        }
        cx.observe(&account, |_, _, cx| cx.notify()).detach();
        cx.observe(&covers, |_, _, cx| cx.notify()).detach();
        let focus_name = matches!(
            mode,
            PlaylistDialogMode::Create(_) | PlaylistDialogMode::Rename(_)
        );
        PlaylistDialog {
            account,
            covers,
            mode,
            name,
            focus_name,
            _subscriptions: subscriptions,
        }
    }

    fn typed_name(&self, cx: &Context<Self>) -> Option<String> {
        let name = self.name.read(cx).text().trim().to_string();
        (!name.is_empty()).then_some(name)
    }

    /// The main action of the current mode.
    fn confirm(&mut self, cx: &mut Context<Self>) {
        let account = self.account.clone();
        match self.mode.clone() {
            PlaylistDialogMode::SaveTo(_) => return,
            PlaylistDialogMode::Create(tracks) => {
                let Some(title) = self.typed_name(cx) else {
                    return;
                };
                account.update(cx, |account, cx| account.create_playlist(title, tracks, cx));
            }
            PlaylistDialogMode::Rename(playlist) => {
                let Some(title) = self.typed_name(cx) else {
                    return;
                };
                if title != playlist.title {
                    account.update(cx, |account, cx| {
                        account.rename_playlist(playlist, title, cx)
                    });
                }
            }
            PlaylistDialogMode::Delete(playlist) => {
                account.update(cx, |account, cx| account.delete_playlist(playlist, cx));
            }
            PlaylistDialogMode::Remove { playlist, track } => {
                account.update(cx, |account, cx| {
                    account.remove_from_playlist(playlist, track, cx)
                });
            }
        }
        cx.emit(PlaylistDialogEvent::Close);
    }

    fn render_playlist_choices(
        &self,
        tracks: &[Track],
        cx: &mut Context<Self>,
    ) -> impl IntoElement {
        let s = *Theme::of(cx);
        let account = self.account.read(cx);
        let playlists = account.editable_playlists().to_vec();
        let loading = account.is_loading_editable_playlists();
        // The library's covers are the playlist mosaics; the dialog only sends a video still.
        let library_covers: Vec<(String, Option<String>)> = account
            .library()
            .map(|library| {
                library
                    .playlists
                    .iter()
                    .map(|playlist| (playlist.id.clone(), playlist.cover_url.clone()))
                    .collect()
            })
            .unwrap_or_default();
        let rows: Vec<_> = playlists
            .into_iter()
            .enumerate()
            .map(|(index, playlist)| {
                let tracks = tracks.to_vec();
                let subtitle = playlist
                    .track_count
                    .map(|count| format!("{count} canciones"))
                    .unwrap_or_default();
                let cover_url = library_covers
                    .iter()
                    .find(|(id, _)| *id == playlist.id)
                    .and_then(|(_, url)| url.clone())
                    .or_else(|| playlist.cover_url.clone());
                let cover = self.covers.update(cx, |covers, cx| {
                    covers.get(&playlist.id, cover_url.as_deref(), CARD_COVER_SIZE, cx)
                });
                ListItem::new(("save-to", index), playlist.title.clone())
                    .subtitle(subtitle)
                    .leading(thumbnail(cover, 40., 8., cx))
                    .on_click(cx.listener(move |this, _: &ClickEvent, _, cx| {
                        let (playlist, tracks) = (playlist.clone(), tracks.clone());
                        this.account.update(cx, |account, cx| {
                            account.add_to_playlist(playlist, tracks, cx)
                        });
                        cx.emit(PlaylistDialogEvent::Close);
                    }))
            })
            .collect();
        let empty = rows.is_empty();
        div()
            .id("save-to-list")
            .max_h(px(360.))
            .overflow_y_scroll()
            .flex()
            .flex_col()
            .children(rows)
            .when(empty, |list| {
                list.child(
                    div()
                        .py_4()
                        .text_sm()
                        .text_color(s.on_surface_variant)
                        .child(if loading {
                            "Cargando tus playlists…"
                        } else {
                            "Aún no tienes playlists propias"
                        }),
                )
            })
    }
}

impl Render for PlaylistDialog {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        if std::mem::take(&mut self.focus_name) {
            self.name.read(cx).focus(window);
        }
        let s = *Theme::of(cx);
        let close = cx.listener(|_, _: &ClickEvent, _, cx| cx.emit(PlaylistDialogEvent::Close));
        let cancel = Button::new("playlist-cancel", "Cancelar")
            .text()
            .on_click(cx.listener(|_, _: &ClickEvent, _, cx| cx.emit(PlaylistDialogEvent::Close)));
        let has_name = self.typed_name(cx).is_some();
        let text = |text: String| div().text_sm().text_color(s.on_surface_variant).child(text);

        let dialog = Dialog::new("playlist-dialog")
            .width(px(440.))
            .on_dismiss(close);
        match self.mode.clone() {
            PlaylistDialogMode::SaveTo(tracks) => dialog
                .title("Guardar en playlist")
                .child(self.render_playlist_choices(&tracks, cx))
                .action(
                    Button::new("playlist-new", "Nueva playlist")
                        .text()
                        .on_click(cx.listener(move |this, _: &ClickEvent, window, cx| {
                            this.mode = PlaylistDialogMode::Create(tracks.clone());
                            this.name.read(cx).focus(window);
                            cx.notify();
                        })),
                )
                .action(cancel),
            PlaylistDialogMode::Create(tracks) => dialog
                .title("Nueva playlist")
                .child(
                    div()
                        .flex()
                        .flex_col()
                        .gap_3()
                        .child(self.name.clone())
                        .child(text(match tracks.len() {
                            0 => "Será privada; podrás agregarle canciones después.".into(),
                            1 => "Será privada y tendrá esta canción.".into(),
                            count => format!("Será privada y tendrá {count} canciones."),
                        })),
                )
                .action(cancel)
                .action(
                    Button::new("playlist-create", "Crear")
                        .filled()
                        .disabled(!has_name)
                        .on_click(cx.listener(|this, _: &ClickEvent, _, cx| this.confirm(cx))),
                ),
            PlaylistDialogMode::Rename(_) => dialog
                .title("Renombrar playlist")
                .child(self.name.clone())
                .action(cancel)
                .action(
                    Button::new("playlist-rename", "Guardar")
                        .filled()
                        .disabled(!has_name)
                        .on_click(cx.listener(|this, _: &ClickEvent, _, cx| this.confirm(cx))),
                ),
            PlaylistDialogMode::Delete(playlist) => dialog
                .title("¿Eliminar la playlist?")
                .child(text(format!(
                    "«{}» se eliminará de tu cuenta de YouTube Music. No se puede deshacer.",
                    playlist.title
                )))
                .action(cancel)
                .action(
                    Button::new("playlist-delete", "Eliminar")
                        .filled()
                        .on_click(cx.listener(|this, _: &ClickEvent, _, cx| this.confirm(cx))),
                ),
            PlaylistDialogMode::Remove { playlist, track } => dialog
                .title("¿Quitar la canción?")
                .child(text(format!(
                    "«{}» se quitará de «{}».",
                    track.title, playlist.title
                )))
                .action(cancel)
                .action(
                    Button::new("playlist-remove", "Quitar")
                        .filled()
                        .on_click(cx.listener(|this, _: &ClickEvent, _, cx| this.confirm(cx))),
                ),
        }
    }
}
