# Inner Package — a reusable ConfigMap emitter consumed by the outer
# Package via `pkg.render`. Lives in its own directory so the import
# site uses `pkg.render("./shared", {...})`.

schema Input:
    """Inputs for the shared ConfigMap emitter."""

    name: str
    """Resource name for the emitted ConfigMap."""

    payload: {str:str} = {}
    """Opaque key/value pairs to embed in `data`."""

input: Input = option("input") or Input { name = "shared" }

resources = [{
    apiVersion: "v1"
    kind: "ConfigMap"
    metadata.name: input.name
    metadata.labels: {
        "app.kubernetes.io/name":      input.name
        "app.kubernetes.io/component": "shared"
    }
    data: input.payload
}]

outputs = [
    { kind: "RawManifests", target: "./" }
]
