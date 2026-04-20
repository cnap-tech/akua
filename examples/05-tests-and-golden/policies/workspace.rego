package akua.policies.workspace

import future.keywords

# Every App resource must carry a team label.
deny[msg] {
    input.resource.kind == "App"
    not input.resource.metadata.labels.team
    msg := sprintf(
        "App %q must have a metadata.labels.team",
        [input.resource.metadata.name],
    )
}

# Production apps must set replicas >= 2.
deny[msg] {
    input.resource.kind == "App"
    input.environment.name == "production"
    input.resource.spec.inputs.production.replicas < 2
    msg := "production apps must have replicas >= 2"
}
