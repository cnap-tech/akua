import ..schemas.environment as s

dev = s.Environment {
    name:    "dev"
    policy:  "tier/dev"
    cluster: "dev-eu"
    region:  "eu-central-1"
    # No budget, no approvers — dev is permissive.
}
