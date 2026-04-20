package akua.policies.workspace

import future.keywords

# Every Deployment rendered from this workspace must carry a team label.
# Policies operate on rendered Kubernetes resources (raw kinds); there are
# no akua-specified kinds to match on.
deny[msg] {
    input.resource.kind == "Deployment"
    not input.resource.metadata.labels.team
    msg := sprintf(
        "Deployment %q must have a metadata.labels.team",
        [input.resource.metadata.name],
    )
}

# Production deployments must set replicas >= 2. `input.environment` is
# the workspace's own environment shape (user-defined; see 03-multi-env-app
# for an example schema). This rule reads .name to detect "production"
# without assuming any other field.
deny[msg] {
    input.resource.kind == "Deployment"
    input.environment.name == "production"
    input.resource.spec.replicas < 2
    msg := "production Deployments must have replicas >= 2"
}
