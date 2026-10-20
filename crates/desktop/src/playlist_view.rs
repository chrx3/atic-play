use std::ops::Range;

use gpui::{
    AnyElement, ClickEvent, Context, Entity, FontWeight, SharedString, Subscription, Window, div,
    prelude::*, px, uniform_list,
};
use gpui_m3::{IconButton, ListItem, Theme};
use ytm_core::{Client, Playlist, PlaylistPage, Track};

use crate::account::{Account, AccountEvent};
use crate::assets;
use crate::covers::Covers;
use crate::download_ui::{download_button, downloaded_mark};
use crate::downloads::Downloads;
use crate::navigation::{Navigation, with_playlist_track_menu};
use crate::playback::Playback;
use crate::playlist_dialog::PlaylistDialogMode;
use crate::search_view::observe_opening;
use crate::ui::{
    CARD_COVER_SIZE, HERO_COVER_RADIUS, failed, format_time, friendly_error, hero,
    is_current_track, mark_playing, placeholder, skeleton_rows, thumbnail,
};

const THUMB_SIZE: u32 = 120;
const COVER_WIDTH: f32 = 200.;

enum State {
    Loading,
    Ready(PlaylistPage),
    Failed(SharedString),
}

/// A playlist page: header with play and shuffle, then its tracks.
pub struct PlaylistView {
    client: Client,
    playback: Entity<Playback>,
    covers: Entity<Covers>,
    navigation: Entity<Navigation>,
    account: Entity<Account>,
    downloads: Entity<Downloads>,
    /// What the sidebar or library already knew, shown while the tracks load.
    summary: Option<Playlist>,
    state: State,
    /// Bumped on every `open`, so a slow playlist cannot replace a newer one.
    generation: u64,
    _subscriptions: Vec<Subscription>,
}

impl PlaylistView {
    pub fn new(
        client: Client,
        playback: Entity<Playback>,
        covers: Entity<Covers>,
        navigation: Entity<Navigation>,
        account: Entity<Account>,
        cx: &mut Context<Self>,
    ) -> Self {
        let downloads = Downloads::global(cx);
        let subscriptions = vec![
            cx.observe(&covers, |_, _, cx| cx.notify()),
            observe_opening(&playback, cx),
            // Whether the playlist is the user's own (rename, delete, take songs out).
            cx.observe(&account, |_, _, cx| cx.notify()),
            // Show a change made to this playlist.
            cx.subscribe(&account, |this, _, event: &AccountEvent, cx| {
                if let AccountEvent::PlaylistChanged(id) = event
                    && let Some(playlist) = this.summary.clone().filter(|p| p.id == *id)
                {
                    this.reload(playlist, cx);
                }
            }),
            // Download progress and the marks on downloaded songs.
            cx.observe(&downloads, |_, _, cx| cx.notify()),
        ];
        PlaylistView {
            client,
            playback,
            covers,
            navigation,
            account,
            downloads,
            summary: None,
            state: State::Loading,
            generation: 0,
            _subscriptions: subscriptions,
        }
    }

    pub fn open(&mut self, playlist: Playlist, cx: &mut Context<Self>) {
        self.state = State::Loading;
        self.reload(playlist, cx);
    }

    /// Loads the playlist again, keeping what is on screen until it arrives.
    fn reload(&mut self, playlist: Playlist, cx: &mut Context<Self>) {
        self.generation += 1;
        let generation = self.generation;
        let request = self.client.playlist(playlist.id.clone());
        self.summary = Some(playlist);
        cx.notify();

        cx.spawn(async move |this, cx| {
            let page = request.await;
            this.update(cx, |this, cx| {
                if this.generation != generation {
                    return;
                }
                this.state = match page {
                    Ok(page) => State::Ready(page),
                    Err(err) => State::Failed(friendly_error(&err)),
                };
                cx.notify();
            })
            .ok();
        })
        .detach();
    }

    fn tracks(&self) -> &[Track] {
        match &self.state {
            State::Ready(page) => &page.tracks,
            _ => &[],
        }
    }

