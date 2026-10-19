use gpui::{
    AnyView, ClickEvent, Context, Entity, EventEmitter, FontWeight, StyleRefinement, Subscription,
    Window, div, linear_color_stop, linear_gradient, prelude::*, px,
};
use gpui_m3::{IconButton, Shape, ShapeName, Theme};

use crate::account::Account;
use crate::covers::Covers;
use crate::lyrics_panel::LyricsPanel;
use crate::playback::Playback;
use crate::ui::{like_button, placeholder, track_cover};

const COVER_SIZE: u32 = 544;
const COVER_WIDTH: f32 = 360.;

pub enum NowPlayingEvent {
    Close,
}

/// Full-window "Reproduciendo": the cover in large, breathing, over an ambient background in the
/// cover's colors (the theme is seeded from it), with the lyrics beside it.
pub struct NowPlayingView {
    playback: Entity<Playback>,
    covers: Entity<Covers>,
    account: Entity<Account>,
    lyrics_panel: Entity<LyricsPanel>,
    _subscriptions: Vec<Subscription>,
}

impl EventEmitter<NowPlayingEvent> for NowPlayingView {}

impl NowPlayingView {
    pub fn new(
        playback: Entity<Playback>,
        covers: Entity<Covers>,
        account: Entity<Account>,
        lyrics_panel: Entity<LyricsPanel>,
        cx: &mut Context<Self>,
    ) -> Self {
        // Only a new track changes this view, not every progress tick.
        let mut last_track = None;
        let subscriptions = vec![
            cx.observe(&playback, move |_, playback, cx| {
                let track = playback.read(cx).current().map(|track| track.id.clone());
                if track != last_track {
                    last_track = track;
                    cx.notify();
                }
            }),
            cx.observe(&covers, |_, _, cx| cx.notify()),
            cx.observe(&account, |_, _, cx| cx.notify()),
        ];
        NowPlayingView {
            playback,
            covers,
            account,
            lyrics_panel,
            _subscriptions: subscriptions,
        }
    }
}

impl Render for NowPlayingView {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let s = *Theme::of(cx);
        let close = IconButton::new("close-now-playing", "chevron-down")
            .tooltip("Cerrar")
            .on_click(cx.listener(|_, _: &ClickEvent, _, cx| cx.emit(NowPlayingEvent::Close)));
        let Some(track) = self.playback.read(cx).current().cloned() else {
            return div()
                .size_full()
                .relative()
                .child(placeholder(
                    "Reproduciendo",
                    "Reproduce algo para empezar",
                    cx,
                ))
                .child(div().absolute().top_3().right_3().child(close))
                .into_any_element();
        };
        let cover_path = self.covers.update(cx, |covers, cx| {
            covers.get(&track.id, track.cover_url.as_deref(), COVER_SIZE, cx)
        });
        let cover = track_cover(
            "now-playing",
            &track.id,
            cover_path,
            (ShapeName::Cookie12, ShapeName::Sunny),
            COVER_WIDTH,
            window,
            cx,
        )
        .breathe(ShapeName::Cookie9, 8.);

        // Two large, slow shapes in the cover's tones drift behind everything.
        let ambient = div()
            .absolute()
            .inset_0()
            .overflow_hidden()
            .child(
                div()
                    .absolute()
                    .top(px(-220.))
                    .left(px(-180.))
                    .opacity(0.35)
                    .child(
                        Shape::new(ShapeName::Blob)
                            .size(px(720.))
                            .color(s.primary_container)
                            .breathe(ShapeName::Blob2, 11.)
                            .spin(90.),
                    ),
            )
            .child(
                div()
                    .absolute()
                    .bottom(px(-260.))
                    .right(px(-200.))
                    .opacity(0.3)
                    .child(
                        Shape::new(ShapeName::Blob2)
                            .size(px(640.))
                            .color(s.tertiary_container)
                            .breathe(ShapeName::Pebble, 13.)
                            .spin(-120.),
                    ),
            );

        let details = div()
            .w(px(COVER_WIDTH))
            .flex()
            .flex_col()
            .items_center()
            .gap_6()
            .child(cover)
            .child(
                div()
                    .w_full()
                    .flex()
                    .flex_col()
                    .items_center()
                    .gap_1()
                    .child(
                        div()
                            .w_full()
                            .text_center()
                            .text_size(px(28.))
                            .font_weight(FontWeight::BOLD)
                            .line_clamp(2)
                            .child(track.title.clone()),
                    )
                    .child(
                        div()
                            .w_full()
                            .text_center()
                            .text_size(px(18.))
                            .text_color(s.on_surface_variant)
                            .truncate()
                            .child(track.artists.clone()),
                    )
                    .children(track.album.clone().map(|album| {
                        div()
                            .w_full()
                            .text_center()
                            .text_sm()
                            .text_color(s.on_surface_variant)
                            .truncate()
                            .child(album)
                    })),
            )
            .children(like_button("like-now-playing", &track, &self.account, cx));

        div()
            .size_full()
            .relative()
            .overflow_hidden()
            .bg(linear_gradient(
                160.,
                linear_color_stop(s.primary_container.opacity(0.55), 0.),
                linear_color_stop(s.surface_container, 0.75),
            ))
            .child(ambient)
            .child(
                div()
                    .absolute()
                    .inset_0()
                    .flex()
                    .items_center()
                    .gap_12()
                    .px_12()
                    .py_8()
                    .child(div().flex_none().child(details))
                    .child(
                        div().flex_1().min_w_0().h_full().max_w(px(640.)).child(
                            AnyView::from(self.lyrics_panel.clone())
                                .cached(StyleRefinement::default().size_full()),
                        ),
                    ),
            )
            .child(div().absolute().top_3().right_3().child(close))
            .into_any_element()
    }
}
