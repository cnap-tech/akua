# akua.mod — human-edited manifest of declared deps.
# Machine-maintained digest + signature ledger lives in akua.sum.

[package]
name    = "04-policy-tier"
version = "0.1.0"
edition = "akua.dev/v1alpha1"

[dependencies]
# akua's reference production tier (signed OCI artifact).
tier-prod = { oci = "oci://policies.akua.dev/tier/production", version = "1.2.0" }

# A Kyverno bundle. akua's Kyverno→Rego converter runs at `akua add` time;
# the compiled Rego lands under .akua/policies/vendor/ and imports into
# our local policies as `data.akua.policies.kyverno.security`.
kyv-sec   = { oci = "oci://policies.akua.dev/kyverno/security", version = "2.0.0" }
