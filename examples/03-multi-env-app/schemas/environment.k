# schemas/environment.k — THIS WORKSPACE'S Environment shape. Not akua-specified.
#
# akua does not ship an `Environment` kind either. This schema models what
# THIS team needs per env: policy tier, cluster binding, budget, approvers,
# soak rules. A different team's Environment might include SLO targets,
# blue/green weights, or per-env secret stores; add fields as you need them.

schema Environment:
    """One deployment environment (dev, staging, production, …)."""

    name: str

    # Which policy tier the environment enforces (see ../policies/ or
    # any `tier/*` published to OCI and pinned in akua.toml).
    policy: str

    # Cluster + region binding.
    cluster: str
    region:  str = "eu-central-1"

    # Optional guardrails. Missing = no budget cap, no approvers, no soak.
    budget:    Budget?
    approvers: [str] = []
    soak:      Soak?

schema Budget:
    monthly: str
    alertAt: [float]

schema Soak:
    after_deploy: str
    fail_fast_on: [str]
