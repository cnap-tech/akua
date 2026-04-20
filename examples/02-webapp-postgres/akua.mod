# akua.mod — declared deps. Human-edited.
# Machine-maintained digest + signature ledger lives in akua.sum.

[package]
name    = "02-webapp-postgres"
version = "0.1.0"
edition = "akua.dev/v1alpha1"

[dependencies]
# CloudNativePG cluster chart (the Postgres operator's packaged cluster CR).
cnpg = { oci = "oci://ghcr.io/cloudnative-pg/charts/cluster", version = "0.20.0" }

# Generic webapp chart — stands in for whatever Helm chart the app team publishes.
webapp = { oci = "oci://ghcr.io/acme/charts/webapp", version = "2.1.0" }
