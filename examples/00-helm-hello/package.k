# Minimal Package that exercises the `helm.template` plugin callable.
#
# Renders the adjacent `./chart/` (a tiny ConfigMap template) through
# akua's shell-out helm engine. Requires the `engine-helm-shell`
# crate feature + a `helm` binary on PATH.
#
# Render:
#
#   akua render --out ./rendered
#
# Inputs flow through KCL's `option()` mechanism; `inputs.example.yaml`
# is auto-discovered when --inputs is omitted.

import akua.helm

schema Input:
    """Public inputs for 00-helm-hello."""

    greeting: str = "hello from akua"
    """Message embedded into the rendered ConfigMap."""

    replicas: int = 1
    """Replica count recorded in the ConfigMap."""

input: Input = option("input") or Input {}

# helm.template returns a list of Kubernetes resources — a single
# ConfigMap in this case. `*_manifests` splats the list into
# `resources`.
_manifests = helm.template(helm.Template {
    chart     = "./chart"
    values    = {
        greeting = input.greeting
        replicas = input.replicas
    }
    release   = "hello"
    namespace = "default"
})

resources = _manifests

outputs = [
    { kind: "RawManifests", target: "./" }
]
