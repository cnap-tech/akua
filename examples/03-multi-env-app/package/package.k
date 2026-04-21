# The reusable Package. In production this would live in its own repo and be
# published as an OCI artifact (e.g. `oci://pkg.acme.corp/webapp-postgres:1.0`);
# shown inline here so the example is self-contained.
#
# Same shape as 02-webapp-postgres but trimmed; see that example for the
# full cross-source-wiring walkthrough.

import akua.ctx
import akua.helm
import akua.ui as ui
import charts.cnpg    as cnpg
import charts.webapp  as webapp

schema Input:
    """Public inputs for the webapp-postgres Package."""

    @ui(order=10, group="Identity")
    appName: str
    """Application name. Used as namespace + resource-name prefix."""

    @ui(order=20, group="Identity", placeholder="checkout.example.com")
    hostname: str
    """Public hostname."""

    @ui(order=30, group="Capacity", widget="slider", min=1, max=20)
    replicas: int = 3
    """Webapp replica count."""

    @ui(order=40, group="Ownership")
    team: str = "platform"
    """Owning team. Labeled onto every rendered resource."""

    database: DatabaseInput = DatabaseInput {}

    check:
        replicas >= 1, "replicas must be at least 1"

schema DatabaseInput:
    @ui(order=1)
    user: str = "app"

input: Input = ctx.input()

_pg = helm.template(cnpg.Chart {
    values = cnpg.Values {
        cluster = {
            name:      "${input.appName}-pg"
            instances: 3
        }
        bootstrap.initdb = {
            database: input.appName
            owner:    input.database.user
        }
    }
})

_app = helm.template(webapp.Chart {
    values = webapp.Values {
        replicaCount         = input.replicas
        ingress.hostname     = input.hostname
        ingress.tls.enabled  = True
        fullnameOverride     = input.appName
        env = [{
            name = "DATABASE_URL"
            valueFrom.secretKeyRef = {
                name = "${input.appName}-pg-app"   # CNPG operator convention
                key  = "uri"
            }
        }]
    }
    postRenderer = lambda r: dict -> dict {
        r.metadata.labels |= {"team": input.team}
        r
    }
})

resources = [*_pg, *_app]
