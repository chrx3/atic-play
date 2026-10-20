//! The mini player: a small window that stays above the others, with the song playing and its
//! controls, for when the main window is out of the way.

use gpui::{
    App, Bounds, ClickEvent, Context, Entity, FocusHandle, Focusable, FontWeight, Global,
    MouseButton, Subscription, Window, WindowBackgroundAppearance, WindowBounds, WindowHandle,
    WindowKind, WindowOptions, div, point, prelude::*, px, size,
};
use gpui_m3::{IconButton, Theme};

use crate::app::YtmApp;
use crate::assets;
use crate::covers::Covers;
use crate::media_session::SeekDirection;
use crate::playback::Playback;
use crate::shortcuts::{
    self, Close, NextTrack, PreviousTrack, SeekBackward, SeekForward, ToggleMiniPlayer, ToggleMute,
    TogglePlay, VolumeDown, VolumeUp,
};
use crate::ui::thumbnail;

const WIDTH: f32 = 400.;
const HEIGHT: f32 = 112.;
/// Room left between the window and the screen edges (and the taskbar below).
const MARGIN: f32 = 24.;
const TASKBAR_ROOM: f32 = 56.;
const COVER: f32 = 80.;
const COVER_PX: u32 = 160;
const SEEK_STEP: std::time::Duration = std::time::Duration::from_secs(5);
const VOLUME_STEP: f32 = 0.05;

struct MiniPlayerWindow(WindowHandle<MiniPlayer>);

impl Global for MiniPlayerWindow {}

/// Opens the mini player, or closes it if it is open.
pub fn toggle(playback: Entity<Playback>, covers: Entity<Covers>, cx: &mut App) {
    if let Some(MiniPlayerWindow(handle)) = cx.try_global::<MiniPlayerWindow>() {
        let handle = *handle;
        cx.remove_global::<MiniPlayerWindow>();
        if handle
            .update(cx, |_, window, _| window.remove_window())
            .is_ok()
        {
            return;
        }
        // It was already closed (from the taskbar, say): open a new one.
    }

    let display = cx.primary_display();
    let origin = display.map_or(point(px(MARGIN), px(MARGIN)), |display| {
        let area = display.bounds();
        point(
            area.origin.x + area.size.width - px(WIDTH + MARGIN),
            area.origin.y + area.size.height - px(HEIGHT + MARGIN + TASKBAR_ROOM),
        )
    });
    let options = WindowOptions {
        window_bounds: Some(WindowBounds::Windowed(Bounds::new(
            origin,
            size(px(WIDTH), px(HEIGHT)),
        ))),
        // No title bar and no taskbar button: it is a widget, moved by dragging its content.
        titlebar: None,
        kind: WindowKind::PopUp,
        is_resizable: false,
        is_minimizable: false,
        window_background: WindowBackgroundAppearance::Opaque,
        ..Default::default()
    };
    match cx.open_window(options, |_, cx| {
        cx.new(|cx| MiniPlayer::new(playback, covers, cx))
    }) {
        Ok(handle) => cx.set_global(MiniPlayerWindow(handle)),
        Err(err) => log::warn!("could not open the mini player: {err:#}"),
    }
}

pub struct MiniPlayer {
    playback: Entity<Playback>,
    covers: Entity<Covers>,
    focus_handle: FocusHandle,
    /// The window is set to stay on top and takes the keyboard on its first frame.
    set_up: bool,
    _subscriptions: Vec<Subscription>,
}

impl MiniPlayer {
    fn new(playback: Entity<Playback>, covers: Entity<Covers>, cx: &mut Context<Self>) -> Self {
        let subscriptions = vec![
            cx.observe(&playback, |_, _, cx| cx.notify()),
            cx.observe(&covers, |_, _, cx| cx.notify()),
        ];
        MiniPlayer {
            playback,
            covers,
            focus_handle: cx.focus_handle(),
            set_up: false,
            _subscriptions: subscriptions,
        }
    }

    fn update_playback(
        &self,
        cx: &mut Context<Self>,
        f: impl FnOnce(&mut Playback, &mut Context<Playback>),
    ) {
        self.playback.update(cx, f);
    }

