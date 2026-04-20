import ....schemas.app as s
import ....package as pkg

checkout_staging = s.App {
    name:    "checkout-staging"
    env:     "staging"
    package: "oci://pkg.acme.corp/webapp-postgres:1.0"
    inputs = pkg.Input {
        appName:  "checkout"
        hostname: "checkout.staging.example.com"
        replicas: 2
        team:     "payments"
    }
    target = s.Target {
        reconciler: "argo"
        cluster:    "staging-eu"
    }
}
