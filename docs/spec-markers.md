# Marker spec — `x-user-input` and `x-input`

> **Status:** canonical spec. This doc defines the two JSON Schema
> extensions Akua uses. Anything else is bundle-author convention.
>
> **Companions:** [`design-notes.md`](./design-notes.md) (design rationale),
> [`vision.md`](./vision.md) (Gen 4 bundle ABI — why the authoring
> flexibility is consumer-safe).

## The split — two orthogonal axes

Akua's values schema has exactly two extension points. They serve
different purposes and can be used independently or together.

| Extension | Axis | Domain |
|---|---|---|
| `x-user-input` | **Customer visibility** | UI layer: does this field appear in the customer-facing install form? |
| `x-input` | **Transform logic** | Render layer: how does this field's value get computed at render time? |

**Everything JSON Schema already handles stays JSON Schema.** Required
fields live in the standard `required: [...]` arrays on parent objects.
Types, enums, patterns, ranges, defaults — all standard. We don't
reinvent any of it.

## Four combinations

```
                │ x-input set                  │ x-input unset
────────────────┼──────────────────────────────┼──────────────────────────
x-user-input    │ User types raw, transform    │ User types raw, value
set             │ computes the final value     │ passes through unchanged
                │                              │
                │ e.g. subdomain → CEL →       │ e.g. adminEmail
                │ full FQDN                    │
────────────────┼──────────────────────────────┼──────────────────────────
x-user-input    │ Derived field — computed     │ Static value from
unset           │ from other fields/inputs,    │ JSON Schema default or
                │ NOT in the customer UI       │ chart defaults. Not
                │                              │ customer-owned.
                │ e.g. values.generatedSecret  │ e.g. values.image.tag
                │ from CEL on tenant id        │ pinned in Chart.yaml
```

## `x-user-input` — universal marker

Akua's opinionated convention for "this field is customer-configurable."

**Shorthand form:**

```yaml
x-user-input: true
```

**Object form** (recommended — allows UI hints):

```yaml
x-user-input:
  order: 10                    # UI ordering hint (lower = earlier)
  label: "Admin Email"         # optional form label override
  description: "…"             # optional form help text
  placeholder: "ops@acme.com"  # optional placeholder
  group: "Contact"             # optional group/section label
```

**Rules:**

- Allowed keys: `order` (integer), `label` (string), `description`
  (string), `placeholder` (string), `group` (string). Implementations
  MUST ignore unknown keys for forward compatibility.
- Absence of `x-user-input` ⇒ field is NOT in the customer UI.
- Third-party bundle assemblers SHOULD honour `x-user-input` so install
  UIs work across bundles. It's the single standardised marker.

## `x-input` — extension bag

**`x-input` is explicitly a freeform extension bag.** Akua's reference
bundle assembler reads specific keys (`cel`, `uniqueIn`). Alternative
transform-language authors add their own keys (`jsonnet`, `wasmFunc`,
`pythonExpr`, …). Akua's core does not enforce a vocabulary.

**Akua-reference keys:**

```yaml
x-input:
  cel: "slugify(value) + '.' + values.env + '.apps.example.com'"
  uniqueIn: "tenant.hostnames"
```

- **`cel`** — a CEL expression evaluated with `value` (this field's
  trimmed raw input) and `values` (the resolved-so-far object) in scope.
  Registered functions: `slugify(s)`, `slugifyMax(s, n)`, everything in
  `cel-interpreter`'s stdlib. Result must be a string.
