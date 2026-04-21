# Minimal Package that exercises the `kustomize.build` plugin callable.
#
# Wraps the adjacent `./overlay/` (which layers a namePrefix + commonLabels
# onto `./base/`) through akua's shell-out kustomize engine. Requires the
# `engine-kustomize-shell` crate feature + a `kustomize` binary on PATH.
#
# Render:
#
#   akua render --package examples/09-kustomize-hello/package.k --out ./rendered
#
# No inputs — the kustomization tree is fully declarative; there's nothing
# for the outer Package to parameterize. `schema Input` is still declared
# for shape consistency with the other examples.

import akua.kustomize

schema Input:
    """No-op inputs placeholder for shape consistency."""

input: Input = option("input") or Input {}

# kustomize.build returns a list of Kubernetes resources. `*_manifests`
# splats it into `resources`.
_manifests = kustomize.build("./overlay")

resources = _manifests

outputs = [
    { kind: "RawManifests", target: "./" }
]
