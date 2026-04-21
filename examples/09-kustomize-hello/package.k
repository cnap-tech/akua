# Minimal Package that exercises the `kustomize.build` plugin callable.
#
# Wraps the adjacent `./overlay/` (which layers a namePrefix + labels
# onto `./base/`) through akua's shell-out kustomize engine. Requires
# the `engine-kustomize-shell` crate feature + a `kustomize` binary on
# PATH.
#
# Render:
#
#   akua render --package examples/09-kustomize-hello/package.k --out ./rendered
#
# No inputs — the kustomization tree is fully declarative.

import akua.kustomize

_manifests = kustomize.build(kustomize.Build {
    path = "./overlay"
})

resources = _manifests
# `outputs` omitted — defaults to `[{kind: "RawManifests", target: "./"}]`.
