//! Keyed [cosign](https://docs.sigstore.dev/cosign/overview/)
//! signature verification for OCI-fetched charts.
//!
//! Phase 6 slice A — the minimum-viable supply-chain gate that works
//! offline (no rekor, no fulcio). Users wire a public key into
//! `akua.toml`:
//!
//! ```toml
//! [signing]
//! cosign_public_key = "./keys/cosign.pub"
//! ```
//!
//! When set, every OCI pull also fetches the chart's cosign `.sig`
//! sidecar and verifies the payload signature with the configured
//! key before the blob is handed to `helm-engine-wasm`. Verification
//! failures fail the render hard — the alternative (log + continue)
//! makes signatures UX theater.
//!
//! Keyless verification (Fulcio cert chain + Rekor transparency log)
//! is deferred to Phase 6 slice B. Most fleets ship keyed first —
//! it runs offline, has no Sigstore dependency, and pairs cleanly
//! with the content-addressed cache Phase 2b B already set up.

use p256::ecdsa::{signature::Verifier, Signature, VerifyingKey};
use p256::pkcs8::DecodePublicKey;
use serde::{Deserialize, Serialize};

/// The canonical cosign simple-signing payload shape. We parse the
/// subset needed to correlate the signature with the blob we're
/// about to render — specifically the `critical.image.docker-manifest-digest`
/// field, which must match the digest the registry just handed us.
#[derive(Debug, Deserialize)]
struct SimpleSigningPayload {
    critical: Critical,
}

#[derive(Debug, Deserialize)]
struct Critical {
    image: ImageRef,
}

#[derive(Debug, Deserialize)]
struct ImageRef {
    #[serde(rename = "docker-manifest-digest")]
    docker_manifest_digest: String,
}

#[derive(Debug, thiserror::Error)]
pub enum CosignError {
    #[error("public key is not a valid PEM-encoded P-256 ECDSA key: {0}")]
    BadPublicKey(String),

    #[error("private key is not a valid PEM-encoded P-256 ECDSA key: {0}")]
    BadPrivateKey(String),

    #[error("signature is not valid DER/raw base64: {0}")]
    BadSignature(String),

    #[error("payload isn't the cosign simple-signing shape: {0}")]
    BadPayload(#[from] serde_json::Error),

    #[error("signature does not verify against the supplied public key: {0}")]
    VerifyFailed(String),

    #[error(
        "payload claims digest `{claimed}`, but we fetched `{actual}` — signature is for a \
         different artifact"
    )]
    DigestMismatch { claimed: String, actual: String },
}

// --- Signing primitive (Phase 7) -------------------------------------------

/// Cosign simple-signing payload produced for an artifact. Output of
/// [`build_simple_signing_payload`]; bytes are what [`sign_keyed`]
/// signs and what consumers verify.
#[derive(Debug, Serialize)]
struct SimpleSigningPayloadOut<'a> {
    critical: CriticalOut<'a>,
    optional: Option<()>,
}

#[derive(Debug, Serialize)]
struct CriticalOut<'a> {
    identity: IdentityOut<'a>,
    image: ImageOut<'a>,
    #[serde(rename = "type")]
    ty: &'a str,
}

#[derive(Debug, Serialize)]
struct IdentityOut<'a> {
    #[serde(rename = "docker-reference")]
    docker_reference: &'a str,
}

#[derive(Debug, Serialize)]
struct ImageOut<'a> {
    #[serde(rename = "docker-manifest-digest")]
    docker_manifest_digest: &'a str,
}

/// Build the simple-signing payload bytes for `(docker_reference,
/// manifest_digest)`. Output is the JSON cosign would emit for
/// `cosign sign oci://...@sha256:...`.
///
/// Returned bytes are what the caller signs + pushes as the `.sig`
/// sidecar's layer. Separated out from [`sign_keyed`] so callers can
/// inspect / modify the payload before signing if a future feature
/// demands it (SLSA predicate embedding, annotations).
pub fn build_simple_signing_payload(docker_reference: &str, manifest_digest: &str) -> Vec<u8> {
    let payload = SimpleSigningPayloadOut {
        critical: CriticalOut {
            identity: IdentityOut { docker_reference },
            image: ImageOut {
                docker_manifest_digest: manifest_digest,
            },
            ty: "cosign container image signature",
        },
        optional: None,
    };
    // Canonical JSON isn't required by cosign but using serde_json's
    // default serialization gives byte-deterministic output for a
    // stable input — two `build + sign` calls yield identical bytes.
    serde_json::to_vec(&payload).expect("simple-signing payload serializes")
}

