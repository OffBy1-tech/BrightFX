// For Remotion Studio: `npx remotion studio --config=test/remotion.config.ts test/src/index.ts`.
// The tests bundle in-process with the same override.
import { Config } from "@remotion/cli/config";
import { webpackOverride } from "./webpack-override.mjs";

Config.overrideWebpackConfig(webpackOverride);
