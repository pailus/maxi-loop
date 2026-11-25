//! Minimal User-Agent parser for the Sentry-style `browser` / `client_os` contexts (`F029`).
//!
//! Backend SDKs (Django, FastAPI, ...) only forward the raw `User-Agent` header; Sentry derives
//! the browser and OS on the server. This covers the common browsers, operating systems and
//! HTTP clients without a regex dependency; anything unknown is left out rather than guessed.

/// Name + optional version of a browser or operating system.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct UaComponent {
    pub name: String,
    pub version: Option<String>,
}

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct ParsedUserAgent {
    pub browser: Option<UaComponent>,
    pub os: Option<UaComponent>,
}

/// Characters of the header that are inspected; real User-Agents are far shorter.
const MAX_UA_LEN: usize = 1024;

/// `(token, browser name)` checked in order: Chromium forks advertise `Chrome/` and Chrome
/// advertises `Safari/`, so the more specific tokens must come first.
const BROWSER_TOKENS: &[(&str, &str)] = &[
    ("EdgiOS/", "Edge"),
    ("EdgA/", "Edge"),
    ("Edg/", "Edge"),
    ("OPR/", "Opera"),
    ("SamsungBrowser/", "Samsung Internet"),
    ("FxiOS/", "Firefox"),
    ("Firefox/", "Firefox"),
    ("CriOS/", "Chrome"),
    ("Chrome/", "Chrome"),
];

pub fn parse_user_agent(raw: &str) -> ParsedUserAgent {
    let ua = match raw.char_indices().nth(MAX_UA_LEN) {
        Some((idx, _)) => &raw[..idx],
        None => raw,
    }
    .trim();
    if ua.is_empty() {
        return ParsedUserAgent::default();
    }
    ParsedUserAgent {
        browser: parse_browser(ua),
        os: parse_os(ua),
    }
}

fn parse_browser(ua: &str) -> Option<UaComponent> {
    let mobile = ua.contains("Mobile");
    for (token, name) in BROWSER_TOKENS {
        if let Some(version) = version_after(ua, token) {
            let name = if *name == "Chrome" && mobile && ua.contains("Android") {
                "Chrome Mobile"
            } else {
                name
            };
            return Some(component(name, Some(version)));
        }
    }
    if ua.contains("Safari/") {
        let name = if mobile { "Mobile Safari" } else { "Safari" };
        return Some(component(name, version_after(ua, "Version/")));
    }
    if ua.starts_with("Mozilla/") {
        return None;
    }
    // Non-browser clients (`curl/8.5.0`, `python-requests/2.32`, `okhttp/4.12`): first product token.
    let first = ua.split_whitespace().next()?;
    let (name, version) = first.split_once('/').unwrap_or((first, ""));
    if name.is_empty() || name.len() > 64 {
        return None;
    }
    let version = clean_version(version);
    Some(component(name, (!version.is_empty()).then_some(version)))
}

fn parse_os(ua: &str) -> Option<UaComponent> {
    if let Some(nt) = version_after(ua, "Windows NT ") {
        let release = match nt.as_str() {
            "10.0" => Some("10"),
            "6.3" => Some("8.1"),
            "6.2" => Some("8"),
            "6.1" => Some("7"),
            _ => None,
        };
        return Some(component("Windows", release.map(str::to_owned)));
    }
    if ua.contains("Windows") {
        return Some(component("Windows", None));
    }
    for token in ["iPhone OS ", "CPU OS "] {
        if let Some(version) = version_after(ua, token) {
            return Some(component("iOS", Some(version)));
        }
    }
    if let Some(version) = version_after(ua, "Mac OS X ") {
        return Some(component("Mac OS X", Some(version)));
    }
    if let Some(version) = version_after(ua, "Android ") {
        return Some(component("Android", Some(version)));
    }
    if ua.contains("CrOS") {
        return Some(component("Chrome OS", None));
    }
    if ua.contains("Ubuntu") {
        return Some(component("Ubuntu", None));
    }
    if ua.contains("Linux") || ua.contains("X11") {
        return Some(component("Linux", None));
    }
    None
}

fn component(name: &str, version: Option<String>) -> UaComponent {
    UaComponent {
        name: name.to_owned(),
        version,
    }
}

