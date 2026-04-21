# akua/pkg — typed wrappers over the `kcl_plugin.pkg` surface.
#
# Users:
#
#   import akua.pkg
#
#   resources = pkg.render("./shared", { name = "frontend" })
#
# The underlying `kcl_plugin.pkg.render` plugin emits a sentinel that
# akua-core expands post-eval — see the pkg_render module docs for
# why this is post-eval and not inline.

import kcl_plugin.pkg as _pkg

# Render a nested Package.k and splice its `resources` list into the
# caller's resources. `path` is the Package.k path (or a directory
# containing one), resolved against the calling Package's directory.
# `inputs` are forwarded to the inner Package's `option("input")`.
render = lambda path: str, inputs: {str:} = {} -> [{str:}] {
    _pkg.render(path, inputs)
}