/// Sign `payload_bytes` with a P-256 ECDSA private key, returning a
/// base64-encoded DER signature. Matches what cosign emits + what
/// [`verify_keyed`] accepts.
///
/// Accepts unencrypted PKCS#8 PEM directly; for PKCS#8-encrypted
/// PEM (`-----BEGIN ENCRYPTED PRIVATE KEY-----`) pass `Some(pass)`
/// as the third argument. An unencrypted key tolerates a
/// passphrase (ignored). Encrypted without a passphrase → typed
/// `BadPrivateKey` error naming the env var the CLI expects.
///
/// Stored passphrases should come from env vars
/// (`$AKUA_COSIGN_PASSPHRASE`) or an OS keychain. Passing via argv
/// leaks the secret to `ps` + shell history, so there's no
/// `--passphrase` flag on any verb.
pub fn sign_keyed(
    private_key_pem: &str,
    payload_bytes: &[u8],
    passphrase: Option<&str>,
) -> Result<String, CosignError> {
    use base64::Engine as _;
    use p256::ecdsa::{signature::Signer, SigningKey};
    use p256::pkcs8::DecodePrivateKey;

    let signing = if is_encrypted_pem(private_key_pem) {
        let pass = passphrase.ok_or_else(|| {
            CosignError::BadPrivateKey(
                "private key is encrypted; set `$AKUA_COSIGN_PASSPHRASE` or an equivalent and re-run".to_string(),
            )
        })?;
        SigningKey::from_pkcs8_encrypted_pem(private_key_pem, pass.as_bytes())
            .map_err(|e| CosignError::BadPrivateKey(format!("decrypt: {e}")))?
    } else {
        SigningKey::from_pkcs8_pem(private_key_pem)
            .map_err(|e: p256::pkcs8::Error| CosignError::BadPrivateKey(e.to_string()))?
    };
    let signature: Signature = signing.sign(payload_bytes);
    Ok(base64::engine::general_purpose::STANDARD.encode(signature.to_der().as_bytes()))
}

/// Heuristic match on the PEM header. `ENCRYPTED PRIVATE KEY` is
/// the PKCS#8 encrypted block type per RFC 5958.
fn is_encrypted_pem(pem: &str) -> bool {
    pem.contains("-----BEGIN ENCRYPTED PRIVATE KEY-----")
}

// --- DSSE envelope --------------------------------------------------------

/// DSSE (Dead Simple Signature Envelope) v1. Wraps a signed payload
/// for in-toto attestations the way cosign's `.att` sidecars carry
/// SLSA provenance. Output JSON:
///
/// ```json
/// {
///   "payloadType": "application/vnd.in-toto+json",
///   "payload":     "<base64 of raw statement bytes>",
///   "signatures":  [{"sig": "<base64 of ECDSA(payloadType, payload)>"}]
/// }
/// ```
///
/// The signature covers the **PAE** (Pre-Auth Encoding), not the
/// raw payload:
///
/// ```text
/// DSSEv1 <type_len> <type> <payload_len> <payload>
/// ```
///
/// PAE prevents signature substitution between envelope types —
/// a signature over a JSON blob can't be re-wrapped as a signature
/// over the same blob with a different `payloadType`.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DsseEnvelope {
    #[serde(rename = "payloadType")]
    pub payload_type: String,
    pub payload: String,
    pub signatures: Vec<DsseSignature>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DsseSignature {
    pub sig: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub keyid: Option<String>,
}

/// Media type DSSE envelopes carry inside cosign's `.att` sidecar.
pub const DSSE_ENVELOPE_MEDIA_TYPE: &str = "application/vnd.dsse.envelope.v1+json";