- **`uniqueIn`** — name of a uniqueness registry this field participates
  in. Surfaced as a hint; the registry itself is a platform concern
  (e.g., CNAP's tenant-hostname uniqueness service).

**Future / alternative keys (illustrative, not owned by Akua):**

```yaml
x-input:
  jsonnet: "std.asciiLower(value) + '.' + $.values.env"    # some future jsonnet bundle
  wasmFunc: "custom:compute_hostname"                       # hand-WASM transform
```

Third-party bundle assemblers that produce their own `schema()` +
`render()` wasm can invent whatever keys fit their model. Akua's
core doesn't validate or reject them.

## What stays in standard JSON Schema

Do not reinvent these:

| Concern | Standard JSON Schema mechanism |
|---|---|
| Required fields | `required: ["field1", "field2"]` on parent object |
| Type | `type: "string"` / `"integer"` / `"boolean"` / `"object"` / `"array"` |
| Enums | `enum: ["a", "b", "c"]` |
| String patterns | `pattern: "^[a-z]+$"` |
| Numeric bounds | `minimum`, `maximum`, `exclusiveMinimum`, `exclusiveMaximum` |
| String length | `minLength`, `maxLength` |
| Defaults | `default: "…"` |
| Conditional shape | `if`/`then`/`else`, `oneOf`, `anyOf` (or use the Gen 4 `schema()` ABI for dynamic schemas — see below) |

## Authoring format vs resolved format (the ABI connection)

This is the key insight that makes `x-input` safe as an extension bag:

**Authoring format** (what bundle authors write):

```yaml
subdomain:
  type: string
  x-user-input:
    order: 10
    label: "Subdomain"
  x-input:
    cel: "slugify(value) + '.apps.example.com'"    # ← bundle-specific
    uniqueIn: "tenant.hostnames"
```

**Resolved format** (what `schema()` returns at runtime — what install
UIs actually see):

```yaml
subdomain:
  type: string
  x-user-input:
    order: 10
    label: "Subdomain"
  # ← no x-input; the bundle has already internalised the transform.
  # Consumers don't need to know the transform language or syntax.
```

**Install UIs consume the resolved format only.** They never parse
`x-input.cel` or `x-input.jsonnet`. The bundle's `schema()` function
evaluates conditions, produces a standard JSON Schema with only
universal markers (`x-user-input` + standard JSON Schema constraints),
and hands it to the UI.

This is the Gen 4 ABI buying us real interop freedom:

- Bundle authors can use any transform language — the `x-input` bag
  doesn't have to be CEL.
- Consumers only need to understand the universal markers + standard
  JSON Schema.
- Evolving the transform layer in one bundle doesn't break consumers
  of another bundle that uses a different transform language.

**Without the `schema()` ABI** (Gen 3), every install UI would have to
understand every bundle's transform syntax. `x-input` couldn't be a bag
— it'd have to be a strict spec. Gen 4 inverts that.

## Rendering output (the `render()` ABI side)

Same principle on the render side:

- **Authoring format:** Helm chart, KCL program, helmfile, Jsonnet,
  whatever.
- **Resolved format** (what `render()` returns): Kubernetes YAML.

The engine is a bundle-internal detail. Consumers apply the YAML.

## Examples

### Customer types raw input, passes through

```yaml
adminEmail:
  type: string
  format: email
  x-user-input: { order: 20 }
required: ["adminEmail"]
```

### Customer types raw, CEL computes hostname

```yaml
subdomain:
  type: string
  x-user-input: { order: 10 }
  x-input:
    cel: "slugify(value) + '.apps.example.com'"
    uniqueIn: "tenant.hostnames"
required: ["subdomain"]
```

### Derived field — not in UI, computed from other inputs

```yaml
fullHostname:
  type: string
  x-input:
    cel: "slugify(values.subdomain) + '.' + values.env + '.apps.example.com'"
```

(No `x-user-input` — the customer doesn't see or type this; it's
computed from `subdomain` + `env`.)

### Static value with constraints — neither marker

```yaml
replicaCount:
  type: integer
  minimum: 1
  default: 3
```

(Pure JSON Schema. No markers needed.)

## Conformance rules

A bundle is **Akua-conformant** at the marker level if it:

1. Uses `x-user-input` for every customer-facing field (no other
   markers substituting).
2. Uses `x-input` for transform logic, with any keys inside that its
   own `schema()` / `render()` implementation understands.
3. Relies on standard JSON Schema for everything JSON Schema
   handles (type, required, enum, pattern, etc.).
4. Returns a resolved schema from `schema()` that uses only
   `x-user-input` and standard JSON Schema — no `x-input` leakage.

A consumer (install UI, form renderer) is **Akua-conformant** if it:

1. Reads `x-user-input` to know which fields to render.
2. Uses JSON Schema's standard constraints for validation.
3. Ignores any unknown `x-*` keys rather than failing.
4. Never parses `x-input` directly — calls the bundle's `schema()`
   ABI for dynamic form updates.

## Deprecated / removed

These legacy markers are no longer recognised:

- `x-install` (was CEP-0006 vocabulary) → use `x-user-input`.
- `x-hostname` (was CEP-0006 vocabulary with implicit slugify) →
  use `x-input: { cel: "slugify(value) + '...' ", uniqueIn: "..." }`.
- `x-input.template` with `{{value}}` sugar → use `x-input.cel`.
- `x-input.slugify: bool` flag → call the `slugify()` CEL function
  directly in `x-input.cel`.

Migration: one-time rewrite during v1alpha1 authoring-format upgrade.
No published packages exist today, so the migration cost is near-zero.
