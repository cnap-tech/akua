import akua.krm.environment as env

production = env.Environment {
    metadata.name = "production"
    spec = env.Spec {
        policy  = "tier/production"
        cluster = "prod-eu"
        region  = "eu-central-1"
        budget  = env.Budget {
            monthly = "$1500"
            alertAt = [0.75, 0.9, 1.0]
        }
        approvers = ["@team/platform", "@team/payments-lead"]
        soak = env.Soak {
            after_deploy = "15m"
            fail_fast_on = [
                "error_rate p99 > 0.01 for 3m"
                "p99_latency > 2s for 5m"
            ]
        }
    }
}
