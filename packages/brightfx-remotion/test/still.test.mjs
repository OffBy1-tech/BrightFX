// The component adds nothing: a Remotion still at frame 30 (t = 1.0 s) is
// compared against the wrapper's own frame at seek(1.0). Chrome stores
// canvas pixels premultiplied and unpremultiplies on PNG export, so the
// comparison is made in premultiplied space with a small tolerance.
import { test } from "node:test";
import assert from "node:assert/strict";
import { execFileSync } from "node:child_process";
import { mkdirSync, readFileSync } from "node:fs";
import { dirname, join } from "node:path";
import { fileURLToPath } from "node:url";
import { PNG } from "pngjs";
import { BrightFX } from "brightfx-js";

const here = dirname(fileURLToPath(import.meta.url));
const pkg = join(here, "..");
const out = join(here, "out");
mkdirSync(out, { recursive: true });
const fixtures = join(pkg, "..", "..", "core", "fixtures");
const config = JSON.parse(readFileSync(join(fixtures, "ffi-seek.config.json"), "utf8"));
const FRAME = 30;
const FPS = 30;
const W = 200;
const H = 120;
const SEED = 42;

function still(id) {
  const file = join(out, `${id}.png`);
  execFileSync(
    "npx",
    ["remotion", "still", "--config=test/remotion.config.ts", "test/src/index.ts", id, file, `--frame=${FRAME}`, "--image-format=png", "--log=error"],
    { cwd: pkg, stdio: "inherit" },
  );
  return PNG.sync.read(readFileSync(file));
}

const engine = await BrightFX.init(readFileSync(join(pkg, "..", "brightfx-js", "wasm", "brightfx_wasm_bg.wasm")));

function referenceSim(effect = config) {
  const sim = engine.create(SEED);
  assert.equal(sim.setConfig(effect).ok, true);
  assert.equal(sim.setViewport(W, H, 1).ok, true);
  sim.seek(FRAME / FPS);
  return sim;
}

test("frame mode still equals the wrapper's frame", () => {
  const png = still("FrameModeTest");
  assert.equal(png.width, W);
  assert.equal(png.height, H);
  const sim = referenceSim();
  sim.render();
  const expected = sim.frame().data;

  let differing = 0;
  let painted = 0;
  for (let i = 0; i < expected.length; i += 4) {
    const ea = expected[i + 3];
    const aa = png.data[i + 3];
    if (ea > 0) painted++;
    let off = Math.abs(ea - aa) > 2;
    for (let c = 0; c < 3 && !off; c++) {
      off = Math.abs(Math.round((expected[i + c] * ea) / 255) - Math.round((png.data[i + c] * aa) / 255)) > 6;
    }
    if (off) differing++;
  }
  assert.ok(painted > 200, `reference frame is vacuous: ${painted} painted pixels`);
  assert.ok(differing <= Math.ceil(W * H * 0.01), `${differing} pixels differ beyond tolerance`);
  sim.dispose();
});

test("sprite mode still has a glyph at every visible particle", () => {
  const png = still("SpriteModeTest");
  const sim = referenceSim();
  const particles = sim
    .particleList()
    .filter((p) => p.a > 0.5 && p.x >= 2 && p.x < W - 2 && p.y >= 2 && p.y < H - 2);
  assert.ok(particles.length > 5, `test is vacuous: ${particles.length} particles in frame`);
  for (const p of particles) {
    const x = Math.round(p.x);
    const y = Math.round(p.y);
    const i = (y * W + x) * 4;
    assert.ok(
      png.data[i] > 200 && png.data[i + 1] < 60 && png.data[i + 3] > 100,
      `no red glyph at (${x}, ${y}): rgba(${png.data[i]}, ${png.data[i + 1]}, ${png.data[i + 2]}, ${png.data[i + 3]})`,
    );
  }
  sim.dispose();
});

// The probe compositions reveal a 6x6 blue square in the bottom-right
// corner, clear of the fixture's particles, whenever the component calls
// `render()` or `frame()` (see test/src/Root.tsx).
const isProbe = (png) => {
  const i = ((H - 3) * W + (W - 3)) * 4;
  return png.data[i] < 60 && png.data[i + 1] < 60 && png.data[i + 2] > 200 && png.data[i + 3] > 200;
};

test("frame mode skips rasterizing a frame with no particles and draws nothing", () => {
  const sim = referenceSim({
    ...config,
    emitterTrack: {
      ...config.emitterTrack,
      triggers: [
        { time: 1.5, kind: "startContinuous" },
        { time: 1.8, kind: "burst" },
      ],
    },
  });
  assert.equal(sim.particleCount(), 0, "test is vacuous: the late effect has particles at the still's time");
  sim.dispose();

  const png = still("EmptyFrameTest");
  assert.ok(!isProbe(png), "render() or frame() ran for a frame with no particles");
  let painted = 0;
  for (let i = 3; i < png.data.length; i += 4) if (png.data[i] > 0) painted++;
  assert.equal(painted, 0, `${painted} pixels drawn for a frame with no particles`);
});

test("frame mode still rasterizes and draws a frame with particles", () => {
  const png = still("ActiveFrameProbeTest");
  assert.ok(isProbe(png), "render()/frame() never ran: the probe is not wired, so the empty-frame test proves nothing");
  let painted = 0;
  for (let y = 0; y < H; y++) {
    for (let x = 0; x < W; x++) {
      if (x >= W - 8 && y >= H - 8) continue;
      if (png.data[(y * W + x) * 4 + 3] > 0) painted++;
    }
  }
  assert.ok(painted > 200, `only ${painted} particle pixels drawn`);
});
