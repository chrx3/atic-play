//! The session in the keyring of the system.
//!
//! These tests write in the keyring of the user, so they are ignored by default: run them by
//! hand with `cargo test -p play-account -- --ignored`.
//!
//! Each test has a credential of its own and they take turns: the Credential Manager of Windows
//! does not like two writes at the same time.

use std::sync::Mutex;
use std::time::{Duration, SystemTime};

use play_account::{SERVICE, Session, Store};

/// The lock of the Credential Manager: only one test writes at a time.
static LOCK: Mutex<()> = Mutex::new(());

/// A store with a credential of our own, so the session of the user is not touched.
fn store(user: &str) -> Store {
    Store::new(SERVICE, user)
}

fn session(cookies: &str) -> Session {
    Session {
        cookies: cookies.to_string(),
        saved_at: SystemTime::UNIX_EPOCH + Duration::from_secs(1_700_000_000),
    }
}

#[test]
#[ignore]
fn the_session_goes_in_the_keyring_and_comes_back() {
    let _guard = LOCK.lock().unwrap_or_else(|err| err.into_inner());
    let store = store("youtube-music-test-small");
    store.clear().expect("the keyring should be there");

    let saved = session("SAPISID=fake-sapisid; SID=fake-sid");
    store.save(&saved).expect("the session should be saved");

    let loaded = store
        .load()
        .expect("the keyring should answer")
        .expect("the session should be there");
    assert_eq!(loaded.cookies, saved.cookies);
    assert_eq!(loaded.saved_at, saved.saved_at);

    store.clear().expect("the session should be forgotten");
    assert!(store.load().expect("the keyring should answer").is_none());
}

#[test]
#[ignore]
fn a_session_bigger_than_the_keyring_goes_in_pieces() {
    let _guard = LOCK.lock().unwrap_or_else(|err| err.into_inner());
    let store = store("youtube-music-test-big");
    store.clear().expect("the keyring should be there");

    // More than the 2.5 KB that the Windows Credential Manager takes.
    let cookies = format!("SAPISID=fake; PREF={}", "x".repeat(5000));
    let saved = session(&cookies);
    store.save(&saved).expect("the session should be saved");

    let loaded = store
        .load()
        .expect("the keyring should answer")
        .expect("the session should be there");
    assert_eq!(loaded.cookies, cookies);

    store.clear().expect("the session should be forgotten");
    assert!(store.load().expect("the keyring should answer").is_none());
}

#[test]
#[ignore]
fn a_session_that_was_never_saved_is_none() {
    let _guard = LOCK.lock().unwrap_or_else(|err| err.into_inner());
    let store = store("youtube-music-test-empty");
    store.clear().expect("the keyring should be there");

    assert!(store.load().expect("the keyring should answer").is_none());
}
