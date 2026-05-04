// Render a Package from an in-memory source buffer — no filesystem,
// no binary, no subprocess. Goes through the napi addon.

import { Akua } from '../src/mod.ts';

const SOURCE = `
schema Input:
    appName: str
    replicas: int = 2

input: Input = option("input") or Input {}

resources = [{
    apiVersion: "v1"
    kind: "ConfigMap"
    metadata.name: input.appName
    data.replicas: str(input.replicas)
}]
`;

const akua = new Akua();

// Defaults — replicas falls back to 2
const defaultYaml = await akua.renderSource({
	source: SOURCE,
	inputs: { appName: 'checkout' },
});
console.log('--- default ---');
console.log(defaultYaml);

// Override — 7 replicas
const customYaml = await akua.renderSource({
	source: SOURCE,
	inputs: { appName: 'checkout', replicas: 7 },
});
console.log('\n--- replicas=7 ---');
console.log(customYaml);