    fn render_rows(&mut self, range: Range<usize>, cx: &mut Context<Self>) -> Vec<AnyElement> {
        let s = *Theme::of(cx);
        let tracks: Vec<Track> = self.tracks()[range.clone()].to_vec();
        tracks
            .into_iter()
            .zip(range)
            .map(|(track, index)| {
                let thumb = self.covers.update(cx, |covers, cx| {
                    covers.get(&track.id, track.cover_url.as_deref(), THUMB_SIZE, cx)
                });
                let mut subtitle = vec![track.artists.clone()];
                subtitle.extend(track.album.clone());
                let row = ListItem::new(("playlist-track", index), track.title.clone())
                    .subtitle(subtitle.join(" · "))
                    .leading(
                        div()
                            .flex()
                            .items_center()
                            .gap_3()
                            .child(
                                div()
                                    .w(px(32.))
                                    .flex()
                                    .justify_end()
                                    .text_sm()
                                    .text_color(s.on_surface_variant)
                                    .child((index + 1).to_string()),
                            )
                            .child(thumbnail(thumb, 40., 6., cx)),
                    )
                    .trailing(
                        div()
                            .flex()
                            .items_center()
                            .gap_3()
                            .children(downloaded_mark(
                                ("playlist-downloaded", index),
                                &track.id,
                                &self.downloads,
                                cx,
                            ))
                            .child(
                                div().text_sm().text_color(s.on_surface_variant).child(
                                    track
                                        .duration_secs
                                        .map(|secs| format_time(secs.into()))
                                        .unwrap_or_default(),
                                ),
                            ),
                    )
                    .on_click(cx.listener(move |this, _: &ClickEvent, _, cx| {
                        let tracks = this.tracks().to_vec();
                        this.playback
                            .update(cx, |playback, cx| playback.play_tracks(tracks, index, cx));
                    }));
                let playing = is_current_track(&self.playback, &track.id, cx);
                let row =
                    with_playlist_track_menu(row, &track, self.summary.as_ref(), &self.navigation);
                mark_playing(row, playing, cx).into_any_element()
            })
            .collect()
    }

