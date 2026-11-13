//! The login window: a program that lives next to the app and signs in with Google.

use std::path::PathBuf;
use std::process::{Command, Stdio};

use anyhow::{Context, anyhow, bail};
use serde::Deserialize;

use crate::Session;

/// The name of the login window.
const LOGIN_BINARY: &str = "atic-play-login";

/// What the login window answers on stdout.
#[derive(Debug, Deserialize)]
struct Answer {
    status: String,
    #[serde(default)]
    message: Option<String>,
}

/// The path of the login window: it lives next to the app.
pub fn login_binary_path() -> PathBuf {
    let name = if cfg!(windows) {
        format!("{LOGIN_BINARY}.exe")
    } else {
        LOGIN_BINARY.to_string()
    };

    match std::env::current_exe() {
        Ok(exe) => exe.with_file_name(name),
        Err(_) => PathBuf::from(name),
    }
}

/// Opens the login window and waits for it.
///
/// `Ok(None)` when the user closed the window without signing in. The session itself is written in
/// the keyring by the login window, so it never travels through the pipe.
pub fn run_login() -> anyhow::Result<Option<Session>> {
    let path = login_binary_path();
    if !path.is_file() {
        bail!(
            "the login window is not next to the app: {}",
            path.display()
        );
    }

    let output = Command::new(&path)
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::inherit())
        .output()
        .with_context(|| format!("could not open the login window: {}", path.display()))?;

    let stdout = String::from_utf8_lossy(&output.stdout);
    let line = stdout
        .lines()
        .map(str::trim)
        .find(|line| !line.is_empty())
        .ok_or_else(|| {
            anyhow!(
                "the login window did not answer (exit code {})",
                output.status
            )
        })?;

    let answer: Answer = serde_json::from_str(line)
        .context("the login window answered something we could not read")?;

    match answer.status.as_str() {
        "ok" => crate::load(),
        "cancelled" => Ok(None),
        other => bail!(
            "{}",
            answer
                .message
                .unwrap_or_else(|| format!("the login window failed: {other}"))
        ),
    }
}
