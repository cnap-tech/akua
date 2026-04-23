//! Credential resolution for OCI registries.
//!
//! Two sources are consulted, in this order:
//!
//! 1. `$XDG_CONFIG_HOME/akua/auth.toml` (or `$HOME/.config/akua/auth.toml`) —
//!    akua-native shape. Recommended when the rest of `akua.toml` +
//!    `akua.lock` already live under source control and credentials
//!    belong in a separate state/secret store.
//! 2. `~/.docker/config.json` — standard docker/podman credential
//!    shape. Supported so `docker login` "just works" for akua on
//!    dev machines.
//!
//! First match wins. Both files are optional — absent files degrade
//! to anonymous pulls, which is what Phase 2b slice B already shipped.

use std::collections::HashMap;
use std::path::PathBuf;

use serde::Deserialize;

/// Credentials for a single registry host. Either basic (sent as
/// `Authorization: Basic <b64>` on the manifest request) or a raw
/// bearer token (skips the registry's token exchange — Docker PATs,
/// GHCR classic PATs, etc. use this shape).
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Credentials {
    Basic { username: String, password: String },
    Bearer { token: String },
}

impl Credentials {
    /// Encode into the `Authorization:` header value the caller can
    /// hand to `reqwest::RequestBuilder::header(…)`.
    pub fn to_authorization_header(&self) -> String {
        use base64::Engine as _;
        match self {
            Credentials::Basic { username, password } => {
                let raw = format!("{username}:{password}");
                let encoded = base64::engine::general_purpose::STANDARD.encode(raw.as_bytes());
                format!("Basic {encoded}")
            }
            Credentials::Bearer { token } => format!("Bearer {token}"),
        }
    }
}

/// Look up credentials for `registry` across both config sources.
/// Pure function: caller owns the file reads via the `load_*`
/// helpers; this function takes an already-populated [`CredsStore`].
/// Separated so tests can build a store without touching the disk.
pub fn for_registry(store: &CredsStore, registry: &str) -> Option<Credentials> {
    store.entries.get(registry).cloned()
}

/// Collected credentials keyed by registry host. Built via
/// [`CredsStore::load`] (reads both config files) or
/// [`CredsStore::empty`] (tests).
#[derive(Debug, Clone, Default)]
pub struct CredsStore {
    pub entries: HashMap<String, Credentials>,
}

impl CredsStore {
    pub fn empty() -> Self {
        Self::default()
    }

    /// Populate from the two config sources. Missing files are not
    /// an error — anonymous pulls were already working before this
    /// module landed; credentials are additive. Parse errors **are**
    /// surfaced so a typo in `auth.toml` doesn't silently fall
    /// through to anonymous and leak.
    pub fn load() -> Result<Self, AuthLoadError> {
        let mut store = Self::default();
        if let Some(path) = akua_auth_path() {
            store.merge_akua_auth(&path)?;
        }
        if let Some(path) = docker_config_path() {
            store.merge_docker_config(&path)?;
        }
        Ok(store)
    }

    fn merge_akua_auth(&mut self, path: &std::path::Path) -> Result<(), AuthLoadError> {
        let body = match std::fs::read_to_string(path) {
            Ok(s) => s,
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(()),
            Err(source) => {
                return Err(AuthLoadError::Io {
                    path: path.to_path_buf(),
                    source,
                });
            }
        };
        let parsed: AkuaAuthFile = toml::from_str(&body).map_err(|source| AuthLoadError::Parse {
            path: path.to_path_buf(),
            source: ParseBackend::Toml(source),
        })?;
        for (registry, entry) in parsed.registries {
            self.entries
                .entry(registry)
                .or_insert_with(|| entry.into_credentials());
        }
        Ok(())
    }

    fn merge_docker_config(&mut self, path: &std::path::Path) -> Result<(), AuthLoadError> {
        let body = match std::fs::read_to_string(path) {
            Ok(s) => s,
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(()),
            Err(source) => {
                return Err(AuthLoadError::Io {
                    path: path.to_path_buf(),
                    source,
                });
            }
        };
        let parsed: DockerConfig =
            serde_json::from_str(&body).map_err(|source| AuthLoadError::Parse {
                path: path.to_path_buf(),
                source: ParseBackend::Json(source),
            })?;
        for (registry, entry) in parsed.auths.unwrap_or_default() {
            if let Some(creds) = entry.into_credentials() {
                // akua auth.toml takes precedence over docker config —
                // use `entry().or_insert(...)` so we don't overwrite
                // a prior registration.
                self.entries.entry(registry).or_insert(creds);
            }
        }
        Ok(())
    }
}

