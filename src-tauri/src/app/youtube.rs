//! YouTube-only navigation policy.
//!
//! Every address a tab can be pointed at, and every navigation a page can
//! start, passes through this module first.
//!
//! The allowlist is deliberately small: YouTube's own origins, the short-link
//! domain, and the Google hosts the sign-in flow redirects through. Anything
//! else is denied in the webview and offered to the system browser instead.

use tauri::Url;

/// The only home page the application has.
pub const HOME: &str = "https://www.youtube.com/";

/// Top-level origins a YouTube page is allowed to navigate to.
///
/// Subresources (thumbnails from ytimg.com, media from googlevideo.com, fonts
/// from gstatic.com) are not navigations and are unaffected by this list.
const NAVIGATION_DOMAINS: &[&str] = &[
    "youtube.com",
    "youtube-nocookie.com",
    "youtu.be",
    // OAuth and the cookie-consent redirect used by signed-out sessions.
    "accounts.google.com",
    "consent.google.com",
];

/// True for `domain` itself or any of its subdomains.
///
/// A suffix comparison alone would accept `youtube.com.evil.com`, so the
/// boundary dot is part of the match.
fn is_domain(host: &str, domain: &str) -> bool {
    let host = host.trim_end_matches('.').to_ascii_lowercase();
    let suffix = format!(".{domain}");
    host == domain || host.ends_with(suffix.as_str())
}

/// True when a URL is a page this application is allowed to show.
pub fn is_youtube_target(url: &Url) -> bool {
    if !matches!(url.scheme(), "http" | "https") {
        return false;
    }
    let Some(host) = url.host_str() else {
        return false;
    };
    NAVIGATION_DOMAINS
        .iter()
        .any(|domain| is_domain(host, domain))
}

/// Whether a navigation request may proceed inside a tab.
///
/// Non-web schemes are handled explicitly: WebView2's own blank documents, the
/// local tab strip and extension option pages are internal, not a route out of
/// YouTube. Everything else (file:, javascript:, custom protocols) is denied.
pub fn allows_navigation(url: &Url) -> bool {
    match url.scheme() {
        "http" | "https" => is_youtube_target(url),
        "about" | "tauri" | "chrome-extension" | "blob" | "data" => true,
        _ => false,
    }
}

/// Resolve anything the user or a saved session asks to open into a YouTube URL.
///
/// Bare hosts such as `youtube.com/watch?v=…` are accepted. A URL outside the
/// allowlist falls back to the home page rather than turning this shell into a
/// browser for arbitrary sites.
pub fn target(raw: &str) -> Url {
    let trimmed = raw.trim();
    let candidate = if trimmed.is_empty() {
        HOME.to_string()
    } else if trimmed.contains("://") {
        trimmed.to_string()
    } else {
        format!("https://{trimmed}")
    };

    Url::parse(&candidate)
        .ok()
        .filter(is_youtube_target)
        .unwrap_or_else(|| Url::parse(HOME).expect("HOME is a valid URL"))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn url(raw: &str) -> Url {
        Url::parse(raw).expect("test URL")
    }

    #[test]
    fn accepts_youtube_and_its_own_subdomains() {
        for raw in [
            "https://www.youtube.com/",
            "https://youtube.com/watch?v=dQw4w9WgXcQ",
            "https://m.youtube.com/",
            "https://music.youtube.com/",
            "https://consent.youtube.com/",
            "https://www.youtube-nocookie.com/embed/abc",
            "https://youtu.be/dQw4w9WgXcQ",
        ] {
            assert!(is_youtube_target(&url(raw)), "denied {raw}");
            assert!(allows_navigation(&url(raw)), "denied navigation to {raw}");
        }
    }

    #[test]
    fn accepts_only_the_sign_in_hosts_outside_youtube() {
        for raw in [
            "https://accounts.google.com/o/oauth2/auth",
            "https://consent.google.com/",
        ] {
            assert!(allows_navigation(&url(raw)), "denied {raw}");
        }
        for raw in [
            "https://www.google.com/search?q=ads",
            "https://support.google.com/youtube/",
            "https://google.com/",
        ] {
            assert!(!allows_navigation(&url(raw)), "allowed {raw}");
        }
    }

    #[test]
    fn rejects_lookalike_hosts_and_other_sites() {
        for raw in [
            "https://youtube.com.evil.com/",
            "https://notyoutube.com/",
            "https://www.youtube.co/",
            "https://evil.com/?next=https://www.youtube.com/",
            "https://doubleclick.net/",
            "file:///C:/Windows/notepad.exe",
            "javascript:alert(1)",
        ] {
            assert!(!is_youtube_target(&url(raw)), "accepted {raw}");
            assert!(!allows_navigation(&url(raw)), "allowed navigation to {raw}");
        }
    }

    #[test]
    fn internal_documents_are_not_a_route_out_of_the_app() {
        for raw in [
            "about:blank",
            "tauri://localhost/tabs.html",
            "chrome-extension://abcdef/options.html",
        ] {
            assert!(allows_navigation(&url(raw)), "denied {raw}");
            assert!(!is_youtube_target(&url(raw)), "tab target leaked {raw}");
        }
    }

    #[test]
    fn every_unknown_target_falls_back_home() {
        for raw in [
            "",
            "   ",
            "https://example.com/watch?v=1",
            "https://youtube.com.evil.com/watch?v=1",
            "javascript:location.href='https://evil.com'",
        ] {
            assert_eq!(target(raw).as_str(), HOME, "wrong target for {raw:?}");
        }
    }

    #[test]
    fn bare_hosts_and_real_links_keep_their_path() {
        assert_eq!(
            target("youtube.com/watch?v=dQw4w9WgXcQ").as_str(),
            "https://youtube.com/watch?v=dQw4w9WgXcQ"
        );
        assert_eq!(
            target("https://www.youtube.com/results?search_query=rust").as_str(),
            "https://www.youtube.com/results?search_query=rust"
        );
        assert_eq!(
            target("https://youtu.be/abc").as_str(),
            "https://youtu.be/abc"
        );
    }
}
