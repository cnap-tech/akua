# Examples

Reference packages demonstrating Akua's capabilities. All examples are placeholders until v4 ships.

- **`hello-package/`** — minimal single-component package with a Helm chart and one user-input field. The "hello world" of Akua.
- **`hybrid-knative-helm/`** — a hybrid package combining a Knative stateless app and a Helm-managed Postgres, with shared user inputs (subdomain) applied to both components.
- **`transform-examples/`** — the same transform logic implemented in different runtimes (TypeScript, Rust→WASM, AssemblyScript, Python via Pyodide, KCL). Demonstrates the polyglot transform contract.

Once v4 ships, each example will be runnable via:

```bash
cd examples/hello-package
akua pkg preview --inputs '{"subdomain": "acme"}'
akua pkg test
akua pkg build
```
