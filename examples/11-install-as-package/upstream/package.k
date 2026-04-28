import akua.ctx

# Upstream Package: a typical webapp install — Deployment + Service +
# PodDisruptionBudget. Authored as a normal Package.k; nothing about
# this file is install-aware.

schema Input:
    """Inputs for the upstream webapp."""
    appName: str
    replicas: int = 2
    image: str = "nginx:1.27"

input: Input = ctx.input()

resources = [
    {
        apiVersion = "apps/v1"
        kind = "Deployment"
        metadata.name = input.appName
        spec.replicas = input.replicas
        spec.selector.matchLabels = {"app" = input.appName}
        spec.template.metadata.labels = {"app" = input.appName}
        spec.template.spec.containers = [{
            name = input.appName
            image = input.image
            ports = [{containerPort = 80}]
        }]
    }
    {
        apiVersion = "v1"
        kind = "Service"
        metadata.name = input.appName
        spec.selector = {"app" = input.appName}
        spec.ports = [{port = 80, targetPort = 80}]
    }
    {
        apiVersion = "policy/v1"
        kind = "PodDisruptionBudget"
        metadata.name = input.appName
        spec.minAvailable = 1
        spec.selector.matchLabels = {"app" = input.appName}
    }
]
