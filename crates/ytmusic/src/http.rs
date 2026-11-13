//! HTTP plumbing: one shared agent, the anonymous visitor id and the JSON POST helper.
//!
//! Everything is blocking, so callers must run it on a background thread.

use std::time::Duration;

use serde_json::{Map, Value};

use crate::auth::{self, Auth};
use crate::{Error, Result};

/// User agent of the web client. Also used to bootstrap `sw.js_data`.
pub(crate) const WEB_USER_AGENT: &str = "Mozilla/5.0 (Windows NT 10.0; Win64; x64) AppleWebKit/537.36 (KHTML, like Gecko) \
     Chrome/131.0.0.0 Safari/537.36";

/// `WEB_REMIX` version used if `sw.js_data` does not tell us the current one.
const FALLBACK_WEB_VERSION: &str = "1.20261104.00.00";

const SW_JS_DATA: &str = "https://music.youtube.com/sw.js_data";
const API_ROOT: &str = "https://music.youtube.com/youtubei/v1";
const ORIGIN: &str = "https://music.youtube.com";

/// Refuse to buffer more than this from one API response.
const MAX_RESPONSE_BYTES: u64 = 64 * 1024 * 1024;

/// Which inner client we pretend to be.
#[derive(Clone, Copy, PartialEq, Eq)]
pub(crate) enum ClientKind {
    /// Browser on music.youtube.com (`WEB_REMIX`).
    Web,
    /// Meta Quest VR app: direct, un-ciphered audio URLs.
    AndroidVr,
    /// Apple Vision Pro app: the fallback when the VR client is rejected.
    VisionOs,
    /// Smart TV client: last resort.
    Tv,
}

/// One `clientName` / `clientVersion` pair and the headers that go with it.
#[derive(Clone, Copy)]
pub(crate) struct ClientSpec {
    pub(crate) name: &'static str,
    /// Numeric id, sent as `X-YouTube-Client-Name`.
    pub(crate) id: &'static str,
    /// Empty means "the `WEB_REMIX` version discovered at startup".
    pub(crate) version: &'static str,
    pub(crate) user_agent: &'static str,
    pub(crate) kind: ClientKind,
}

/// The browser client used for search, browse and next.
pub(crate) const WEB: ClientSpec = ClientSpec {
    name: "WEB_REMIX",
    id: "67",
    version: "",
    user_agent: WEB_USER_AGENT,
    kind: ClientKind::Web,
};

const ANDROID_VR: ClientSpec = ClientSpec {
    name: "ANDROID_VR",
    id: "28",
    version: "1.65.10",
    user_agent: "com.google.android.apps.youtube.vr.oculus/1.65.10 (Linux; U; Android 12L; \
                 eureka-user Build/SQ3A.220605.009.A1) gzip",
    kind: ClientKind::AndroidVr,
};

const VISIONOS: ClientSpec = ClientSpec {
    name: "VISIONOS",
    id: "101",
    version: "1.03.00",
    user_agent: "Mozilla/5.0 (VisionOS; 1.3; Vision Pro) AppleWebKit/605.1.15 (KHTML, like \
                 Gecko) Version/17.4 Safari/605.1.15",
    kind: ClientKind::VisionOs,
};

const TVHTML5: ClientSpec = ClientSpec {
    name: "TVHTML5",
    id: "7",
    version: "7.20250101.19.00",
    user_agent: "Mozilla/5.0 (ChromiumStylePlatform) Cobalt/Version",
    kind: ClientKind::Tv,
};

/// Clients to try, in order, when we need a direct audio URL.
///
/// The order matters: the `ANDROID_VR` URLs stop serving data after about a megabyte (the
/// `n` parameter of the URL is not transformed, so YouTube throttles the download), which is
/// not enough for a whole song. The other clients serve the whole file.
pub(crate) const PLAYER_CLIENTS: [ClientSpec; 3] = [VISIONOS, TVHTML5, ANDROID_VR];

