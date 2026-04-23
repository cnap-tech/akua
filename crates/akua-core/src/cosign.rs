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
use serde::Deserialize;

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

    key.verify(payload_bytes, &signature)
        .map_err(|e: p256::ecdsa::Error| CosignError::VerifyFailed(e.to_string()))?;

    let payload: SimpleSigningPayload = serde_json::from_slice(payload_bytes)?;
    if payload.critical.image.docker_manifest_digest != expected_digest {
        return Err(CosignError::DigestMismatch {
            claimed: payload.critical.image.docker_manifest_digest,
            actual: expected_digest.to_string(),
        });
    }
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
        // Flip one byte in the middle.
        tampered[10] ^= 0xff;
        let err = verify_keyed(&pem, &tampered, &sig, digest).unwrap_err();
        assert!(
            matches!(err, CosignError::VerifyFailed(_) | CosignError::BadPayload(_)),
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
        let err = verify_keyed(
            "not a pem",
            b"{}",
            "AAAA",
            "sha256:00",
        )
        .unwrap_err();
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
}
