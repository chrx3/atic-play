//! The login window of Atic Play: Google in a web view, then the session in the keyring.
//!
//! The window opens the sign in page of Google. When the user is done, the navigation reaches
//! `music.youtube.com` and the account is in the cookies of the web view: we keep the ones we
//! need, write them in the keyring and answer `{"status":"ok"}` on stdout.
//!
//! The answer is always one line of JSON:
//!
//! - `{"status":"ok"}`: the account is signed in and the session is in the keyring.
//! - `{"status":"cancelled"}`: the user closed the window.
//! - `{"status":"error","message":"..."}`: something went wrong.

// The window is a program of its own: it should not open a console next to it.
#![cfg_attr(windows, windows_subsystem = "windows")]

use std::cell::Cell;
use std::rc::Rc;
use std::thread;
use std::time::{Duration, SystemTime};

use play_account::{self, Cookie, Session};
use tao::dpi::LogicalSize;
use tao::event::{Event, WindowEvent};
use tao::event_loop::{ControlFlow, EventLoopBuilder};
use tao::window::WindowBuilder;
use wry::{WebContext, WebViewBuilder};

/// The sign in page of Google, which sends us to YouTube Music.
const LOGIN_URL: &str = "https://accounts.google.com/ServiceLogin?service=youtube&continue=https%3A%2F%2Fmusic.youtube.com%2F";

/// Where the account lives.
const MUSIC_URL: &str = "https://music.youtube.com/";

/// The title of the window.
const TITLE: &str = "Iniciar sesión — Atic Play";

/// How often we look at the cookies of the web view.
const TICK: Duration = Duration::from_millis(500);

/// The tick of the clock that looks at the cookies.
struct Tick;

fn main() {
    logger::init();

    let args: Vec<String> = std::env::args().skip(1).collect();
    let printed = Rc::new(Cell::new(false));

    let result = if args.iter().any(|arg| arg == "--logout") {
        play_account::logout()
            .map(|()| "ok")
            .map_err(|err| format!("could not log out: {err:#}"))
    } else {
        login(&printed)
    };

    // The window answers by itself and the process ends with it; this is only for the `--logout`.
    match result {
        Ok(status) => report(&printed, status, None),
        Err(message) => {
            report(&printed, "error", Some(&message));
            std::process::exit(1);
        }
    }
}

/// Opens the login window and waits for the user.
///
/// It never comes back: the window answers and the process ends with it.
fn login(printed: &Rc<Cell<bool>>) -> ! {
    let event_loop = EventLoopBuilder::<Tick>::with_user_event().build();
    let proxy = event_loop.create_proxy();

    let window = match WindowBuilder::new()
        .with_title(TITLE)
        .with_inner_size(LogicalSize::new(480.0, 720.0))
        .with_min_inner_size(LogicalSize::new(360.0, 480.0))
        .build(&event_loop)
    {
        Ok(window) => window,
        Err(err) => {
            report(
                printed,
                "error",
                Some(&format!("could not open the window: {err}")),
            );
            std::process::exit(1);
        }
    };

    set_icon(&window);

    // A web view with a data directory of our own, so it does not mix with the other ones.
    let data_dir = play_account::webview_data_dir();
    if let Err(err) = std::fs::create_dir_all(&data_dir) {
        report(
            printed,
            "error",
            Some(&format!("could not create {}: {err}", data_dir.display())),
        );
        std::process::exit(1);
    }
    let mut context = WebContext::new(Some(data_dir));

    let webview = match WebViewBuilder::new_with_web_context(&mut context)
        .with_url(LOGIN_URL)
        .with_navigation_handler(|url| {
            log::debug!("navigation: {url}");
            true
        })
        .build(&window)
    {
        Ok(webview) => webview,
        Err(err) => {
            report(
                printed,
                "error",
                Some(&format!("could not open the web view: {err}")),
            );
            std::process::exit(1);
        }
    };

    // A clock that wakes us up to look at the cookies while the user signs in.
    thread::spawn(move || {
        loop {
            thread::sleep(TICK);
            if proxy.send_event(Tick).is_err() {
                break;
            }
        }
    });

    let printed = printed.clone();
    event_loop.run(move |event, _, control_flow| {
        match event {
            Event::UserEvent(Tick) => {
                if let Some(session) = session_in_webview(&webview) {
                    match play_account::save(&session) {
                        Ok(()) => {
                            report(&printed, "ok", None);
                            *control_flow = ControlFlow::Exit;
                        }
                        Err(err) => {
                            // The keyring may be busy: we try again with the next tick.
                            log::error!("could not save the session: {err:#}");
                        }
                    }
                }
            }
            Event::WindowEvent {
                event: WindowEvent::CloseRequested,
                ..
            }
            | Event::WindowEvent {
                event: WindowEvent::Destroyed,
                ..
            } => {
                report(&printed, "cancelled", None);
                *control_flow = ControlFlow::Exit;
            }
            _ => {}
        }
    })
}

/// The session of the account, when the web view already has it.
fn session_in_webview(webview: &wry::WebView) -> Option<Session> {
    let url = webview.url().ok()?;
    if !is_music_url(&url) {
        return None;
    }

    let cookies: Vec<Cookie> = webview
        .cookies_for_url(MUSIC_URL)
        .ok()?
        .iter()
        .map(|cookie| Cookie {
            name: cookie.name().to_string(),
            value: cookie.value().to_string(),
            domain: cookie.domain().map(str::to_string),
        })
        .collect();

    if !play_account::cookies::has_account(&cookies) {
        return None;
    }

    let header = play_account::cookies::header(&play_account::cookies::filter(&cookies));
    if header.is_empty() {
        return None;
    }

    Some(Session {
        cookies: header,
        saved_at: SystemTime::now(),
    })
}

