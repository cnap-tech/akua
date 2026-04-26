//! Fetch OCI-distributed packages into a local content-addressed
//! cache. Two artifact families today:
//!
//! - **Helm charts** — `application/vnd.cncf.helm.chart.content.v1.tar+gzip`,
//!   the canonical helm-v3+ format.
//! - **KCL ecosystem packages** — `application/vnd.oci.image.layer.v1.tar`
//!   (plain tar) carrying `org.kcllang.package.*` (or legacy
//!   `org.kclpkg.package.*`) annotations on the manifest. Published by
//!   `kpm push` and consumed transparently by `import <name>` in a
//!   Package's KCL source. See `examples/10-kcl-ecosystem/` for a
//!   worked example.
//!
//! ## Protocol summary
//!
//! ```text
//! oci://ghcr.io/grafana/helm-charts/grafana:7.3.0
//!        └── registry ──┘└── repository ──┘└tag┘
//! ```
//!
//! Two HTTPS GETs, per the distribution spec:
//!
//! 1. `/v2/<repo>/manifests/<tag>` with
//!    `Accept: application/vnd.oci.image.manifest.v1+json` → JSON
//!    manifest listing layers + (optional) `annotations`.
//! 2. Pick the layer matching the artifact family — Helm by media type,
//!    KCL by media type *and* manifest annotations.
//! 3. `/v2/<repo>/blobs/<digest>` → the tarball bytes (gzipped for
//!    Helm, plain for KCL).
//!
//! ## Scope
//!
//! - Public registries (ghcr.io, registry-1.docker.io, quay.io). All
//!   three use `WWW-Authenticate: Bearer` on the initial manifest
//!   request; we do the anonymous token dance transparently.
//! - User-supplied credentials via `~/.config/akua/auth.toml` and
//!   `~/.docker/config.json`; on miss the fetch surfaces
//!   [`OciFetchError::AuthRequired`].
//! - One-layer artifacts only — every Helm chart and KCL module
//!   we've seen is single-layer.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use serde::Deserialize;
use sha2::{Digest, Sha256};

use crate::hex::hex_encode;
use crate::oci_auth::{self, Credentials, CredsStore};
use crate::oci_transport::{
    build_client, get_with_auth, parse_ref, OciRef, TokenCache, TransportError,
};

/// Media type the helm-v3+ OCI chart format uses for the chart blob.
const HELM_CHART_LAYER_MEDIA_TYPE: &str = "application/vnd.cncf.helm.chart.content.v1.tar+gzip";

/// Media type kpm uses for KCL ecosystem packages (plain tar). The
/// manifest is identified as a *KCL* package via the
/// `KCL_PACKAGE_ANNOTATION_KEYS` annotations — this media type alone
/// is too generic (other artifact types also use it).
const KCL_LAYER_MEDIA_TYPE: &str = "application/vnd.oci.image.layer.v1.tar";

/// Annotation keys kpm stamps onto KCL package manifests. `org.kcllang.*`
/// is the current canonical form (kpm 0.10+); `org.kclpkg.*` is the
/// older form some packages still publish under. Presence of either on
/// the manifest's `annotations` map is the kind discriminator.
const KCL_PACKAGE_ANNOTATION_KEYS: &[&str] =
    &["org.kcllang.package.name", "org.kclpkg.package.name"];

/// OCI image manifest media type. Some registries (ghcr.io with
/// compatibility mode, ECR) still serve `application/vnd.docker.*`
/// instead — we accept both in the request headers.
const OCI_MANIFEST_MEDIA_TYPES: &[&str] = &[
    "application/vnd.oci.image.manifest.v1+json",
    "application/vnd.docker.distribution.manifest.v2+json",
];

/// Which artifact family a fetched OCI manifest is. Carried through to
/// the resolver so the render pipeline knows how to materialize the
/// resulting cache directory (Helm = a chart hand to `helm.template`;
/// KCL = an `ExternalPkg` registered with the KCL evaluator).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PackageKind {
    HelmChart,
    KclModule,
}

/// Decompression flavour for the layer blob. Tied 1:1 to `PackageKind`
/// today but kept distinct because the kind is *what* the artifact is
/// while the format is *how* the bytes are framed — they could diverge
/// (e.g. a future KCL gzip flavour) without changing the kind enum.
#[derive(Debug, Clone, Copy)]
enum ArchiveFormat {
    GzipTar,
    PlainTar,
}

