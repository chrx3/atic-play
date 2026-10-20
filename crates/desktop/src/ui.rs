//! Small building blocks shared by the views.

use std::path::PathBuf;

use std::f32::consts::TAU;

use gpui::{
    AnyElement, App, ClickEvent, Div, ElementId, Entity, FontWeight, Hsla, ObjectFit, SharedString,
    Stateful, StyledImage, Window, div, img, linear_color_stop, linear_gradient, prelude::*, px,
};
use gpui_m3::{Button, IconButton, LoadingIndicator, Shape, ShapeName, Spring, Theme, motion};
use ytm_core::Track;

use crate::account::Account;
use crate::assets;
use crate::playback::Playback;

pub fn format_time(total_secs: u64) -> String {
    format!("{}:{:02}", total_secs / 60, total_secs % 60)
}

/// A heart that likes or unlikes `track`, filled when liked; `None` while signed out.
pub fn like_button(
    id: impl Into<ElementId>,
    track: &Track,
    account: &Entity<Account>,
    cx: &App,
) -> Option<IconButton> {
    let account_state = account.read(cx);
    if !account_state.is_signed_in() {
        return None;
    }
    let liked = account_state.is_liked(&track.id);
    let account = account.clone();
    let track = track.clone();
    Some(
        IconButton::new(
            id,
            if liked {
                assets::HEART_FILL
            } else {
                assets::HEART
            },
        )
        .selected(liked)
        .tooltip(if liked { "Quitar me gusta" } else { "Me gusta" })
        .on_click(move |_: &ClickEvent, _, cx| {
            let track = track.clone();
            account.update(cx, |account, cx| account.toggle_like(track, cx));
        }),
    )
}

/// A square cover, or a tonal placeholder while it downloads.
pub fn thumbnail(path: Option<PathBuf>, size: f32, radius: f32, cx: &App) -> AnyElement {
    match path {
        // Covers are square, but video thumbnails (songs read from a YouTube playlist) are 16:9:
        // crop them to the square in their middle, where the artwork is.
        // GPUI does not clip a covering image to its bounds, so the box around it does.
        Some(path) => div()
            .size(px(size))
            .flex_none()
            .rounded(px(radius))
            .overflow_hidden()
            .child(
                img(path)
                    .size_full()
                    .rounded(px(radius))
                    .object_fit(ObjectFit::Cover),
            )
            .into_any_element(),
        None => div()
            .size(px(size))
            .flex_none()
            .rounded(px(radius))
            .bg(Theme::of(cx).surface_container_highest)
            .into_any_element(),
    }
}

/// Whether `track_id` is the song playing (or loaded in the player).
pub fn is_current_track(playback: &Entity<Playback>, track_id: &str, cx: &App) -> bool {
    playback
        .read(cx)
        .current()
        .is_some_and(|current| current.id == track_id)
}

/// Marks the row of the song that is playing: a tint of the accent color and a bar at its
/// start. Static on purpose, so long lists do not redraw every frame.
pub fn mark_playing(row: Div, playing: bool, cx: &App) -> Div {
    if !playing {
        return row;
    }
    let s = Theme::of(cx);
    row.relative()
        .rounded(px(12.))
        .bg(gpui_m3::theme::alpha(s.primary, 0.12))
        .child(
            div()
                .absolute()
                .left_0()
                .top(px(12.))
                .bottom(px(12.))
                .w(px(4.))
                .rounded_full()
                .bg(s.primary),
        )
}

/// Corner of the large covers in page headers: M3 Expressive's extra-large shape.
pub const HERO_COVER_RADIUS: f32 = 28.;

/// The header of an album, artist or playlist page: a panel washed with the color of its cover
/// (`seed`, once known), fading into the page.
pub fn hero(seed: Option<Hsla>, cx: &App) -> Div {
    let s = Theme::of(cx);
    let tint = seed.unwrap_or(s.primary_container);
    div()
        .mx_2()
        .mb_4()
        .p_6()
        .rounded(px(HERO_COVER_RADIUS + 8.))
        .bg(linear_gradient(
            180.,
            linear_color_stop(gpui_m3::theme::alpha(tint, 0.42), 0.),
            linear_color_stop(gpui_m3::theme::alpha(tint, 0.), 1.),
        ))
}

