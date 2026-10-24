//! Minimal HTTPS GET over NSURLSession (system TLS, no extra dependency).
//! Ephemeral session: no cookies, no credential storage, no URL cache.

use block2::RcBlock;
use objc2_foundation::{
    NSData, NSError, NSHTTPURLResponse, NSString, NSURL, NSURLRequestCachePolicy, NSURLResponse,
    NSURLSession, NSURLSessionConfiguration,
};
use std::sync::mpsc;
use std::time::Duration;

#[derive(Debug)]
pub struct Response {
    pub status: u16,
    pub body: Vec<u8>,
}

pub fn get(
    url: &str,
    headers: &[(&str, &str)],
    timeout: Duration,
    max_bytes: usize,
) -> Result<Response, String> {
    let ns_url = NSURL::URLWithString(&NSString::from_str(url)).ok_or("invalid URL")?;
    let config = NSURLSessionConfiguration::ephemeralSessionConfiguration();
    config.setTimeoutIntervalForRequest(timeout.as_secs_f64());
    config.setTimeoutIntervalForResource(timeout.as_secs_f64() * 1.5);
    config.setURLCache(None);
    config.setHTTPCookieStorage(None);
    let session = NSURLSession::sessionWithConfiguration(&config);
    let request = objc2_foundation::NSMutableURLRequest::requestWithURL_cachePolicy_timeoutInterval(
        &ns_url,
        NSURLRequestCachePolicy::ReloadIgnoringLocalCacheData,
        timeout.as_secs_f64(),
    );
    for (k, v) in headers {
        request.setValue_forHTTPHeaderField(Some(&NSString::from_str(v)), &NSString::from_str(k));
    }
    let (tx, rx) = mpsc::channel::<Result<Response, String>>();
    let block = RcBlock::new(
        move |data: *mut NSData, response: *mut NSURLResponse, error: *mut NSError| {
            // SAFETY: NSURLSession passes valid (possibly null) objects that
            // live for the duration of the completion handler.
            let (data, response, error) =
                unsafe { (data.as_ref(), response.as_ref(), error.as_ref()) };
            let r = if let Some(e) = error {
                Err(e.localizedDescription().to_string())
            } else {
                let status = response
                    .and_then(|r| r.downcast_ref::<NSHTTPURLResponse>())
                    .map(|r| r.statusCode() as u16)
                    .unwrap_or(0);
                let body = data.map(|d| d.to_vec()).unwrap_or_default();
                Ok(Response { status, body })
            };
            let _ = tx.send(r);
        },
    );
    // SAFETY: the completion block only captures a `Sender` (Send) and is
    // called once by NSURLSession on its delegate queue.
    let task = unsafe { session.dataTaskWithRequest_completionHandler(&request, &block) };
    task.resume();
    session.finishTasksAndInvalidate();
    let r = match rx.recv_timeout(timeout * 2) {
        Ok(r) => r,
        Err(_) => {
            task.cancel();
            return Err("request timed out".into());
        }
    };
    if let Ok(resp) = &r
        && resp.body.len() > max_bytes
    {
        return Err("response too large".into());
    }
    r
}

/// Percent-encodes a query component (RFC 3986 unreserved kept).
pub fn encode_component(s: &str) -> String {
    let mut out = String::with_capacity(s.len() * 3);
    for b in s.bytes() {
        match b {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'_' | b'.' | b'~' => {
                out.push(b as char)
            }
            _ => out.push_str(&format!("%{b:02X}")),
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn encodes_query_components() {
        assert_eq!(encode_component("Daft Punk & co"), "Daft%20Punk%20%26%20co");
        assert_eq!(encode_component("é"), "%C3%A9");
    }
}
