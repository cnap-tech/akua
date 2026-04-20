# schemas/app.k — THIS WORKSPACE'S App shape. Not akua-specified.
#
# akua does not ship an `App` kind. Every workspace defines its own App
# schema, shaped to its deployment reality. This one captures what THIS
# team's apps need; yours will look different.
#
# This file lives in the workspace. Change it to fit your needs.

import ..package as pkg

schema App:
    """One deployed instance of a package in one environment."""

    # Stable name for this deploy — "checkout-production", "checkout-staging", …
    name: str

    # Reference to an Environment by name (see ../environments/).
    env: str

    # OCI ref of the Package this App instantiates.
    package: str

    # The customer values that satisfy the Package's Input schema.
    # Fully typed: a typo in a field name is a compile error.
    inputs: pkg.Input

    # Where the rendered manifests go to reconcile from.
    target: Target

schema Target:
    reconciler: "argo" | "flux" | "kro" | "kubectl"
    cluster:    str
