import ..schemas.environment as s

staging = s.Environment {
    name:    "staging"
    policy:  "tier/startup"
    cluster: "staging-eu"
    region:  "eu-central-1"
    budget = s.Budget {
        monthly: "$200"
        alertAt: [0.75, 0.9, 1.0]
    }
}
