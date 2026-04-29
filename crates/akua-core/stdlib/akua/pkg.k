# akua/pkg — typed wrapper over the `kcl_plugin.pkg` surface.
#
# Users:
#
#   import akua.pkg
#
#   resources = pkg.render(pkg.Render {
#       package = "upstream"
#       inputs  = { name = "frontend" }
#   })
#
# `package` is the dep alias declared in this Package's `akua.toml`
# `[dependencies]`. The host's resolver maps it to the upstream's
# directory; user code never writes a filesystem path. The legacy
# `path = "./..."` form still works for migration but `akua lint`
# will surface a fix-it (see CLAUDE.md "No filesystem paths in
# user-authored KCL.").
#
# Options-schema call style matches `akua.helm` / `akua.kustomize`. The
# underlying `kcl_plugin.pkg.render` host plugin loads + renders the
# inner Package synchronously and returns the resulting list — so
# list-comprehension patches and filter expressions on the result work
# natively.

import kcl_plugin.pkg as _pkg

schema Render:
    """Options for a single nested-Package render."""

    package?: str
    """Dep alias from this Package's `akua.toml` `[dependencies]`. The
    host resolves it to the upstream's `package.k`. Preferred over
    `path` — typed, no path strings in user code."""

    path?: str
    """**Deprecated.** Filesystem path to the nested `package.k` (or a
    directory containing one). Relative paths resolve against the
    calling Package's directory. Use `package` instead."""

    inputs: {str:} = {}
    """Inputs forwarded to the inner Package's `ctx.input()`. The
    upstream's `Input` schema validates field names + types on the
    inner side. Consumer-side typing (reaching upstream's `Input`
    schema directly at the call site) needs a synthesized stub module
    per dep — separate work."""

    check:
        package or path, "exactly one of `package` or `path` must be set"

# Render a nested Package.k and splice its `resources` list into the
# caller's resources. The schema itself is passed through to the
# plugin, which reads fields by name.
render = lambda opts: Render -> [{str:}] {
    _pkg.render(opts)
}
