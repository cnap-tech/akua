import { defineConfig } from 'vite';
import wasm from 'vite-plugin-wasm';

// GitHub Pages deploys under `/<repo>/` when using the default
// `<user>.github.io/<repo>/` path. Set via env at build time so
// dev still runs at `/`.
const base = process.env.PLAYGROUND_BASE ?? '/';

export default defineConfig({
  base,
  // wasm-bindgen's bundler output uses `import init from
  // './foo.wasm'` (ESM-integration-proposal shape) which Vite /
  // Rollup don't handle natively. vite-plugin-wasm bridges this.
  // Top-level await (the SDK uses it in init) is already supported
  // natively at `target: 'es2022'` below.
  plugins: [wasm()],
  build: {
    target: 'es2022',
  },
  server: {
    port: 5173,
    strictPort: true,
  },
  optimizeDeps: {
    exclude: ['@akua/sdk'],
  },
});
