//! The cookies of the web view that identify the account, and the `Cookie` header we build.

/// The cookies we keep from the web view.
///
/// The web view asks Google for a lot of cookies; the ones below are the ones that say who the
/// user is. The rest (preferences, experiment flags…) are of no use to us and do not fit in the
/// keyring.
pub const KEPT: &[&str] = &[
    "SAPISID",
    "__Secure-3PAPISID",
    "__Secure-1PAPISID",
    "SID",
    "HSID",
    "SSID",
    "APISID",
    "__Secure-1PSID",
    "__Secure-3PSID",
    "LOGIN_INFO",
    "SIDCC",
    "__Secure-1PSIDTS",
    "__Secure-3PSIDTS",
    "PREF",
];

/// The cookie that proves the account is signed in.
pub const ACCOUNT_COOKIE: &str = "SAPISID";

/// The other cookie that proves the account is signed in.
pub const ACCOUNT_COOKIE_SECURE: &str = "__Secure-3PAPISID";

/// One cookie of the web view.
#[derive(Clone, Debug, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct Cookie {
    pub name: String,
    pub value: String,
    /// The domain of the cookie, when the web view tells it.
    pub domain: Option<String>,
}

impl Cookie {
    pub fn new(name: impl Into<String>, value: impl Into<String>) -> Self {
        Self {
            name: name.into(),
            value: value.into(),
            domain: None,
        }
    }

    /// The same cookie, with its domain.
    pub fn with_domain(mut self, domain: impl Into<String>) -> Self {
        self.domain = Some(domain.into());
        self
    }
}

/// `true` when the cookie is one of the ones we need to identify the account.
pub fn is_kept(name: &str) -> bool {
    KEPT.contains(&name)
}

/// Keeps only the cookies we need, in the order of [`KEPT`].
///
/// The order is fixed so the same session always gives the same header.
pub fn filter(cookies: &[Cookie]) -> Vec<Cookie> {
    let mut kept: Vec<Cookie> = cookies
        .iter()
        .filter(|cookie| is_kept(&cookie.name))
        .cloned()
        .collect();

    kept.sort_by_key(|cookie| {
        KEPT.iter()
            .position(|name| *name == cookie.name)
            .unwrap_or(usize::MAX)
    });

    kept
}

/// The value of the `Cookie` header for these cookies: `name=value; name=value`.
pub fn header(cookies: &[Cookie]) -> String {
    cookies
        .iter()
        .map(|cookie| format!("{}={}", cookie.name, cookie.value))
        .collect::<Vec<_>>()
        .join("; ")
}

/// `true` when the cookies say that an account is signed in.
pub fn has_account(cookies: &[Cookie]) -> bool {
    cookies
        .iter()
        .any(|cookie| cookie.name == ACCOUNT_COOKIE || cookie.name == ACCOUNT_COOKIE_SECURE)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn cookies() -> Vec<Cookie> {
        vec![
            Cookie::new("PREF", "f1=1"),
            Cookie::new("SID", "s-value"),
            Cookie::new("HSID", "h-value"),
            Cookie::new("SAPISID", "the-account"),
            Cookie::new("NID", "not-ours"),
            Cookie::new("SSID", "ss-value"),
            Cookie::new("LOGIN_INFO", "encoded"),
        ]
    }

    #[test]
    fn it_keeps_only_the_cookies_we_need() {
        let kept = filter(&cookies());
        let names: Vec<&str> = kept.iter().map(|cookie| cookie.name.as_str()).collect();
        assert_eq!(
            names,
            vec!["SAPISID", "SID", "HSID", "SSID", "LOGIN_INFO", "PREF",]
        );
    }

    #[test]
    fn the_order_does_not_depend_on_the_web_view() {
        let mut mixed = cookies();
        mixed.reverse();
        let kept = filter(&mixed);
        let names: Vec<&str> = kept.iter().map(|cookie| cookie.name.as_str()).collect();
        assert_eq!(names[0], "SAPISID");
        assert_eq!(names[1], "SID");
        assert_eq!(names.len(), 6);
    }

    #[test]
    fn the_header_looks_like_a_cookie_header() {
        assert_eq!(
            header(&filter(&cookies())),
            "SAPISID=the-account; SID=s-value; HSID=h-value; SSID=ss-value; \
             LOGIN_INFO=encoded; PREF=f1=1"
        );
    }

    #[test]
    fn an_empty_list_gives_an_empty_header() {
        assert_eq!(header(&[]), "");
    }

    #[test]
    fn the_account_needs_one_of_the_two_cookies() {
        assert!(has_account(&[Cookie::new("SAPISID", "x")]));
        assert!(has_account(&[Cookie::new("__Secure-3PAPISID", "x")]));
        assert!(!has_account(&[Cookie::new("SID", "x")]));
        assert!(!has_account(&[]));
    }

    #[test]
    fn a_cookie_of_another_domain_is_not_ours() {
        assert!(is_kept("SAPISID"));
        assert!(!is_kept("NID"));
        assert!(!is_kept("sapisid"));
    }
}