pub const CARD_WIDTH: f32 = 180.;
/// Cover size downloaded for cards (twice the card width for sharp HiDPI rendering).
pub const CARD_COVER_SIZE: u32 = 320;
/// Hover group of a [`media_card`], for parts shown only while it is hovered.
pub const MEDIA_CARD_GROUP: &str = "media-card";

/// A clickable album, artist or song card: artwork, title and an optional subtitle. `busy`
/// shows a loading indicator over the artwork while its tracks are fetched.
pub fn media_card(
    id: impl Into<ElementId>,
    artwork: impl IntoElement,
    title: impl Into<SharedString>,
    subtitle: Option<SharedString>,
    busy: bool,
    cx: &App,
) -> Stateful<Div> {
    let s = *Theme::of(cx);
    div()
        .id(id)
        .group(MEDIA_CARD_GROUP)
        .w(px(CARD_WIDTH + 24.))
        .p_3()
        .flex()
        .flex_col()
        .gap_1()
        .rounded(px(16.))
        .cursor_pointer()
        .hover(move |style| style.bg(s.surface_container_high))
        .child(div().relative().child(artwork).when(busy, |artwork| {
            artwork.child(
                div()
                    .absolute()
                    .inset_0()
                    .flex()
                    .items_center()
                    .justify_center()
                    .child(LoadingIndicator::new().contained(true).size(px(56.))),
            )
        }))
        .child(
            div()
                .pt_2()
                .w_full()
                .font_weight(FontWeight::MEDIUM)
                .truncate()
                .child(title.into()),
        )
        .children(subtitle.map(|subtitle| {
            div()
                .w_full()
                .text_sm()
                .text_color(s.on_surface_variant)
                .truncate()
                .child(subtitle)
        }))
}

/// The cover of the track `track_id` in the first of `shapes`. When the track changes, the new
/// cover pops in while its outline morphs toward the second shape and settles back. `slot` tells apart the places that
/// show the same track (element ids must be unique in a window).
pub fn track_cover(
    slot: &str,
    track_id: &str,
    path: Option<PathBuf>,
    (shape, morph): (ShapeName, ShapeName),
    size: f32,
    window: &mut Window,
    cx: &mut App,
) -> Shape {
    let id = SharedString::from(format!("{slot}-cover-{track_id}"));
    let [scale, turn] = motion::animate_from(
        ElementId::from((id.clone(), 0usize)),
        Spring::SPATIAL,
        [0.8, -0.4],
        [1., 0.],
        window,
        cx,
    );
    let arriving = motion::entrance(
        ElementId::from((id.clone(), 1usize)),
        Spring::EFFECTS,
        0.,
        window,
        cx,
    ) < 0.6;
    let cover = Shape::new(shape)
        .size(px(size))
        .scale(scale)
        .rotate(turn)
        .breathe_on_hover(ElementId::from((id, 2usize)), morph)
        .hovered(arriving);
    match path {
        Some(path) => cover.image(path.as_path()),
        None => cover.color(Theme::of(cx).surface_container_highest),
    }
}

/// A round artist picture, or a tonal circle while it downloads.
pub fn avatar(path: Option<PathBuf>, size: f32, cx: &App) -> Shape {
    let shape = Shape::new(ShapeName::Circle).size(px(size));
    match path {
        Some(path) => shape.image(path.as_path()),
        None => shape.color(Theme::of(cx).surface_container_highest),
    }
}

/// 1234567 → "1,2 M", as YouTube shows subscriber counts.
pub fn compact_count(count: u64) -> String {
    match count {
        0..1_000 => count.to_string(),
        1_000..1_000_000 => format!("{:.1} K", count as f64 / 1e3).replace('.', ","),
        _ => format!("{:.1} M", count as f64 / 1e6).replace('.', ","),
    }
}

/// Section title used across the views.
pub fn section_title(title: impl Into<SharedString>) -> Div {
    div()
        .text_size(px(20.))
        .font_weight(gpui::FontWeight::MEDIUM)
        .child(title.into())
}

/// A centered message for empty views and views still to come.
pub fn placeholder(title: impl Into<SharedString>, body: impl Into<SharedString>, cx: &App) -> Div {
    div()
        .size_full()
        .flex()
        .flex_col()
        .items_center()
        .justify_center()
        .gap_2()
        .child(section_title(title))
        .child(
            div()
                .text_color(Theme::of(cx).on_surface_variant)
                .child(body.into()),
        )
}

