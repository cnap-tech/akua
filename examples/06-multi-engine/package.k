# Example 06 — multi-engine
#
# One Package, four sources, one render output (raw YAML):
#   - Helm        (upstream webapp chart)
#   - Kustomize   (./overlays base → ServiceMonitor + ConfigMap patch)
#   - kro RGD     (platform glue as a transformation producing an RGD manifest)
#   - Inline KCL  (NetworkPolicy authored directly in KCL)
#
# Render:
#   akua add
#   akua render --inputs inputs.yaml --out ./deploy

import akua.ctx
import akua.kustomize
import akua.kro
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

input: Input = ctx.input()

# --- 1. Helm source ---
# `charts.<name>` synthesizes a `template` lambda + typed `Values`
# schema per dep — alias-method form, mirrors `pkgs.<name>.render` for
# Akua packages. No `import akua.helm` needed at the call site.
_app = webapp.template(webapp.TemplateOpts {
    values = webapp.Values {
        replicaCount        = input.replicas
        ingress.hostname    = input.hostname
        ingress.tls.enabled = True
        fullnameOverride    = input.appName
    }
})

# --- 2. Kustomize source ---
# Engine-direct: kustomize bases are within-Package directories, not
# typed deps, so they don't get a `bases.<name>` alias. Same form
# every Package uses for kro RGDs and OCI manifest fetches.
_monitor = kustomize.build(kustomize.Build {
    path = "./overlays"
})

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

# --- 4. kro RGD transformation ---
# Genuine runtime late-binding: DNS records and cert references whose status
# fields are observed post-apply. `kro.rgd` produces the ResourceGraphDefinition
# as a regular K8s manifest that kro's controller picks up from the rendered
# YAML — not a separate emit kind.
_glue = kro.rgd(kro.Rgd {
    definition = platform.RGD
    instance   = {
        metadata.name      = input.appName
        spec.domain        = input.hostname
        spec.ingress_class = "traefik"
    }
})

# --- aggregate ---
resources = [*_app, *_monitor, _netpol, _glue]
