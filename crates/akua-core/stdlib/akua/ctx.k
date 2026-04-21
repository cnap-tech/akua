# akua/ctx — ambient-context accessors for Package authors.
#
# Centralizes every piece of data akua injects into a render: the
# user-supplied `Input` today, plus future ambient context like cluster
# version, environment metadata, and render-time platform hints that
# don't belong in the public `Input` schema (because consumers don't
# set them — akua does).
#
#   import akua.ctx
#
#   schema Input:
#       replicas: int = 2
#
#   input: Input = ctx.input()
#
# KCL's structural typing converts the returned dict into an `Input`
# instance at the assignment site — defaults fill in, `check:` blocks
# run, field-type mismatches fail at render. Full validation without
# the Package spelling akua's plumbing strings.

# Read the user-supplied `"input"` option. Returns an empty dict when
# no inputs were passed, so schema defaults apply.
input = lambda -> {str:} {
    option("input") or {}
}
