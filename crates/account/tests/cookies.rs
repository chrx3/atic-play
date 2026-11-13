//! The cookies of the web view and the header we build with them.

use play_account::Cookie;
use play_account::cookies;

/// The cookies of the example, as the web view gives them.
fn example() -> Vec<Cookie> {
    let path = concat!(env!("CARGO_MANIFEST_DIR"), "/tests/fixtures/cookies.json");
    let text = std::fs::read_to_string(path).expect("the example of cookies should be there");
    serde_json::from_str(&text).expect("the example of cookies should be readable")
}

#[test]
fn the_example_has_the_cookies_of_an_account() {
    let cookies = example();
    assert!(cookies::has_account(&cookies));
    assert!(cookies.iter().any(|cookie| cookie.name == "SAPISID"));
    assert!(cookies.iter().any(|cookie| cookie.name == "NID"));
}

#[test]
fn only_the_cookies_of_the_account_are_kept() {
    let kept = cookies::filter(&example());
    let names: Vec<&str> = kept.iter().map(|cookie| cookie.name.as_str()).collect();

    assert_eq!(
        names,
        vec![
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
        ]
    );
}

#[test]
fn the_header_has_the_cookies_of_the_account_and_nothing_else() {
    let header = cookies::header(&cookies::filter(&example()));

    assert!(header.starts_with("SAPISID=fake-sapisid-value; "));
    assert!(header.contains("SID=fake-sid-value"));
    assert!(header.contains("PREF=f1=abc&f2=def"));
    assert!(!header.contains("NID"));
    assert!(!header.contains("CONSENT"));
    assert!(!header.contains("IDE"));
    assert_eq!(header.matches(';').count(), 13);
}

#[test]
fn a_session_without_the_account_cookie_is_not_a_session() {
    let cookies: Vec<Cookie> = vec![
        Cookie::new("SID", "x"),
        Cookie::new("HSID", "y"),
        Cookie::new("PREF", "z"),
    ];

    assert!(!cookies::has_account(&cookies));
    assert_eq!(
        cookies::header(&cookies::filter(&cookies)),
        "SID=x; HSID=y; PREF=z"
    );
}
