# Example 02 — webapp-postgres
#
# Cross-source wiring. A webapp consumes a Postgres connection URL from a
# Secret created by the CloudNativePG operator — entirely by convention,
# no runtime late-binding required, all deterministic at CI time.
#
# Demonstrates:
#   - two Helm sources in one package
#   - per-source input mapping (`cluster.name`, `ingress.hostname`, etc.)
#   - cross-source references via predictable operator naming
#   - optional postRenderer for cross-cutting mutation (team label)
#
# Render:
#   akua render --inputs inputs.yaml --out ./rendered

import akua.helm
import charts.cnpg    as cnpg
import charts.webapp  as webapp

schema Input:
    """Customer-facing contract. Six fields; four have defaults."""
    appName:  str
    hostname: str
    replicas: int = 3
    team:     str = "platform"
    database: {
        user: str = "app"
    }

input: Input = option("input") or Input {}

# Postgres cluster. CloudNativePG's operator creates a Secret named
# `${cluster.name}-app` with key `uri` — predictable by convention.
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
        monitoring.enabled: True
    }
})

# Application. References the conventional secret name above.
# If CNPG's operator changes its naming convention, this is the one
# place we'd update — still at CI time, still deterministic.
_app = helm.template(webapp.Chart {
    values = webapp.Values {
        replicaCount         = input.replicas
        ingress.hostname     = input.hostname
        ingress.tls.enabled  = True
        fullnameOverride     = input.appName
        env = [
            {
                name = "DATABASE_URL"
                valueFrom.secretKeyRef = {
                    name = "${input.appName}-pg-app"   # CNPG convention
                    key  = "uri"
                }
            }
        ]
    }
    # Cross-cutting: label every rendered resource with the team.
    # Applied per-resource post-render.
    postRenderer = lambda r: dict -> dict {
        r.metadata.labels |= {"team": input.team}
        r
    }
})

resources = [*_pg, *_app]
