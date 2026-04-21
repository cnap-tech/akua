# Example 07 — package reuse
#
# Composes a base Package (pinned by OCI digest in akua.toml) and adds local
# resources on top. The base's Input schema nests into this Package's Input,
# preserving end-to-end type safety.
#
# Render:
#   akua add
#   akua render --inputs inputs.yaml

import akua.ctx
import akua.pkg                            # package-as-source engine callable
import akua.ui as ui
import packages.platform_base as base      # pinned via akua.toml / akua.lock

schema Input:
    """Public input. Composes the base's schema under .base."""

    # Nest the base's Input. Consumers must fill both the base's required
    # fields and our local additions below.
    base: base.Input

    @ui(order=90, group="Feature flags")
    dashboard_enabled: bool = True
    """Enable the local experimental dashboard overlay."""

    @ui(order=91, group="Feature flags")
    experiment_X: bool = False
    """Roll out experiment X to this tenant."""

    @ui(order=92, group="Routing")
    extra_hostnames: [str] = []
    """Additional hostnames beyond the base's primary hostname."""

    check:
        len(extra_hostnames) < 10, "at most 10 extra hostnames"

input: Input = ctx.input()

# --- Base composition ---
# `pkg.render(Package, inputs)` is the package-as-engine callable, parallel
# to `helm.template(chart, values)`. Returns the base's resources[] list.
_base = pkg.render(base.Package, input.base)

# --- Local additions ---

# Experimental dashboard ConfigMap, gated by the local feature flag.
_dashboard = {
    apiVersion: "v1"
    kind:       "ConfigMap"
    metadata = {
        name:      "${input.base.appName}-dashboard"
        namespace: input.base.appName
        labels.feature: "dashboard"
    }
    data = {
        enabled: str(input.dashboard_enabled)
        experiment_X: str(input.experiment_X)
    }
} if input.dashboard_enabled else Undefined

# Extra Ingresses for every additional hostname, all pointing at the same
# Service the base created.
_extra_ingresses = [
    {
        apiVersion: "networking.k8s.io/v1"
        kind:       "Ingress"
        metadata = {
            name:      "${input.base.appName}-extra-${i}"
            namespace: input.base.appName
        }
        spec = {
            rules = [{
                host = host
                http.paths = [{
                    path: "/"
                    pathType: "Prefix"
                    backend.service = {
                        name: input.base.appName
                        port.number: 80
                    }
                }]
            }]
        }
    }
    for i, host in input.extra_hostnames
]

# --- Aggregate ---
# The base's resources flow through unchanged. Local additions compose on top.
# The order in `resources` is irrelevant — reconcilers derive a graph from
# owner references, not list position.
resources = [*_base, _dashboard, *_extra_ingresses]

# Metadata for `akua inspect` and audit surfaces.
metadata = {
    name:        "checkout-with-dashboard"
    version:     "0.1.0"
    description: "platform-base + experimental dashboard + multi-host routing"
    publisher:   "github.com/acme/checkout"

    # The walk that `akua verify` follows: this Package's attestation
    # references the base's attestation, which references its source charts.
    extends: ["oci://pkg.acme.corp/platform-base@sha256:c7e4b8a1..."]
}
