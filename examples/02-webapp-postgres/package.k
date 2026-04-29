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

import akua.ctx
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

input: Input = ctx.input()

# Each `charts.<name>` import lands a synthesized stub with a
# `template` lambda + typed `Values` schema. The engine
# (`akua.helm`) doesn't appear at the call site — the stub dispatches.
_pg = cnpg.template(cnpg.TemplateOpts {
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
_app = webapp.template(webapp.TemplateOpts {
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
})

# Cross-cutting label applied to the webapp resources — same effect
# as a chart postRenderer, expressed as a list comprehension after
# the alias-method call returns the typed list. Mirrors example 11's
# overlay pattern.
_labeled = [r | {metadata.labels |= {"team": input.team}} for r in _app]

resources = [*_pg, *_labeled]
