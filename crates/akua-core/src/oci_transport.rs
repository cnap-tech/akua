//! Shared HTTP + auth plumbing for OCI fetch + push.
//!
//! Kept in a dedicated module so `oci_fetcher` (GET paths for chart
//! pulls) and `oci_pusher` (POST/PUT paths for `akua publish`) don't
//! duplicate the bearer-challenge dance, token cache, and ref
//! parser. Anything OCI-spec-level ("how do you talk to a
//! distribution-API registry") lives here; anything akua-specific
//! ("what media types are helm charts" / "which layer is the
//! signature") stays in the caller.

use std::time::Duration;

use serde::Deserialize;

use crate::oci_auth::Credentials;

/// Parsed OCI reference. `oci://<registry>/<repo>` → the tuple. Tests
/// cover this below so non-network parser changes don't regress.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct OciRef {
    pub registry: String,
    pub repository: String,
}

/// HTTP scheme to use when talking to `registry`. `https` everywhere
/// except loopback hosts (`localhost`, `127.0.0.1`, `[::1]`) — matches
/// the convention `docker`, `oras`, `crane`, and `skopeo` use for
/// self-hosted local / dev / test registries. The match is on the
/// hostname *and* an optional `:port`, so `localhost:5000` and
/// `127.0.0.1:8443` both qualify.
pub(crate) fn registry_scheme(registry: &str) -> &'static str {
    // Strip an optional `:<port>` to get the bare host. IPv6 literals
    // are bracketed (`[::1]:5000`) so a trailing port is the only colon
    // *outside* the brackets; for bare-IPv6 (`::1`) we match the whole
    // string against the loopback set below.
    let host = if let Some(rest) = registry.strip_prefix('[') {
        rest.split(']').next().unwrap_or(rest)
    } else if registry.matches(':').count() == 1 {
        registry.split(':').next().unwrap_or(registry)
    } else {
        registry
    };
    if matches!(host, "localhost" | "127.0.0.1" | "::1") {
        "http"
    } else {
        "https"
    }
}

/// Parse `oci://<registry>/<path/to/repo>` → `OciRef`. Scheme is
/// required — bare registry refs are an ambiguity the spec forbids.
pub(crate) fn parse_ref(s: &str) -> Result<OciRef, TransportError> {
    let rest = s
        .strip_prefix("oci://")
        .ok_or_else(|| TransportError::BadRef(s.to_string()))?;
    let (registry, repository) = rest
        .split_once('/')
        .ok_or_else(|| TransportError::BadRef(s.to_string()))?;
    if registry.is_empty() || repository.is_empty() {
        return Err(TransportError::BadRef(s.to_string()));
    }
    Ok(OciRef {
        registry: registry.to_string(),
        repository: repository.to_string(),
    })
}

/// Build a reqwest blocking client. Single place so all OCI calls
/// share a user-agent + timeout policy.
pub(crate) fn build_client() -> Result<reqwest::blocking::Client, TransportError> {
    reqwest::blocking::Client::builder()
        .timeout(Duration::from_secs(60))
        .user_agent(concat!("akua/", env!("CARGO_PKG_VERSION")))
        .build()
        .map_err(|source| TransportError::Http {
            url: "<client-construction>".to_string(),
            source,
        })
}

/// Bearer-token cache scoped to a single OCI operation. Keeps the
/// first challenge-traded token hot for subsequent manifest + blob
/// requests that share a scope.
#[derive(Default)]
pub(crate) struct TokenCache {
    pub token: Option<String>,
}

#[derive(Debug, thiserror::Error)]
pub enum TransportError {
    #[error("invalid OCI reference `{0}`: expected `oci://<registry>/<repo>`")]
    BadRef(String),

    #[error("http error on `{url}`: {source}")]
    Http {
        url: String,
        #[source]
        source: reqwest::Error,
    },

    #[error("registry returned {status} for `{url}`: {body}")]
    Status {
        url: String,
        status: u16,
        body: String,
    },