/// Result of a successful fetch. The tarball has been pulled, the
/// digest verified, and the unpacked tree lives at `root_dir`
/// (containing `Chart.yaml` for Helm or `kcl.mod` for KCL).
#[derive(Debug, Clone)]
pub struct FetchedArtifact {
    /// Absolute path to the unpacked package root.
    pub root_dir: PathBuf,
    /// sha256 of the pulled blob, prefixed `sha256:`.
    pub blob_digest: String,
    /// Whether this is a Helm chart or a KCL package — the resolver
    /// needs to know to route it to the right consumer.
    pub kind: PackageKind,
}

/// Backwards-compatible alias. Retained so external callers that named
/// the type `FetchedChart` keep compiling; new code should use
/// [`FetchedArtifact`] directly.
pub type FetchedChart = FetchedArtifact;

#[derive(Debug, thiserror::Error)]
pub enum OciFetchError {
    /// Anything the shared transport surfaces — bad ref, HTTP failure,
    /// non-2xx status, auth rejected. Kept as a nested enum so the
    /// CLI can pattern-match if it wants to distinguish connection-
    /// level flakes from digest mismatches.
    #[error(transparent)]
    Transport(#[from] TransportError),

    /// Registry returned 401 after we exhausted every credential the
    /// caller gave us. Distinct from a malformed config (see
    /// [`OciFetchError::AuthConfig`]) — this is "credentials are
    /// valid but the registry rejected them / we have none".
    #[error("registry `{registry}` rejected auth. Configure credentials in `~/.config/akua/auth.toml` or `docker login` for `~/.docker/config.json`.")]
    AuthRequired { registry: String },

    /// Auth config file exists but couldn't be parsed. Surfaced
    /// separately so `fetch` doesn't silently fall through to an
    /// anonymous pull when a user clearly intended to authenticate.
    #[error("auth config parse error: {detail}")]
    AuthConfig { detail: String },

    /// Cosign signature verification was requested (public key
    /// configured) but failed. Distinct from `AuthRequired` — here
    /// the registry talked to us just fine, but the signer check
    /// didn't pan out.
    #[cfg(feature = "cosign-verify")]
    #[error("cosign verify failed for `{oci_ref}@{manifest_digest}`: {source}")]
    CosignVerify {
        oci_ref: String,
        manifest_digest: String,
        #[source]
        source: crate::cosign::CosignError,
    },

    /// The cosign signature sidecar is missing on the registry.
    /// Treated as a hard failure when a public key is configured:
    /// opting into signing means "unsigned == unsafe."
    #[cfg(feature = "cosign-verify")]
    #[error(
        "cosign signature for `{oci_ref}@{manifest_digest}` is missing or malformed at \
         `{sig_tag}`: {detail}"
    )]
    CosignSignatureMissing {
        oci_ref: String,
        manifest_digest: String,
        sig_tag: String,
        detail: String,
    },

    #[error("manifest for `{oci_ref}:{version}` has no recognised package layer — expected helm `{HELM_CHART_LAYER_MEDIA_TYPE}` or KCL `{KCL_LAYER_MEDIA_TYPE}` with `org.kcllang.package.*` annotations")]
    NoChartLayer { oci_ref: String, version: String },

    #[error("pulled blob digest `{actual}` doesn't match layer-declared `{declared}`")]
    ManifestDigestMismatch { actual: String, declared: String },

    #[error("pulled blob digest `{actual}` doesn't match lockfile-pinned `{expected}` for `{oci_ref}:{version}`")]
    LockDigestMismatch {
        oci_ref: String,
        version: String,
        actual: String,
        expected: String,
    },

    #[error("i/o at `{}`: {source}", path.display())]
    Io {
        path: PathBuf,
        #[source]
        source: std::io::Error,
    },

    #[error("malformed manifest JSON: {0}")]
    ManifestParse(#[from] serde_json::Error),

    #[error("extracting chart tarball: {0}")]
    Extract(String),
}

// --- Manifest shape -------------------------------------------------------

/// Subset of the OCI image manifest we actually use. `annotations` is
/// load-bearing for KCL-package detection — kpm doesn't ship a unique
/// layer media type, only manifest annotations naming the package.
#[derive(Debug, Deserialize)]
struct OciManifest {
    layers: Vec<OciLayer>,
    #[serde(default)]
    annotations: BTreeMap<String, String>,
}

