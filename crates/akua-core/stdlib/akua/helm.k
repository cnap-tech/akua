# akua/helm — typed wrappers over the `kcl_plugin.helm` surface.
#
# Users:
#
#   import akua.helm
#
#   resources = helm.template("./chart", { replicas = 3 })
#
# The underlying `kcl_plugin.helm.template` plugin is what actually
# runs (dispatched by akua-core's plugin bridge). This file exists so
# authoring code isn't forced to reach into KCL's raw `kcl_plugin.*`
# namespace — users see `akua.helm.template`, implementers can swap
# the backend without breaking imports.

import kcl_plugin.helm as _helm

# Render a Helm chart into a list of Kubernetes resources.
#
# `chart` is a filesystem path (directory or `.tgz`) resolved against
# the calling Package's directory. `values` is a values tree serialized
# to YAML for helm. `release` + `namespace` default to `"release"` /
# `"default"`.
#
# Returns a list of resource dicts — splat with `*` into `resources`.
template = lambda chart: str, values: {str:} = {}, release: str = "release", namespace: str = "default" -> [{str:}] {
    _helm.template(chart, values, release, namespace)
}