    #[error("registry `{registry}` rejected auth. Configure credentials in `~/.config/akua/auth.toml` or `docker login` for `~/.docker/config.json`.")]
    AuthRequired { registry: String },
}

/// Parsed `WWW-Authenticate: Bearer realm=...,service=...,scope=...`
/// challenge. Registries use quoted values per RFC 7235.
#[derive(Debug)]
pub(crate) struct BearerChallenge {
    pub realm: String,
    pub service: Option<String>,
    pub scope: Option<String>,
}

impl BearerChallenge {
    pub(crate) fn from_resp(resp: &reqwest::blocking::Response) -> Option<Self> {
        let hdr = resp.headers().get("WWW-Authenticate")?.to_str().ok()?;
        let rest = hdr.strip_prefix("Bearer ")?;
        let mut out = BearerChallenge {
            realm: String::new(),
            service: None,
            scope: None,
        };
        for part in rest.split(',') {
            let (k, v) = part.trim().split_once('=')?;
            let v = v.trim().trim_matches('"').to_string();
            match k.trim() {
                "realm" => out.realm = v,
                "service" => out.service = Some(v),
                "scope" => out.scope = Some(v),
                _ => {}
            }
        }
        if out.realm.is_empty() {
            return None;
        }
        Some(out)
    }
}

/// Exchange a bearer challenge for an access token. When `creds` is
/// `Some` the auth header is attached to the realm request (Basic
/// for username/password, Bearer for a raw PAT). Anonymous omits the
/// header and gets a public-scope token.
pub(crate) fn fetch_token(
    client: &reqwest::blocking::Client,
    challenge: &BearerChallenge,
    creds: Option<&Credentials>,
) -> Result<String, TransportError> {
    let mut req = client.get(&challenge.realm);
    if let Some(c) = creds {
        req = req.header("Authorization", c.to_authorization_header());
    }
    let mut query: Vec<(&str, &str)> = Vec::new();
    if let Some(service) = &challenge.service {
        query.push(("service", service));
    }
    if let Some(scope) = &challenge.scope {
        query.push(("scope", scope));
    }
    if !query.is_empty() {
        req = req.query(&query);
    }
    let resp = req.send().map_err(|source| TransportError::Http {
        url: challenge.realm.clone(),
        source,
    })?;
    if !resp.status().is_success() {
        let status = resp.status();
        let body = resp.text().unwrap_or_default();
        return Err(TransportError::Status {
            url: challenge.realm.clone(),
            status: status.as_u16(),
            body,
        });
    }

    #[derive(Deserialize)]
    struct TokenResp {
        #[serde(default)]
        token: String,
        #[serde(default)]
        access_token: String,
    }
    let body: TokenResp = resp.json().map_err(|source| TransportError::Http {
        url: challenge.realm.clone(),
        source,
    })?;
    let tok = if !body.token.is_empty() {
        body.token
    } else {
        body.access_token
    };
    if tok.is_empty() {
        return Err(TransportError::AuthRequired {
            registry: challenge
                .service
                .clone()
                .unwrap_or_else(|| challenge.realm.clone()),
        });
    }
    Ok(tok)
}

/// Apply the current cached bearer token (or a raw PAT if that's
/// all we have). Basic creds don't get attached directly — they're
/// forwarded to the realm endpoint via `fetch_token` on a 401.
pub(crate) fn apply_bearer(
    req: reqwest::blocking::RequestBuilder,
    token_cache: &TokenCache,
    creds: Option<&Credentials>,
) -> reqwest::blocking::RequestBuilder {
    if let Some(tok) = &token_cache.token {
        return req.bearer_auth(tok);
    }
    if let Some(Credentials::Bearer { token }) = creds {
        return req.bearer_auth(token);
    }
    req
}

