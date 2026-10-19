// The component adds nothing: a Remotion still at frame 30 (t = 1.0 s) is
// compared against the wrapper's own frame at seek(1.0). Chrome stores
// canvas pixels premultiplied and unpremultiplies on PNG export, so the
// comparison is made in premultiplied space with a small tolerance.
import { after, test } from "node:test";
import assert from "node:assert/strict";
import { readFileSync } from "node:fs";
import { dirname, join } from "node:path";
import { fileURLToPath } from "node:url";
import { bundle } from "@remotion/bundler";
import { getCompositions, openBrowser, renderFrames } from "@remotion/renderer";
import { PNG } from "pngjs";
import { BrightFX } from "brightfx-js";
import { webpackOverride } from "./webpack-override.mjs";

const here = dirname(fileURLToPath(import.meta.url));
const pkg = join(here, "..");
const fixtures = join(pkg, "..", "..", "core", "fixtures");
const config = JSON.parse(readFileSync(join(fixtures, "ffi-seek.config.json"), "utf8"));
const shared = JSON.parse(readFileSync(join(here, "fixtures", "probe-effects.json"), "utf8"));
const withTriggers = (triggers) => ({ ...config, emitterTrack: { ...config.emitterTrack, triggers } });
const FRAME = 30;
const FPS = 30;
const W = 200;
const H = 120;
const SEED = 42;

// The compositions are bundled once and rendered in one browser, each
// render in its own tab. Bundling in-process hands over the tab's console
// as it was logged, rather than as the CLI formats and routes it.
const serveUrl = await bundle({ entryPoint: join(here, "src", "index.ts"), rootDir: pkg, webpackOverride });
const browser = await openBrowser("chrome", { logLevel: "error" });
after(() => browser.close({ silent: true }));
const compositions = new Map(
  (await getCompositions(serveUrl, { puppeteerInstance: browser, logLevel: "error" })).map((c) => [c.id, c]),
);

// Frames `from..to` rendered in one tab, one after another, so the
// component stays mounted across them the way it does in a real render.
// `scale` is Remotion's scale, which sets the page's devicePixelRatio.
//
// Returns, in order, every setBounds/setViewport call made while rendering
// (`calls`), HookTest's record of the size of each simulation it got
// (`uses`), and the tab's whole console output (`output`). `frames` decodes
// the PNGs when first read, and only at scale 1: the probe helpers assume
// W x H. Remotion prints each of the tab's console lines itself as it
// arrives, whatever the logLevel, so a passing render still shows its
// warnings.
async function render(id, from, to = from, { scale = 1 } = {}) {
  const composition = compositions.get(id);
  assert.ok(composition, `no composition ${id}`);
  const lines = [];
  const buffers = [];
  await renderFrames({
    serveUrl,
    composition,
    puppeteerInstance: browser,
    inputProps: {},
    frameRange: [from, to],
    concurrency: 1,
    scale,
    imageFormat: "png",
    outputDir: null,
    logLevel: "error",
    onStart: () => {},
    onFrameUpdate: () => {},
    onFrameBuffer: (buffer, frame) => buffers.push({ frame, buffer }),
    onBrowserLog: ({ text }) => lines.push(text),
  });
  assert.equal(buffers.length, to - from + 1, `expected ${to - from + 1} frames, got ${buffers.length}`);
  buffers.sort((x, y) => x.frame - y.frame);

  // A tab Remotion replaces after a crash remounts the composition and
  // sizes a new simulation, which the exact-count checks would blame on
  // the hook.
  const pages = loggedJson(lines, shared.pageLogTag);
  assert.equal(pages.length, 1, `${id}: rendered in ${pages.length} page loads, so the call log is not one mount's`);
  const calls = loggedJson(lines, shared.callLogTag);
  // Every composition sizes a simulation; none logged means the probe
  // patch is not wired, and every call check would be vacuous.
  assert.ok(calls.length > 0, `${id}: no setBounds/setViewport call was logged`);
  assert.ok(
    calls.every((c) => c.dpr === scale),
    `${id}: devicePixelRatio was ${calls.map((c) => c.dpr)}, not the scale of ${scale}`,
  );
  let frames;
  return {
    calls,
    uses: loggedJson(lines, shared.useLogTag),
    output: lines.join("\n"),
    get frames() {
      assert.equal(scale, 1, `${id}: frames are decoded at scale 1 only`);
      return (frames ??= buffers.map(({ buffer }) => PNG.sync.read(buffer)));
    },
  };
}