#[derive(Debug, Deserialize)]
struct OciLayer {
    #[serde(rename = "mediaType")]
    media_type: String,
    digest: String,
    #[serde(default)]
    size: u64,
}

/// One package family detected on a manifest. Tied 1:1 to the layer
/// + archive format the registry will hand back.
struct DetectedLayer<'a> {
    kind: PackageKind,
    layer: &'a OciLayer,
    format: ArchiveFormat,
}

/// Identify which package family a manifest carries and where in its
/// `layers` the bytes live. Helm wins ties (a manifest tagged with both
/// kpm annotations *and* a Helm layer would be served as Helm).
fn detect_package(manifest: &OciManifest) -> Option<DetectedLayer<'_>> {
    if let Some(layer) = manifest
        .layers
        .iter()
        .find(|l| l.media_type == HELM_CHART_LAYER_MEDIA_TYPE)
    {
        return Some(DetectedLayer {
            kind: PackageKind::HelmChart,
            layer,
            format: ArchiveFormat::GzipTar,
        });
    }
    let is_kcl = KCL_PACKAGE_ANNOTATION_KEYS
        .iter()
        .any(|k| manifest.annotations.contains_key(*k));
    if is_kcl {
        if let Some(layer) = manifest
            .layers
            .iter()
            .find(|l| l.media_type == KCL_LAYER_MEDIA_TYPE)
        {
            return Some(DetectedLayer {
                kind: PackageKind::KclModule,
                layer,
                format: ArchiveFormat::PlainTar,
            });
        }
    }
    None
}

/// Detect what kind a directory tree is, from the marker file at its
/// root (or in a single wrapper subdir). Returns `None` when neither
/// marker is found — signals cache miss / unrecognised tree.
///
/// Public so chart_resolver can route path/git deps without
/// re-implementing the heuristic. The same logic answers two
/// questions: "is this a recognised package?" (Some/None) and
/// "which kind?" (the variant).
pub fn detect_kind(dir: &Path) -> Option<PackageKind> {
    detect_kind_at(dir, "Chart.yaml", PackageKind::HelmChart)
        .or_else(|| detect_kind_at(dir, "kcl.mod", PackageKind::KclModule))
}

fn detect_kind_at(dir: &Path, marker: &str, kind: PackageKind) -> Option<PackageKind> {
    if dir.join(marker).is_file() {
        return Some(kind);
    }
    let rd = std::fs::read_dir(dir).ok()?;
    rd.flatten()
        .find(|e| {
            let p = e.path();
            p.is_dir() && p.join(marker).is_file()
        })
        .map(|_| kind)
}

// --- Public entry point ---------------------------------------------------

/// Cache-hit lookup only. Returns `Some(FetchedArtifact)` when the
/// content-addressed cache already has the blob, `None` otherwise.
/// Used by the resolver's offline path so air-gapped renders succeed
/// as long as `akua add` populated the cache earlier.
///
/// The kind is detected from cached contents (Chart.yaml vs kcl.mod)
/// — callers don't need to remember it.
pub fn fetch_from_cache(cache_root: &Path, digest: &str) -> Option<FetchedArtifact> {
    let cached = cache_dir_for(cache_root, digest);
    let kind = detect_kind(&cached)?;
    let root_dir = find_package_root(&cached, kind).ok()?;
    Some(FetchedArtifact {
        root_dir,
        blob_digest: digest.to_string(),
        kind,
    })
}

/// Knobs for [`fetch_with_opts`]. All fields named at the call-site
/// so adding a new field isn't a breaking change for existing callers.
/// Field is present unconditionally; the verify *call* is cfg'd so
/// offline-only builds still compile against `FetchOpts`.
#[derive(Debug)]
pub struct FetchOpts<'a> {
    /// Lockfile-pinned digest to verify against. `None` → accept
    /// whatever the registry serves and record the digest for next
    /// time (see `akua.lock`).
    pub expected_digest: Option<&'a str>,

    /// Auth source. Pass `&CredsStore::empty()` for anonymous pulls.
    pub creds: &'a CredsStore,

    /// Cosign public key (PEM-encoded) that must have signed this
    /// chart's manifest. When `Some` *and* the `cosign-verify`
    /// feature is built, the fetcher pulls the `.sig` sidecar and
    /// verifies the signature — a mismatch fails the pull hard with
    /// [`OciFetchError::CosignVerify`]. With the feature off, this
    /// field is silently ignored so binary callers keep working.
    pub cosign_public_key_pem: Option<&'a str>,
}

