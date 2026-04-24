// Smoke test: load the Node-target akua-wasm bundle, render a
// pure-KCL Package with inputs, assert the rendered YAML contains
// what the KCL program emitted. Proves the whole chain:
// wasm32-unknown-unknown build → wasm-bindgen glue → Node
// WebAssembly.instantiate → KCL eval → YAML out.
//
// Run via `task test:akua-wasm`.

import { render, version } from "./pkg-nodejs/akua_wasm.js";

const SOURCE = `
schema Input:
    replicas: int = 2

input: Input = option("input") or Input {}

resources = [{
    apiVersion: "v1"
    kind: "ConfigMap"
    metadata.name: "smoke"
    data.count: str(input.replicas)
}]
`;

function assert(cond, msg) {
    if (!cond) {
        console.error(`✗ ${msg}`);
        process.exit(1);
    }
    console.log(`✓ ${msg}`);
}

// 1. Version exported and non-empty.
const v = version();
assert(typeof v === "string" && v.length > 0, `version() returns "${v}"`);

// 2. Default-inputs render — schema default (replicas: 2) applies.
const defaultYaml = render("package.k", SOURCE, null);
assert(defaultYaml.includes("kind: ConfigMap"), "default render emits ConfigMap");
assert(
    defaultYaml.includes("count: '2'") || defaultYaml.includes("count: 2"),
    `default replicas=2 threaded through to ConfigMap (got: ${defaultYaml.trim()})`,
);

// 3. Inputs JSON override — replicas=7 wins over the schema default.
const customYaml = render("package.k", SOURCE, JSON.stringify({ replicas: 7 }));
assert(
    customYaml.includes("count: '7'") || customYaml.includes("count: 7"),
    `inputs JSON threaded through to ConfigMap (got: ${customYaml.trim()})`,
);

// 4. KCL diagnostic surfaces as JS exception.
let threw = false;
try {
    render("package.k", "this is not valid kcl", null);
} catch (e) {
    threw = true;
    assert(
        String(e).length > 0,
        `syntax error surfaces as JS exception: ${String(e).slice(0, 80)}...`,
    );
}
assert(threw, "render() throws on KCL syntax error");

console.log("\nsmoke: all assertions passed");