/// Version right after `token`, with `_` separators (Apple) turned into dots.
fn version_after(ua: &str, token: &str) -> Option<String> {
    let start = ua.find(token)? + token.len();
    let version = clean_version(&ua[start..]);
    (!version.is_empty()).then_some(version)
}

/// Leading `[0-9._]` run, `_` → `.`, at most 4 components and 32 chars.
fn clean_version(raw: &str) -> String {
    let run: String = raw
        .chars()
        .take_while(|c| c.is_ascii_digit() || *c == '.' || *c == '_')
        .take(32)
        .map(|c| if c == '_' { '.' } else { c })
        .collect();
    let parts: Vec<&str> = run
        .trim_matches('.')
        .split('.')
        .filter(|p| !p.is_empty())
        .take(4)
        .collect();
    parts.join(".")
}

#[cfg(test)]
mod tests {
    use super::*;

    type Pair = Option<(String, Option<String>)>;

    fn parsed(ua: &str) -> (Pair, Pair) {
        let p = parse_user_agent(ua);
        (
            p.browser.map(|c| (c.name, c.version)),
            p.os.map(|c| (c.name, c.version)),
        )
    }

    fn some(name: &str, version: Option<&str>) -> Pair {
        Some((name.to_owned(), version.map(str::to_owned)))
    }

    #[test]
    fn desktop_chrome_on_windows() {
        let ua = "Mozilla/5.0 (Windows NT 10.0; Win64; x64) AppleWebKit/537.36 (KHTML, like Gecko) Chrome/131.0.0.0 Safari/537.36";
        assert_eq!(
            parsed(ua),
            (
                some("Chrome", Some("131.0.0.0")),
                some("Windows", Some("10"))
            )
        );
    }

    #[test]
    fn edge_is_not_reported_as_chrome() {
        let ua = "Mozilla/5.0 (Windows NT 10.0; Win64; x64) AppleWebKit/537.36 (KHTML, like Gecko) Chrome/131.0.0.0 Safari/537.36 Edg/131.0.2903.70";
        assert_eq!(parsed(ua).0, some("Edge", Some("131.0.2903.70")));
    }

    #[test]
    fn safari_on_iphone_uses_version_token_and_dotted_ios_version() {
        let ua = "Mozilla/5.0 (iPhone; CPU iPhone OS 17_1_2 like Mac OS X) AppleWebKit/605.1.15 (KHTML, like Gecko) Version/17.1 Mobile/15E148 Safari/604.1";
        assert_eq!(
            parsed(ua),
            (
                some("Mobile Safari", Some("17.1")),
                some("iOS", Some("17.1.2"))
            )
        );
    }

    #[test]
    fn firefox_on_linux_and_chrome_on_android() {
        let ff = "Mozilla/5.0 (X11; Linux x86_64; rv:133.0) Gecko/20100101 Firefox/133.0";
        assert_eq!(
            parsed(ff),
            (some("Firefox", Some("133.0")), some("Linux", None))
        );
        let android = "Mozilla/5.0 (Linux; Android 14; Pixel 8) AppleWebKit/537.36 (KHTML, like Gecko) Chrome/131.0.0.0 Mobile Safari/537.36";
        assert_eq!(
            parsed(android),
            (
                some("Chrome Mobile", Some("131.0.0.0")),
                some("Android", Some("14"))
            )
        );
    }

    #[test]
    fn http_clients_use_their_product_token() {
        assert_eq!(parsed("curl/8.5.0"), (some("curl", Some("8.5.0")), None));
        assert_eq!(
            parsed("python-requests/2.32.3"),
            (some("python-requests", Some("2.32.3")), None)
        );
    }

    #[test]
    fn oversized_multibyte_header_is_bounded_without_panicking() {
        let ua = format!(
            "Mozilla/5.0 (Windows NT 10.0) {} Firefox/133.0",
            "é".repeat(5000)
        );
        let p = parse_user_agent(&ua);
        assert_eq!(p.os.map(|c| c.name), Some("Windows".to_owned()));
        assert_eq!(
            p.browser, None,
            "token beyond the inspected prefix is ignored"
        );
    }

    #[test]
    fn empty_or_unknown_mozilla_strings_yield_nothing() {
        assert_eq!(parsed(""), (None, None));
        assert_eq!(parsed("Mozilla/5.0 (compatible)"), (None, None));
    }
}