/// Fetch and extract a Helm OCI chart into `cache_root`. Convenience
/// wrapper around [`fetch_with_opts`] that loads credentials from
/// the standard config files (`~/.config/akua/auth.toml`,
/// `~/.docker/config.json`). Credential parse errors bubble up as
/// an `OciFetchError::AuthRequired` pointer at the config — better
/// than silently falling through to an anonymous pull, which would
/// leak the fact that a user intended to authenticate.
pub fn fetch(
    oci_ref: &str,
    version: &str,
    cache_root: &Path,
    expected_digest: Option<&str>,
) -> Result<FetchedArtifact, OciFetchError> {
    let creds = oci_auth::CredsStore::load().map_err(|source| OciFetchError::AuthConfig {
        detail: source.to_string(),
    })?;
    fetch_with_opts(
        oci_ref,
        version,
        cache_root,
        &FetchOpts {
            expected_digest,
            creds: &creds,
            cosign_public_key_pem: None,
        },
    )
}

/// Full-option variant. All the other `fetch*` entry points funnel
/// through here.
pub fn fetch_with_opts(
    oci_ref: &str,
    version: &str,
    cache_root: &Path,
    opts: &FetchOpts<'_>,
) -> Result<FetchedArtifact, OciFetchError> {
    let expected_digest = opts.expected_digest;
    let creds = opts.creds;
    let parsed = parse_ref(oci_ref)?;

    // Fast path: if we know the digest (lockfile has it) and it's
    // cached, return immediately. Verifies that cached dirs are indexed
    // by exactly the digest we'd otherwise be pulling.
    if let Some(digest) = expected_digest {
        let cached = cache_dir_for(cache_root, digest);
        if let Some(kind) = detect_kind(&cached) {
            let root_dir = find_package_root(&cached, kind)?;
            return Ok(FetchedArtifact {
                root_dir,
                blob_digest: digest.to_string(),
                kind,
            });
        }
    }

    let client = build_client()?;

    let registry_creds = oci_auth::for_registry(creds, &parsed.registry);
    let mut token = TokenCache::default();
    let manifest_url = format!(
        "https://{}/v2/{}/manifests/{}",
        parsed.registry, parsed.repository, version
    );
    let manifest_bytes = get_manifest(
        &client,
        &manifest_url,
        &parsed.registry,
        registry_creds.as_ref(),
        &mut token,
    )?;
    let manifest: OciManifest = serde_json::from_slice(&manifest_bytes)?;

    // Cosign signs the manifest digest, so compute that up front.
    // Registries may return a `Docker-Content-Digest` header with the
    // same value; we compute from bytes for portability across
    // registries (some proxies strip the header).
    let manifest_digest = format!("sha256:{}", hex_encode(&Sha256::digest(&manifest_bytes)));

    #[cfg(feature = "cosign-verify")]
    if let Some(pub_key_pem) = opts.cosign_public_key_pem {
        verify_cosign_signature(
            &client,
            &parsed,
            &manifest_digest,
            pub_key_pem,
            registry_creds.as_ref(),
            &mut token,
        )?;
    }

    let detected = detect_package(&manifest).ok_or_else(|| OciFetchError::NoChartLayer {
        oci_ref: oci_ref.to_string(),
        version: version.to_string(),
    })?;
    let layer_digest = detected.layer.digest.clone();

    let blob_url = format!(
        "https://{}/v2/{}/blobs/{}",
        parsed.registry, parsed.repository, layer_digest
    );
    let blob_bytes = get_blob(
        &client,
        &blob_url,
        &parsed.registry,
        registry_creds.as_ref(),
        &mut token,
    )?;

    // Verify: what the registry handed us must match the layer's
    // self-declared digest, AND (if the lockfile pins one) match that.
    let actual_digest = format!("sha256:{}", hex_encode(&Sha256::digest(&blob_bytes)));
    if actual_digest != layer_digest {
        return Err(OciFetchError::ManifestDigestMismatch {
            actual: actual_digest,
            declared: layer_digest,
        });
    }
    if let Some(expected) = expected_digest {
        if actual_digest != expected {
            return Err(OciFetchError::LockDigestMismatch {
                oci_ref: oci_ref.to_string(),
                version: version.to_string(),
                actual: actual_digest,
                expected: expected.to_string(),
            });
        }
    }

    let target = cache_dir_for(cache_root, &actual_digest);
    extract_blob(&blob_bytes, &target, detected.format)?;

    // Helm charts tar a single top-level `<chart-name>/` dir; KCL
    // packages do the same with `<pkg-name>/`. `find_package_root`
    // descends into either layout and returns the dir holding the
    // marker file (`Chart.yaml` / `kcl.mod`).
    let root_dir = find_package_root(&target, detected.kind)?;
    let kind = detected.kind;

    Ok(FetchedArtifact {
        root_dir,
        blob_digest: actual_digest,
        kind,
    })
}

