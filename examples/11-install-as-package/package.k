import akua.ctx
import akua.pkg

# An "install" Package: composes a third-party Akua package
# (`upstream`) and applies tenant-specific overlay + filter + extras.
# This is the install-as-Package shape — no separate install schema,
# no helm-values-only escape hatch. Just KCL composing KCL.

schema Input:
    """Inputs for the install."""
    tenant: str
    app: str
    replicas: int = 2

input: Input = ctx.input()

# 1. Render the upstream Package with caller-supplied inputs. The
#    `package = "upstream"` alias resolves via this Package's
#    `[dependencies]` — no path strings here. Inputs are an explicit
#    dict; the upstream's `Input` schema validates field names + types
#    on the inner side via `ctx.input()`. (Reaching upstream's typed
#    `Input` at this call site needs a synthesized stub module per
#    dep — follow-up work.)
_up = pkg.render(pkg.Render {
    package = "upstream"
    inputs  = {
        appName  = input.app
        replicas = input.replicas
    }
})

# 2. Overlay: apply a tenant label to every upstream resource.
_patched = [r | {
    metadata.labels = {"install.cnap.tech/tenant" = input.tenant}
} for r in _up]

# 3. Filter: this install doesn't want PDBs (single-replica policy).
_filtered = [r for r in _patched if r.kind != "PodDisruptionBudget"]

# 4. Extras: append an install-meta ConfigMap.
_extras = [{
    apiVersion = "v1"
    kind = "ConfigMap"
    metadata.name = input.app + "-install-meta"
    metadata.labels = {"install.cnap.tech/tenant" = input.tenant}
    data.tenant = input.tenant
    data.upstreamApp = input.app
}]

resources = _filtered + _extras
