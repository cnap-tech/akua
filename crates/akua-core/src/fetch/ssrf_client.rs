//! SSRF-hardened HTTP client + URL-redaction helper.
//!
//! Two concerns kept together because they co-protect the fetch path:
//!
//! - [`ssrf_safe_client`] configures `reqwest` with a redirect policy
//!   that re-validates every hop against [`crate::ssrf::validate_host`].
//!   A public registry can't 302 us to a cloud-metadata IP.
//! - [`HttpError`] wraps `reqwest::Error` so its `Display` no longer
//!   leaks request URLs (which may embed `user:pass@` userinfo when
//!   the originating `package.yaml` was attacker-authored).
//! - [`redact_userinfo`] is the same scrubber used everywhere error
//!   messages interpolate a URL or repo string.

use super::FetchError;

/// Wrapper around `reqwest::Error` whose `Display` strips the URL.
/// reqwest's default `Display` embeds the full request URL — if a
/// user-authored `package.yaml` contained `oci://user:pass@host/...`,
/// the credentials could leak into logs or error chains. The
/// underlying error is still accessible via `source()` for debugging
/// by developers who already have the credential material.
#[derive(Debug)]
pub struct HttpError(reqwest::Error);

impl std::fmt::Display for HttpError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        // `without_url` is only available on owned errors (it mutates).
        // We hold a reference — hand-strip any `https://user:pass@`
        // prefix that might appear in the Display output.
        write!(f, "{}", redact_userinfo(&self.0.to_string()))
    }
}

impl std::error::Error for HttpError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        self.0.source()
    }
}

impl From<reqwest::Error> for HttpError {
    fn from(e: reqwest::Error) -> Self {
        Self(e)
    }
}

/// Scrub any `<scheme>://user:password@` userinfo fragment in the
/// given string. Used at error-message construction time so a
/// credential-bearing URL authored by an attacker can't ride an error
/// chain into logs.
pub fn redact_userinfo(s: &str) -> String {
    // Pattern: `scheme://user[:pass]@host` — replace with `scheme://<redacted>@host`.
    let mut out = String::with_capacity(s.len());
    let mut i = 0;
    let bytes = s.as_bytes();
    while i < bytes.len() {
        // Look for `://` followed by `@` before any `/`.
        if bytes[i..].starts_with(b"://") {
            out.push_str("://");
            i += 3;
            // Scan to the next `@`, `/`, space, or end.
            let mut j = i;
            let mut saw_at = false;
            while j < bytes.len() {
                match bytes[j] {
                    b'@' => {
                        saw_at = true;
                        break;
                    }
                    b'/' | b' ' | b'"' | b'`' => break,
                    _ => j += 1,
                }
            }
            if saw_at {
                out.push_str("<redacted>@");
                i = j + 1;
            }
            continue;
        }
        out.push(bytes[i] as char);
        i += 1;
    }
    out
}

/// Reject private/loopback/link-local IP-literal hosts in `repo`.
/// Applies to both `oci://` and `http(s)://`. Pure URL check — DNS
/// names pass through (network-layer egress policy is the proper
/// mitigation for DNS-rebinding attacks).
pub(super) fn validate_repo_ssrf(repo: &str) -> Result<(), FetchError> {
    let host = repo
        .strip_prefix("oci://")
        .or_else(|| repo.strip_prefix("https://"))
        .or_else(|| repo.strip_prefix("http://"))
        .unwrap_or(repo)
        .split('/')
        .next()
        .unwrap_or("");
    crate::ssrf::validate_host(host)
}

/// reqwest client whose redirect policy re-runs the SSRF check on
/// every hop — prevents a public registry from 302-ing us to a
/// private IP (cloud metadata exfil).
pub(super) fn ssrf_safe_client(max_redirects: usize) -> Result<reqwest::Client, FetchError> {
    let policy = reqwest::redirect::Policy::custom(move |attempt| {
        if attempt.previous().len() >= max_redirects {
            return attempt.error("too many redirects");
        }
        if let Some(host) = attempt.url().host_str() {
            if crate::ssrf::validate_host(host).is_err() {
                return attempt.error("redirect to private-range host rejected");
            }
        }
        attempt.follow()
    });
    reqwest::Client::builder()
        .redirect(policy)
        .build()
        .map_err(|e| FetchError::ClientConfig(format!("reqwest client: {e}")))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn redact_userinfo_strips_basic_userinfo() {
        assert_eq!(
            redact_userinfo("https://alice:s3cret@registry.example.com/v2/foo"),
            "https://<redacted>@registry.example.com/v2/foo"
        );
        assert_eq!(
            redact_userinfo("oci://u:p@ghcr.io/parent/chart"),
            "oci://<redacted>@ghcr.io/parent/chart"
        );
    }

    #[test]
    fn redact_userinfo_leaves_clean_urls_alone() {
        assert_eq!(
            redact_userinfo("https://ghcr.io/stefanprodan/charts/podinfo"),
            "https://ghcr.io/stefanprodan/charts/podinfo"
        );
    }
}
