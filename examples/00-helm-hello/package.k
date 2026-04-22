# Minimal Package that exercises the `helm.template` plugin callable.
#
# Renders the adjacent `./chart/` (a tiny ConfigMap template) through
# akua's embedded wasmtime-hosted helm engine. No `helm` binary
# required; no shell-out. See docs/security-model.md + Phase 1 in
# docs/roadmap.md for why.
#
# Render:
#
#   akua render --out ./rendered
#
# `./chart` is a raw-string path resolved under the Package dir — the
# original simplest-possible shape. Example 01 shows the typed
# `import charts.<name>` form that Phase 2a landed; `akua render
# --strict` only accepts that form.
#
# Inputs flow through KCL's `option()` mechanism; `inputs.example.yaml`
# is auto-discovered when --inputs is omitted.

import akua.ctx
import akua.helm

schema Input:
    """Public inputs for 00-helm-hello."""

    greeting: str = "hello from akua"
    """Message embedded into the rendered ConfigMap."""

    replicas: int = 1
    """Replica count recorded in the ConfigMap."""

input: Input = ctx.input()

# helm.template returns a list of Kubernetes resources — a single
# ConfigMap in this case. `*_manifests` splats the list into
# `resources`.
_manifests = helm.template(helm.Template {
    chart     = "./chart"
    values    = {
        greeting = input.greeting
        replicas = input.replicas
    }
    release   = "hello"
    namespace = "default"
})

resources = _manifests
