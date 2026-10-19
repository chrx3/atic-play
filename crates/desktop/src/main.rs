mod account;
mod app;
mod assets;
mod audio_wave;
mod covers;
mod google_sign_in;
mod home_view;
mod library_view;
mod lyrics_panel;
mod media_session;
mod now_playing_view;
mod playback;
mod player_bar;
mod playlist_view;
mod queue_panel;
mod search_view;
mod settings;
mod sidebar;
mod sign_in;
mod ui;

use std::path::{Path, PathBuf};
use std::process::ExitCode;

use gpui::{
    App, Application, Bounds, TitlebarOptions, WindowBounds, WindowOptions, prelude::*, px, size,
};
use gpui_m3::Theme;
use ytm_core::Client;

use crate::app::YtmApp;
use crate::settings::SavedBounds;

pub const APP_NAME: &str = "Atic-play";

/// Where the session and rustypipe's cache live: `%APPDATA%\atic-play` on Windows,
/// `$XDG_DATA_HOME/atic-play` (or `~/.local/share/atic-play`) elsewhere.
fn data_dir() -> PathBuf {
    let base = std::env::var_os("APPDATA")
        .or_else(|| std::env::var_os("XDG_DATA_HOME"))
        .map(PathBuf::from)
        .or_else(|| std::env::var_os("HOME").map(|home| PathBuf::from(home).join(".local/share")))
        .unwrap_or_else(std::env::temp_dir);
    base.join("atic-play")
}

/// Downloaded tracks kept on disk; about 600 songs at Premium quality.
const AUDIO_CACHE_BYTES: u64 = 2 * 1024 * 1024 * 1024;

/// The audio cache goes in `%LOCALAPPDATA%` on Windows: it is large and should not roam.
fn audio_cache_dir(data_dir: &Path) -> PathBuf {
    std::env::var_os("LOCALAPPDATA")
        .map(|local| PathBuf::from(local).join("atic-play"))
        .unwrap_or_else(|| data_dir.to_path_buf())
        .join("audio")
}

fn main() -> ExitCode {
    let args: Vec<_> = std::env::args_os().collect();
    if args.get(1).is_some_and(|arg| arg == google_sign_in::ARG) {
        return match (args.get(2), args.get(3)) {
            (Some(cookies), Some(profile)) => google_sign_in::run(cookies.into(), profile.into()),
            _ => ExitCode::from(google_sign_in::FAILED),
        };
    }

    Application::new()
        .with_assets(assets::Assets)
        .run(|cx: &mut App| {
            gpui_m3::init(cx);
            Theme::set(true, None, cx);

            let data_dir = data_dir();
            settings::init(&data_dir, cx);
            let client = match Client::new(&data_dir) {
                Ok(client) => {
                    client.with_audio_cache(audio_cache_dir(&data_dir), AUDIO_CACHE_BYTES)
                }
                Err(err) => {
                    eprintln!("could not start the YouTube Music client: {err:#}");
                    cx.quit();
                    return;
                }
            };

            let window_bounds = match settings::get(cx).window {
                Some(saved) => saved.to_window(),
                None => {
                    WindowBounds::Windowed(Bounds::centered(None, size(px(1280.), px(800.)), cx))
                }
            };
            cx.open_window(
                WindowOptions {
                    window_bounds: Some(window_bounds),
                    titlebar: Some(TitlebarOptions {
                        title: Some(APP_NAME.into()),
                        ..Default::default()
                    }),
                    ..Default::default()
                },
                |window, cx| {
                    window.on_window_should_close(cx, |window, cx| {
                        let bounds = SavedBounds::from_window(window.window_bounds());
                        settings::update(cx, |settings| settings.window = Some(bounds));
                        true
                    });
                    cx.new(|cx| YtmApp::new(client, data_dir, cx))
                },
            )
            .expect("could not open the main window");
            cx.on_window_closed(|cx| {
                if cx.windows().is_empty() {
                    cx.quit();
                }
            })
            .detach();
            cx.activate(true);
        });
    ExitCode::SUCCESS
}
