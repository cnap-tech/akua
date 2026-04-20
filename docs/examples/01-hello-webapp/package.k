# Example 01 — hello-webapp
#
# Simplest possible akua Package. One schema input (hostname), one source
# (the upstream nginx chart), raw-manifest output.
#
# Render:
#   akua render --inputs inputs.yaml --out ./rendered

import akua.helm
import charts.nginx as nginx

schema Input:
    """Public input contract. Four fields; three have sensible defaults."""
    hostname: str
    replicas: int = 2
    tls:      bool = True
    name:     str = "hello"

input: Input

# Consume the upstream nginx chart unchanged.
# The `inputs` block maps our public schema into the chart's native values.
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

# One output. Default format is raw Kubernetes manifests committed to git.
resources = _nginx

outputs = [
    {
        kind:   "RawManifests"
        target: "./"
    }
]
