// Shared by remotion.config.ts (Studio) and still.test.mjs (the in-process
// bundle), so both build the compositions the same way.
//
// The wrapper's wasm glue references its binary with `new URL(..., import.meta.url)`
// for the default path we never take. Leave it as an asset rather than a
// WebAssembly module so webpack does not try to instantiate it at bundle time.
export const webpackOverride = (config) => ({
  ...config,
  experiments: { ...(config.experiments ?? {}), asyncWebAssembly: false, syncWebAssembly: false },
  module: {
    ...config.module,
    rules: [...(config.module?.rules ?? []), { test: /\.wasm$/, type: "asset/resource" }],
  },
});
