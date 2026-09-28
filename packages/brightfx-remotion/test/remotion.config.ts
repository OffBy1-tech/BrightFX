import { Config } from "@remotion/cli/config";

// The wrapper's wasm glue references its binary with `new URL(..., import.meta.url)`
// for the default path we never take. Leave it as an asset rather than a
// WebAssembly module so webpack does not try to instantiate it at bundle time.
Config.overrideWebpackConfig((config) => ({
  ...config,
  experiments: { ...(config.experiments ?? {}), asyncWebAssembly: false, syncWebAssembly: false },
  module: {
    ...config.module,
    rules: [...(config.module?.rules ?? []), { test: /\.wasm$/, type: "asset/resource" }],
  },
}));