/// Small HTTP client shared by every call of a [`crate::Client`].
pub(crate) struct Http {
    agent: ureq::Agent,
    visitor_data: String,
    web_version: String,
    language: String,
    region: String,
    /// Present when the client was built with the cookies of a signed-in session.
    auth: Option<Auth>,
}

impl Http {
    pub(crate) fn new(language: &str, region: &str, auth: Option<Auth>) -> Result<Self> {
        let language = default_language(language);
        let region = default_region(region);
        let agent = build_agent();
        // The cookies go along so the visitor id belongs to the signed-in session too.
        let (visitor_data, web_version) = bootstrap(&agent, &language, &region, auth.as_ref())?;
        log::debug!("visitor id ready ({} bytes)", visitor_data.len());
        Ok(Self {
            agent,
            visitor_data,
            web_version,
            language,
            region,
            auth,
        })
    }

    pub(crate) fn is_signed_in(&self) -> bool {
        self.auth.is_some()
    }

    /// POSTs one `youtubei` request and returns the parsed JSON body.
    pub(crate) fn post(
        &self,
        endpoint: &str,
        client: &ClientSpec,
        body: Map<String, Value>,
    ) -> Result<Value> {
        let version = if client.version.is_empty() {
            self.web_version.clone()
        } else {
            client.version.to_string()
        };

        let mut payload = Map::new();
        payload.insert(
            "context".to_string(),
            Value::Object(self.context(client, &version)),
        );
        for (key, value) in body {
            payload.insert(key, value);
        }

        let url = format!("{API_ROOT}/{endpoint}?prettyPrint=false");
        let mut request = self
            .agent
            .post(&url)
            .header("Content-Type", "application/json")
            .header("X-Goog-Api-Format-Version", "1")
            .header("X-YouTube-Client-Name", client.id)
            .header("X-YouTube-Client-Version", version.as_str())
            .header("X-Goog-Visitor-Id", self.visitor_data.as_str())
            .header("Origin", ORIGIN)
            .header("X-Origin", ORIGIN)
            .header("Referer", "https://music.youtube.com/")
            .header("Accept-Language", self.accept_language())
            .header("User-Agent", client.user_agent);

        // The browser client carries the account; the audio clients stay anonymous.
        if let (ClientKind::Web, Some(auth)) = (client.kind, self.auth.as_ref()) {
            let authorization = auth.authorization(auth::now_seconds());
            request = request
                .header("Cookie", auth.cookies())
                .header("Authorization", authorization.as_str())
                .header("X-Goog-AuthUser", "0");
        }

        let response = request
            .send_json(Value::Object(payload))
            .map_err(|err| Error::Network(format!("{endpoint}: {err}")))?;

        let status = response.status().as_u16();
        let text = response
            .into_body()
            .with_config()
            .limit(MAX_RESPONSE_BYTES)
            .read_to_string()
            .map_err(|err| Error::Network(format!("{endpoint}: reading the answer: {err}")))?;

        if status == 401 || status == 403 {
            // A web call needs an account (or its cookies expired). The player clients have
            // their own anonymous auth, so their 401/403 stay ordinary network errors.
            if client.kind == ClientKind::Web {
                return Err(Error::SignedOut);
            }
        }

        if !(200..300).contains(&status) {
            return Err(Error::Network(format!(
                "{endpoint}: HTTP {status}: {}",
                snippet(&text)
            )));
        }

        serde_json::from_str(&text).map_err(|err| Error::Parse(format!("{endpoint}: {err}")))
    }

