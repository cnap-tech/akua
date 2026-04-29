# akua/helm — typed wrapper over the `kcl_plugin.helm` surface.
#
# **For typed Helm-chart deps, prefer the alias-method form:**
#
#   import charts.webapp as webapp
#
#   resources = webapp.template(webapp.TemplateOpts {
#       values = webapp.Values { replicaCount = 3 }
#   })
#
# The `charts.<name>` stub the resolver synthesizes per dep already
# routes through this engine — calling `helm.template` directly is the
# escape hatch for cases the stub doesn't cover (e.g. dynamic dispatch
# on a runtime variable, or a chart not declared in `akua.toml`).
#
# Direct usage (advanced / fallback):
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
# dicts helm produced — splat with `*` into `resources`. The schema
# itself is passed through to the plugin, which reads fields by name.
template = lambda opts: Template -> [{str:}] {
    _helm.template(opts)
}
