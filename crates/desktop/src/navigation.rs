//! Where the user goes from inside a view: album and artist pages, and the song menu.

use gpui::{
    App, ClickEvent, Context, Div, Entity, EventEmitter, InteractiveElement, IntoElement,
    MouseButton, ParentElement, Pixels, Point, Window, div,
};
use gpui_m3::{Menu, MenuItem, context_menu};
use ytm_core::{Album, Artist, Track};

use crate::account::Account;
use crate::assets;
use crate::playback::Playback;
use crate::toasts;

pub enum NavigationEvent {
    /// The album page; the summary, when known, is shown while the album loads.
    OpenAlbum {
        id: String,
        summary: Option<Album>,
    },
    OpenArtist {
        id: String,
        summary: Option<Artist>,
    },
}

/// Views ask it to open pages (the app listens) and to show the song menu (the app draws it,
/// on top of everything, so only one is open at a time).
pub struct Navigation {
    track_menu: Option<(Point<Pixels>, Track)>,
}

impl EventEmitter<NavigationEvent> for Navigation {}

impl Navigation {
    pub fn new() -> Self {
        Navigation { track_menu: None }
    }

    pub fn open_album(&mut self, id: String, summary: Option<Album>, cx: &mut Context<Self>) {
        self.track_menu = None;
        cx.emit(NavigationEvent::OpenAlbum { id, summary });
        cx.notify();
    }

    pub fn open_artist(&mut self, id: String, summary: Option<Artist>, cx: &mut Context<Self>) {
        self.track_menu = None;
        cx.emit(NavigationEvent::OpenArtist { id, summary });
        cx.notify();
    }

    pub fn show_track_menu(
        &mut self,
        position: Point<Pixels>,
        track: Track,
        cx: &mut Context<Self>,
    ) {
        self.track_menu = Some((position, track));
        cx.notify();
    }

    pub fn close_track_menu(&mut self, cx: &mut Context<Self>) {
        if self.track_menu.take().is_some() {
            cx.notify();
        }
    }

    pub fn track_menu(&self) -> Option<&(Point<Pixels>, Track)> {
        self.track_menu.as_ref()
    }
}

/// Wraps a song row so a right click opens the song menu for `track`.
pub fn with_track_menu(
    row: impl IntoElement,
    track: &Track,
    navigation: &Entity<Navigation>,
) -> Div {
    let track = track.clone();
    let navigation = navigation.clone();
    div()
        .on_mouse_down(MouseButton::Right, move |event, _, cx| {
            let track = track.clone();
            navigation.update(cx, |navigation, cx| {
                navigation.show_track_menu(event.position, track, cx)
            });
        })
        .child(row)
}

type TrackAction = Box<dyn Fn(&Track, &mut Window, &mut App)>;

/// The open song menu: queueing, going to the album or artist, and liking when signed in.
pub fn render_track_menu(
    navigation: &Entity<Navigation>,
    playback: &Entity<Playback>,
    account: &Entity<Account>,
    cx: &App,
) -> Option<impl IntoElement> {
    let (position, track) = navigation.read(cx).track_menu()?.clone();
    let account_state = account.read(cx);
    let liked = account_state
        .is_signed_in()
        .then(|| account_state.is_liked(&track.id));

    // Every item acts on the song and closes the menu.
    let item = |id: &'static str, label: &'static str, icon: &'static str, action: TrackAction| {
        let navigation = navigation.clone();
        let track = track.clone();
        MenuItem::new(id, label)
            .icon(icon)
            .on_click(move |_: &ClickEvent, window, cx| {
                action(&track, window, cx);
                navigation.update(cx, |navigation, cx| navigation.close_track_menu(cx));
            })
    };

    let mut menu = Menu::new("track-menu")
        .width(240.)
        .direction(gpui_m3::PopDirection::Down);
    let to_playback = playback.clone();
    menu = menu.item(item(
        "play-next",
        "Reproducir a continuación",
        assets::SKIP_NEXT,
        Box::new(move |track, _, cx| {
            toasts::show(format!("«{}» suena a continuación", track.title), cx);
            let track = track.clone();
            to_playback.update(cx, |playback, cx| playback.play_next(track, cx));
        }),
    ));
    let to_playback = playback.clone();
    menu = menu.item(item(
        "add-to-queue",
        "Agregar a la cola",
        assets::QUEUE,
        Box::new(move |track, _, cx| {
            toasts::show(format!("«{}» agregada a la cola", track.title), cx);
            let track = track.clone();
            to_playback.update(cx, |playback, cx| playback.add_to_queue(track, cx));
        }),
    ));

    if track.album_id.is_some() || track.artist_id.is_some() {
        menu = menu.separator();
    }
    if let Some(album_id) = track.album_id.clone() {
        let to_navigation = navigation.clone();
        menu = menu.item(item(
            "go-to-album",
            "Ir al álbum",
            assets::ALBUM,
            Box::new(move |_, _, cx| {
                let album_id = album_id.clone();
                to_navigation.update(cx, |navigation, cx| {
                    navigation.open_album(album_id, None, cx)
                });
            }),
        ));
    }
    if let Some(artist_id) = track.artist_id.clone() {
        let to_navigation = navigation.clone();
        menu = menu.item(item(
            "go-to-artist",
            "Ir al artista",
            assets::PERSON,
            Box::new(move |_, _, cx| {
                let artist_id = artist_id.clone();
                to_navigation.update(cx, |navigation, cx| {
                    navigation.open_artist(artist_id, None, cx)
                });
            }),
        ));
    }

    if let Some(liked) = liked {
        let to_account = account.clone();
        menu = menu.separator().item(item(
            "like",
            if liked { "Quitar me gusta" } else { "Me gusta" },
            if liked {
                assets::HEART_FILL
            } else {
                assets::HEART
            },
            Box::new(move |track, _, cx| {
                toasts::show(
                    if liked {
                        "Quitada de «Me gusta»"
                    } else {
                        "Agregada a «Me gusta»"
                    },
                    cx,
                );
                let track = track.clone();
                to_account.update(cx, |account, cx| account.toggle_like(track, cx));
            }),
        ));
    }

    let navigation = navigation.clone();
    Some(context_menu(
        position,
        div()
            .id("track-menu-area")
            .occlude()
            .on_mouse_down_out(move |_, _, cx| {
                navigation.update(cx, |navigation, cx| navigation.close_track_menu(cx));
            })
            .child(menu),
    ))
}