/// Build + sign a DSSE envelope. Returns the envelope JSON bytes —
/// ready for `oci_pusher::push_attestation`. Same passphrase
/// handling as [`sign_keyed`].
pub fn sign_dsse(
    private_key_pem: &str,
    payload_type: &str,
    payload_bytes: &[u8],
    passphrase: Option<&str>,
) -> Result<Vec<u8>, CosignError> {
    let pae = dsse_pae(payload_type, payload_bytes);
    let sig_b64 = sign_keyed(private_key_pem, &pae, passphrase)?;

    use base64::Engine as _;
    let envelope = DsseEnvelope {
        payload_type: payload_type.to_string(),
        payload: base64::engine::general_purpose::STANDARD.encode(payload_bytes),
        signatures: vec![DsseSignature {
            sig: sig_b64,
            keyid: None,
        }],
    };
    Ok(serde_json::to_vec(&envelope)?)
}

/// Verify a DSSE envelope: parse, reconstruct PAE, ECDSA-verify any
/// of the contained signatures against the public key. Returns the
/// raw decoded payload on success — callers then parse it as the
/// `payloadType` indicates.
pub fn verify_dsse(public_key_pem: &str, envelope_bytes: &[u8]) -> Result<Vec<u8>, CosignError> {
    use base64::Engine as _;
    use p256::ecdsa::signature::Verifier;

    let envelope: DsseEnvelope =
        serde_json::from_slice(envelope_bytes).map_err(CosignError::BadPayload)?;
    let payload = base64::engine::general_purpose::STANDARD
        .decode(envelope.payload.as_bytes())
        .map_err(|e| CosignError::BadSignature(e.to_string()))?;
    let pae = dsse_pae(&envelope.payload_type, &payload);

    let key = VerifyingKey::from_public_key_pem(public_key_pem)
        .map_err(|e: p256::pkcs8::spki::Error| CosignError::BadPublicKey(e.to_string()))?;

    // Any one signature verifying is enough — matches DSSE spec's
    // "at least one signature must verify" semantic for consumer-
    // side verification with a known key.
    for sig_entry in &envelope.signatures {
        let sig_bytes =
            match base64::engine::general_purpose::STANDARD.decode(sig_entry.sig.as_bytes()) {
                Ok(b) => b,
                Err(_) => continue,
            };
        let Ok(signature) =
            Signature::from_der(&sig_bytes).or_else(|_| Signature::from_slice(&sig_bytes))
        else {
            continue;
        };
        if key.verify(&pae, &signature).is_ok() {
            return Ok(payload);
        }
    }
    Err(CosignError::VerifyFailed(
        "no DSSE signature verified against the supplied public key".to_string(),
    ))
}

/// DSSE Pre-Auth Encoding. Binding the payload type + length into
/// what gets signed is what prevents cross-type signature reuse.
fn dsse_pae(payload_type: &str, payload: &[u8]) -> Vec<u8> {
    let mut out = Vec::with_capacity(payload.len() + 64);
    out.extend_from_slice(b"DSSEv1 ");
    out.extend_from_slice(payload_type.len().to_string().as_bytes());
    out.push(b' ');
    out.extend_from_slice(payload_type.as_bytes());
    out.push(b' ');
    out.extend_from_slice(payload.len().to_string().as_bytes());
    out.push(b' ');
    out.extend_from_slice(payload);
    out
}

