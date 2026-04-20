# App KRM (cluster-facing kind; KCL canonical, YAML view via `akua export`).
# References the Package by OCI ref; carries per-environment typed inputs.
#
# Render a single environment:
#   akua render --env=production --out=./deploy/production
# Render all at once:
#   akua render --all-envs --out=./deploy
#
# Produce the YAML view (reconciler-consumable):
#   akua export apps/checkout/app.k --format=yaml > apps/checkout/app.yaml

import akua.krm.app as app
import ...package as pkg

checkout = app.App {
    metadata = {
        name: "checkout"
        namespace: "payments"
        labels: {
            "app.kubernetes.io/name": "checkout"
            "team": "payments"
        }
    }

    spec = app.Spec {
        package = "oci://pkg.acme.corp/webapp-postgres:1.0"

        # Per-environment inputs. Each value satisfies pkg.Input.
        # No invented YAML-file sidecar syntax — just typed KCL values.
        inputs = {
            dev        = pkg.Input {
                appName  = "checkout"
                hostname = "checkout.dev.example.com"
                replicas = 1
                team     = "payments"
            }
            staging    = pkg.Input {
                appName  = "checkout"
                hostname = "checkout.staging.example.com"
                replicas = 2
                team     = "payments"
            }
            production = pkg.Input {
                appName  = "checkout"
                hostname = "checkout.example.com"
                replicas = 5
                team     = "payments"
                database = pkg.DatabaseInput { user = "checkout_app" }
            }
        }

        # Reconciliation target per environment.
        targets = {
            dev        = app.Target { reconciler: "argo", cluster: "dev-eu" }
            staging    = app.Target { reconciler: "argo", cluster: "staging-eu" }
            production = app.Target { reconciler: "argo", cluster: "prod-eu" }
        }
    }
}
