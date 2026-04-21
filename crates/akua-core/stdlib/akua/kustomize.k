# akua/kustomize — typed wrapper over the `kcl_plugin.kustomize` surface.
#
# Users:
#
#   import akua.kustomize
#
#   resources = kustomize.build(kustomize.Build {
#       path = "./overlays/prod"
#   })
#
# Options-schema call style matches `akua.helm` — every argument is named
# + docstring'd at the call site. The underlying `kcl_plugin.kustomize.build`
# plugin is what actually runs (dispatched by akua-core's plugin bridge).

import kcl_plugin.kustomize as _kustomize

schema Build:
    """Options for a single `kustomize build` invocation."""

    path: str
    """Kustomization root — a directory containing
    `kustomization.yaml` / `.yml` / `Kustomization`. Relative paths
    resolve against the calling Package's directory."""

# Render a kustomize overlay directory into a list of Kubernetes resources.
# Splat with `*` into `resources`. The schema itself is passed through
# to the plugin, which reads fields by name.
build = lambda opts: Build -> [{str:}] {
    _kustomize.build(opts)
}
