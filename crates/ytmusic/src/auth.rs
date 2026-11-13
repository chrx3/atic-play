//! Signing authenticated `youtubei` requests.
//!
//! The login window hands us the `Cookie` header of a signed-in session. From it we keep the
//! whole header plus the secret used by `SAPISIDHASH`, the hash Google wants on every
//! authenticated call.
//!
//! Nothing here may reach a log or an error message: cookies, the SAPISID value and the
//! `Authorization` value are all secrets. [`Auth`] has a redacted `Debug` to make that hard to
//! break by accident.

use std::fmt;

use sha1::{Digest, Sha1};

/// Origin the request is bound to; it is part of the hash input.
pub(crate) const AUTH_ORIGIN: &str = "https://music.youtube.com";

/// Cookies that carry the secret, most specific first.
const SAPISID_COOKIES: [&str; 3] = ["SAPISID", "__Secure-3PAPISID", "__Secure-1PAPISID"];

/// A signed-in session: the cookie header to replay and the secret to sign requests with.
pub(crate) struct Auth {
    cookies: String,
    sapisid: String,
}

impl Auth {
    /// Reads a `Cookie` header value (`NAME=value; NAME2=value2`).
    ///
    /// Returns `None` when the header has no usable session secret.
    pub(crate) fn parse(cookies: &str) -> Option<Self> {
        let cookies = normalize_cookies(cookies);
        if cookies.is_empty() {
            return None;
        }
        let sapisid = SAPISID_COOKIES
            .iter()
            .find_map(|name| cookie_value(&cookies, name))?;
        Some(Self { cookies, sapisid })
    }

    /// The normalized `Cookie` header value.
    pub(crate) fn cookies(&self) -> &str {
        &self.cookies
    }

    /// `SAPISIDHASH <seconds>_<sha1>` for the given moment.
    pub(crate) fn authorization(&self, seconds: u64) -> String {
        format!(
            "SAPISIDHASH {}_{}",
            seconds,
            sapisid_hash(seconds, &self.sapisid)
        )
    }
}

impl fmt::Debug for Auth {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("Auth { <redacted> }")
    }
}

/// `SHA-1("<seconds> <sapisid> https://music.youtube.com")` in lowercase hex.
pub(crate) fn sapisid_hash(seconds: u64, sapisid: &str) -> String {
    let mut hasher = Sha1::new();
    hasher.update(format!("{seconds} {sapisid} {AUTH_ORIGIN}").as_bytes());
    hex(&hasher.finalize())
}

/// Current Unix time in seconds (0 if the clock is before 1970, which only breaks auth).
pub(crate) fn now_seconds() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|elapsed| elapsed.as_secs())
        .unwrap_or(0)
}

fn hex(bytes: &[u8]) -> String {
    use std::fmt::Write;

    let mut out = String::with_capacity(bytes.len() * 2);
    for byte in bytes {
        // Writing into a String cannot fail.
        let _ = write!(out, "{byte:02x}");
    }
    out
}

/// Drops empty or malformed pairs and joins the rest with `"; "`.
///
/// Accepts newlines as separators too, in case the login window gave us one cookie per line.
fn normalize_cookies(raw: &str) -> String {
    let mut pairs = Vec::new();
    for part in raw.split([';', '\n', '\r']) {
        let part = part.trim();
        if part.is_empty() || !part.contains('=') {
            continue;
        }
        // A header value must not smuggle control characters.
        if part.bytes().any(|byte| byte < 0x20 || byte == 0x7f) {
            continue;
        }
        pairs.push(part);
    }
    pairs.join("; ")
}

/// Value of one cookie out of a normalized `Cookie` header.
fn cookie_value(cookies: &str, name: &str) -> Option<String> {
    cookies
        .split(';')
        .filter_map(|pair| pair.trim().split_once('='))
        .find(|(key, _)| key.trim() == name)
        .map(|(_, value)| value.trim().to_string())
        .filter(|value| !value.is_empty())
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Independently computed: SHA-1 of `"1700000000 TEST-SAPISID https://music.youtube.com"`.
    const HASH_1700000000: &str = "571de27910e9c9de19cc07790c1c47735abdd796";
    /// SHA-1 of `"0 abc https://music.youtube.com"`.
    const HASH_ZERO_ABC: &str = "804f75218bdee2a56a0e69d12dd713e736eec508";

    fn auth(cookies: &str) -> Auth {
        Auth::parse(cookies).expect("the test cookies have a SAPISID")
    }

    #[test]
    fn the_sapisid_hash_matches_hand_computed_values() {
        assert_eq!(sapisid_hash(1_700_000_000, "TEST-SAPISID"), HASH_1700000000);
        assert_eq!(sapisid_hash(0, "abc"), HASH_ZERO_ABC);
    }

    #[test]
    fn the_authorization_header_is_the_timestamp_then_the_hash() {
        assert_eq!(
            auth("SAPISID=TEST-SAPISID").authorization(1_700_000_000),
            format!("SAPISIDHASH 1700000000_{HASH_1700000000}")
        );
    }

    #[test]
    fn the_secret_prefers_sapisid_then_the_secure_variants() {
        assert_eq!(auth("SAPISID=one").sapisid, "one");
        assert_eq!(
            auth("__Secure-3PAPISID=two; VISITOR_INFO1_LIVE=x").sapisid,
            "two"
        );
        assert_eq!(
            auth("__Secure-1PAPISID=three; __Secure-3PAPISID=two").sapisid,
            "two"
        );
    }

    #[test]
    fn cookies_are_normalized_without_losing_values_with_equals() {
        let parsed = auth("  SID=abc== ;  SAPISID=secret ;\n __Secure-3PSID=zzz\r\n");
        assert_eq!(
            parsed.cookies(),
            "SID=abc==; SAPISID=secret; __Secure-3PSID=zzz"
        );
        assert_eq!(
            auth("SAPISID=a=b=c").cookies(),
            "SAPISID=a=b=c",
            "values may contain '='"
        );
    }

    #[test]
    fn a_cookie_header_without_a_secret_is_rejected() {
        assert!(Auth::parse("").is_none());
        assert!(Auth::parse("   ; ; ").is_none());
        assert!(Auth::parse("VISITOR_INFO1_LIVE=abc; PREF=hl=es").is_none());
        assert!(Auth::parse("SAPISID=").is_none());
        assert!(Auth::parse("SAPISID").is_none());
    }

    #[test]
    fn cookie_pairs_with_control_characters_are_dropped() {
        let parsed = auth("SAPISID=secret; X=bad\u{1}value");
        assert_eq!(parsed.cookies(), "SAPISID=secret");
    }

    #[test]
    fn the_debug_output_is_redacted() {
        let debug = format!("{:?}", auth("SAPISID=super-secret"));
        assert_eq!(debug, "Auth { <redacted> }");
        assert!(!debug.contains("super-secret"));
    }
}