    fn context(&self, client: &ClientSpec, version: &str) -> Map<String, Value> {
        let mut ctx_client = Map::new();
        ctx_client.insert("clientName".into(), Value::from(client.name));
        ctx_client.insert("clientVersion".into(), Value::from(version));
        ctx_client.insert("hl".into(), Value::from(self.language.as_str()));
        ctx_client.insert("gl".into(), Value::from(self.region.as_str()));
        ctx_client.insert(
            "visitorData".into(),
            Value::from(self.visitor_data.as_str()),
        );

        match client.kind {
            ClientKind::Web => {}
            ClientKind::AndroidVr => {
                ctx_client.insert("osName".into(), Value::from("Android"));
                ctx_client.insert("osVersion".into(), Value::from("12L"));
                ctx_client.insert("androidSdkVersion".into(), Value::from(32));
                ctx_client.insert("deviceMake".into(), Value::from("Oculus"));
                ctx_client.insert("deviceModel".into(), Value::from("Quest 3"));
            }
            ClientKind::VisionOs => {
                ctx_client.insert("osName".into(), Value::from("visionOS"));
                ctx_client.insert("osVersion".into(), Value::from("1.3"));
                ctx_client.insert("deviceMake".into(), Value::from("Apple"));
                ctx_client.insert("deviceModel".into(), Value::from("Vision Pro"));
            }
            ClientKind::Tv => {
                ctx_client.insert("platform".into(), Value::from("TV"));
            }
        }

        let mut context = Map::new();
        context.insert("client".into(), Value::Object(ctx_client));
        context
    }

    fn accept_language(&self) -> String {
        format!("{}-{},{};q=0.9", self.language, self.region, self.language)
    }
}

fn build_agent() -> ureq::Agent {
    ureq::Agent::config_builder()
        .timeout_global(Some(Duration::from_secs(60)))
        .timeout_connect(Some(Duration::from_secs(15)))
        // We want to read the error body instead of ureq turning 4xx into an opaque error.
        .http_status_as_error(false)
        .user_agent(WEB_USER_AGENT)
        .build()
        .into()
}

/// Fetches the session: `visitorData` plus the current `WEB_REMIX` version.
fn bootstrap(
    agent: &ureq::Agent,
    language: &str,
    region: &str,
    auth: Option<&Auth>,
) -> Result<(String, String)> {
    let accept_language = format!("{language}-{region},{language};q=0.9");
    let mut request = agent
        .get(SW_JS_DATA)
        .header("Accept-Language", accept_language.as_str())
        .header("User-Agent", WEB_USER_AGENT);
    if let Some(auth) = auth {
        request = request.header("Cookie", auth.cookies());
    }
    let response = request
        .call()
        .map_err(|err| Error::Network(format!("visitor id: {err}")))?;

    let status = response.status().as_u16();
    let text = response
        .into_body()
        .with_config()
        .limit(4 * 1024 * 1024)
        .read_to_string()
        .map_err(|err| Error::Network(format!("visitor id: reading the answer: {err}")))?;

    if !(200..300).contains(&status) {
        return Err(Error::Network(format!(
            "visitor id: HTTP {status}: {}",
            snippet(&text)
        )));
    }

    let data: Value = serde_json::from_str(strip_anti_xssi(&text))
        .map_err(|err| Error::Parse(format!("visitor id: {err}")))?;

    let visitor_data = find_visitor_data(&data).ok_or_else(|| {
        Error::Parse("visitor id: sw.js_data did not contain a visitorData".to_string())
    })?;
    let web_version =
        find_client_version(&data).unwrap_or_else(|| FALLBACK_WEB_VERSION.to_string());

    Ok((visitor_data, web_version))
}

/// `sw.js_data` (and some other endpoints) prefix JSON with `)]}'` to defeat old XSS tricks.
pub(crate) fn strip_anti_xssi(text: &str) -> &str {
    let trimmed = text.trim_start();
    match trimmed.strip_prefix(")]}'") {
        Some(rest) => rest.trim_start(),
        None => trimmed,
    }
}

/// Walks the JSON looking for the anonymous visitor id, a base64url string starting with `Cg`.
pub(crate) fn find_visitor_data(value: &Value) -> Option<String> {
    match value {
        Value::String(text) => is_visitor_data(text).then(|| text.to_string()),
        Value::Array(items) => items.iter().find_map(find_visitor_data),
        Value::Object(map) => map.values().find_map(find_visitor_data),
        _ => None,
    }
}

fn is_visitor_data(text: &str) -> bool {
    // It is a URL-encoded base64 protobuf, so `%` and `=` show up when the id has padding.
    text.len() >= 30
        && text.len() <= 4096
        && text.starts_with("Cg")
        && text
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || matches!(b, b'_' | b'-' | b'%' | b'='))
}

