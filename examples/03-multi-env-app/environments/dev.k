import akua.krm.environment as env

dev = env.Environment {
    metadata.name = "dev"
    spec = env.Spec {
        policy  = "tier/dev"
        cluster = "dev-eu"
        region  = "eu-central-1"
        # No budget, no approvers — dev is permissive.
    }
}