const still = async (id, frame = FRAME) => (await render(id, frame)).frames[0];

// The JSON after `tag` on each console line that starts with it.
function loggedJson(lines, tag) {
  const marker = `${tag} `;
  return lines
    .filter((line) => line.startsWith(marker))
    .map((line) => {
      try {
        return JSON.parse(line.slice(marker.length));
      } catch (error) {
        throw new Error(`unparsable ${tag} line: ${line}`, { cause: error });
      }
    });
}

// A sizing call's arguments as compared. A setViewport that leaves out its
// scale reads as passing the default of 1; any scale that was passed,
// including a NaN (logged as the string "NaN"), is compared as it is. Null
// args (a simulation never sized) stay null.
const sizeArgs = (method, args) => (method === "setViewport" && args?.length === 2 ? [...args, 1] : args);
// The calls' method and sizes, without the devicePixelRatio.
const sizing = (calls) => calls.map(({ method, args }) => ({ method, args: sizeArgs(method, args) }));

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

test("frame mode still equals the wrapper's frame", async () => {
  const png = await still("FrameModeTest");
  assert.equal(png.width, W);
  assert.equal(png.height, H);
  assert.ok(probeShown(png, "raster"), "render()/frame() never ran: the probe is not wired, so the empty-frame test proves nothing");
  const differing = differingFromReference(png);
  assert.ok(differing <= DIFF_BUDGET, `${differing} pixels differ beyond tolerance`);
});