/// Looks for the `WEB_REMIX` version, a string like `1.20260101.01.00`.
pub(crate) fn find_client_version(value: &Value) -> Option<String> {
    match value {
        Value::String(text) => is_client_version(text).then(|| text.to_string()),
        Value::Array(items) => items.iter().find_map(find_client_version),
        Value::Object(map) => map.values().find_map(find_client_version),
        _ => None,
    }
}

fn is_client_version(text: &str) -> bool {
    let mut parts = text.split('.');
    let (Some(a), Some(b), Some(c), Some(d), None) = (
        parts.next(),
        parts.next(),
        parts.next(),
        parts.next(),
        parts.next(),
    ) else {
        return false;
    };
    !a.is_empty()
        && a.len() <= 3
        && a.bytes().all(|x| x.is_ascii_digit())
        && b.len() == 8
        && b.bytes().all(|x| x.is_ascii_digit())
        && c.len() == 2
        && c.bytes().all(|x| x.is_ascii_digit())
        && d.len() == 2
        && d.bytes().all(|x| x.is_ascii_digit())
}

fn default_language(language: &str) -> String {
    let language = language.trim();
    if language.is_empty() {
        "es".to_string()
    } else {
        language.to_lowercase()
    }
}

fn default_region(region: &str) -> String {
    let region = region.trim();
    if region.is_empty() {
        "CL".to_string()
    } else {
        region.to_uppercase()
    }
}

/// First line of an HTML/JSON error body, for error messages.
fn snippet(text: &str) -> String {
    let line = text.lines().next().unwrap_or("").trim();
    let mut out: String = line.chars().take(200).collect();
    if line.chars().count() > 200 {
        out.push('…');
    }
    out
}

#[cfg(test)]
mod tests {
    use serde_json::Value;

    use super::*;

    fn sw_js_data() -> Value {
        let raw = include_str!("../tests/fixtures/sw_js_data.json");
        serde_json::from_str(strip_anti_xssi(raw)).expect("the fixture is JSON after the prefix")
    }

    #[test]
    fn the_anti_xssi_prefix_is_removed() {
        assert_eq!(strip_anti_xssi(")]}'\n\n[[1]]"), "[[1]]");
        assert_eq!(strip_anti_xssi(")]}'[1]"), "[1]");
        assert_eq!(strip_anti_xssi("  [[1]]"), "[[1]]");
    }

    #[test]
    fn the_visitor_id_is_found_in_sw_js_data() {
        let data = sw_js_data();
        assert_eq!(
            find_visitor_data(&data).as_deref(),
            Some("CgtmYWtlLXZpc2l0b3ItaWQtb25seS1mb3ItdGVzdHM")
        );
    }

    #[test]
    fn the_web_client_version_is_found_in_sw_js_data() {
        let data = sw_js_data();
        assert_eq!(
            find_client_version(&data).as_deref(),
            Some("1.20261104.00.00")
        );
    }

    #[test]
    fn client_version_shapes_are_checked() {
        assert!(is_client_version("1.20261104.00.00"));
        assert!(is_client_version("1.20000101.00.00"));
        assert!(!is_client_version("1.20261104.00"));
        assert!(!is_client_version("v1.20261104.00.00"));
        assert!(!is_client_version("1.2026110.00.00"));
        assert!(!is_client_version(""));
    }

    #[test]
    fn short_visitor_like_strings_do_not_count() {
        assert!(!is_visitor_data("Cg"));
        assert!(!is_visitor_data("Cgsomething with spaces inside here"));
        assert!(is_visitor_data(
            "CgtmYWtlLXZpc2l0b3ItaWQtb25seS1mb3ItdGVzdHM"
        ));
        // Real ids come percent-encoded from sw.js_data.
        assert!(is_visitor_data(
            "CgtIZHQ2NC1fTnEtMCiAiNLXBjIKCgJDTBIEGgAgYEgBYqoCCqcCQ3RvQkNBVVNqQUVCUkQ%3D%3D"
        ));
    }
}
