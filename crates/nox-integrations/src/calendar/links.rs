//! Video-call link detection in an event's URL, location and notes.

use nox_core::model::calendar::{CallService, VideoCall};

fn classify(url: &str) -> Option<CallService> {
    // `\` is a path separator for WHATWG URL parsers ("evil.com\@zoom.us"
    // opens evil.com): refuse it, and anything non-ASCII, outright.
    if !url.is_ascii() || url.contains('\\') {
        return None;
    }
    let lower = url.to_ascii_lowercase();
    let (scheme, rest) = lower.split_once("://")?;
    match scheme {
        "zoommtg" | "zoomus" => return Some(CallService::Zoom),
        "msteams" => return Some(CallService::Teams),
        "facetime" => return Some(CallService::FaceTime),
        "webex" | "wbx" => return Some(CallService::Webex),
        "https" | "http" => {}
        _ => return None,
    }
    let host = rest.split(['/', '?', '#']).next()?.split('@').next_back()?;
    let host = host.split(':').next()?;
    let path = &rest[rest.find('/').unwrap_or(rest.len())..];
    let is = |domain: &str| host == domain || host.ends_with(&format!(".{domain}"));
    if host == "meet.google.com" && path.len() > 1 {
        Some(CallService::Meet)
    } else if is("zoom.us") || is("zoomgov.com") {
        (path.starts_with("/j/")
            || path.starts_with("/my/")
            || path.starts_with("/s/")
            || path.starts_with("/w/"))
        .then_some(CallService::Zoom)
    } else if host == "teams.microsoft.com"
        || host == "teams.live.com"
        || host == "teams.cloud.microsoft"
    {
        (path.contains("meetup-join") || path.starts_with("/meet/") || path.contains("/l/meet"))
            .then_some(CallService::Teams)
    } else if host == "facetime.apple.com" {
        Some(CallService::FaceTime)
    } else if is("webex.com") {
        Some(CallService::Webex)
    } else if host == "whereby.com"
        || host == "meet.jit.si"
        || is("gotomeeting.com")
        || host == "meet.goto.com"
    {
        Some(CallService::Other)
    } else {
        None
    }
}

/// Splits free text into URL-like tokens (scheme://… up to whitespace or a
/// delimiter), trimming trailing punctuation.
fn urls(text: &str) -> impl Iterator<Item = &str> {
    text.split(|c: char| {
        c.is_whitespace() || matches!(c, '<' | '>' | '"' | '\'' | '(' | ')' | '[' | ']')
    })
    .filter_map(|tok| {
        let start = tok.find("://")?;
        // Walk back to the start of the scheme.
        let scheme_start = tok[..start]
            .rfind(|c: char| !(c.is_ascii_alphanumeric() || c == '+' || c == '-' || c == '.'))
            .map(|i| i + 1)
            .unwrap_or(0);
        let url = tok[scheme_start..].trim_end_matches(['.', ',', ';', ':', '!', '?']);
        (url.len() > start - scheme_start + 3).then_some(url)
    })
}

/// First video-call link found, in the order URL field, location, notes.
pub fn detect(url: Option<&str>, location: Option<&str>, notes: Option<&str>) -> Option<VideoCall> {
    [url, location, notes]
        .into_iter()
        .flatten()
        .flat_map(urls)
        .find_map(|u| {
            classify(u).map(|service| VideoCall {
                service,
                url: u.to_owned(),
            })
        })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn svc(text: &str) -> Option<CallService> {
        detect(None, None, Some(text)).map(|c| c.service)
    }

    #[test]
    fn detects_common_services() {
        assert_eq!(
            svc("Join: https://meet.google.com/abc-defg-hij"),
            Some(CallService::Meet)
        );
        assert_eq!(
            svc("https://example.zoom.us/j/123456789?pwd=x."),
            Some(CallService::Zoom)
        );
        assert_eq!(
            svc("<https://teams.microsoft.com/l/meetup-join/19%3ameeting_x/0?context=y>"),
            Some(CallService::Teams)
        );
        assert_eq!(
            svc("facetime://user@example.com"),
            Some(CallService::FaceTime)
        );
        assert_eq!(
            svc("https://facetime.apple.com/join#v=1&p=x"),
            Some(CallService::FaceTime)
        );
        assert_eq!(
            svc("https://acme.webex.com/meet/someone"),
            Some(CallService::Webex)
        );
    }

    #[test]
    fn ignores_lookalikes_and_plain_pages() {
        assert_eq!(svc("https://zoom.us.evil.example/j/1"), None);
        assert_eq!(svc("https://evil.com\\@zoom.us/j/1"), None);
        assert_eq!(svc("https://zооm.us/j/1"), None, "Cyrillic homoglyphs");
        assert_eq!(svc("https://notzoom.us/j/1"), None);
        assert_eq!(svc("https://zoom.us/pricing"), None);
        assert_eq!(svc("https://meet.google.com/"), None);
        assert_eq!(svc("Salle 4, 2e étage"), None);
    }

    #[test]
    fn precedence_and_trimming() {
        let c = detect(
            Some("https://example.org/agenda"),
            Some("Zoom (https://x.zoom.us/j/42),"),
            Some("https://meet.google.com/aaa-bbbb-ccc"),
        )
        .unwrap();
        assert_eq!(c.service, CallService::Zoom);
        assert_eq!(c.url, "https://x.zoom.us/j/42");
    }
}
