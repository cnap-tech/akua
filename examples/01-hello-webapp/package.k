# Example 01 — hello-webapp
#
# Simplest possible akua Package. One source (the upstream nginx chart),
# four public inputs with UI hints, raw-manifest output.
#
# Render:
#   akua add                              # resolve deps → writes akua.lock
#   akua render --inputs inputs.yaml      # render to ./rendered by default

import akua.ctx
import akua.helm
import akua.ui as ui            # @ui decorator lives here
import charts.nginx as nginx    # resolved from akua.toml

schema Input:
    """Public inputs for the hello-webapp Package."""

    @ui(order=10, group="Identity", placeholder="hello.example.com")
    hostname: str
    """Public hostname the site answers on. Must be a valid RFC 1123 DNS name."""

    @ui(order=20, group="Identity")
    name: str = "hello"
    """Internal name used as a resource-name prefix. Lowercase, hyphen-separated."""

    @ui(order=30, group="Capacity", widget="slider", min=1, max=20)
    replicas: int = 2
    """Replica count for the Deployment. Minimum 1."""

    @ui(order=40, group="Security")
    tls: bool = True
    """Enable TLS on the Ingress. On by default; flip off for local testing."""

    check:
        replicas >= 1, "replicas must be at least 1"

input: Input = ctx.input()

# Consume the upstream nginx chart unchanged. Public schema ↔ chart values
# wiring happens inline; no chart fork needed.
_nginx = helm.template(nginx.Chart {
    values = nginx.Values {
        replicaCount = input.replicas
        ingress = {
            enabled:  True
            hostname: input.hostname
            tls:      input.tls
        }
        fullnameOverride = input.name
    }
})

resources = _nginx
