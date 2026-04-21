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
# underlying `kcl_plugin.pkg.render` plugin emits a sentinel that
# akua-core expands post-eval — see the pkg_render module docs for why
# this is post-eval and not inline.

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
