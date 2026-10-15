// Swift smoke harness for the BrightFX C ABI.
// Replays the shared driving protocol and compares against the Rust fixture.

import CBrightFX
import Foundation

struct EmitterFrame: Decodable {
    let x: Float
    let y: Float
    let vx: Float
    let vy: Float
}

struct Expectation: Decodable {
    let seed: UInt64
    let frames: Int
    let dt: Float
    let burstFrame: Int
    let emitterFrames: [EmitterFrame]
    let particleFloats: UInt32
    let tolerance: Float
    let particleCount: UInt32
    let buffer: [Float]
}

struct SeekExpectation: Decodable {
    let seed: UInt64
    let seekTimes: [Float]
    let particleFloats: UInt32
    let tolerance: Float
    let particleCount: UInt32
    let buffer: [Float]
    /// Only the pre-roll fixture records one: where its forward check seeks.
    let forwardTime: Float?
}

struct FrameExpectation: Decodable {
    let seed: UInt64
    let frames: Int
    let dt: Float
    let burstFrame: Int
    let emitterFrames: [EmitterFrame]
    let width: UInt32
    let height: UInt32
    let scale: Float
    let channelTolerance: Int
    let maxDifferingPixels: Int
    let nonzeroPixels: Int
    let rgbaFile: String
}

/// A failed check. Thrown rather than calling C's `exit(1)` at the point of
/// failure so that the `defer` blocks in `run()` release every simulation on
/// the way out: `exit` does not unwind Swift's defer stack, and a harness
/// that leaks on failure would be a poor template for anything longer-lived.
struct Failure: Error, CustomStringConvertible {
    let message: String
    init(_ message: String) { self.message = message }
    var description: String { message }
}

/// Copies an envelope out and releases it; Rust owns the buffer, so only
/// bfx_string_free may free it.
func takeEnvelope(_ envelopePtr: UnsafeMutablePointer<CChar>?, _ function: String) throws -> String {
    guard let envelopePtr else { throw Failure("\(function) returned NULL") }
    defer { bfx_string_free(envelopePtr) }
    return String(cString: envelopePtr)
}

/// Applies a config and fails unless it was accepted.
@discardableResult
func applyConfig(_ sim: OpaquePointer, _ json: String, _ label: String) throws -> String {
    let envelope = try takeEnvelope(json.withCString({ bfx_set_config(sim, $0) }), "bfx_set_config (\(label))")
    guard envelope.contains("\"ok\":true") else { throw Failure("\(label) rejected: \(envelope)") }
    return envelope
}

func setBounds(_ sim: OpaquePointer?, _ width: Float, _ height: Float) throws -> String {
    try takeEnvelope(bfx_set_bounds(sim, width, height), "bfx_set_bounds")
}

/// Sets the bounds and fails unless they were accepted.
func applyBounds(_ sim: OpaquePointer, _ width: Float, _ height: Float) throws {
    let envelope = try setBounds(sim, width, height)
    guard envelope.contains("\"ok\":true") else { throw Failure("bounds \(width) x \(height) rejected: \(envelope)") }
}

/// Replays the emitter states the fixture recorded, one advance per frame,
/// with the burst on the recorded frame.
func drive(_ sim: OpaquePointer, frames: [EmitterFrame], burstFrame: Int, dt: Float) {
    for (frame, e) in frames.enumerated() {
        bfx_set_emitter(sim, e.x, e.y, e.vx, e.vy, true)
        if frame == burstFrame { bfx_trigger_burst(sim) }
        bfx_advance(sim, dt)
    }
}

/// Compares the live particle buffer against a recorded one within tolerance.
/// A non-finite float on either side fails outright: `abs(a - b) >
/// tolerance` is false whenever either operand is NaN. At tolerance 0 the
/// property is "bit-identical", so the bit patterns are compared -- that
/// is the only way -0.0 and +0.0 differ.
func assertBufferMatches(
    _ sim: OpaquePointer, count expectedCount: UInt32, stride: UInt32, buffer expected: [Float], tolerance: Float
) throws {
    let count = bfx_particle_count(sim)
    guard count == expectedCount else {
        throw Failure("particle count diverged: \(count) vs \(expectedCount)")
    }
    guard let base = bfx_buffer_ptr(sim) else { throw Failure("bfx_buffer_ptr returned NULL") }
    let actual = UnsafeBufferPointer(start: base, count: Int(count * stride))
    guard actual.count == expected.count else {
        throw Failure("buffer length diverged: \(actual.count) vs \(expected.count)")
    }
    for index in 0..<actual.count {
        let (got, wanted) = (actual[index], expected[index])
        guard got.isFinite, wanted.isFinite else {
            throw Failure("float \(index) is not finite: got \(got), expected \(wanted)")
        }
        let differs = tolerance == 0 ? got.bitPattern != wanted.bitPattern : abs(got - wanted) > tolerance
        if differs { throw Failure("float \(index) drifted: got \(got), expected \(wanted)") }
    }
}

