# Outer Package: composes two ConfigMaps from a reusable nested
# Package via `pkg.render`. Exercises akua's Package-of-Packages
# composition — the inner Package lives at ./shared/ and is loaded
# + rendered fresh for each `pkg.render` call.
#
# Render:
#
#   akua render --package examples/08-pkg-compose/package.k --out ./rendered
#
# Produces two ConfigMaps: `config-frontend` and `config-backend`,
# each with its own data payload from the outer's inputs.

import akua.pkg

schema ComponentInput:
    """Per-component inputs — name + payload passed through to the
    shared ConfigMap schema in `./shared/`."""

    name: str
    data: {str:str} = {}

schema Input:
    frontend: ComponentInput = ComponentInput { name = "config-frontend" }
    backend:  ComponentInput = ComponentInput { name = "config-backend"  }

input: Input = option("input") or Input {}

# Each call lands as a sentinel at eval time; the akua render
# pipeline walks the resources list post-eval, expands each
# sentinel by loading + rendering `./shared/package.k` with the
# supplied inputs, then splices the nested resources in place.
_frontend = pkg.render("./shared", {
    name    = input.frontend.name
    payload = input.frontend.data
})

_backend = pkg.render("./shared", {
    name    = input.backend.name
    payload = input.backend.data
})

resources = _frontend + _backend

outputs = [
    { kind: "RawManifests", target: "./" }
]