/// Verify a cosign signature end-to-end:
///
/// 1. Parse `public_key_pem` as a P-256 ECDSA verifying key.
/// 2. Decode `signature_b64` as the raw / DER signature cosign emits.
/// 3. ECDSA-verify the signature covers `payload_bytes` under that key.
/// 4. Parse `payload_bytes` as a cosign simple-signing JSON blob.
/// 5. Check the payload's claimed manifest digest matches `expected_digest`.
///
/// Returns `Ok(())` on full success. Any step failing surfaces the
/// reason so the CLI layer can tell "bad key" from "wrong signer"
/// from "signature for the wrong chart."
pub fn verify_keyed(
    public_key_pem: &str,
    payload_bytes: &[u8],
    signature_b64: &str,
    expected_digest: &str,
) -> Result<(), CosignError> {
    let key = VerifyingKey::from_public_key_pem(public_key_pem)
        .map_err(|e: p256::pkcs8::spki::Error| CosignError::BadPublicKey(e.to_string()))?;

    use base64::Engine as _;
    let sig_bytes = base64::engine::general_purpose::STANDARD
        .decode(signature_b64.trim())
        .map_err(|e| CosignError::BadSignature(e.to_string()))?;

    // cosign emits DER-encoded signatures by default (RFC 3279). Fall
    // back to the raw fixed-length form some older tools used when
    // DER parse fails.
    let signature = Signature::from_der(&sig_bytes)
        .or_else(|_| Signature::from_slice(&sig_bytes))
        .map_err(|e| CosignError::BadSignature(e.to_string()))?;

    // Parse + digest-correlate before the ECDSA verify. Garbage
    // payloads fail fast without spending CPU on the crypto step,
    // and a malformed payload is operationally closer to "sig for a
    // different artifact" than "wrong signer" — the ordering lines
    // up with how users debug failures.
    let payload: SimpleSigningPayload = serde_json::from_slice(payload_bytes)?;
    if payload.critical.image.docker_manifest_digest != expected_digest {
        return Err(CosignError::DigestMismatch {
            claimed: payload.critical.image.docker_manifest_digest,
            actual: expected_digest.to_string(),
        });
    }

    key.verify(payload_bytes, &signature)
        .map_err(|e: p256::ecdsa::Error| CosignError::VerifyFailed(e.to_string()))?;
    Ok(())
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use p256::ecdsa::{signature::Signer, SigningKey};
    use p256::pkcs8::EncodePublicKey;

    /// Produce a (pem public key, signature base64) pair for a given
    /// payload. Used by every keyed-verify test so we don't depend on
    /// a pre-generated fixture file.
    fn sign_fixture(payload: &[u8]) -> (String, String) {
        use base64::Engine as _;
        let mut rng = rand::rngs::OsRng;
        let signing = SigningKey::random(&mut rng);
        let verifying = signing.verifying_key();
        let pem = verifying.to_public_key_pem(Default::default()).unwrap();
        let signature: Signature = signing.sign(payload);
        let b64 = base64::engine::general_purpose::STANDARD.encode(signature.to_der().as_bytes());
        (pem, b64)
    }

    fn payload_for(digest: &str) -> Vec<u8> {
        format!(
            r#"{{"critical":{{"identity":{{"docker-reference":"example.com/x"}},"image":{{"docker-manifest-digest":"{digest}"}},"type":"cosign container image signature"}},"optional":null}}"#
        )
        .into_bytes()
    }

    #[test]
    fn verifies_valid_signature_and_matching_digest() {
        let digest = "sha256:deadbeef";
        let payload = payload_for(digest);
        let (pem, sig) = sign_fixture(&payload);
        verify_keyed(&pem, &payload, &sig, digest).expect("verifies");
    }

    #[test]
    fn rejects_wrong_public_key() {
        let digest = "sha256:deadbeef";
        let payload = payload_for(digest);
        let (_good_pem, sig) = sign_fixture(&payload);
        // Use a different key — still a valid P-256 pem, just not
        // the one that signed the payload.
        let (other_pem, _other_sig) = sign_fixture(b"anything else");
        let err = verify_keyed(&other_pem, &payload, &sig, digest).unwrap_err();
        assert!(matches!(err, CosignError::VerifyFailed(_)), "got {err:?}");
    }

    #[test]
    fn rejects_tampered_payload() {
        let digest = "sha256:deadbeef";
        let payload = payload_for(digest);
        let (pem, sig) = sign_fixture(&payload);
        let mut tampered = payload.clone();
        // Flip one byte. Depending on where in the JSON the flip
        // lands, we either end up with a JSON parse failure (fast-path
        // before the ECDSA verify) or an ECDSA mismatch — both are
        // acceptable outcomes for "someone tampered with the payload."
        tampered[10] ^= 0xff;
        let err = verify_keyed(&pem, &tampered, &sig, digest).unwrap_err();
        assert!(
            matches!(
                err,
                CosignError::VerifyFailed(_) | CosignError::BadPayload(_)
            ),
            "got {err:?}"
        );
    }

    #[test]
    fn rejects_wrong_digest() {
        let declared = "sha256:deadbeef";
        let payload = payload_for(declared);
        let (pem, sig) = sign_fixture(&payload);
        // Payload says `declared`; we fetched something else.
        let err = verify_keyed(&pem, &payload, &sig, "sha256:00000000").unwrap_err();
        assert!(
            matches!(err, CosignError::DigestMismatch { .. }),
            "got {err:?}"
        );
    }

    #[test]
    fn rejects_malformed_public_key() {
        let err = verify_keyed("not a pem", b"{}", "AAAA", "sha256:00").unwrap_err();
        assert!(matches!(err, CosignError::BadPublicKey(_)), "got {err:?}");
    }

    #[test]
    fn rejects_malformed_signature_base64() {
        let (pem, _) = sign_fixture(b"x");
        let err = verify_keyed(&pem, b"{}", "!!!not base64!!!", "sha256:00").unwrap_err();
        assert!(matches!(err, CosignError::BadSignature(_)), "got {err:?}");
    }

    #[test]
    fn rejects_malformed_payload() {
        let payload = b"{not json".to_vec();
        let (pem, sig) = sign_fixture(&payload);
        let err = verify_keyed(&pem, &payload, &sig, "sha256:00").unwrap_err();
        assert!(matches!(err, CosignError::BadPayload(_)), "got {err:?}");
    }

    // --- Signing + round-trip ----------------------------------------------

    /// Produce a (public PEM, private PEM) pair for round-trip tests.
    fn keypair_fixture() -> (String, String) {
        use p256::ecdsa::SigningKey;
        use p256::pkcs8::{EncodePrivateKey, EncodePublicKey, LineEnding};
        let mut rng = rand::rngs::OsRng;
        let signing = SigningKey::random(&mut rng);
        let verifying = signing.verifying_key();
        let priv_pem = signing.to_pkcs8_pem(LineEnding::LF).unwrap().to_string();
        let pub_pem = verifying.to_public_key_pem(LineEnding::LF).unwrap();
        (pub_pem, priv_pem)
    }

    #[test]
    fn build_simple_signing_payload_has_canonical_fields() {
        let payload = build_simple_signing_payload(
            "ghcr.io/acme/app",
            "sha256:deadbeefdeadbeefdeadbeefdeadbeefdeadbeefdeadbeefdeadbeefdeadbeef",
        );
        let s = std::str::from_utf8(&payload).unwrap();
        assert!(s.contains("\"docker-reference\":\"ghcr.io/acme/app\""));
        assert!(s.contains("\"docker-manifest-digest\":\"sha256:deadbeef"));
        assert!(s.contains("\"type\":\"cosign container image signature\""));
    }

    #[test]
    fn sign_then_verify_roundtrips() {
        let (pub_pem, priv_pem) = keypair_fixture();
        let digest = "sha256:f00";
        let payload = build_simple_signing_payload("ghcr.io/acme/app", digest);
        let signature = sign_keyed(&priv_pem, &payload, None).expect("sign");
        verify_keyed(&pub_pem, &payload, &signature, digest).expect("verify");
    }

    #[test]
    fn sign_rejects_malformed_private_key() {
        let err = sign_keyed("not a pem", b"anything", None).unwrap_err();
        assert!(matches!(err, CosignError::BadPrivateKey(_)), "got {err:?}");
    }

    /// Generate an encrypted PKCS#8 PEM via `to_pkcs8_encrypted_pem`.
    /// Produces the same `-----BEGIN ENCRYPTED PRIVATE KEY-----`
    /// shape cosign-cli / openssl emit.
    fn encrypted_key_fixture(pass: &str) -> (String, String) {
        use p256::ecdsa::SigningKey;
        use p256::pkcs8::{EncodePrivateKey, EncodePublicKey, LineEnding};

        let mut rng = rand::rngs::OsRng;
        let signing = SigningKey::random(&mut rng);
        let verifying = signing.verifying_key();

        let pub_pem = verifying.to_public_key_pem(LineEnding::LF).unwrap();
        let priv_pem = signing
            .to_pkcs8_encrypted_pem(&mut rng, pass.as_bytes(), LineEnding::LF)
            .expect("encrypt pkcs8");
        (pub_pem, priv_pem.to_string())
    }

    #[test]
    fn sign_with_encrypted_key_and_correct_passphrase_verifies() {
        let pass = "correct horse battery staple";
        let (pub_pem, priv_pem) = encrypted_key_fixture(pass);
        let payload = payload_for("sha256:deadbeef");
        let sig = sign_keyed(&priv_pem, &payload, Some(pass)).expect("sign");
        verify_keyed(&pub_pem, &payload, &sig, "sha256:deadbeef").expect("verify");
    }

    #[test]
    fn sign_encrypted_key_without_passphrase_errors() {
        let (_pub_pem, priv_pem) = encrypted_key_fixture("anything");
        let err = sign_keyed(&priv_pem, b"x", None).unwrap_err();
        match err {
            CosignError::BadPrivateKey(msg) => {
                assert!(
                    msg.contains("AKUA_COSIGN_PASSPHRASE"),
                    "expected env-var hint in error: {msg}"
                );
            }
            other => panic!("expected BadPrivateKey, got {other:?}"),
        }
    }

    #[test]
    fn sign_encrypted_key_with_wrong_passphrase_errors() {
        let (_pub_pem, priv_pem) = encrypted_key_fixture("right");
        let err = sign_keyed(&priv_pem, b"x", Some("wrong")).unwrap_err();
        match err {
            CosignError::BadPrivateKey(msg) => {
                assert!(msg.contains("decrypt"), "got {msg}");
            }
            other => panic!("expected BadPrivateKey, got {other:?}"),
        }
    }

    #[test]
    fn passphrase_is_ignored_for_unencrypted_key() {
        let (pub_pem, priv_pem) = keypair_fixture();
        let payload = payload_for("sha256:abc");
        // Supplying a passphrase on a plain key must not fail —
        // falls back to the unencrypted path.
        let sig = sign_keyed(&priv_pem, &payload, Some("ignored")).expect("sign");
        verify_keyed(&pub_pem, &payload, &sig, "sha256:abc").expect("verify");
    }

    #[test]
    fn is_encrypted_pem_detects_header() {
        assert!(is_encrypted_pem(
            "-----BEGIN ENCRYPTED PRIVATE KEY-----\nfoo\n-----END ENCRYPTED PRIVATE KEY-----\n"
        ));
        assert!(!is_encrypted_pem(
            "-----BEGIN PRIVATE KEY-----\nbar\n-----END PRIVATE KEY-----\n"
        ));
        assert!(!is_encrypted_pem("not a pem"));
    }

    // --- DSSE envelope round-trip -----------------------------------------

    #[test]
    fn dsse_pae_matches_spec_layout() {
        // DSSEv1 <type_len> <type> <payload_len> <payload>
        let pae = dsse_pae("application/x-demo", b"hello");
        let expected = b"DSSEv1 18 application/x-demo 5 hello";
        assert_eq!(pae, expected);
    }

    #[test]
    fn dsse_sign_then_verify_roundtrips() {
        let (pub_pem, priv_pem) = keypair_fixture();
        let payload = br#"{"_type":"https://in-toto.io/Statement/v1"}"#;
        let envelope = sign_dsse(&priv_pem, "application/vnd.in-toto+json", payload, None).unwrap();
        let recovered = verify_dsse(&pub_pem, &envelope).expect("verify");
        assert_eq!(recovered, payload);
    }

    #[test]
    fn dsse_verify_rejects_tampered_payload() {
        let (pub_pem, priv_pem) = keypair_fixture();
        let payload = b"original";
        let envelope_bytes = sign_dsse(&priv_pem, "application/test", payload, None).unwrap();

        // Tamper with the base64 payload inside the envelope — the
        // signature was over the *original* bytes.
        let mut envelope: DsseEnvelope = serde_json::from_slice(&envelope_bytes).unwrap();
        use base64::Engine as _;
        envelope.payload = base64::engine::general_purpose::STANDARD.encode(b"tampered");
        let tampered = serde_json::to_vec(&envelope).unwrap();

        let err = verify_dsse(&pub_pem, &tampered).unwrap_err();
        assert!(matches!(err, CosignError::VerifyFailed(_)), "got {err:?}");
    }

    #[test]
    fn dsse_verify_rejects_cross_type_substitution() {
        // Sign under one payloadType, swap the type on the envelope.
        // PAE prevents the signature from verifying against the
        // swapped type.
        let (pub_pem, priv_pem) = keypair_fixture();
        let payload = b"cross-type";
        let envelope_bytes = sign_dsse(&priv_pem, "type/a", payload, None).unwrap();
        let mut envelope: DsseEnvelope = serde_json::from_slice(&envelope_bytes).unwrap();
        envelope.payload_type = "type/b".to_string();
        let tampered = serde_json::to_vec(&envelope).unwrap();
        let err = verify_dsse(&pub_pem, &tampered).unwrap_err();
        assert!(matches!(err, CosignError::VerifyFailed(_)), "got {err:?}");
    }
}
