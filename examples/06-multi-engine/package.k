# Example 06 — multi-engine
#
# One Package, four sources:
#   - Helm        (upstream webapp chart)
#   - Kustomize   (./overlays base → ServiceMonitor + ConfigMap patch)
#   - kro RGD     (platform glue, runtime late-binding)
#   - Inline KCL  (NetworkPolicy authored directly in KCL)
#
# Two named outputs:
#   - `static`  → raw manifests for ArgoCD/Flux
#   - `runtime` → RGD for kro to reconcile at deploy time
#
# Render:
#   akua add
#   akua render --inputs inputs.yaml

import akua.helm
import akua.kustomize
import akua.rgd
import charts.webapp   as webapp
import rgds.platform   as platform

schema Input:
    """Public inputs for the multi-engine example."""

    appName:  str
    hostname: str
    replicas: int = 3
    team:     str = "platform"

    check:
        replicas >= 1, "replicas must be at least 1"

input: Input = option("input") or Input {}

# --- 1. Helm source ---
_app = helm.template(
    webapp.Chart {
        values = webapp.Values {
            replicaCount        = input.replicas
            ingress.hostname    = input.hostname
            ingress.tls.enabled = True
            fullnameOverride    = input.appName
        }
    },
    output = "static",
)

# --- 2. Kustomize source ---
# Builds ./overlays — a small overlay adding a ServiceMonitor and patching
# a ConfigMap. Kustomize runs offline; output is deterministic.
_monitor = kustomize.build(
    "./overlays",
    output = "static",
)

# --- 3. Inline KCL-native resource ---
# NetworkPolicy denying all ingress except from the app's own namespace.
# No engine required — just a typed dict that ends up in resources[].
_netpol = {
    apiVersion: "networking.k8s.io/v1"
    kind:       "NetworkPolicy"
    metadata = {
        name:      "${input.appName}-default-deny"
        namespace: input.appName
        labels.team: input.team
    }
    spec = {
        podSelector = {}
        policyTypes = ["Ingress"]
        ingress = [{
            from = [{
                namespaceSelector.matchLabels = {
                    "kubernetes.io/metadata.name": input.appName
                }
            }]
        }]
    }
}

# --- 4. kro RGD source ---
# Genuine runtime late-binding: DNS records and cert references whose status
# fields are observed post-apply. Routed to the `runtime` output so kro's
# controller reconciles it.
_glue = rgd.instantiate(
    platform.RGD,
    {
        metadata.name      = input.appName
        spec.domain        = input.hostname
        spec.ingress_class = "traefik"
    },
    output = "runtime",
)

# --- aggregate ---
resources = [*_app, *_monitor, _netpol, *_glue]

outputs = [
    { name: "static",  kind: "RawManifests",            target: "./deploy/static" }
    { name: "runtime", kind: "ResourceGraphDefinition", target: "./deploy/rgd"    }
]