/// GET with the retry-on-401-bearer-challenge pattern. Shared
/// between `oci_fetcher` and `oci_puller` — both need the same
/// auth + decorate shape for pulls. The `decorate` closure lets
/// callers add `Accept:` headers per request type.
pub(crate) fn get_with_auth(
    client: &reqwest::blocking::Client,
    url: &str,
    registry: &str,
    creds: Option<&Credentials>,
    token_cache: &mut TokenCache,
    decorate: impl Fn(reqwest::blocking::RequestBuilder) -> reqwest::blocking::RequestBuilder,
) -> Result<Vec<u8>, TransportError> {
    let req = apply_bearer(decorate(client.get(url)), token_cache, creds);
    let resp = req.send().map_err(|source| TransportError::Http {
        url: url.to_string(),
        source,
    })?;
    if resp.status().as_u16() != 401 {
        return ensure_ok(resp, url);
    }

    let challenge =
        BearerChallenge::from_resp(&resp).ok_or_else(|| TransportError::AuthRequired {
            registry: registry.to_string(),
        })?;
    let token = fetch_token(client, &challenge, creds)?;
    token_cache.token = Some(token.clone());

    let retry_req = decorate(client.get(url)).bearer_auth(&token);
    let retry = retry_req.send().map_err(|source| TransportError::Http {
        url: url.to_string(),
        source,
    })?;
    if retry.status().as_u16() == 401 {
        return Err(TransportError::AuthRequired {
            registry: registry.to_string(),
        });
    }
    ensure_ok(retry, url)
}

/// Success-path unwrap: on 2xx pull the body as bytes, on anything
/// else capture a short body for diagnostics.
pub(crate) fn ensure_ok(
    resp: reqwest::blocking::Response,
    url: &str,
) -> Result<Vec<u8>, TransportError> {
    let status = resp.status();
    if !status.is_success() {
        let body = resp.text().unwrap_or_default();
        let truncated = if body.len() > 300 {
            &body[..300]
        } else {
            body.as_str()
        };
        return Err(TransportError::Status {
            url: url.to_string(),
            status: status.as_u16(),
            body: truncated.to_string(),
        });
    }
    resp.bytes()
        .map(|b| b.to_vec())
        .map_err(|source| TransportError::Http {
            url: url.to_string(),
            source,
        })
}

// ---------------------------------------------------------------------------
// Tests — parser coverage + challenge parser. HTTP-requiring tests
// live in the integration-test crates.
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_standard_refs() {
        assert_eq!(
            parse_ref("oci://ghcr.io/acme/app").unwrap(),
            OciRef {
                registry: "ghcr.io".into(),
                repository: "acme/app".into()
            }
        );
        assert_eq!(
            parse_ref("oci://registry-1.docker.io/bitnamicharts/nginx").unwrap(),
            OciRef {
                registry: "registry-1.docker.io".into(),
                repository: "bitnamicharts/nginx".into()
            }
        );
    }

    #[test]
    fn rejects_refs_without_scheme_or_repo() {
        assert!(matches!(
            parse_ref("ghcr.io/x/y"),
            Err(TransportError::BadRef(_))
        ));
        assert!(matches!(
            parse_ref("oci://ghcr.io"),
            Err(TransportError::BadRef(_))
        ));
        assert!(matches!(parse_ref(""), Err(TransportError::BadRef(_))));
    }

    #[test]
    fn registry_scheme_uses_https_for_real_registries() {
        assert_eq!(registry_scheme("ghcr.io"), "https");
        assert_eq!(registry_scheme("registry-1.docker.io"), "https");
        assert_eq!(registry_scheme("registry.example.com:5000"), "https");
    }

    #[test]
    fn registry_scheme_uses_http_for_loopback() {
        // Bare loopback hostnames + with port — both must downgrade to
        // http so local mock registries (incl. our test fixtures) work
        // without TLS termination.
        assert_eq!(registry_scheme("localhost"), "http");
        assert_eq!(registry_scheme("localhost:5000"), "http");
        assert_eq!(registry_scheme("127.0.0.1"), "http");
        assert_eq!(registry_scheme("127.0.0.1:8443"), "http");
        assert_eq!(registry_scheme("::1"), "http");
        assert_eq!(registry_scheme("[::1]"), "http");
    }
}
