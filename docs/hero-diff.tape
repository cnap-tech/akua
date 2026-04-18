# VHS tape — renders docs/hero-diff.gif for the README.
#
# Records `akua diff` between two real podinfo versions, showing
# the structural delta (version bump, values changes) in a compact
# terminal frame. Target runtime: ~8 seconds.
#
# Render with:
#   vhs docs/hero-diff.tape

Output docs/hero-diff.gif

Set Shell "bash"
Set FontSize 18
Set Width 1280
Set Height 720
Set Theme "Catppuccin Mocha"
Set TypingSpeed 40ms

# Subtle pause at start so the capture doesn't feel abrupt.
Sleep 500ms

Type "akua diff oci://ghcr.io/stefanprodan/charts/podinfo:6.6.0 oci://ghcr.io/stefanprodan/charts/podinfo:6.7.1"
Sleep 500ms
Enter

# Give the pull + diff time to complete. Two OCI pulls over the
# wire on cold cache take ~4-6s on a typical connection.
Sleep 8s
