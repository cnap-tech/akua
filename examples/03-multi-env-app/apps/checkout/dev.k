import ....schemas.app as s
import ....package as pkg

checkout_dev = s.App {
    name:    "checkout-dev"
    env:     "dev"
    package: "oci://pkg.acme.corp/webapp-postgres:1.0"
    inputs = pkg.Input {
        appName:  "checkout"
        hostname: "checkout.dev.example.com"
        replicas: 1
        team:     "payments"
    }
    target = s.Target {
        reconciler: "argo"
        cluster:    "dev-eu"
    }
}