// --- Cache layout ---------------------------------------------------------

/// Deterministic cache path: `<root>/sha256/<hex>/`. Content-addressed,
/// so two manifests pointing at the same blob (e.g. a chart tagged as
/// both `7.3.0` and `latest`) share the directory.
fn cache_dir_for(root: &Path, digest: &str) -> PathBuf {
    let hex = digest.strip_prefix("sha256:").unwrap_or(digest);
    root.join("sha256").join(hex)
}

/// Return the directory inside `cache_dir` that holds the
/// kind-appropriate marker (`Chart.yaml` for Helm; `kcl.mod` for KCL).
/// Handles both tar layouts: marker at root *or* in a single
/// `<name>/` wrapper subdirectory. Fails with [`OciFetchError::Extract`]
/// when the cached tree is missing the expected marker entirely —
/// usually means the cache slot was clobbered by a different artifact.
fn find_package_root(cache_dir: &Path, kind: PackageKind) -> Result<PathBuf, OciFetchError> {
    let marker = match kind {
        PackageKind::HelmChart => "Chart.yaml",
        PackageKind::KclModule => "kcl.mod",
    };
    if cache_dir.join(marker).is_file() {
        return Ok(cache_dir.to_path_buf());
    }
    let rd = std::fs::read_dir(cache_dir).map_err(|source| OciFetchError::Io {
        path: cache_dir.to_path_buf(),
        source,
    })?;
    for entry in rd.flatten() {
        let p = entry.path();
        if p.is_dir() && p.join(marker).is_file() {
            return Ok(p);
        }
    }
    Err(OciFetchError::Extract(format!(
        "no {marker} found under {}",
        cache_dir.display()
    )))
}

// --- Cosign signature pull + verify ---------------------------------------

/// Cosign's signature manifest media types. The layer has its own
/// distinct media type so we can pick it out reliably even on
/// registries that hand back slightly different manifest shapes.
#[cfg(feature = "cosign-verify")]
const COSIGN_SIG_LAYER_MEDIA_TYPE: &str = "application/vnd.dev.cosign.simplesigning.v1+json";

/// The annotation key cosign stashes the base64-encoded ECDSA
/// signature under on the signature manifest's layer entry.
#[cfg(feature = "cosign-verify")]
const COSIGN_SIGNATURE_ANNOTATION: &str = "dev.cosignproject.cosign/signature";

#[cfg(feature = "cosign-verify")]
#[derive(Debug, Deserialize)]
struct CosignSigManifest {
    layers: Vec<CosignSigLayer>,
}

#[cfg(feature = "cosign-verify")]
#[derive(Debug, Deserialize)]
struct CosignSigLayer {
    #[serde(rename = "mediaType")]
    media_type: String,
    digest: String,
    #[serde(default)]
    annotations: std::collections::HashMap<String, String>,
}

