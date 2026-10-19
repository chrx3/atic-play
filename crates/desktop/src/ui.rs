//! Small building blocks shared by the views.

use std::path::PathBuf;

use gpui::{
    AnyElement, App, Div, ElementId, FontWeight, ObjectFit, SharedString, Stateful, StyledImage,
    div, img, prelude::*, px,
};
use gpui_m3::{LoadingIndicator, Shape, ShapeName, Theme};

pub fn format_time(total_secs: u64) -> String {
    format!("{}:{:02}", total_secs / 60, total_secs % 60)
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

pub const CARD_WIDTH: f32 = 180.;
/// Cover size downloaded for cards (twice the card width for sharp HiDPI rendering).
pub const CARD_COVER_SIZE: u32 = 320;

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
