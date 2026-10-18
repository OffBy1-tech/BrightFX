// The component adds nothing: a Remotion still at frame 30 (t = 1.0 s) is
// compared against the wrapper's own frame at seek(1.0). Chrome stores
// canvas pixels premultiplied and unpremultiplies on PNG export, so the
// comparison is made in premultiplied space with a small tolerance.
import { test } from "node:test";
import assert from "node:assert/strict";
import { execFileSync, spawnSync } from "node:child_process";
import { mkdirSync, readdirSync, readFileSync, rmSync } from "node:fs";
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
const shared = JSON.parse(readFileSync(join(here, "fixtures", "probe-effects.json"), "utf8"));
const withTriggers = (triggers) => ({ ...config, emitterTrack: { ...config.emitterTrack, triggers } });
const FRAME = 30;
const FPS = 30;
const W = 200;
const H = 120;
const SEED = 42;

function still(id, frame = FRAME) {
  const file = join(out, `${id}.png`);
  execFileSync(
    "npx",
    ["remotion", "still", "--config=test/remotion.config.ts", "test/src/index.ts", id, file, `--frame=${frame}`, "--image-format=png", "--log=error"],
    { cwd: pkg, stdio: "inherit" },
  );
  return PNG.sync.read(readFileSync(file));
}

// Frames `from..to` rendered in one tab, one after another, so the
// component stays mounted across them the way it does in a real render.
// Returns the frames and every setBounds/setViewport call made while
// rendering them, in order (see the call log in test/src/Root.tsx), read
// from the browser console that --log=verbose forwards. `scale` is
// Remotion's --scale, which sets the page's devicePixelRatio.
function range(id, from, to, { scale = 1 } = {}) {
  const dir = join(out, scale === 1 ? id : `${id}@${scale}x`);
  rmSync(dir, { recursive: true, force: true });
  const run = spawnSync(
    "npx",
    ["remotion", "render", "--config=test/remotion.config.ts", "test/src/index.ts", id, dir, "--sequence", "--image-format=png", `--frames=${from}-${to}`, "--concurrency=1", `--scale=${scale}`, "--log=verbose"],
    { cwd: pkg, encoding: "utf8", maxBuffer: 64 * 1024 * 1024 },
  );
  const log = `${run.stdout}${run.stderr}`;
  assert.equal(run.status, 0, `remotion render ${id} failed:\n${log}`);
  const calls = [...log.matchAll(new RegExp(`${shared.callLogTag} (\\{.*\\})$`, "gm"))].map((m) => JSON.parse(m[1]));
  const files = readdirSync(dir).filter((f) => f.endsWith(".png")).sort();
  assert.equal(files.length, to - from + 1, `expected ${to - from + 1} frames, got ${files.length}`);
  return { frames: files.map((f) => PNG.sync.read(readFileSync(join(dir, f)))), calls };
}

// The calls' method and arguments, without the devicePixelRatio.
const sizing = (calls) => calls.map(({ method, args }) => ({ method, args }));

const painted = (png) => {
  let n = 0;
  for (let i = 3; i < png.data.length; i += 4) if (png.data[i] > 0) n++;
  return n;
};

// Probe markers (see test/src/Root.tsx): squares pinned to the bottom-right
// corner, revealed when the component calls the probed Simulation method.
const PROBE_BOX_W = Math.max(...Object.values(shared.probes).map((p) => p.right)) + shared.probeSize;
const inProbeBox = (x, y) => x >= W - PROBE_BOX_W && y >= H - shared.probeSize;
function probePixel(png, name) {
  const { right } = shared.probes[name];
  const i = ((H - shared.probeSize / 2) * W + (W - right - shared.probeSize / 2)) * 4;
  return png.data.subarray(i, i + 4);
}
function probeShown(png, name) {
  const px = probePixel(png, name);
  return px[3] > 200 && shared.probes[name].color.every((c, k) => Math.abs(px[k] - c) < 40);
}

const engine = await BrightFX.init(readFileSync(join(pkg, "..", "brightfx-js", "wasm", "brightfx_wasm_bg.wasm")));