/// A load that failed: what failed, why in plain words, and a button to try again.
pub fn failed(
    title: impl Into<SharedString>,
    error: impl Into<SharedString>,
    on_retry: impl Fn(&ClickEvent, &mut Window, &mut App) + 'static,
    cx: &App,
) -> Div {
    placeholder(title, error, cx).child(
        div().pt_2().child(
            Button::new("retry", "Reintentar")
                .filled()
                .on_click(on_retry),
        ),
    )
}

/// What to tell the user about a failed request; the details go to the log.
pub fn friendly_error(err: &anyhow::Error) -> SharedString {
    log::warn!("{err:#}");
    let details = format!("{err:#}").to_lowercase();
    let offline = [
        "timed out",
        "timeout",
        "connect",
        "dns",
        "error sending request",
    ]
    .iter()
    .any(|sign| details.contains(sign));
    if offline {
        "No hay conexión con YouTube Music. Revisa tu internet e inténtalo de nuevo.".into()
    } else {
        "Algo salió mal al cargar. Inténtalo de nuevo en un momento.".into()
    }
}

/// Seconds per skeleton pulse.
const SKELETON_PERIOD: f32 = 1.6;
/// How far behind each skeleton block pulses, so the pulse sweeps down a list like a shimmer.
const SKELETON_STAGGER: f32 = 0.12;

/// A tonal block that pulses where content is still loading.
#[derive(IntoElement)]
pub struct Skeleton {
    width: Option<f32>,
    height: f32,
    radius: f32,
    order: usize,
}

/// A skeleton block `width` × `height` (full width when `width` is `None`).
pub fn skeleton(width: Option<f32>, height: f32, radius: f32) -> Skeleton {
    Skeleton {
        width,
        height,
        radius,
        order: 0,
    }
}

impl Skeleton {
    /// Its place in a list: later blocks pulse a little later.
    pub fn order(mut self, order: usize) -> Self {
        self.order = order;
        self
    }
}

impl RenderOnce for Skeleton {
    fn render(self, window: &mut Window, cx: &mut App) -> impl IntoElement {
        // 30 fps is plenty for a slow pulse; with reduced motion the clock stays at 0.
        let t = motion::continuous_at(30., window, cx);
        let phase = (t - self.order as f32 * SKELETON_STAGGER) / SKELETON_PERIOD * TAU;
        let opacity = 0.7 + 0.3 * phase.cos();
        div()
            .flex_none()
            .h(px(self.height))
            .map(|block| match self.width {
                Some(width) => block.w(px(width)),
                None => block.w_full(),
            })
            .rounded(px(self.radius))
            .bg(Theme::of(cx).surface_container_highest)
            .opacity(opacity)
    }
}

/// Skeleton song rows: a thumbnail and two lines of text each.
pub fn skeleton_rows(count: usize) -> Div {
    div().flex().flex_col().children((0..count).map(|index| {
        // Varied line lengths read as text rather than as a grid of bars.
        let title = 140. + (index * 53 % 120) as f32;
        div()
            .h(px(64.))
            .px_4()
            .flex()
            .items_center()
            .gap_4()
            .child(skeleton(Some(40.), 40., 6.).order(index))
            .child(
                div()
                    .flex()
                    .flex_col()
                    .gap_2()
                    .child(skeleton(Some(title), 12., 6.).order(index))
                    .child(skeleton(Some(title * 0.6), 10., 5.).order(index)),
            )
    }))
}

/// Skeleton cards for a wrapping grid of albums or artists. `round` for artist pictures.
pub fn skeleton_cards(count: usize, round: bool) -> Div {
    div()
        .flex()
        .flex_wrap()
        .gap_2()
        .children((0..count).map(|index| {
            div()
                .w(px(CARD_WIDTH + 24.))
                .p_3()
                .flex()
                .flex_col()
                .gap_2()
                .child(
                    skeleton(
                        Some(CARD_WIDTH),
                        CARD_WIDTH,
                        if round { CARD_WIDTH / 2. } else { 12. },
                    )
                    .order(index),
                )
                .child(
                    div()
                        .pt_1()
                        .child(skeleton(Some(CARD_WIDTH * 0.8), 12., 6.).order(index)),
                )
                .child(skeleton(Some(CARD_WIDTH * 0.5), 10., 5.).order(index))
        }))
}