    fn on_playback(
        &self,
        action: impl Fn(&mut Playback, &mut Context<Playback>) + 'static,
        cx: &mut Context<Self>,
    ) -> impl Fn(&ClickEvent, &mut Window, &mut App) + 'static {
        cx.listener(move |this, _: &ClickEvent, _, cx| this.update_playback(cx, &action))
    }
}

impl Focusable for MiniPlayer {
    fn focus_handle(&self, _: &App) -> FocusHandle {
        self.focus_handle.clone()
    }
}

impl Render for MiniPlayer {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        if !std::mem::replace(&mut self.set_up, true) {
            keep_on_top(window);
            window.focus(&self.focus_handle);
        }
        let s = *Theme::of(cx);
        let playback = self.playback.read(cx);
        let track = playback.current().cloned();
        let playing = playback.is_playing();
        let loading = playback.is_loading();
        let progress = playback.duration().map_or(0., |duration| {
            (playback.position().as_secs_f32() / duration.as_secs_f32().max(1.)).clamp(0., 1.)
        });
        let cover = track.as_ref().and_then(|track| {
            self.covers.update(cx, |covers, cx| {
                covers.get(&track.id, track.cover_url.as_deref(), COVER_PX, cx)
            })
        });
        let (title, artists) = match &track {
            Some(track) => (track.title.clone(), track.artists.clone()),
            None => ("Nada sonando".to_string(), String::new()),
        };
        let has_track = track.is_some();

        // The cover and the text move the window, like a title bar.
        let info = div()
            .flex_1()
            .min_w_0()
            .h_full()
            .flex()
            .items_center()
            .gap_3()
            .on_mouse_down(MouseButton::Left, |_, window, _| window.start_window_move())
            .child(thumbnail(cover, COVER, 16., cx))
            .child(
                div()
                    .flex_1()
                    .min_w_0()
                    .flex()
                    .flex_col()
                    .gap_1()
                    .child(
                        div()
                            .font_weight(FontWeight::MEDIUM)
                            .truncate()
                            .child(title),
                    )
                    .child(
                        div()
                            .text_sm()
                            .text_color(s.on_surface_variant)
                            .truncate()
                            .child(artists),
                    )
                    // Moved on each playback tick, not every frame: the widget stays cheap.
                    .child(
                        div()
                            .mt_1()
                            .h(px(4.))
                            .w_full()
                            .rounded_full()
                            .bg(s.surface_container_highest)
                            .child(
                                div()
                                    .h_full()
                                    .w(gpui::relative(progress))
                                    .rounded_full()
                                    .bg(s.primary),
                            ),
                    ),
            );

        let controls = div()
            .flex()
            .flex_col()
            .items_end()
            .gap_1()
            .child(
                div()
                    .flex()
                    .gap_1()
                    .child(
                        IconButton::new("mini-open-main", "monitor")
                            .size(px(28.))
                            .tooltip("Abrir Atic-play")
                            .on_click(|_: &ClickEvent, _, cx| show_main_window(cx)),
                    )
                    .child(
                        IconButton::new("mini-close", "x")
                            .size(px(28.))
                            .tooltip("Cerrar mini reproductor")
                            .on_click(|_: &ClickEvent, window, cx| {
                                cx.remove_global::<MiniPlayerWindow>();
                                window.remove_window();
                            }),
                    ),
            )
            .child(
                div()
                    .flex()
                    .items_center()
                    .gap_1()
                    .child(
                        IconButton::new("mini-previous", assets::SKIP_PREVIOUS)
                            .disabled(!has_track)
                            .tooltip("Anterior")
                            .on_click(self.on_playback(Playback::previous, cx)),
                    )
                    .child(
                        IconButton::new(
                            "mini-play",
                            if playing { assets::PAUSE } else { assets::PLAY },
                        )
                        .filled()
                        .hover_morph(true)
                        .size(px(44.))
                        .disabled(!has_track || loading)
                        .tooltip(if playing { "Pausar" } else { "Reproducir" })
                        .on_click(self.on_playback(Playback::toggle_pause, cx)),
                    )
                    .child(
                        IconButton::new("mini-next", assets::SKIP_NEXT)
                            .disabled(!has_track)
                            .tooltip("Siguiente")
                            .on_click(self.on_playback(Playback::next, cx)),
                    ),
            );

