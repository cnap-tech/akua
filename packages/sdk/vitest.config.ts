import { defineConfig } from 'vitest/config';

export default defineConfig({
  test: {
    globals: false,
    environment: 'node',
    // Each test calls `init()` on the Node entry, which loads the
    // nodejs wasm-pack artifact synchronously. Keep tests serial so
    // the shared WASM runtime isn't torn down mid-test.
    fileParallelism: false,
  },
});
