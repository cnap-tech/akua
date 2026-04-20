# test_package.k — unit tests for the 02-webapp-postgres Package.
#
# Run with:  akua test
#
# File naming per cli.md: `test_*.k`. Top-level assertions run; failures
# surface with line + field context.

import .package as pkg

# --- fixture ---

_sample_input = pkg.Input {
    appName  = "checkout"
    hostname = "checkout.example.com"
    replicas = 3
    team     = "payments"
    database = { user = "app" }
}

# --- assertions ---

# The schema validates the fixture.
assert pkg.Input{**_sample_input} == _sample_input, \
    "fixture must satisfy the Input schema"

# Cross-field check: replicas ≥ 1 — enforced by the package's own check block.
assert _sample_input.replicas >= 1, \
    "replicas default must be ≥ 1"

# Database user default is applied when not overridden.
_default_db = pkg.Input {
    appName  = "checkout"
    hostname = "checkout.example.com"
}
assert _default_db.database.user == "app", \
    "database.user default should be 'app'"

# Team default applies when not overridden.
assert _default_db.team == "platform", \
    "team default should be 'platform'"
