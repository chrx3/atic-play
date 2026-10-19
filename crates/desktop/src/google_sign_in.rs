//! The `--sign-in` mode: the app relaunches itself as a small browser window where the user
//! signs in to Google. Once YouTube Music has a session, its cookies are written as a Netscape
//! `cookies.txt` for the main process and the window closes.
//!
//! It runs as its own process because tao and GPUI each need to own the main thread's event loop.

use std::fmt::Write as _;
use std::path::{Path, PathBuf};
use std::process::ExitCode;
use std::time::{Duration, Instant};

use tao::dpi::LogicalSize;
use tao::event::{Event, StartCause, WindowEvent};
use tao::event_loop::{ControlFlow, EventLoop};
use tao::window::WindowBuilder;
use wry::cookie::Cookie;
use wry::{WebContext, WebViewBuilder};

/// `atic-play --sign-in <cookies file> <browser profile dir>`
pub const ARG: &str = "--sign-in";

/// Google's sign-in page, returning to YouTube Music, which sets the youtube.com session.
const START_URL: &str =
    "https://accounts.google.com/ServiceLogin?continue=https%3A%2F%2Fmusic.youtube.com%2F";
const YOUTUBE_MUSIC_URL: &str = "https://music.youtube.com";
/// How often the window checks whether the session exists yet.
const POLL: Duration = Duration::from_secs(1);

/// Exit codes the main process reads.
pub const SIGNED_IN: u8 = 0;
pub const CANCELLED: u8 = 1;
pub const FAILED: u8 = 2;

pub fn run(cookies_path: PathBuf, profile_dir: PathBuf) -> ExitCode {
    match open_window(cookies_path, profile_dir) {
        Ok(never) => match never {},
        Err(err) => {
            eprintln!("sign-in window: {err}");
            ExitCode::from(FAILED)
        }
    }
}

fn open_window(
    cookies_path: PathBuf,
    profile_dir: PathBuf,
) -> Result<std::convert::Infallible, Box<dyn std::error::Error>> {
    let event_loop = EventLoop::new();
    let window = WindowBuilder::new()
        .with_title(format!("Iniciar sesión · {}", crate::APP_NAME))
        .with_inner_size(LogicalSize::new(480.0, 720.0))
        .build(&event_loop)?;
    // A profile of its own: the session never mixes with the user's browsers.
    let mut context = WebContext::new(Some(profile_dir));
    let webview = WebViewBuilder::new_with_web_context(&mut context)
        .with_url(START_URL)
        .build(&window)?;

    event_loop.run(move |event, _, control_flow| {
        // The context must outlive the webview.
        let _ = &context;
        *control_flow = ControlFlow::WaitUntil(Instant::now() + POLL);
        match event {
            Event::WindowEvent {
                event: WindowEvent::CloseRequested,
                ..
            } => std::process::exit(CANCELLED.into()),
            Event::NewEvents(StartCause::ResumeTimeReached { .. }) => {
                let cookies = match webview.cookies_for_url(YOUTUBE_MUSIC_URL) {
                    Ok(cookies) => cookies,
                    Err(err) => {
                        eprintln!("could not read cookies: {err}");
                        return;
                    }
                };
                if !cookies.iter().any(is_session_cookie) {
                    return;
                }
                let code = match write_cookies(&cookies_path, &cookies) {
                    Ok(()) => SIGNED_IN,
                    Err(err) => {
                        eprintln!("could not save the session: {err}");
                        FAILED
                    }
                };
                std::process::exit(code.into());
            }
            _ => {}
        }
    })
}

/// The cookie YouTube signs API requests with; it exists only once signed in.
fn is_session_cookie(cookie: &Cookie) -> bool {
    matches!(cookie.name(), "SAPISID" | "__Secure-3PAPISID")
        && cookie
            .domain()
            .is_some_and(|domain| domain.ends_with("youtube.com"))
}

fn write_cookies(path: &Path, cookies: &[Cookie]) -> std::io::Result<()> {
    std::fs::write(path, netscape_cookies(cookies))
}

/// Formats cookies as a Netscape `cookies.txt`, the format rustypipe and yt-dlp read.
fn netscape_cookies(cookies: &[Cookie]) -> String {
    let mut out = String::from("# Netscape HTTP Cookie File\n");
    for cookie in cookies {
        let Some(domain) = cookie.domain() else {
            continue;
        };
        // The cookie crate drops the leading dot; YouTube's cookies apply to subdomains.
        let domain = if domain.starts_with('.') {
            domain.to_string()
        } else {
            format!(".{domain}")
        };
        let expires = cookie
            .expires_datetime()
            .map_or(0, |expires| expires.unix_timestamp().max(0));
        let _ = writeln!(
            out,
            "{domain}\tTRUE\t{path}\t{secure}\t{expires}\t{name}\t{value}",
            path = cookie.path().unwrap_or("/"),
            secure = if cookie.secure().unwrap_or(false) {
                "TRUE"
            } else {
                "FALSE"
            },
            name = cookie.name(),
            value = cookie.value(),
        );
    }
    out
}