function referenceSim(effect = config, frame = FRAME) {
  const sim = engine.create(SEED);
  assert.equal(sim.setConfig(effect).ok, true);
  assert.equal(sim.setViewport(W, H, 1).ok, true);
  sim.seek(frame / FPS);
  return sim;
}

// How many pixels of `png` differ from the wrapper's own frame at `frame`,
// compared in premultiplied space, outside the probe box.
function differingFromReference(png, frame = FRAME) {
  const sim = referenceSim(config, frame);
  sim.render();
  const expected = sim.frame().data;
  sim.dispose();
  let differing = 0;
  let painted = 0;
  for (let i = 0; i < expected.length; i += 4) {
    if (inProbeBox((i / 4) % W, Math.floor(i / 4 / W))) continue;
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
  return differing;
}

const DIFF_BUDGET = Math.ceil(W * H * 0.01);

test("frame mode still equals the wrapper's frame", () => {
  const png = still("FrameModeTest");
  assert.equal(png.width, W);
  assert.equal(png.height, H);
  assert.ok(probeShown(png, "raster"), "render()/frame() never ran: the probe is not wired, so the empty-frame test proves nothing");
  const differing = differingFromReference(png);
  assert.ok(differing <= DIFF_BUDGET, `${differing} pixels differ beyond tolerance`);
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

test("frame mode skips rasterizing a frame with no particles and draws nothing", () => {
  const sim = referenceSim(withTriggers(shared.lateStartTriggers));
  assert.equal(sim.particleCount(), 0, "test is vacuous: the late effect has particles at the still's time");
  sim.dispose();

  const png = still("EmptyFrameTest");
  assert.ok(probeShown(png, "seek"), "seek never ran, so no sim-backed render happened and this proves nothing");
  assert.ok(!probeShown(png, "raster"), "render() or frame() ran for a frame with no particles");
  const drawn = [...Array(W * H).keys()].filter((p) => !inProbeBox(p % W, Math.floor(p / W)) && png.data[p * 4 + 3] > 0);
  assert.equal(drawn.length, 0, `${drawn.length} pixels drawn for a frame with no particles`);
});

test("past the track's duration nothing is seeked, rasterized, or drawn", () => {
  const frame = 75; // 2.5 s; the seek fixture's track is 2 s long
  assert.ok(frame / FPS > config.emitterTrack.duration);
  const sim = referenceSim(config, frame);
  assert.ok(sim.particleCount() > 0, "test is vacuous: seek past the end would not have frozen any particles");
  sim.dispose();

  const png = still("PastTrackEndTest", frame);
  assert.ok(!probeShown(png, "seek"), "seek ran past the track's end");
  assert.ok(!probeShown(png, "raster"), "render() or frame() ran past the track's end");
  assert.equal(painted(png), 0, "a frozen frame was drawn past the track's end");
});

test("a canvas left from an earlier frame does not survive into an empty one", () => {
  // One burst at 0.5 s; render across the frame where its particles have
  // all died, in one tab, so the canvas that drew them is still mounted.
  const effect = withTriggers(shared.burstOnceTriggers);
  const from = 16;
  const to = 59;
  const counts = [];
  for (let f = from; f <= to; f++) {
    const sim = referenceSim(effect, f);
    counts.push(sim.particleCount());
    sim.dispose();
  }
  const firstEmpty = counts.findIndex((c, i) => c === 0 && counts.slice(i).every((n) => n === 0));
  assert.ok(firstEmpty > 0 && counts[0] > 0, `test is vacuous: particle counts ${counts.join(",")}`);

  const { frames } = range("BurstRangeTest", from, to);
  assert.ok(frames.slice(0, firstEmpty).some((png) => painted(png) > 0), "no frame before the burst died drew anything");
  frames.slice(firstEmpty).forEach((png, i) => {
    assert.equal(painted(png), 0, `frame ${from + firstEmpty + i} has no particles but shows ${painted(png)} pixels`);
  });
});

test("switching from sprite to frame mode mid-render draws the frame-mode frame", () => {
  // Frames 25-29 are sprite mode, 30-35 frame mode, in one tab, so the
  // switch frame starts out holding the sprite-mode simulation.
  const from = 25;
  const to = 35;
  const { frames, calls } = range("ModeSwitchTest", from, to);
  frames.forEach((png, i) => assert.ok(painted(png) > 0, `frame ${from + i} drew nothing`));
  assert.deepEqual(sizing(calls), [
    { method: "setBounds", args: [W, H] },
    { method: "setViewport", args: [W, H, 1] },
  ]);
  for (let f = FRAME; f <= to; f++) {
    const differing = differingFromReference(frames[f - from], f);
    assert.ok(differing <= DIFF_BUDGET, `frame ${f}: ${differing} pixels differ beyond tolerance`);
  }
});

test("sprite mode gives the simulation the composition's bounds, once", () => {
  // With W and H distinct, a swapped size shows up here.
  assert.ok(W !== H, `test is vacuous: ${W}x${H} cannot show a swap`);
  const { calls } = range("SpriteBoundsTest", FRAME, FRAME + 1);
  assert.deepEqual(sizing(calls), [{ method: "setBounds", args: [W, H] }]);
});

test("frame mode leaves the bounds to setViewport", () => {
  const { frames, calls } = range("FrameModeTest", FRAME, FRAME + 1);
  assert.ok(probeShown(frames[0], "raster"), "the frame never rasterized, so this proves nothing");
  assert.deepEqual(sizing(calls), [{ method: "setViewport", args: [W, H, 1] }]);
});

test("the sizes are the composition's, not its device pixels", () => {
  // At --scale=2 the page's devicePixelRatio is 2; the simulation works
  // in the composition's own units either way.
  for (const [id, expected] of [
    ["SpriteBoundsTest", { method: "setBounds", args: [W, H] }],
    ["FrameModeTest", { method: "setViewport", args: [W, H, 1] }],
  ]) {
    const { calls } = range(id, FRAME, FRAME, { scale: 2 });
    assert.ok(calls.length > 0 && calls.every((c) => c.dpr === 2), `${id}: test is vacuous, devicePixelRatio was ${calls.map((c) => c.dpr)}`);
    assert.deepEqual(sizing(calls), [expected], id);
  }
});

test("a rejected bounds size is passed on, warned about, and does not cancel the render", () => {
  // A host measuring a container before layout passes 0. It must reach
  // setBounds (not be mistaken for "no bounds") and must not abort the
  // render: the next layout pass brings the real size.
  const { frames, calls } = range("ZeroBoundsTest", FRAME, FRAME);
  assert.deepEqual(sizing(calls), [{ method: "setBounds", args: [0, 0] }]);
  assert.ok(probeShown(frames[0], "loaded"), "the hook never returned a simulation");
});

test("bounds ignored beside a viewport cannot keep the hook from loading", () => {
  const { frames, calls } = range("ViewportNaNBoundsTest", FRAME, FRAME);
  assert.ok(probeShown(frames[0], "loaded"), "a NaN bounds next to a viewport left the hook returning null");
  assert.deepEqual(sizing(calls), [{ method: "setViewport", args: [W, H, 1] }], "setBounds ran although a viewport was given");
});

test("a size that changes mid-render rebuilds the simulation for the new one", () => {
  // Bounds 0x0 before frame 30, 640x360 from it; a viewport 100x60, then
  // 640x360. The simulation built for the first size must be replaced,
  // once, and the hook must return the new one.
  for (const [id, method, first, second] of [
    ["GrowingBoundsTest", "setBounds", [0, 0], [640, 360]],
    ["GrowingViewportTest", "setViewport", [100, 60, 1], [640, 360, 1]],
  ]) {
    const { frames, calls } = range(id, FRAME - 2, FRAME + 2);
    assert.deepEqual(sizing(calls), [{ method, args: first }, { method, args: second }], id);
    frames.forEach((png, i) => assert.ok(probeShown(png, "loaded"), `${id}: frame ${FRAME - 2 + i} has no simulation`));
  }
});