        div()
            .track_focus(&self.focus_handle)
            .key_context(shortcuts::KEY_CONTEXT)
            .on_action(cx.listener(|this, _: &TogglePlay, _, cx| {
                this.update_playback(cx, Playback::toggle_pause)
            }))
            .on_action(
                cx.listener(|this, _: &NextTrack, _, cx| this.update_playback(cx, Playback::next)),
            )
            .on_action(cx.listener(|this, _: &PreviousTrack, _, cx| {
                this.update_playback(cx, Playback::previous)
            }))
            .on_action(cx.listener(|this, _: &SeekForward, _, cx| {
                this.update_playback(cx, |playback, cx| {
                    playback.seek_by(SeekDirection::Forward, SEEK_STEP, cx)
                })
            }))
            .on_action(cx.listener(|this, _: &SeekBackward, _, cx| {
                this.update_playback(cx, |playback, cx| {
                    playback.seek_by(SeekDirection::Backward, SEEK_STEP, cx)
                })
            }))
            .on_action(cx.listener(|this, _: &VolumeUp, _, cx| {
                this.update_playback(cx, |playback, cx| {
                    playback.set_volume(playback.volume() + VOLUME_STEP, cx)
                })
            }))
            .on_action(cx.listener(|this, _: &VolumeDown, _, cx| {
                this.update_playback(cx, |playback, cx| {
                    playback.set_volume(playback.volume() - VOLUME_STEP, cx)
                })
            }))
            .on_action(cx.listener(|this, _: &ToggleMute, _, cx| {
                this.update_playback(cx, Playback::toggle_mute)
            }))
            .on_action(|_: &Close, window, cx| {
                cx.remove_global::<MiniPlayerWindow>();
                window.remove_window();
            })
            .on_action(|_: &ToggleMiniPlayer, window, cx| {
                cx.remove_global::<MiniPlayerWindow>();
                window.remove_window();
            })
            .size_full()
            .p_3()
            .flex()
            .items_center()
            .gap_3()
            .bg(s.surface_container)
            .text_color(s.on_surface)
            .font_family(gpui_m3::theme::FONT_FAMILY)
            .child(info)
            .child(controls)
    }
}

/// Brings the main window to the front.
fn show_main_window(cx: &mut App) {
    let main = cx
        .windows()
        .into_iter()
        .find_map(|window| window.downcast::<YtmApp>());
    if let Some(main) = main {
        main.update(cx, |_, window, _| window.activate_window())
            .ok();
    }
}

/// Keeps the window above all others. GPUI's pop-up windows have no taskbar button on Windows,
/// but are not kept on top.
#[cfg(windows)]
fn keep_on_top(window: &Window) {
    use raw_window_handle::{HasWindowHandle, RawWindowHandle};

    #[link(name = "user32")]
    unsafe extern "system" {
        fn SetWindowPos(
            hwnd: isize,
            insert_after: isize,
            x: i32,
            y: i32,
            width: i32,
            height: i32,
            flags: u32,
        ) -> i32;
    }
    const HWND_TOPMOST: isize = -1;
    const SWP_NOSIZE: u32 = 0x0001;
    const SWP_NOMOVE: u32 = 0x0002;
    const SWP_NOACTIVATE: u32 = 0x0010;

    let Ok(handle) = HasWindowHandle::window_handle(window) else {
        return;
    };
    if let RawWindowHandle::Win32(handle) = handle.as_raw() {
        // SAFETY: the handle belongs to this live window; only its z-order changes.
        let placed = unsafe {
            SetWindowPos(
                handle.hwnd.get(),
                HWND_TOPMOST,
                0,
                0,
                0,
                0,
                SWP_NOSIZE | SWP_NOMOVE | SWP_NOACTIVATE,
            )
        };
        if placed == 0 {
            log::warn!("could not keep the mini player on top");
        }
    }
}

#[cfg(not(windows))]
fn keep_on_top(_: &Window) {}
