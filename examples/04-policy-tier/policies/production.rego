# production.rego — local tier for my_org's production environment.
#
# Composes two imports (akua's reference tier/production + a Kyverno security
# bundle converted to Rego) and adds workspace-specific rules.
#
# Rule: the package name is reverse-DNS:
#   akua.policies.<org>.<name>

package akua.policies.my_org_production

import future.keywords

# Compile-resolved imports. Declared in akua.toml, pinned by digest in akua.lock.
# Never runtime string lookups — `kyverno.check({bundle: "oci://..."})` is
# explicitly disallowed.
import data.akua.policies.tier.production as base_tier
import data.akua.policies.kyverno.security as kyv

# --- Inherit from imports ---

# Every deny message from the reference tier flows through.
deny[msg] {
    msg := base_tier.deny[_]
}

# Every deny from the Kyverno security bundle (converted to Rego at build
# time) flows through. We evaluate the compiled Rego — Kyverno itself is
# not invoked at eval time.
deny[msg] {
    msg := kyv.deny[_]
}

# --- Local workspace rules ---

# Every production Deployment must carry a team label. The imported tier
# enforces ownership labels in general; we specifically require `team`.
deny[msg] {
    input.resource.kind == "Deployment"
    input.environment.name == "production"
    not input.resource.metadata.labels.team
    msg := sprintf(
        "production Deployment %q must have a metadata.labels.team",
        [input.resource.metadata.name],
    )
}

# Cross-resource aggregation — Rego's sweet spot. Sum the CPU requests of
# every Deployment in the current batch and reject if the total exceeds the
# environment's budget (workspace-defined schema; see 03-multi-env-app).
deny[msg] {
    deployments := [r | r := input.resources[_]; r.kind == "Deployment"]
    total_cpu := sum([
        cpu_millicores(r.spec.template.spec.containers[_].resources.requests.cpu) |
        r := deployments[_]
    ])
    budget := input.environment.budget.cpu_millicores
    total_cpu > budget
    msg := sprintf(
        "total CPU requests %dm exceed environment budget %dm",
        [total_cpu, budget],
    )
}

# Helper — parse a Kubernetes CPU string to millicores.
# "500m" → 500, "2" → 2000.
cpu_millicores(v) := n {
    endswith(v, "m")
    n := to_number(substring(v, 0, count(v) - 1))
}
cpu_millicores(v) := n {
    not endswith(v, "m")
    n := to_number(v) * 1000
}

# --- akua runtime builtins ---
# Only use these for things that fundamentally need runtime context.
# Static rules stay as imports above.

# Gate: reject packages older than v2 in production. The akua.package
# builtin resolves the package ref to its metadata; the attestation chain
# is read from the OCI registry at eval time (cacheable per CI run).
deny[msg] {
    pkg := akua.package(input.resource.metadata.annotations["akua.dev/package"])
    pkg.schema.version < "2.0"
    msg := sprintf(
        "package %s is at schema v%s; production requires v2+",
        [pkg.ref, pkg.schema.version],
    )
}
