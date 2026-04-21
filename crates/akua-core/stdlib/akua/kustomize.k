# akua/kustomize — typed wrapper over the `kcl_plugin.kustomize` surface.
#
# Users:
#
#   import akua.kustomize
#
#   resources = kustomize.build("./overlays/prod")
#
# The underlying `kcl_plugin.kustomize.build` plugin is what runs
# (dispatched by akua-core's plugin bridge). This file lets authoring
# code write `akua.kustomize.build` instead of reaching into KCL's
# raw `kcl_plugin.*` namespace.

import kcl_plugin.kustomize as _kustomize

# Render a kustomize overlay directory into a list of Kubernetes resources.
#
# `path` is the kustomization root — a directory containing
# `kustomization.yaml` / `.yml` / `Kustomization`. Resolved against
# the calling Package's directory.
#
# Returns a list of resource dicts — splat with `*` into `resources`.
build = lambda path: str -> [{str:}] {
    _kustomize.build(path)
}
