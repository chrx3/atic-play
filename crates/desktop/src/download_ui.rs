//! The download toggle of playlist and album pages, and the mark on downloaded songs.

use gpui::{AnyElement, App, ClickEvent, ElementId, Entity, SharedString, div, prelude::*, px};
use gpui_m3::{Icon, IconButton, Ring, Theme, Tooltip};
use ytm_core::Track;

use crate::assets;
use crate::downloads::Downloads;

/// Downloads the collection `id` (a playlist or album) on click. While its songs download it
/// shows a ring with the progress, and once they are all on disk a check; clicking either one
/// stops keeping it offline.
pub fn download_button(
    id: &str,
    title: &str,
    tracks: &[Track],
    downloads: &Entity<Downloads>,
    cx: &App,
) -> AnyElement {
    let state = downloads.read(cx).state(id);
    let remove = {
        let downloads = downloads.clone();
        let id = id.to_string();
        move |_: &ClickEvent, _: &mut gpui::Window, cx: &mut App| {
            downloads.update(cx, |downloads, cx| downloads.remove(&id, cx));
        }
    };

    match state {
        None => {
            let downloads = downloads.clone();
            let (id, title, tracks) = (id.to_string(), title.to_string(), tracks.to_vec());
            IconButton::new("download", assets::DOWNLOAD)
                .size(px(48.))
                .disabled(tracks.is_empty())
                .tooltip("Descargar")
                .on_click(move |_: &ClickEvent, _, cx| {
                    let (id, title, tracks) = (id.clone(), title.clone(), tracks.clone());
                    downloads.update(cx, |downloads, cx| {
                        downloads.download(id, title, tracks, cx)
                    });
                })
                .into_any_element()
        }
        Some(state) if state.active => {
            let s = *Theme::of(cx);
            let progress = state.done as f32 / state.total.max(1) as f32;
            let label: SharedString = format!(
                "Descargando {} de {} · Clic para cancelar",
                state.done, state.total
            )
            .into();
            div()
                .id("download-progress")
                .h(px(48.))
                .px_2()
                .flex()
                .items_center()
                .gap_2()
                .rounded_full()
                .cursor_pointer()
                .hover(move |style| style.bg(s.surface_container_high))
                .tooltip(Tooltip::text(label))
                .on_click(remove)
                .child(
                    Ring::new("download-ring", progress)
                        .size(px(28.))
                        .stroke(3.),
                )
                .child(
                    div()
                        .text_sm()
                        .text_color(s.on_surface_variant)
                        .child(format!("{}/{}", state.done, state.total)),
                )
                .into_any_element()
        }
        Some(_) => IconButton::new("download", assets::DOWNLOAD_DONE)
            .size(px(48.))
            .selected(true)
            .tooltip("Descargada · Clic para quitar la descarga")
            .on_click(remove)
            .into_any_element(),
    }
}

/// A small mark for a song kept offline; `None` for the rest.
pub fn downloaded_mark(
    id: impl Into<ElementId>,
    track_id: &str,
    downloads: &Entity<Downloads>,
    cx: &App,
) -> Option<AnyElement> {
    if !downloads.read(cx).is_downloaded(track_id) {
        return None;
    }
    Some(
        div()
            .id(id)
            .flex_none()
            .tooltip(Tooltip::text("Descargada"))
            .child(
                Icon::new(assets::OFFLINE)
                    .size(px(16.))
                    .color(Theme::of(cx).primary),
            )
            .into_any_element(),
    )
}
