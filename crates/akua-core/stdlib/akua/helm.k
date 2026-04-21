# akua/helm — typed wrapper over the `kcl_plugin.helm` surface.
#
# Users:
#
#   import akua.helm
#
#   resources = helm.template(helm.Template {
#       chart     = "./chart"
#       values    = { replicas = 3 }
#       release   = "myapp"
#       namespace = "prod"
#   })
#
# Options-schema call style so every argument is named + docstring'd at
# the call site. The underlying `kcl_plugin.helm.template` plugin is
# what actually runs (dispatched by akua-core's plugin bridge).

import kcl_plugin.helm as _helm

schema Template:
    """Options for a single `helm template` invocation."""

    chart: str
    """Filesystem path to a chart directory or `.tgz`. Relative paths
    resolve against the calling Package's directory."""

    values: {str:} = {}
    """Arbitrary values tree, serialized to YAML for helm."""

    release: str = "release"
    """Helm release name. Lowercase alphanumerics + `-`, max 53 chars."""

    namespace: str = "default"
    """Kubernetes namespace the release targets. DNS-1123 label rules."""

# Render a Helm chart into a list of Kubernetes resources. Returns the
# dicts helm produced — splat with `*` into `resources`.
template = lambda opts: Template -> [{str:}] {
    _helm.template(opts.chart, opts.values, opts.release, opts.namespace)
}
