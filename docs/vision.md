# Vision — a universal renderer ABI for Kubernetes packages

> **Status:** opinionated forward-look. Not implemented. Captures where
> Akua aims to drive the ecosystem beyond v1.
>
> **Companions:** [`design-notes.md`](./design-notes.md) (current design),
> [`roadmap.md`](./roadmap.md) (phase status), [`use-cases.md`](./use-cases.md)
> (today's flows).

## The framing — four generations

Kubernetes packaging has evolved through three generations and is due
for a fourth. Each generation moved more responsibility off the
consumer (deployer) and into the distribution format.

### Gen 1 — raw YAML in git

A directory of `.yaml` files, committed. Consumer needs to know only
`kubectl apply`. Distribution is copy-paste. No customisation, no
composition, no versioning beyond git history.

Still the right choice for tiny single-resource configs.

### Gen 2 — templating engines per producer

Helm charts, Kustomize overlays, Jsonnet, KCL, Timoni, ytt, Cue, …
Each engine solved templating *its* way. Deployers had to support each
engine explicitly or give up. ArgoCD supports Helm + Kustomize + Jsonnet +
plugins; Flux supports Helm + Kustomize + post-renderers. The
fragmentation era.

Authors win (composition, DRY, customisation). Consumers lose
(every engine is a new dependency to install and maintain).

### Gen 3 — Helm chart on OCI (where we are today)

The ecosystem collapsed onto Helm as the de facto winner. Charts
packaged as OCI artifacts, addressed by digest, fetched over the same
infrastructure as container images. ArgoCD and Flux both support
`oci://` Helm refs natively.

Two things improved: **distribution primitive standardises** (OCI);
**versioning and deduplication** (digests, layer dedup). But the
**renderer** — the thing that turns chart + values into manifests —
is still one specific program (Helm), tied to one template language
(Go + Sprig), with one set of semantics.

This works when Helm is sufficient. When it isn't, you're back to
Gen 2's fragmentation: Kustomize overlays, Jsonnet sidecars, CMPs.

### Gen 4 — WASM renderer as the distribution primitive

**Ship the renderer, not the format.**

Instead of "here's a Helm chart, hope you have a compatible Helm
renderer," the package contains:

- The sources (chart files, KCL programs, overlays — doesn't matter)
- The renderer itself, compiled to WASM

The deployer pulls the package and runs the embedded WASM through
any WASI-compatible runtime. Input: a values JSON. Output: Kubernetes
YAML.

The consumer's contract collapses to **one ABI**: `render(values) → yaml`.
Nothing else. Doesn't matter if the author used Helm, KCL, Kustomize,
Jsonnet, or something new that doesn't exist yet — if it compiles to
WASM matching the ABI, any deployer runs it.

**This is the same trick OCI did for container builds.** Before
container images, "here's my Java app, hope you have Java 11, and
Node 18, and Postgres client libs…" was the deploy conversation. After
container images: "here's an image, run it." Every consumer implements
`docker run`, not Java or Node or Postgres. The image embeds whatever
runtime it needs.

Gen 4 applies the same inversion to rendering. Deployers implement
`wasmtime run`. Packages embed whatever engine they need.

## Why this works *now* (and not five years ago)

- **WASM / WASI 0.3** are production-grade. Wasmtime is stable,
  performant, sandboxed. CNCF-hosted projects (containerd, Fermyon,
  Dylibso) build on it.
- **Helm 4 HIP-0026 (shipped Nov 2025)** already uses Extism WASM
  for plugins. Template-function plugins are next. The Helm community
  is moving this direction.
- **OCI 1.1** has the `referrers` API and stable multi-layer
  manifests. The plumbing for composite artifacts is standard.
- **Determinism concerns** for AI-era supply chain (SLSA, reproducible
  builds, attestation) push hard toward sealed artifacts. A WASM
  renderer is the ultimate seal: the engine that produced the output
  *is* the artifact.

Akua's existing work — native Rust KCL, embedded `helm-engine.wasm`,
`oci-client` for publish/fetch, SLSA attestation — already implements
the primitives. The Gen 4 thesis is the natural extension: expose
what we already do as a consumable standard.

## The bundle format (technical sketch)

### Manifest shape

An Akua package bundle is an OCI artifact with multi-layer manifest:

```
Manifest (mediaType: application/vnd.akua.package.v1+json)
├── config blob  (application/vnd.akua.package.config.v1+json)
│   {
│     "abiVersion": "akua.tech/renderer-abi/v1",
│     "packageName": "hello-app",
│     "packageVersion": "0.1.0",
│     "engine": { "name": "helm-v4", "digest": "sha256:<engine-digest>" },
│     "schema": "sha256:<values-schema-digest>"   // refers to schema layer
│   }
├── layer 0  (application/vnd.akua.engine.v1+wasm)
│   → akua-engine.wasm  (deterministic digest; shared across all packages
│                         using the same engine version)
├── layer 1  (application/vnd.akua.sources.v1.tar+gzip)
│   → chart tarball / KCL program / etc. (package-specific)
├── layer 2  (application/vnd.akua.schema.v1+json)
│   → values.schema.json with x-user-input + CEL
├── layer 3  (application/vnd.akua.metadata.v1+yaml)
│   → .akua/metadata.yaml provenance
└── (optional)
    referrers: [in-toto SLSA attestation blob]
```

### Size + dedup math

| Storage model | 1 package | 1000 packages (same engine) |
|---|---|---|
| Engine embedded inline in a single blob | 75 MB | 75 GB |
| Engine as separate layer | 75 MB | ~125 MB |

**600× reduction** in registry + client-cache bytes. Every OCI
registry (ghcr, DockerHub, Harbor, ECR, …) already deduplicates
layers by digest. Client implementations (`containerd`, `oras`,
`docker`) already cache layers locally. No new infrastructure.

### The renderer ABI

One exported function plus a minimal memory ABI:

```
render(
    values_ptr: i32, values_len: i32,      // JSON-encoded values
    release_ptr: i32, release_len: i32,    // JSON: { name, namespace, revision }
) -> i32                                     // C-string ptr to JSON result

result_len(ptr: i32) -> i32                  // length of the C-string result

free(ptr: i32)                               // release the buffer
```

Result JSON:

```json
{
  "manifests": { "<template-path>": "<rendered yaml>" },
  "error": ""
}
```

That's the whole spec. ~30 lines of documentation.

### Consumer contract

A compliant deployer:

1. Pulls the package manifest via OCI.
2. Extracts the engine layer + sources layer + schema + metadata.
3. Instantiates the engine as a wasip1 reactor module, `_initialize`.
4. Feeds it user values (merged per environment, components, CEL
   transforms already applied).
5. Reads the rendered manifests.
6. Applies to the cluster or packages for `helm install` or
   whatever deploy mechanism fits.

No knowledge of Helm, KCL, helmfile, Kustomize required. The engine
inside the bundle handles its own semantics.

### Determinism guarantee

Engine layer digest + schema digest + sources digest + values bytes
→ deterministic output. Same inputs on any host → same bytes.

Package publishers attest this with SLSA; registries store the
attestation as a referrer; consumers verify via cosign.

## What Akua ships toward Gen 4 today

Already implemented (and re-usable as the reference bundle components):

- ✅ `akua-core`: schema extraction, CEL transforms, umbrella assembly
- ✅ `crates/helm-engine-wasm`: Helm v4 template engine compiled to
  wasip1, hosted via wasmtime. The reference engine.
- ✅ `akua-core::publish`: native OCI push with Helm-compatible media
  types + annotations. Same plumbing extends to Akua-specific media
  types.
- ✅ `akua-core::fetch`: native OCI + HTTP chart fetching via `oci-client`.
- ✅ `akua-core::attest`: SLSA v1 predicate emission for cosign.
- ✅ Determinism: pure-algorithm core is already `wasm-pack`-compiled
  for browsers via `@akua/core-wasm`.

The pieces exist. Gen 4 = reorganise the `publish` output to emit a
multi-layer bundle instead of a single Helm chart.

## Adoption path

### Near term (within Akua, no ecosystem buy-in required)

1. **Spec the ABI.** Single markdown file, <50 lines. Publish as
   `akua.tech/renderer-abi/v1`.
2. **Ship `akua publish --bundle`.** Second output mode alongside the
   current Helm chart. Default stays Helm chart (compat).
3. **Ship `akua render-bundle <oci-ref>`.** Reference consumer — fetches
   a bundle and renders locally. Validates the ABI end-to-end.
4. **Bundle engines for kcl and helmfile.** Today only helm is embedded;
   extending is mechanical.

At this point Akua is self-contained Gen 4 — produces bundles, consumes
bundles, demonstrates the ABI. Useful even with zero ecosystem adoption.

### Medium term (lightweight ecosystem integration)

5. **`akua-cmp` sidecar for ArgoCD.** Small container that implements
   ArgoCD's CMP protocol and renders Akua bundles. Cluster admins install
   once. Every ArgoCD Application can point at an Akua bundle.
6. **Flux post-renderer.** Similar shape, Flux-flavoured.
7. **`helm install --wasm` plugin** that invokes the bundle's engine
   directly. For `helm`-centric shops that want to adopt without changing
   their deploy tooling.

### Long term (ecosystem consensus)

8. **CNCF proposal.** Submit the renderer ABI to TAG App Delivery for
   interop standardisation. De-facto first, spec later — the same path
   OCI image spec took from Docker's internal format.
9. **Helm / ArgoCD / Flux native support.** Once the spec stabilises and
   real packages exist, lobby for direct support that obsoletes the
   sidecar/plugin intermediaries.
10. **Multi-engine bundles.** A package using helm + KCL + kustomize
    embeds all three engines (layer-shared, so cost is once per engine
    version globally). The renderer inside the bundle orchestrates.

## Why Akua specifically

Nobody else is in a position to do this:

- **Helm** is the incumbent; standardising away from Helm-as-renderer
  is a conflict of interest.
- **ArgoCD / Flux** are deploy tools; they consume, not author.
- **Kustomize / KCL / helmfile / Timoni** each have their own engine;
  standardising means admitting their engine is one of many.
- **CNCF alone** is too slow to drive this without a reference implementation.

Akua is **engine-agnostic by construction**. We already embed three
engines (helm, kcl, helmfile) with the design committed to adding
more. A spec and bundle format produced by "the tool that implements
five engines" carries more weight than one from any single-engine
producer.

## Risks — honest

1. **Inertia.** Helm-chart-on-OCI is "good enough" for most teams.
   Gen 4 needs to offer something concrete that Helm-chart-on-OCI
   doesn't.
   - **Pitch:** multi-engine packages, engine version lock, sandboxed
     deploy-time rendering, engine-agnostic deployers.
   - **Reality check:** most teams don't have multi-engine pain. The
     sharp use case is likely SaaS platforms shipping configurable
     products to many tenants where engine diversity matters.

2. **Adoption curve.** Took OCI images ~5 years to become default.
   Gen 4 is probably a similar horizon.
   - **Mitigation:** ship the self-contained path (bundle produce +
     consume via Akua) first. Value exists even without ecosystem
     adoption.

3. **Standardisation politics.** CNCF processes are slow and
   consensus-driven. A spec written by a small project may not get
   endorsed.
   - **Mitigation:** de-facto spec + reference implementations +
     growing user base first. Formal standardisation is a follow-on.

4. **Determinism vs flexibility.** If the bundle re-renders at deploy
   time with fresh values, we lose build-time content addressing of
   the final YAML. If it's immutable post-build, what did we gain?
   - **Resolution:** two modes. **Sealed bundle** = bundle contains
     pre-rendered manifests, engine is for verification only.
     **Live bundle** = bundle re-renders each deploy with fresh values.
     Package author picks. Attestation covers whichever mode.

5. **Binary size per engine.** 75 MB helm-engine is large even once.
   - **Mitigation:** the fork path (15 MB, documented in
     `crates/helm-engine-wasm/README.md`) collapses this when bandwidth
     matters. Layer dedup means every subsequent package pays zero.

## Relationship to v1alpha1

The `package.yaml` we just settled is the **authoring** format. Gen 4
concerns the **distribution** format. They're independent:

| Layer | Format | What changes |
|---|---|---|
| Authoring | `package.yaml` (v1alpha1, just locked) | User-facing declarative config |
| Composition | Umbrella chart + merged values | Akua-core internals |
| Distribution — today | Helm chart on OCI | Gen 3 |
| Distribution — Gen 4 | WASM bundle on OCI (multi-layer) | Additive alongside Gen 3 |

Nothing in v1alpha1 precludes Gen 4. We can ship bundle output as a
`--format wasm-bundle` flag on `akua publish` without touching the
authoring format.

## What this doc is for

1. **External pitch.** When someone asks "what is Akua?" the three-line
   answer is "a build tool that produces deployable Kubernetes packages
   with the renderer embedded — a Gen 4 take on the chart-on-OCI
   model." Anyone familiar with OCI image adoption recognises the
   pattern.
2. **Internal north star.** When we debate "should Akua support X?",
   the answer is often "yes if X maps cleanly onto the Gen 4 thesis;
   no if X is Gen 3 feature-bloat."
3. **Contributor ambition.** Open-source projects need a pitch that
   excites potential contributors. "We're fixing the Helm/KCL/Kustomize
   fragmentation" is a bigger pitch than "we're a nicer Helm wrapper."
4. **Adoption signal.** Teams evaluating Akua today (Gen 3, works fine)
   are also implicitly voting on the Gen 4 direction. Making that
   explicit shortens decision cycles.

## Specifically *not* v1

The bundle format, the reference consumers, the standardisation work
— none of this is v1alpha1 or v1. It's Phase 8+ material. v1 is:

- Stable authoring format (`package.yaml` v1alpha1 → v1)
- Rock-solid Gen 3 output (Helm chart on OCI)
- Full engine roster (helm, kcl, helmfile)
- CLI + library + wasm core

Once v1 ships and is in production use, we build toward Gen 4. Don't
let ambition delay utility.
