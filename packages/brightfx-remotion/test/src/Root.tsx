import React from "react";
import { Composition, staticFile } from "remotion";
import { BrightFX } from "../../src/index";
import seekConfig from "../../../../core/fixtures/ffi-seek.config.json";

const WASM = staticFile("brightfx_wasm_bg.wasm");
const effect = seekConfig as unknown as React.ComponentProps<typeof BrightFX>["effect"];

const FrameModeTest: React.FC = () => <BrightFX effect={effect} seed={42} mode="frame" wasmSrc={WASM} />;

const SpriteModeTest: React.FC = () => (
  <BrightFX
    effect={effect}
    seed={42}
    mode="sprite"
    wasmSrc={WASM}
    render={() => <div style={{ width: 4, height: 4, background: "#ff0000" }} />}
  />
);

export const Root: React.FC = () => (
  <>
    <Composition id="FrameModeTest" component={FrameModeTest} durationInFrames={60} fps={30} width={200} height={120} />
    <Composition id="SpriteModeTest" component={SpriteModeTest} durationInFrames={60} fps={30} width={200} height={120} />
  </>
);