/// `true` when the url is YouTube Music.
fn is_music_url(url: &str) -> bool {
    let Some(rest) = url.strip_prefix("https://") else {
        return false;
    };
    let host = rest.split(['/', '?', '#']).next().unwrap_or("");
    host.eq_ignore_ascii_case("music.youtube.com")
}

/// Writes the answer on stdout: one line of JSON and nothing else.
///
/// The cookies never go through here: the session is in the keyring.
fn report(printed: &Cell<bool>, status: &str, message: Option<&str>) {
    if printed.replace(true) {
        return;
    }

    let mut line = format!("{{\"status\":\"{status}\"");
    if let Some(message) = message {
        line.push_str(&format!(",\"message\":{}", json_string(message)));
    }
    line.push('}');

    println!("{line}");
}

/// A string as JSON.
fn json_string(text: &str) -> String {
    serde_json::to_string(text).unwrap_or_else(|_| "\"\"".to_string())
}

/// The icon of the app, when we find it.
#[cfg(all(windows, target_pointer_width = "64"))]
fn icon_path() -> Option<std::path::PathBuf> {
    use std::path::PathBuf;

    let exe = std::env::current_exe().ok()?;
    let dir = exe.parent()?;

    let candidates = [
        dir.join("atic-play.ico"),
        dir.join("assets").join("brand").join("atic-play.ico"),
        // While we develop, the icon is still in the app crate.
        PathBuf::from(env!("CARGO_MANIFEST_DIR"))
            .join("..")
            .join("app")
            .join("assets")
            .join("brand")
            .join("atic-play.ico"),
    ];

    candidates.into_iter().find(|path| path.is_file())
}

/// Puts the icon of the app in the window.
#[cfg(all(windows, target_pointer_width = "64"))]
fn set_icon(window: &tao::window::Window) {
    use std::os::windows::ffi::OsStrExt;
    use tao::platform::windows::WindowExtWindows;

    let Some(path) = icon_path() else {
        return;
    };
    let wide: Vec<u16> = path
        .as_os_str()
        .encode_wide()
        .chain(std::iter::once(0))
        .collect();

    unsafe {
        let module = win::GetModuleHandleW(std::ptr::null());
        let big = win::LoadImageW(
            module,
            wide.as_ptr(),
            win::IMAGE_ICON,
            win::GetSystemMetrics(win::SM_CXICON),
            win::GetSystemMetrics(win::SM_CYICON),
            win::LR_LOADFROMFILE,
        );
        if big.is_null() {
            log::debug!("the icon {} could not be loaded", path.display());
            return;
        }

        let small = win::LoadImageW(
            module,
            wide.as_ptr(),
            win::IMAGE_ICON,
            win::GetSystemMetrics(win::SM_CXSMICON),
            win::GetSystemMetrics(win::SM_CYSMICON),
            win::LR_LOADFROMFILE,
        );

        let hwnd = window.hwnd() as *mut _;
        win::SendMessageW(hwnd, win::WM_SETICON, win::ICON_BIG as usize, big as isize);
        if !small.is_null() {
            win::SendMessageW(
                hwnd,
                win::WM_SETICON,
                win::ICON_SMALL as usize,
                small as isize,
            );
        }
    }
}

/// Puts the icon of the app in the window.
#[cfg(not(all(windows, target_pointer_width = "64")))]
fn set_icon(_window: &tao::window::Window) {}

/// The few calls to Windows we need for the icon.
#[cfg(all(windows, target_pointer_width = "64"))]
mod win {
    use std::ffi::c_void;

    pub const IMAGE_ICON: u32 = 1;
    pub const LR_LOADFROMFILE: u32 = 0x0010;
    pub const WM_SETICON: u32 = 0x0080;
    pub const ICON_BIG: u32 = 1;
    pub const ICON_SMALL: u32 = 0;
    pub const SM_CXICON: i32 = 11;
    pub const SM_CYICON: i32 = 12;
    pub const SM_CXSMICON: i32 = 49;
    pub const SM_CYSMICON: i32 = 50;

    #[link(name = "user32")]
    unsafe extern "system" {
        pub fn LoadImageW(
            hinst: *mut c_void,
            name: *const u16,
            image_type: u32,
            cx: i32,
            cy: i32,
            load: u32,
        ) -> *mut c_void;

        pub fn SendMessageW(hwnd: *mut c_void, message: u32, wparam: usize, lparam: isize)
        -> isize;

        pub fn GetSystemMetrics(index: i32) -> i32;
    }

    #[link(name = "kernel32")]
    unsafe extern "system" {
        pub fn GetModuleHandleW(module: *const u16) -> *mut c_void;
    }
}

/// A logger that writes to stderr, so the app that opened the window sees what it is doing.
mod logger {
    use log::{Level, Metadata, Record};

    struct Stderr;

    static LOGGER: Stderr = Stderr;

    impl log::Log for Stderr {
        fn enabled(&self, metadata: &Metadata) -> bool {
            metadata.level() <= level()
        }

        fn log(&self, record: &Record) {
            if self.enabled(record.metadata()) {
                eprintln!("[login] {}: {}", record.level(), record.args());
            }
        }

        fn flush(&self) {}
    }

    /// The level of the messages, from `RUST_LOG`.
    fn level() -> Level {
        match std::env::var("RUST_LOG")
            .unwrap_or_default()
            .to_lowercase()
            .as_str()
        {
            "trace" => Level::Trace,
            "debug" => Level::Debug,
            "warn" => Level::Warn,
            "error" => Level::Error,
            _ => Level::Info,
        }
    }

    pub fn init() {
        let _ = log::set_logger(&LOGGER);
        log::set_max_level(level().to_level_filter());
    }
}