/// Pull `sha256-<hex>.sig` sidecar manifest + its payload blob and
/// verify via [`crate::cosign::verify_keyed`]. All error paths funnel
/// through `CosignSignatureMissing` or `CosignVerify` so the CLI can
/// produce a distinct exit code.
#[cfg(feature = "cosign-verify")]
fn verify_cosign_signature(
    client: &reqwest::blocking::Client,
    parsed: &OciRef,
    manifest_digest: &str,
    public_key_pem: &str,
    creds: Option<&Credentials>,
    token: &mut TokenCache,
) -> Result<(), OciFetchError> {
    // Cosign's sig tag swaps `sha256:` for `sha256-` so it's a valid
    // tag per the distribution spec (colons aren't allowed in tags).
    let hex = manifest_digest
        .strip_prefix("sha256:")
        .unwrap_or(manifest_digest);
    let sig_tag = format!("sha256-{hex}.sig");

    let oci_ref = format!("oci://{}/{}", parsed.registry, parsed.repository);

    let sig_manifest_url = format!(
        "https://{}/v2/{}/manifests/{}",
        parsed.registry, parsed.repository, sig_tag
    );
    let sig_manifest_bytes =
        get_manifest(client, &sig_manifest_url, &parsed.registry, creds, token).map_err(
            |source| OciFetchError::CosignSignatureMissing {
                oci_ref: oci_ref.clone(),
                manifest_digest: manifest_digest.to_string(),
                sig_tag: sig_tag.clone(),
                detail: source.to_string(),
            },
        )?;

    let sig_manifest: CosignSigManifest =
        serde_json::from_slice(&sig_manifest_bytes).map_err(|e| {
            OciFetchError::CosignSignatureMissing {
                oci_ref: oci_ref.clone(),
                manifest_digest: manifest_digest.to_string(),
                sig_tag: sig_tag.clone(),
                detail: format!("manifest parse: {e}"),
            }
        })?;

    let sig_layer = sig_manifest
        .layers
        .into_iter()
        .find(|l| l.media_type == COSIGN_SIG_LAYER_MEDIA_TYPE)
        .ok_or_else(|| OciFetchError::CosignSignatureMissing {
            oci_ref: oci_ref.clone(),
            manifest_digest: manifest_digest.to_string(),
            sig_tag: sig_tag.clone(),
            detail: format!("no layer with media type {COSIGN_SIG_LAYER_MEDIA_TYPE}"),
        })?;

    let signature_b64 = sig_layer
        .annotations
        .get(COSIGN_SIGNATURE_ANNOTATION)
        .ok_or_else(|| OciFetchError::CosignSignatureMissing {
            oci_ref: oci_ref.clone(),
            manifest_digest: manifest_digest.to_string(),
            sig_tag: sig_tag.clone(),
            detail: format!("layer missing `{COSIGN_SIGNATURE_ANNOTATION}` annotation"),
        })?
        .clone();

    let payload_url = format!(
        "https://{}/v2/{}/blobs/{}",
        parsed.registry, parsed.repository, sig_layer.digest
    );
    let payload_bytes =
        get_blob(client, &payload_url, &parsed.registry, creds, token).map_err(|source| {
            OciFetchError::CosignSignatureMissing {
                oci_ref: oci_ref.clone(),
                manifest_digest: manifest_digest.to_string(),
                sig_tag: sig_tag.clone(),
                detail: format!("payload blob: {source}"),
            }
        })?;

    crate::cosign::verify_keyed(
        public_key_pem,
        &payload_bytes,
        &signature_b64,
        manifest_digest,
    )
    .map_err(|source| OciFetchError::CosignVerify {
        oci_ref,
        manifest_digest: manifest_digest.to_string(),
        source,
    })
}

// --- HTTP helpers ---------------------------------------------------------

fn get_manifest(
    client: &reqwest::blocking::Client,
    url: &str,
    registry: &str,
    creds: Option<&Credentials>,
    token: &mut TokenCache,
) -> Result<Vec<u8>, OciFetchError> {
    Ok(get_with_auth(client, url, registry, creds, token, |req| {
        let mut req = req;
        for media in OCI_MANIFEST_MEDIA_TYPES {
            req = req.header("Accept", *media);
        }
        req
    })?)
}

fn get_blob(
    client: &reqwest::blocking::Client,
    url: &str,
    registry: &str,
    creds: Option<&Credentials>,
    token: &mut TokenCache,
) -> Result<Vec<u8>, OciFetchError> {
    Ok(get_with_auth(client, url, registry, creds, token, |req| {
        req
    })?)
}

// --- Tarball extraction ---------------------------------------------------

