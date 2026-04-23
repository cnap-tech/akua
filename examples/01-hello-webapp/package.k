# Example 01 — hello-webapp
#
# Smallest useful akua Package. One local-path chart dep, a few public
# inputs, raw-manifest output.
#
# Render:
#   akua render --inputs inputs.yaml      # render to ./rendered by default

import akua.ctx
import charts.nginx as nginx    # Phase 2a: resolved from akua.toml

schema Input:
    """Public inputs for the hello-webapp Package."""

    name: str = "hello"
    """Internal name used as a resource-name prefix. Lowercase, hyphen-separated."""

    replicas: int = 2
    """Replica count for the Deployment."""

    check:
        replicas >= 1, "replicas must be at least 1"

input: Input = ctx.input()

# Phase 2b slice C: the generated `charts.nginx` module exposes a
# `template()` callable pre-filled with the resolved chart path, so
# the Package only restates the options that matter.
resources = nginx.template(nginx.TemplateOpts {
    values = {
        replicaCount = input.replicas
        fullnameOverride = input.name
    }
    release = input.name
})