    fn render_header(&self, cx: &mut Context<Self>) -> impl IntoElement {
        let s = *Theme::of(cx);
        let page = match &self.state {
            State::Ready(page) => Some(page),
            _ => None,
        };
        // The loaded page knows more (count, cover) than the summary it was opened with.
        let playlist = page.map(|page| &page.playlist).or(self.summary.as_ref());
        let cover = playlist.and_then(|playlist| {
            self.covers.update(cx, |covers, cx| {
                covers.get(
                    &playlist.id,
                    playlist.cover_url.as_deref(),
                    CARD_COVER_SIZE,
                    cx,
                )
            })
        });
        let title = playlist
            .map(|playlist| playlist.title.clone())
            .unwrap_or_default();
        let mut meta = Vec::new();
        if let Some(owner) = playlist.and_then(|playlist| playlist.owner.clone()) {
            meta.push(owner);
        }
        if let Some(page) = page {
            meta.push(format!("{} canciones", page.tracks.len()));
        }
        let has_tracks = page.is_some_and(|page| !page.tracks.is_empty());
        // Only the user's own playlists can be renamed and deleted.
        let editable = playlist
            .filter(|playlist| self.account.read(cx).is_editable(&playlist.id))
            .cloned();

        let seed = cover
            .as_deref()
            .and_then(|cover| self.covers.update(cx, |covers, cx| covers.seed(cover, cx)));
        hero(seed, cx)
            .flex()
            .items_end()
            .gap_6()
            .child(
                div()
                    .rounded(px(HERO_COVER_RADIUS))
                    .shadow_lg()
                    .child(thumbnail(cover, COVER_WIDTH, HERO_COVER_RADIUS, cx)),
            )
            .child(
                div()
                    .flex_1()
                    .min_w_0()
                    .flex()
                    .flex_col()
                    .gap_2()
                    .child(
                        div()
                            .text_sm()
                            .text_color(s.on_surface_variant)
                            .child("Playlist"),
                    )
                    .child(
                        div()
                            .text_size(px(32.))
                            .font_weight(FontWeight::BOLD)
                            .truncate()
                            .child(title),
                    )
                    .child(
                        div()
                            .text_sm()
                            .text_color(s.on_surface_variant)
                            .child(meta.join(" · ")),
                    )
                    .child(
                        div()
                            .pt_2()
                            .flex()
                            .items_center()
                            .gap_3()
                            .child(
                                IconButton::new("playlist-play", assets::PLAY)
                                    .filled()
                                    .hover_morph(true)
                                    .size(px(56.))
                                    .disabled(!has_tracks)
                                    .tooltip("Reproducir")
                                    .on_click(cx.listener(|this, _: &ClickEvent, _, cx| {
                                        let tracks = this.tracks().to_vec();
                                        this.playback.update(cx, |playback, cx| {
                                            playback.play_tracks(tracks, 0, cx)
                                        });
                                    })),
                            )
                            .child(
                                IconButton::new("playlist-shuffle", assets::SHUFFLE)
                                    .tonal()
                                    .size(px(48.))
                                    .disabled(!has_tracks)
                                    .tooltip("Reproducir en aleatorio")
                                    .on_click(cx.listener(|this, _: &ClickEvent, _, cx| {
                                        let tracks = this.tracks().to_vec();
                                        this.playback.update(cx, |playback, cx| {
                                            playback.play_shuffled(tracks, cx)
                                        });
                                    })),
                            )
                            .children(page.filter(|_| has_tracks).map(|page| {
                                download_button(
                                    &page.playlist.id,
                                    &page.playlist.title,
                                    &page.tracks,
                                    &self.downloads,
                                    cx,
                                )
                            }))
                            .children(editable.clone().map(|playlist| {
                                let navigation = self.navigation.clone();
                                IconButton::new("playlist-rename", "pen")
                                    .tooltip("Renombrar")
                                    .on_click(move |_: &ClickEvent, _, cx| {
                                        let mode = PlaylistDialogMode::Rename(playlist.clone());
                                        navigation.update(cx, |navigation, cx| {
                                            navigation.edit_playlists(mode, cx)
                                        });
                                    })
                            }))
                            .children(editable.map(|playlist| {
                                let navigation = self.navigation.clone();
                                IconButton::new("playlist-delete", "trash")
                                    .tooltip("Eliminar playlist")
                                    .on_click(move |_: &ClickEvent, _, cx| {
                                        let mode = PlaylistDialogMode::Delete(playlist.clone());
                                        navigation.update(cx, |navigation, cx| {
                                            navigation.edit_playlists(mode, cx)
                                        });
                                    })
                            })),
                    ),
            )
    }
}

impl Render for PlaylistView {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        if self.summary.is_none() {
            return placeholder("Playlist", "Elige una playlist de la barra lateral", cx)
                .into_any_element();
        }
        let s = *Theme::of(cx);
        let body = match &self.state {
            State::Loading => div().px_4().child(skeleton_rows(10)).into_any_element(),
            State::Failed(error) => failed(
                "No se pudo abrir la playlist",
                error.clone(),
                cx.listener(|this, _: &ClickEvent, _, cx| {
                    if let Some(playlist) = this.summary.clone() {
                        this.open(playlist, cx);
                    }
                }),
                cx,
            )
            .pt_16()
            .into_any_element(),
            State::Ready(page) if page.tracks.is_empty() => div()
                .pt_16()
                .flex()
                .justify_center()
                .text_color(s.on_surface_variant)
                .child("Esta playlist está vacía")
                .into_any_element(),
            State::Ready(page) => uniform_list(
                "playlist-tracks",
                page.tracks.len(),
                cx.processor(|this, range, _, cx| this.render_rows(range, cx)),
            )
            .flex_1()
            .px_4()
            .pb_4()
            .into_any_element(),
        };
        div()
            .size_full()
            .flex()
            .flex_col()
            .child(self.render_header(cx))
            .child(body)
            .into_any_element()
    }
}
