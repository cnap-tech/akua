# @akua/playground

Browser playground for [@akua/sdk](https://jsr.io/@akua/sdk) —
pull + inspect Helm charts in a browser tab, no backend.

Live: **https://cnap-tech.github.io/akua/**

## What it is

A single-page [Vite](https://vitejs.dev/) app that:

1. Loads `@akua/sdk/browser` (workspace dep, tracks in-tree SDK).
2. Prompts for a Helm chart reference
   (`https://<repo>/<chart>:<version>`).
3. Calls `pullChart(ref)` then `inspectChartBytes(bytes)` — both
   run in-page via WebAssembly.
4. Renders the parsed `Chart.yaml` + dependency list + a raw
   view.

No server involved. Browser DevTools → Network tab verifies the
request goes directly to the Helm repo.

## Scope

- ✅ HTTPS Helm repos with CORS (Jetstack, Grafana, JFrog public).
- ❌ `oci://` registries — most omit CORS headers
  (including ghcr.io). Use the CLI or Node SDK for those.

## Dev

```sh
# from monorepo root
bun install

# dev server at http://localhost:5173
bun run --filter @akua/playground dev

# production build → packages/playground/dist/
bun run --filter @akua/playground build
```

The build asset base path is controlled by `PLAYGROUND_BASE`
(default `/`, set to `/akua/` in the Pages deploy workflow).

## Deploy

GitHub Pages via `.github/workflows/playground-deploy.yml`.
Auto-deploys on push to `main` whenever `packages/playground/`,
`packages/sdk/`, or the Rust core changes.
