import .package as pkg

# --- fixtures ---

_minimal = pkg.Input {
    appName  = "app"
    hostname = "app.example.com"
}

_full = pkg.Input {
    appName  = "app"
    hostname = "app.example.com"
    replicas = 5
    tls      = False
}

# --- assertions ---

# Defaults.
assert _minimal.replicas == 2, "replicas default is 2"
assert _minimal.tls == True,   "tls default is True"

# Overrides stick.
assert _full.replicas == 5, "replicas override should take effect"
assert _full.tls == False,  "tls override should take effect"

# check: block enforces the invariant (we can't construct Input {replicas=0}).
# The failing construction is commented out to keep the test file itself
# compilable; akua test runs a companion negative-test runner for check:
# violations.
# _invalid = pkg.Input { appName = "a", hostname = "h", replicas = 0 }
