# akua/pkg — typed wrapper over the `kcl_plugin.pkg` surface.
#
# Users:
#
#   import akua.pkg
#
#   resources = pkg.render(pkg.Render {
#       path   = "./shared"
#       inputs = { name = "frontend" }
#   })
#
# Options-schema call style matches `akua.helm` / `akua.kustomize`. The
# underlying `kcl_plugin.pkg.render` host plugin loads + renders the
# inner Package synchronously and returns the resulting list — so
# list-comprehension patches and filter expressions on the result work
# natively.

import kcl_plugin.pkg as _pkg

schema Render:
    """Options for a single nested-Package render."""

    path: str
    """Path to the nested `package.k` (or a directory containing one).
    Relative paths resolve against the calling Package's directory."""

    inputs: {str:} = {}
    """Inputs forwarded to the inner Package's `ctx.input()`."""

# Render a nested Package.k and splice its `resources` list into the
# caller's resources. The schema itself is passed through to the
# plugin, which reads fields by name.
render = lambda opts: Render -> [{str:}] {
    _pkg.render(opts)
}
