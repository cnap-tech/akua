import akua.ctx
import charts.nginx as nginx

schema Input:
    """A minimal Package surface chosen to keep golden diffs readable."""

    appName:  str
    hostname: str
    replicas: int = 2
    tls:      bool = True

    check:
        replicas >= 1, "replicas must be at least 1"

input: Input = ctx.input()

_app = nginx.template(nginx.TemplateOpts {
    values = nginx.Values {
        replicaCount      = input.replicas
        ingress.enabled   = True
        ingress.hostname  = input.hostname
        ingress.tls       = input.tls
        fullnameOverride  = input.appName
    }
})

resources = _app