test("sprite mode still has a glyph at every visible particle", async () => {
  const png = await still("SpriteModeTest");
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

test("frame mode skips rasterizing a frame with no particles and draws nothing", async () => {
  const sim = referenceSim(withTriggers(shared.lateStartTriggers));
  assert.equal(sim.particleCount(), 0, "test is vacuous: the late effect has particles at the still's time");
  sim.dispose();

  const png = await still("EmptyFrameTest");
  assert.ok(probeShown(png, "seek"), "seek never ran, so no sim-backed render happened and this proves nothing");
  assert.ok(!probeShown(png, "raster"), "render() or frame() ran for a frame with no particles");
  const drawn = [...Array(W * H).keys()].filter((p) => !inProbeBox(p % W, Math.floor(p / W)) && png.data[p * 4 + 3] > 0);
  assert.equal(drawn.length, 0, `${drawn.length} pixels drawn for a frame with no particles`);
});

test("past the track's duration nothing is seeked, rasterized, or drawn", async () => {
  const frame = 75; // 2.5 s; the seek fixture's track is 2 s long
  assert.ok(frame / FPS > config.emitterTrack.duration);
  const sim = referenceSim(config, frame);
  assert.ok(sim.particleCount() > 0, "test is vacuous: seek past the end would not have frozen any particles");
  sim.dispose();

  const png = await still("PastTrackEndTest", frame);
  assert.ok(!probeShown(png, "seek"), "seek ran past the track's end");
  assert.ok(!probeShown(png, "raster"), "render() or frame() ran past the track's end");
  assert.equal(painted(png), 0, "a frozen frame was drawn past the track's end");
});

test("a canvas left from an earlier frame does not survive into an empty one", async () => {
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

  const { frames } = await render("BurstRangeTest", from, to);
  assert.ok(frames.slice(0, firstEmpty).some((png) => painted(png) > 0), "no frame before the burst died drew anything");
  frames.slice(firstEmpty).forEach((png, i) => {
    assert.equal(painted(png), 0, `frame ${from + firstEmpty + i} has no particles but shows ${painted(png)} pixels`);
  });
});

test("switching from sprite to frame mode mid-render draws the frame-mode frame", async () => {
  // Sprite mode before the switch frame, frame mode from it, in one tab,
  // so the switch frame starts out holding the sprite-mode simulation.
  const from = shared.switchFrame - 5;
  const to = shared.switchFrame + 5;
  const { frames, calls } = await render("ModeSwitchTest", from, to);
  frames.forEach((png, i) => assert.ok(painted(png) > 0, `frame ${from + i} drew nothing`));
  assert.deepEqual(sizing(calls), [
    { method: "setBounds", args: [W, H] },
    { method: "setViewport", args: [W, H, 1] },
  ]);
  for (let f = shared.switchFrame; f <= to; f++) {
    const differing = differingFromReference(frames[f - from], f);
    assert.ok(differing <= DIFF_BUDGET, `frame ${f}: ${differing} pixels differ beyond tolerance`);
  }
});

test("the mode switch sizes the same way at devicePixelRatio 2", async () => {
  // The pixel comparison above needs scale 1; this checks only the calls,
  // so a rebuilt viewport scaled to device pixels shows here.
  const { calls } = await render("ModeSwitchTest", shared.switchFrame - 1, shared.switchFrame + 1, { scale: 2 });
  assert.deepEqual(sizing(calls), [
    { method: "setBounds", args: [W, H] },
    { method: "setViewport", args: [W, H, 1] },
  ]);
});

// The initial-size checks run at scale 2, where the page's
// devicePixelRatio is 2 (render() checks that it was): the simulation works
// in the composition's own units either way, so these also catch a size
// scaled to device pixels. The size-change test does the same for the
// rebuild path.
test("sprite mode gives the simulation the composition's bounds, once", async () => {
  // With W and H distinct, a swapped size shows up here.
  assert.ok(W !== H, `test is vacuous: ${W}x${H} cannot show a swap`);
  const { calls } = await render("SpriteModeTest", FRAME, FRAME + 1, { scale: 2 });
  assert.deepEqual(sizing(calls), [{ method: "setBounds", args: [W, H] }]);
});

test("frame mode leaves the bounds to setViewport", async () => {
  // That this composition rasterizes is the first test's check.
  const { calls } = await render("FrameModeTest", FRAME, FRAME + 1, { scale: 2 });
  assert.deepEqual(sizing(calls), [{ method: "setViewport", args: [W, H, 1] }]);
});

test("a rejected bounds size is passed on, warned about, and does not cancel the render", async () => {
  // A host measuring a container before layout passes 0. It must reach
  // setBounds (not be mistaken for "no bounds") and must not abort the
  // render: the next layout pass brings the real size.
  const { calls, uses, output } = await render("ZeroBoundsTest", FRAME, FRAME + 1);
  assert.deepEqual(sizing(calls), [{ method: "setBounds", args: [0, 0] }]);
  assert.match(output, /BrightFX bounds rejected/, "the rejected size was not warned about");
  assert.ok(uses.length > 0, "the hook never returned a simulation");
});

test("NaN bounds without a viewport reach setBounds as NaN and do not keep the hook from loading", async () => {
  const { calls, uses, output } = await render("NaNBoundsTest", FRAME, FRAME + 1);
  assert.deepEqual(sizing(calls), [{ method: "setBounds", args: ["NaN", "NaN"] }]);
  assert.match(output, /BrightFX bounds rejected/, "the rejected size was not warned about");
  assert.ok(uses.length > 0, "a NaN bounds left the hook returning null");
  assert.ok(
    uses.every((u) => u.args[0] === "NaN" && u.args[1] === "NaN"),
    `the hook returned a simulation sized ${JSON.stringify(uses.map((u) => u.args))}`,
  );
});

test("bounds ignored beside a viewport cannot keep the hook from loading", async () => {
  const { calls, uses } = await render("ViewportNaNBoundsTest", FRAME, FRAME + 1);
  assert.ok(uses.length > 0, "a NaN bounds next to a viewport left the hook returning null");
  assert.deepEqual(sizing(calls), [{ method: "setViewport", args: [W, H, 1] }]);
});

test("a size that changes mid-render rebuilds the simulation for the new one", async (t) => {
  // Bounds 0x0 before the switch frame, 640x360 from it; a viewport 100x60,
  // then 640x360. The simulation built for the first size must be
  // replaced, once, and from the switch frame on the hook must return only
  // the new one. At scale 2, so the rebuild is also checked against
  // device pixels.
  const { switchFrame } = shared;
  for (const [id, method, first, second] of [
    ["GrowingBoundsTest", "setBounds", [0, 0], [640, 360]],
    ["GrowingViewportTest", "setViewport", [100, 60, 1], [640, 360, 1]],
  ]) {
    await t.test(id, async () => {
      const { calls, uses } = await render(id, switchFrame - 2, switchFrame + 2, { scale: 2 });
      assert.deepEqual(sizing(calls), [{ method, args: first }, { method, args: second }]);
      for (let f = switchFrame - 2; f <= switchFrame + 2; f++) {
        const got = uses.filter((u) => u.frame === f).map((u) => sizeArgs(method, u.args));
        assert.ok(got.length > 0, `frame ${f} has no simulation`);
        assert.deepEqual(got, got.map(() => (f < switchFrame ? first : second)), `frame ${f} used a simulation of the wrong size`);
      }
    });
  }
});
