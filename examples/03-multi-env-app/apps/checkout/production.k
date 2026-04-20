import ....schemas.app as s
import ....package as pkg

checkout_production = s.App {
    name:    "checkout-production"
    env:     "production"
    package: "oci://pkg.acme.corp/webapp-postgres:1.0"
    inputs = pkg.Input {
        appName:  "checkout"
        hostname: "checkout.example.com"
        replicas: 5
        team:     "payments"
        database: pkg.DatabaseInput { user: "checkout_app" }
    }
    target = s.Target {
        reconciler: "argo"
        cluster:    "prod-eu"
    }
}