/// Unpack an OCI layer tarball into `dest`. `format` selects whether
/// to decompress (Helm = `.tar.gz`) or unpack the bytes directly (KCL
/// kpm = plain `.tar`). Tar's unpacker guards against absolute/escape
/// paths internally; we still write to a staging dir and atomically
/// rename so concurrent akua processes don't see partial state.
fn extract_blob(bytes: &[u8], dest: &Path, format: ArchiveFormat) -> Result<(), OciFetchError> {
    let parent = dest.parent().ok_or_else(|| {
        OciFetchError::Extract(format!("cache path has no parent: {}", dest.display()))
    })?;
    std::fs::create_dir_all(parent).map_err(|source| OciFetchError::Io {
        path: parent.to_path_buf(),
        source,
    })?;
    if dest.exists() {
        // Another pull already landed this digest — pre-empted race.
        return Ok(());
    }

    let staging = tempfile::Builder::new()
        .prefix("akua-oci-stage-")
        .tempdir_in(parent)
        .map_err(|source| OciFetchError::Io {
            path: parent.to_path_buf(),
            source,
        })?;

    match format {
        ArchiveFormat::GzipTar => {
            let gz = flate2::read::GzDecoder::new(bytes);
            let mut archive = tar::Archive::new(gz);
            archive.set_overwrite(true);
            archive.set_preserve_permissions(false);
            archive
                .unpack(staging.path())
                .map_err(|e| OciFetchError::Extract(e.to_string()))?;
        }
        ArchiveFormat::PlainTar => {
            let mut archive = tar::Archive::new(bytes);
            archive.set_overwrite(true);
            archive.set_preserve_permissions(false);
            archive
                .unpack(staging.path())
                .map_err(|e| OciFetchError::Extract(e.to_string()))?;
        }
    }

    // Atomic move into the final content-addressed slot. Fall back to
    // a recursive copy on rename error — common causes are cross-
    // device moves (TMPDIR != cache root) or a racing pull that won
    // the slot between our existence check and our rename.
    match std::fs::rename(staging.path(), dest) {
        Ok(()) => {
            // `staging` has been moved out; dropping the TempDir would
            // try to delete the new location. Defuse it.
            let _ = staging.keep();
            Ok(())
        }
        Err(_) if dest.exists() => Ok(()), // racing pull won
        Err(_) => {
            crate::walk::copy_tree(staging.path(), dest).map_err(|source| OciFetchError::Io {
                path: dest.to_path_buf(),
                source,
            })
        }
    }
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    // Ref-parsing tests live next to the parser in `oci_transport`.

    #[test]
    fn cache_dir_is_content_addressed() {
        let root = Path::new("/cache");
        let a = cache_dir_for(root, "sha256:abc123");
        let b = cache_dir_for(root, "abc123");
        assert_eq!(a, b);
        assert_eq!(a, Path::new("/cache/sha256/abc123"));
    }

    /// Round-trip: build a fake chart tarball, call extract_blob, verify
    /// the unpacked tree has Chart.yaml + templates/. Mirrors what
    /// `fetch()` does minus the HTTP dance.
    #[test]
    fn extract_blob_unpacks_helm_shaped_tarball() {
        let mut buf = Vec::new();
        {
            let gz = flate2::write::GzEncoder::new(&mut buf, flate2::Compression::default());
            let mut tar_b = tar::Builder::new(gz);

            let mut hdr = tar::Header::new_gnu();
            hdr.set_size(60);
            hdr.set_mode(0o644);
            hdr.set_cksum();
            tar_b
                .append_data(
                    &mut hdr.clone(),
                    "nginx/Chart.yaml",
                    &b"apiVersion: v2\nname: nginx\nversion: 0.1.0\nappVersion: \"1\"\n"[..],
                )
                .unwrap();

            let body = b"apiVersion: v1\nkind: ConfigMap\nmetadata:\n  name: x\n";
            hdr.set_size(body.len() as u64);
            hdr.set_cksum();
            tar_b
                .append_data(&mut hdr, "nginx/templates/cm.yaml", &body[..])
                .unwrap();

            tar_b.finish().unwrap();
        }

        let tmp = tempfile::tempdir().unwrap();
        let dest = tmp.path().join("sha256").join("deadbeef");
        extract_blob(&buf, &dest, ArchiveFormat::GzipTar).unwrap();
        assert!(
            dest.join("nginx/Chart.yaml").is_file(),
            "Chart.yaml at {:?}",
            dest
        );
        let root = find_package_root(&dest, PackageKind::HelmChart).unwrap();
        assert!(root.ends_with("nginx"));
    }

    #[test]
    fn extract_blob_idempotent_on_existing_cache_entry() {
        // Second pull of the same digest must no-op rather than fail.
        let mut buf = Vec::new();
        {
            let gz = flate2::write::GzEncoder::new(&mut buf, flate2::Compression::default());
            let mut tar_b = tar::Builder::new(gz);
            let mut hdr = tar::Header::new_gnu();
            let body = b"apiVersion: v2\nname: x\nversion: 0.1.0\n";
            hdr.set_size(body.len() as u64);
            hdr.set_mode(0o644);
            hdr.set_cksum();
            tar_b
                .append_data(&mut hdr, "x/Chart.yaml", &body[..])
                .unwrap();
            tar_b.finish().unwrap();
        }

        let tmp = tempfile::tempdir().unwrap();
        let dest = tmp.path().join("sha256").join("deadbeef");
        extract_blob(&buf, &dest, ArchiveFormat::GzipTar).unwrap();
        extract_blob(&buf, &dest, ArchiveFormat::GzipTar).expect("second call must be a no-op");
    }

    /// Round-trip a kpm-style plain tar carrying a `kcl.mod`.
    /// `find_package_root` should descend into the wrapping `<pkg>/`
    /// directory and `detect_kind` should label the cache entry
    /// as `KclModule`.
    #[test]
    fn extract_blob_unpacks_kcl_plain_tar() {
        let mut buf = Vec::new();
        {
            let mut tar_b = tar::Builder::new(&mut buf);
            let mut hdr = tar::Header::new_gnu();

            let mod_body = b"[package]\nname = \"k8s\"\nversion = \"1.31.2\"\n";
            hdr.set_size(mod_body.len() as u64);
            hdr.set_mode(0o644);
            hdr.set_cksum();
            tar_b
                .append_data(&mut hdr.clone(), "k8s/kcl.mod", &mod_body[..])
                .unwrap();

            let kcl_body = b"schema Deployment:\n    name: str\n";
            hdr.set_size(kcl_body.len() as u64);
            hdr.set_cksum();
            tar_b
                .append_data(&mut hdr, "k8s/api/apps/v1/deployment.k", &kcl_body[..])
                .unwrap();

            tar_b.finish().unwrap();
        }

        let tmp = tempfile::tempdir().unwrap();
        let dest = tmp.path().join("sha256").join("c0ffee");
        extract_blob(&buf, &dest, ArchiveFormat::PlainTar).unwrap();
        assert!(dest.join("k8s/kcl.mod").is_file());
        let root = find_package_root(&dest, PackageKind::KclModule).unwrap();
        assert!(root.ends_with("k8s"));
        assert_eq!(detect_kind(&dest), Some(PackageKind::KclModule));
    }

    #[test]
    fn detect_package_prefers_helm_then_kcl_via_annotation() {
        // Manifest with both Helm + KCL layers + KCL annotation —
        // Helm wins. (We don't expect this in the wild, but it's the
        // documented tie-break.)
        let manifest = OciManifest {
            layers: vec![
                OciLayer {
                    media_type: HELM_CHART_LAYER_MEDIA_TYPE.into(),
                    digest: "sha256:helm".into(),
                    size: 1,
                },
                OciLayer {
                    media_type: KCL_LAYER_MEDIA_TYPE.into(),
                    digest: "sha256:kcl".into(),
                    size: 1,
                },
            ],
            annotations: BTreeMap::from([(
                "org.kcllang.package.name".to_string(),
                "k8s".to_string(),
            )]),
        };
        let detected = detect_package(&manifest).unwrap();
        assert_eq!(detected.kind, PackageKind::HelmChart);
        assert_eq!(detected.layer.digest, "sha256:helm");
        assert!(matches!(detected.format, ArchiveFormat::GzipTar));

        // KCL-only: plain-tar layer + annotation.
        let manifest = OciManifest {
            layers: vec![OciLayer {
                media_type: KCL_LAYER_MEDIA_TYPE.into(),
                digest: "sha256:kcl".into(),
                size: 1,
            }],
            annotations: BTreeMap::from([(
                "org.kclpkg.package.name".to_string(),
                "legacy".to_string(),
            )]),
        };
        let detected = detect_package(&manifest).unwrap();
        assert_eq!(detected.kind, PackageKind::KclModule);
        assert!(matches!(detected.format, ArchiveFormat::PlainTar));

        // Plain tar without KCL annotation → not a package we know.
        let manifest = OciManifest {
            layers: vec![OciLayer {
                media_type: KCL_LAYER_MEDIA_TYPE.into(),
                digest: "sha256:other".into(),
                size: 1,
            }],
            annotations: BTreeMap::new(),
        };
        assert!(detect_package(&manifest).is_none());
    }
}
