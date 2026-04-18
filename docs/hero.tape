# VHS tape — renders docs/hero.gif for the README hero slot.
#
# Three-scene operator journey:
#   1. Scaffold a new package from scratch (`akua init`).
#   2. Pull + inspect any Helm chart from an OCI registry —
#      without `helm` on PATH.
#   3. Structurally diff two chart versions.
#
# Render with:
#   vhs docs/hero.tape
#
# ~25 s total runtime. 1280×720 Catppuccin-Mocha theme, font size
# tuned for legibility on GitHub's README viewport.

Output docs/hero.gif

Set Shell "bash"
Set FontSize 14
Set Width 1280
Set Height 720
Set Theme "Catppuccin Mocha"
Set TypingSpeed 25ms
Set Padding 30

# Hermetic workdir so the tape leaves no files behind under
# /docs/. Hide/Show suppresses the setup lines from the capture.
Hide
Type "cd $(mktemp -d) && clear"
Enter
Sleep 300ms
Show

# Scene 1 — scaffold a package in one command.
Type "# Start a new cloud-native package:"
Sleep 400ms
Enter
Type "akua init demo && cd demo && ls"
Sleep 300ms
Enter
Sleep 2500ms

Type "clear"
Sleep 200ms
Enter
Sleep 200ms

# Scene 2 — pull any OCI chart, no helm binary involved.
Type "# Pull any OCI chart live — no \`helm\` binary required:"
Sleep 500ms
Enter
Type "akua inspect --oci oci://ghcr.io/stefanprodan/charts/podinfo:6.7.1 | head -18"
Sleep 300ms
Enter
Sleep 4500ms

Type "clear"
Sleep 200ms
Enter
Sleep 200ms

# Scene 3 — structural diff between two chart versions.
Type "# What changed between two versions? (Structural, not rendered.)"
Sleep 500ms
Enter
Type "akua diff oci://ghcr.io/stefanprodan/charts/podinfo:6.6.0 oci://ghcr.io/stefanprodan/charts/podinfo:6.7.1"
Sleep 400ms
Enter
Sleep 7500ms