#[derive(Debug, thiserror::Error)]
pub enum AuthLoadError {
    #[error("i/o reading auth config at {}: {source}", path.display())]
    Io {
        path: PathBuf,
        #[source]
        source: std::io::Error,
    },

    #[error("malformed auth config at {}: {source}", path.display())]
    Parse {
        path: PathBuf,
        #[source]
        source: ParseBackend,
    },
}

#[derive(Debug, thiserror::Error)]
pub enum ParseBackend {
    #[error("{0}")]
    Toml(#[from] toml::de::Error),
    #[error("{0}")]
    Json(#[from] serde_json::Error),
}

// --- File shapes -----------------------------------------------------------

#[derive(Debug, Deserialize)]
struct AkuaAuthFile {
    #[serde(default)]
    registries: HashMap<String, AkuaAuthEntry>,
}

#[derive(Debug, Deserialize)]
struct AkuaAuthEntry {
    #[serde(default)]
    username: Option<String>,
    #[serde(default)]
    password: Option<String>,
    #[serde(default)]
    token: Option<String>,
}

impl AkuaAuthEntry {
    fn into_credentials(self) -> Credentials {
        // Bearer wins when present — PATs are single-field, Basic
        // needs both halves.
        if let Some(token) = self.token {
            return Credentials::Bearer { token };
        }
        Credentials::Basic {
            username: self.username.unwrap_or_default(),
            password: self.password.unwrap_or_default(),
        }
    }
}

#[derive(Debug, Deserialize)]
struct DockerConfig {
    #[serde(default)]
    auths: Option<HashMap<String, DockerAuthEntry>>,
}

#[derive(Debug, Deserialize)]
struct DockerAuthEntry {
    /// Base64-encoded `username:password`. Standard docker format.
    #[serde(default)]
    auth: Option<String>,
    #[serde(default)]
    username: Option<String>,
    #[serde(default)]
    password: Option<String>,
    /// Not supported here: `credsStore` / `credHelpers` invoke an
    /// external binary (`docker-credential-helper`) — that's shell-
    /// out, forbidden by CLAUDE.md. Users on creds-helper flows
    /// need to export directly to `akua auth.toml`.
    #[serde(default)]
    #[allow(dead_code)]
    credential_helper: Option<String>,
}

impl DockerAuthEntry {
    fn into_credentials(self) -> Option<Credentials> {
        // The base64 `auth` field is the canonical shape; username +
        // password may also be set verbatim (rare but legal).
        if let Some(b64) = self.auth {
            use base64::Engine as _;
            if let Ok(decoded) = base64::engine::general_purpose::STANDARD.decode(b64.trim()) {
                if let Ok(text) = String::from_utf8(decoded) {
                    if let Some((user, pass)) = text.split_once(':') {
                        return Some(Credentials::Basic {
                            username: user.to_string(),
                            password: pass.to_string(),
                        });
                    }
                }
            }
        }
        match (self.username, self.password) {
            (Some(username), Some(password)) => Some(Credentials::Basic { username, password }),
            _ => None,
        }
    }
}

// --- Path discovery --------------------------------------------------------

fn akua_auth_path() -> Option<PathBuf> {
    if let Ok(xdg) = std::env::var("XDG_CONFIG_HOME") {
        if !xdg.is_empty() {
            return Some(PathBuf::from(xdg).join("akua/auth.toml"));
        }
    }
    std::env::var("HOME")
        .ok()
        .filter(|s| !s.is_empty())
        .map(|home| PathBuf::from(home).join(".config/akua/auth.toml"))
}

fn docker_config_path() -> Option<PathBuf> {
    if let Ok(dc) = std::env::var("DOCKER_CONFIG") {
        if !dc.is_empty() {
            return Some(PathBuf::from(dc).join("config.json"));
        }
    }
    std::env::var("HOME")
        .ok()
        .filter(|s| !s.is_empty())
        .map(|home| PathBuf::from(home).join(".docker/config.json"))
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn credentials_basic_to_header() {
        let c = Credentials::Basic {
            username: "alice".to_string(),
            password: "secret".to_string(),
        };
        // `alice:secret` → base64 → `YWxpY2U6c2VjcmV0`
        assert_eq!(c.to_authorization_header(), "Basic YWxpY2U6c2VjcmV0");
    }

    #[test]
    fn credentials_bearer_to_header() {
        let c = Credentials::Bearer {
            token: "ghp_abcdef".to_string(),
        };
        assert_eq!(c.to_authorization_header(), "Bearer ghp_abcdef");
    }

    #[test]
    fn empty_store_returns_none() {
        let store = CredsStore::empty();
        assert!(for_registry(&store, "ghcr.io").is_none());
    }

    #[test]
    fn akua_auth_toml_username_password() {
        let tmp = tempfile::tempdir().unwrap();
        let path = tmp.path().join("auth.toml");
        std::fs::write(
            &path,
            r#"
[registries."ghcr.io"]
username = "alice"
password = "s3cr3t"
"#,
        )
        .unwrap();
        let mut store = CredsStore::empty();
        store.merge_akua_auth(&path).unwrap();
        match for_registry(&store, "ghcr.io").unwrap() {
            Credentials::Basic { username, password } => {
                assert_eq!(username, "alice");
                assert_eq!(password, "s3cr3t");
            }
            c => panic!("expected Basic, got {c:?}"),
        }
    }

    #[test]
    fn akua_auth_toml_bearer_wins_over_username() {
        let tmp = tempfile::tempdir().unwrap();
        let path = tmp.path().join("auth.toml");
        std::fs::write(
            &path,
            r#"
[registries."ghcr.io"]
username = "ignored"
token    = "ghp_abc"
"#,
        )
        .unwrap();
        let mut store = CredsStore::empty();
        store.merge_akua_auth(&path).unwrap();
        assert_eq!(
            for_registry(&store, "ghcr.io"),
            Some(Credentials::Bearer {
                token: "ghp_abc".to_string()
            })
        );
    }

    #[test]
    fn docker_config_auth_base64_decodes() {
        let tmp = tempfile::tempdir().unwrap();
        let path = tmp.path().join("config.json");
        // `alice:s3cr3t` → YWxpY2U6czNjcjN0
        std::fs::write(
            &path,
            r#"{
  "auths": {
    "ghcr.io": { "auth": "YWxpY2U6czNjcjN0" }
  }
}"#,
        )
        .unwrap();
        let mut store = CredsStore::empty();
        store.merge_docker_config(&path).unwrap();
        match for_registry(&store, "ghcr.io").unwrap() {
            Credentials::Basic { username, password } => {
                assert_eq!(username, "alice");
                assert_eq!(password, "s3cr3t");
            }
            other => panic!("expected Basic, got {other:?}"),
        }
    }

    #[test]
    fn docker_config_username_password_fallback() {
        let tmp = tempfile::tempdir().unwrap();
        let path = tmp.path().join("config.json");
        std::fs::write(
            &path,
            r#"{
  "auths": {
    "ghcr.io": { "username": "alice", "password": "s3cr3t" }
  }
}"#,
        )
        .unwrap();
        let mut store = CredsStore::empty();
        store.merge_docker_config(&path).unwrap();
        assert!(matches!(
            for_registry(&store, "ghcr.io"),
            Some(Credentials::Basic { .. })
        ));
    }

