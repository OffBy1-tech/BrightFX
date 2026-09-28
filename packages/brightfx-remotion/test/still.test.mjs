// The component adds nothing: a Remotion still at frame 30 (t = 1.0 s) is
// compared against the wrapper's own frame at seek(1.0). Chrome stores
// canvas pixels premultiplied and unpremultiplies on PNG export, so the
// comparison is made in premultiplied space with a small tolerance.
import { test } from "node:test";
import assert from "node:assert/strict";
import { execFileSync } from "node:child_process";
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
function range(id, from, to) {
  const dir = join(out, id);
  rmSync(dir, { recursive: true, force: true });
  execFileSync(
    "npx",
    ["remotion", "render", "--config=test/remotion.config.ts", "test/src/index.ts", id, dir, "--sequence", "--image-format=png", `--frames=${from}-${to}`, "--concurrency=1", "--log=error"],
    { cwd: pkg, stdio: "inherit" },
  );
  const files = readdirSync(dir).filter((f) => f.endsWith(".png")).sort();
  assert.equal(files.length, to - from + 1, `expected ${to - from + 1} frames, got ${files.length}`);
  return files.map((f) => PNG.sync.read(readFileSync(join(dir, f))));
}

const painted = (png) => {
  let n = 0;
  for (let i = 3; i < png.data.length; i += 4) if (png.data[i] > 0) n++;
  return n;
};

// Probe markers (see test/src/Root.tsx): squares pinned to the bottom-right
// corner, revealed when the component calls the probed Simulation method.
const PROBE_BOX_W = Math.max(...Object.values(shared.probes).map((p) => p.right)) + shared.probeSize;
const inProbeBox = (x, y) => x >= W - PROBE_BOX_W && y >= H - shared.probeSize;
function probeShown(png, name) {
  const { right, color } = shared.probes[name];
  const i = ((H - shared.probeSize / 2) * W + (W - right - shared.probeSize / 2)) * 4;
  return png.data[i + 3] > 200 && color.every((c, k) => Math.abs(png.data[i + k] - c) < 40);
}

const engine = await BrightFX.init(readFileSync(join(pkg, "..", "brightfx-js", "wasm", "brightfx_wasm_bg.wasm")));

function referenceSim(effect = config, frame = FRAME) {
  const sim = engine.create(SEED);
  assert.equal(sim.setConfig(effect).ok, true);
  assert.equal(sim.setViewport(W, H, 1).ok, true);
  sim.seek(frame / FPS);
  return sim;
}

test("frame mode still equals the wrapper's frame", () => {
  const png = still("FrameModeTest");
  assert.equal(png.width, W);
  assert.equal(png.height, H);
  assert.ok(probeShown(png, "raster"), "render()/frame() never ran: the probe is not wired, so the empty-frame test proves nothing");
  const sim = referenceSim();
  sim.render();
  const expected = sim.frame().data;

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

  const frames = range("BurstRangeTest", from, to);
  assert.ok(frames.slice(0, firstEmpty).some((png) => painted(png) > 0), "no frame before the burst died drew anything");
  frames.slice(firstEmpty).forEach((png, i) => {
    assert.equal(painted(png), 0, `frame ${from + firstEmpty + i} has no particles but shows ${painted(png)} pixels`);
  });
});
