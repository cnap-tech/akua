import akua.krm.environment as env

staging = env.Environment {
    metadata.name = "staging"
    spec = env.Spec {
        policy  = "tier/startup"
        cluster = "staging-eu"
        region  = "eu-central-1"
        budget  = env.Budget {
            monthly = "$200"
            alertAt = [0.75, 0.9, 1.0]
        }
    }
}
