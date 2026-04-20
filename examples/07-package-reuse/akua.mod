[package]
name    = "07-package-reuse"
version = "0.1.0"
edition = "akua.dev/v1alpha1"

[dependencies]
# Another akua Package consumed as a source engine.
# Pinned by OCI ref + version; digest is recorded in akua.sum.
#
# In production this would point at a real published Package. For this
# example, imagine platform-base is a webapp-postgres stack authored by a
# platform team and published with signed provenance.
platform-base = { oci = "oci://pkg.acme.corp/platform-base", version = "1.0.0" }