/// `sim` has been seeked forward to `time`; a fresh handle seeked straight
/// there must hold the same buffer -- at tolerance 0, because
/// "bit-identical" is the whole property. An empty pool would compare
/// vacuously.
func assertMatchesFreshSeek(
    _ sim: OpaquePointer, configJson: String, seed: UInt64, stride: UInt32, time: Float
) throws {
    let count = bfx_particle_count(sim)
    guard count > 0 else { throw Failure("pool is empty at \(time): the comparison would be vacuous") }
    guard let base = bfx_buffer_ptr(sim) else { throw Failure("bfx_buffer_ptr returned NULL") }
    let floats = Array(UnsafeBufferPointer(start: base, count: Int(count * stride)))

    guard let fresh = bfx_simulation_new(seed) else { throw Failure("bfx_simulation_new returned NULL") }
    defer { bfx_simulation_free(fresh) }
    try applyConfig(fresh, configJson, "fresh-seek config")
    bfx_seek(fresh, time)
    try assertBufferMatches(fresh, count: count, stride: stride, buffer: floats, tolerance: 0)
}

func run() throws {
    let fixtures = URL(fileURLWithPath: #filePath)
        .deletingLastPathComponent()
        .deletingLastPathComponent()
        .deletingLastPathComponent()
        .appendingPathComponent("fixtures")
    func text(_ name: String) throws -> String {
        try String(contentsOf: fixtures.appendingPathComponent(name), encoding: .utf8)
    }
    func decode<T: Decodable>(_ name: String) throws -> T {
        try JSONDecoder().decode(T.self, from: Data(contentsOf: fixtures.appendingPathComponent(name)))
    }

    let configJson = try text("ffi-smoke.config.json")
    let expected: Expectation = try decode("ffi-smoke.expected.json")

    // --- stride ---------------------------------------------------------------
    guard bfx_particle_floats() == expected.particleFloats else {
        throw Failure("stride mismatch: \(bfx_particle_floats()) vs \(expected.particleFloats)")
    }
    print("  ok  stride matches the Rust constant")

    // --- fixture sanity ---------------------------------------------------------
    guard expected.particleCount > 0 else {
        throw Failure("fixture is vacuous — no particles to compare")
    }
    guard expected.emitterFrames.count == expected.frames else {
        throw Failure("emitter frames do not cover every frame")
    }
    print("  ok  fixture is not vacuous")

    // --- driving protocol -----------------------------------------------------
    guard let sim = bfx_simulation_new(expected.seed) else { throw Failure("bfx_simulation_new returned NULL") }
    defer { bfx_simulation_free(sim) }
    let smokeEnvelope = try applyConfig(sim, configJson, "config")
    guard smokeEnvelope.contains("particle pool") else {
        throw Failure("the smoke config's worst case is over the pool, but set_config did not warn: \(smokeEnvelope)")
    }
    drive(sim, frames: expected.emitterFrames, burstFrame: expected.burstFrame, dt: expected.dt)
    try assertBufferMatches(
        sim, count: expected.particleCount, stride: expected.particleFloats,
        buffer: expected.buffer, tolerance: expected.tolerance)
    print("  ok  the driving protocol reproduces the Rust buffer")

    // --- seek protocol ----------------------------------------------------------
    let seekConfigJson = try text("ffi-seek.config.json")
    let seekExpected: SeekExpectation = try decode("ffi-seek.expected.json")
    guard seekExpected.particleCount > 0, !seekExpected.seekTimes.isEmpty else {
        throw Failure("seek fixture is vacuous")
    }
    print("  ok  the seek fixture is not vacuous")

    guard let seekSim = bfx_simulation_new(seekExpected.seed) else { throw Failure("bfx_simulation_new returned NULL") }
    defer { bfx_simulation_free(seekSim) }
    try applyConfig(seekSim, seekConfigJson, "seek config")
    for time in seekExpected.seekTimes { bfx_seek(seekSim, time) }
    try assertBufferMatches(
        seekSim, count: seekExpected.particleCount, stride: seekExpected.particleFloats,
        buffer: seekExpected.buffer, tolerance: seekExpected.tolerance)
    print("  ok  seek reproduces the Rust buffer from a baked track")

    // --- forward-seek protocol -------------------------------------------------
    let forwardExpected: SeekExpectation = try decode("ffi-seek-forward.expected.json")
    guard forwardExpected.particleCount > 0 else { throw Failure("forward-seek fixture is vacuous") }

    guard let forwardSim = bfx_simulation_new(forwardExpected.seed) else { throw Failure("bfx_simulation_new returned NULL") }
    defer { bfx_simulation_free(forwardSim) }
    try applyConfig(forwardSim, seekConfigJson, "forward-seek config")
    for time in forwardExpected.seekTimes { bfx_seek(forwardSim, time) }
    try assertBufferMatches(
        forwardSim, count: forwardExpected.particleCount, stride: forwardExpected.particleFloats,
        buffer: forwardExpected.buffer, tolerance: forwardExpected.tolerance)

    try assertMatchesFreshSeek(
        forwardSim, configJson: seekConfigJson, seed: forwardExpected.seed,
        stride: forwardExpected.particleFloats, time: forwardExpected.seekTimes[forwardExpected.seekTimes.count - 1])
    print("  ok  forward seeks reproduce the Rust buffer and match a fresh seek exactly")

    // --- pre-roll protocol -----------------------------------------------------
    let prerollConfigJson = try text("ffi-preroll.config.json")
    let prerollExpected: SeekExpectation = try decode("ffi-preroll.expected.json")
    guard prerollExpected.particleCount > 0, prerollExpected.seekTimes.count == 1, prerollExpected.seekTimes[0] == 0,
        let prerollForward = prerollExpected.forwardTime
    else {
        throw Failure("pre-roll fixture is vacuous, does not record seek(0), or has no forwardTime")
    }

    guard let prerollSim = bfx_simulation_new(prerollExpected.seed) else { throw Failure("bfx_simulation_new returned NULL") }
    defer { bfx_simulation_free(prerollSim) }
    try applyConfig(prerollSim, prerollConfigJson, "pre-roll config")
    for time in prerollExpected.seekTimes { bfx_seek(prerollSim, time) }
    try assertBufferMatches(
        prerollSim, count: prerollExpected.particleCount, stride: prerollExpected.particleFloats,
        buffer: prerollExpected.buffer, tolerance: prerollExpected.tolerance)

    // Leaving the pre-roll is a forward seek: bit-identical to a fresh one.
    bfx_seek(prerollSim, prerollForward)
    try assertMatchesFreshSeek(
        prerollSim, configJson: prerollConfigJson, seed: prerollExpected.seed,
        stride: prerollExpected.particleFloats, time: prerollForward)
    print("  ok  a pre-rolled track is already running at t=0 and steps on bit-identically")

    // --- frame protocol --------------------------------------------------------
    let frameConfigJson = try text("ffi-frame.config.json")
    let frameExpected: FrameExpectation = try decode("ffi-frame.expected.json")
    let frameBytes = [UInt8](try Data(contentsOf: fixtures.appendingPathComponent(frameExpected.rgbaFile)))
    guard frameExpected.nonzeroPixels > 500 else { throw Failure("frame fixture is vacuous") }

    guard let frameSim = bfx_simulation_new(frameExpected.seed) else { throw Failure("bfx_simulation_new returned NULL") }
    defer { bfx_simulation_free(frameSim) }
    try applyConfig(frameSim, frameConfigJson, "frame config")

    let viewportEnvelope = try takeEnvelope(
        bfx_set_viewport(frameSim, frameExpected.width, frameExpected.height, frameExpected.scale), "bfx_set_viewport")
    guard viewportEnvelope.contains("\"ok\":true") else { throw Failure("viewport rejected: \(viewportEnvelope)") }

    drive(frameSim, frames: frameExpected.emitterFrames, burstFrame: frameExpected.burstFrame, dt: frameExpected.dt)
    bfx_render(frameSim)

    guard bfx_frame_width(frameSim) == frameExpected.width, bfx_frame_height(frameSim) == frameExpected.height else {
        throw Failure("frame dimensions diverged")
    }
    guard let frameBase = bfx_frame_ptr(frameSim) else { throw Failure("bfx_frame_ptr returned NULL") }
    let frameLen = Int(bfx_frame_len(frameSim))
    guard frameLen == frameBytes.count else { throw Failure("frame length diverged: \(frameLen) vs \(frameBytes.count)") }
    let actualFrame = UnsafeBufferPointer(start: frameBase, count: frameLen)

    var differing = 0
    for px in stride(from: 0, to: frameLen, by: 4) {
        for c in 0..<4 where abs(Int(actualFrame[px + c]) - Int(frameBytes[px + c])) > frameExpected.channelTolerance {
            differing += 1
            break
        }
    }
    guard differing <= frameExpected.maxDifferingPixels else {
        throw Failure("\(differing) pixels drifted beyond \(frameExpected.channelTolerance) per channel")
    }
    print("  ok  the render protocol reproduces the Rust frame")

    // --- culling and warnings ---------------------------------------------------
    // No viewport here (sprite mode): bfx_set_bounds is the only way to give
    // cullMargin a frame. The config also overflows the particle pool, which
    // set_config reports as a warning without rejecting it.
    let cullConfigJson = try text("ffi-cull.config.json")
    guard let cullSim = bfx_simulation_new(7) else { throw Failure("bfx_simulation_new returned NULL") }
    defer { bfx_simulation_free(cullSim) }
    let cullEnvelope = try applyConfig(cullSim, cullConfigJson, "cull config")
    guard cullEnvelope.contains("particle pool") else {
        throw Failure("a config that overflows the pool carried no warning: \(cullEnvelope)")
    }
    print("  ok  set_config warns about a pool overflow and still accepts the config")

    try applyBounds(cullSim, 200, 200)
    bfx_set_emitter(cullSim, 50, 50, 0, 0, true)
    for _ in 0..<5 { bfx_advance(cullSim, expected.dt) }
    guard bfx_particle_count(cullSim) > 0 else { throw Failure("cull check is vacuous: nothing spawned") }
    // A size measured before layout (0, or NaN from an undefined) is
    // rejected and keeps the frame, rather than turning culling off.
    let liveBeforeZero = bfx_particle_count(cullSim)
    let zeroEnvelope = try setBounds(cullSim, 0, 200)
    guard zeroEnvelope.contains("\"ok\":false"), bfx_particle_count(cullSim) == liveBeforeZero else {
        throw Failure("a zero size was not rejected cleanly: \(zeroEnvelope)")
    }
    print("  ok  bfx_set_bounds rejects a zero size with a message")
    try applyBounds(cullSim, 1, 1)
    guard bfx_particle_count(cullSim) == 0 else {
        throw Failure("bfx_set_bounds did not cull the live particles: \(bfx_particle_count(cullSim)) left")
    }
    print("  ok  bfx_set_bounds culls particles outside the new frame")

    // set_config re-culls too. A 1000 margin around a 1 x 1 frame holds the
    // particles (so this also shows bfx_set_bounds took effect: without
    // bounds the later cull could not happen); margin 0 then drops them at once.
    guard let reCullSim = bfx_simulation_new(7) else { throw Failure("bfx_simulation_new returned NULL") }
    defer { bfx_simulation_free(reCullSim) }
    try applyConfig(reCullSim, cullConfigJson.replacingOccurrences(of: "\"cullMargin\": 0.0", with: "\"cullMargin\": 1000.0"), "wide-margin config")
    try applyBounds(reCullSim, 1, 1)
    bfx_set_emitter(reCullSim, 50, 50, 0, 0, true)
    for _ in 0..<5 { bfx_advance(reCullSim, expected.dt) }
    guard bfx_particle_count(reCullSim) > 0 else { throw Failure("set_config cull check is vacuous: nothing held") }
    try applyConfig(reCullSim, cullConfigJson, "narrow-margin config")
    guard bfx_particle_count(reCullSim) == 0 else {
        throw Failure("set_config did not cull the live particles: \(bfx_particle_count(reCullSim)) left")
    }
    print("  ok  set_config with a smaller cullMargin culls at once")

    // --- error handling -------------------------------------------------------
    let badEnvelope = try takeEnvelope("{ not json".withCString { bfx_set_config(sim, $0) }, "bfx_set_config (bad input)")
    guard badEnvelope.contains("\"ok\":false") else { throw Failure("bad JSON was accepted: \(badEnvelope)") }
    print("  ok  an invalid config is rejected with a message, not a crash")

    // --- null tolerance -------------------------------------------------------
    guard bfx_particle_count(nil) == 0, bfx_buffer_ptr(nil) == nil else {
        throw Failure("null handle was not tolerated")
    }
    bfx_advance(nil, 0.016)
    bfx_seek(nil, 1.0)
    guard try setBounds(nil, 100, 100).contains("\"ok\":false") else { throw Failure("bfx_set_bounds accepted a null handle") }
    guard bfx_frame_len(nil) == 0, bfx_frame_ptr(nil) == nil else { throw Failure("null handle was not tolerated by frame accessors") }
    bfx_render(nil)
    bfx_simulation_free(nil)
    bfx_string_free(nil)
    print("  ok  null handles are tolerated")

    print("swift harness: all checks passed")
}

do {
    try run()
} catch {
    // A `Failure` prints its message; a fixture I/O or decoding error prints
    // Foundation's description of it.
    FileHandle.standardError.write(Data("swift harness FAILED: \(error)\n".utf8))
    exit(1)
}
