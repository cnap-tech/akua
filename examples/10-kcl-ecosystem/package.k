# Package that consumes the upstream `kcl-lang/k8s` schema bundle to
# author a typed Kubernetes Deployment.
#
# `import k8s.api.apps.v1` resolves through the external_pkg the
# resolver registers from akua.toml's `k8s` dep — same machinery as
# Helm-chart deps, but the unpacked tar IS already a KCL package
# (no synthesized wrapper). The Deployment schema's own internal
# `import apimachinery.pkg.apis.meta.v1` resolves within the same
# package because kpm bundles apimachinery alongside api/ inside k8s.
#
# Render:
#
#   akua render --package examples/10-kcl-ecosystem/package.k --out ./rendered
#
# Bigger pitch: any package on ghcr.io/kcl-lang/* (or any kpm-published
# OCI ref) plugs in the same way. akua handles the digest pin, signature
# verify-on-pull, and typed resolution in the wasmtime sandbox; the KCL
# ecosystem provides the schemas.

import k8s.api.apps.v1 as apps
import k8s.api.core.v1 as corev1
import akua.ctx

schema Input:
    """Public inputs for 10-kcl-ecosystem."""

    name: str = "hello"
    """Name of the Deployment + selector label."""

    image: str = "nginx:1.27"
    """Container image."""

    replicas: int = 2
    """Replica count."""

input: Input = ctx.input()

resources = [apps.Deployment {
    metadata.name = input.name
    metadata.labels = {"app.kubernetes.io/name" = input.name}
    spec = {
        replicas = input.replicas
        selector.matchLabels = {"app.kubernetes.io/name" = input.name}
        template = {
            metadata.labels = {"app.kubernetes.io/name" = input.name}
            spec.containers = [corev1.Container {
                name = input.name
                image = input.image
            }]
        }
    }
}]
