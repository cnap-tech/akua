# `E_PATH_ESCAPE` — plugin path escapes the Package directory

## What happened

A KCL plugin (`helm.template`, `kustomize.build`, `pkg.render`, …) was called with a path argument that resolved to a location **outside** the Package's own directory. Akua's render sandbox refuses these by design: the Package directory is the only filesystem region a render can read from, and `..` traversal / symlink escape is the most common way an untrusted Package can try to break out.

Typical message:

```
plugin path `../upstream` resolved to `/private/tmp/spike1/upstream`,
which escapes the Package directory `/private/tmp/spike1/install`
```

## Why akua refuses

`akua render` runs each Package inside a wasmtime sandbox with read-only filesystem preopens scoped to the Package directory. A path that resolves outside that root is — by construction — unreachable through the sandbox's capabilities. We surface the error early instead of letting it manifest as a confusing wasmtime open-file failure deeper in the render.

See [`docs/security-model.md`](../security-model.md) for the full threat model.

## How to fix it

Two correct paths, in order of preference:

### 1. Vendor the dependency as a subdirectory

If you control the layout, move (or copy) the upstream into a subdirectory of your Package:

```
my-install/
├── akua.toml
├── package.k
└── vendor/
    └── upstream/
        ├── akua.toml
        └── package.k
```

Then your plugin call uses a Package-relative path:

```kcl
_up = pkg.render({
    path = "./vendor/upstream"
    inputs = { ... }
})
```

This is the right answer for monorepo / co-developed pairs where vendoring is acceptable. The vendored copy is part of the Package's signed surface.

### 2. Declare the dep in `akua.toml` and reference the resolved alias

For separately versioned dependencies (especially OCI-published ones), declare it in `akua.toml`:

```toml
# akua.toml
[dependencies]
upstream = { oci = "oci://example.com/charts/webapp", version = "1.2.0" }
# or
upstream = { path = "../upstream" }   # resolved at build time
```

Then reference the resolved alias from `package.k`:

```kcl
# Helm chart dep — pass the resolved path to helm.template:
import charts.nginx
resources = helm.template({ chart = nginx.path, values = ... })

# KCL/Akua-package dep — once you depend on it, you can `import` it
# directly (the resolver mounts it as a KCL ExternalPkg):
import upstream
resources = upstream.resources + extras
```

`akua lock` records the resolved digest; `akua render` reads the dep from the local cache (under `~/.cache/akua/`), and the sandbox preopens that cache root in addition to the Package directory.

## What NOT to do

- **Don't pass absolute paths to plugins** (`/var/cache/...`). The sandbox refuses anything outside its preopened roots, even if you `chmod` your way to readability.
- **Don't symlink your way around it.** Akua canonicalizes plugin paths before the under-Package check; a `./link → ../upstream` symlink resolves the same as `../upstream` and gets rejected the same way.

## See also

- [`docs/lockfile-format.md`](../lockfile-format.md) — `akua.toml` `[dependencies]` syntax.
- [`docs/package-format.md`](../package-format.md) — Package authoring shape.
- [`docs/security-model.md`](../security-model.md) — sandbox invariants.
