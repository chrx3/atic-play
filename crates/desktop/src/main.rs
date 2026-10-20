mod account;
mod album_view;
mod app;
mod artist_view;
mod assets;
mod audio_wave;
mod covers;
mod download_ui;
mod downloads;
mod google_sign_in;
mod history;
mod history_view;
mod home_view;
mod library_view;
mod lyrics_panel;
mod media_session;
mod navigation;
mod now_playing_view;
mod playback;
mod player_bar;
mod playlist_view;
mod queue_panel;
mod search_box;
mod search_view;
mod settings;
mod settings_view;
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

/// Where the audio cache and offline downloads go: `%LOCALAPPDATA%` on Windows, because they
/// are large and should not roam with the profile.
fn local_dir(data_dir: &Path) -> PathBuf {
    std::env::var_os("LOCALAPPDATA")
        .map(|local| PathBuf::from(local).join("atic-play"))
        .unwrap_or_else(|| data_dir.to_path_buf())
}

fn main() -> ExitCode {
    // Our own crates log what they do (cached or downloaded, itag, errors); dependencies only
    // warnings. rustypipe is off: it marks every request's tracing span at error level, which
    // would read as a failure on each call, and its real errors reach our logs anyway.
    // `RUST_LOG` overrides it.
    env_logger::Builder::from_env(
        env_logger::Env::default()
            .default_filter_or("warn,rustypipe=off,tracing::span=off,atic_play=info,ytm_core=info"),
    )
    .init();

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
            let local_dir = local_dir(&data_dir);
            let client = match Client::new(&data_dir) {
                Ok(client) => client.with_audio_cache(
                    local_dir.join("audio"),
                    local_dir.join("offline"),
                    AUDIO_CACHE_BYTES,
                ),
                Err(err) => {
                    log::error!("could not start the YouTube Music client: {err:#}");
                    cx.quit();
                    return;
                }
            };
            downloads::init(client.clone(), &data_dir, cx);
            history::init(&data_dir, cx);

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
