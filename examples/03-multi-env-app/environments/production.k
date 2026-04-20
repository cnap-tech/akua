import ..schemas.environment as s

production = s.Environment {
    name:    "production"
    policy:  "tier/production"
    cluster: "prod-eu"
    region:  "eu-central-1"
    budget = s.Budget {
        monthly: "$1500"
        alertAt: [0.75, 0.9, 1.0]
    }
    approvers: ["@team/platform", "@team/payments-lead"]
    soak = s.Soak {
        after_deploy: "15m"
        fail_fast_on: [
            "error_rate p99 > 0.01 for 3m"
            "p99_latency > 2s for 5m"
        ]
    }
}