    #[test]
    fn akua_auth_takes_precedence_over_docker_config() {
        let tmp = tempfile::tempdir().unwrap();
        let akua = tmp.path().join("auth.toml");
        let docker = tmp.path().join("config.json");
        std::fs::write(
            &akua,
            r#"[registries."ghcr.io"]
token = "akua-wins"
"#,
        )
        .unwrap();
        std::fs::write(
            &docker,
            r#"{ "auths": { "ghcr.io": { "auth": "WDpY" } } }"#,
        )
        .unwrap();

        let mut store = CredsStore::empty();
        store.merge_akua_auth(&akua).unwrap();
        store.merge_docker_config(&docker).unwrap();
        assert_eq!(
            for_registry(&store, "ghcr.io"),
            Some(Credentials::Bearer {
                token: "akua-wins".to_string()
            })
        );
    }

    #[test]
    fn missing_file_is_not_an_error() {
        let mut store = CredsStore::empty();
        store
            .merge_akua_auth(std::path::Path::new("/no/such/auth.toml"))
            .unwrap();
        store
            .merge_docker_config(std::path::Path::new("/no/such/config.json"))
            .unwrap();
        assert!(store.entries.is_empty());
    }

    #[test]
    fn malformed_akua_auth_toml_surfaces_parse_error() {
        let tmp = tempfile::tempdir().unwrap();
        let path = tmp.path().join("auth.toml");
        std::fs::write(&path, "not valid toml {{{").unwrap();
        let mut store = CredsStore::empty();
        let err = store.merge_akua_auth(&path).unwrap_err();
        assert!(matches!(err, AuthLoadError::Parse { .. }));
    }

    #[test]
    fn malformed_docker_config_surfaces_parse_error() {
        let tmp = tempfile::tempdir().unwrap();
        let path = tmp.path().join("config.json");
        std::fs::write(&path, "not json {{{").unwrap();
        let mut store = CredsStore::empty();
        let err = store.merge_docker_config(&path).unwrap_err();
        assert!(matches!(err, AuthLoadError::Parse { .. }));
    }
}
