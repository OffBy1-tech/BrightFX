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

/// Applies a config, releasing the Rust-owned envelope, and fails unless it
/// was accepted.
func applyConfig(_ sim: OpaquePointer, _ json: String, _ label: String) throws {
    guard let envelopePtr = json.withCString({ bfx_set_config(sim, $0) }) else {
        throw Failure("bfx_set_config returned NULL for \(label)")
    }
    let envelope = String(cString: envelopePtr)
    bfx_string_free(envelopePtr)
    guard envelope.contains("\"ok\":true") else { throw Failure("\(label) rejected: \(envelope)") }
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
    for index in 0..<actual.count where abs(actual[index] - expected[index]) > tolerance {
        throw Failure("float \(index) drifted: got \(actual[index]), expected \(expected[index])")
    }
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
    try applyConfig(sim, configJson, "config")
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

    // The forward handle's own buffer becomes the expectation the fresh
    // seek has to reproduce -- at tolerance 0, because "bit-identical" is
    // the whole property.
    let forwardCount = bfx_particle_count(forwardSim)
    guard let forwardBase = bfx_buffer_ptr(forwardSim) else { throw Failure("bfx_buffer_ptr returned NULL") }
    let forwardFloats = Array(
        UnsafeBufferPointer(start: forwardBase, count: Int(forwardCount * forwardExpected.particleFloats)))

    guard let freshSim = bfx_simulation_new(forwardExpected.seed) else { throw Failure("bfx_simulation_new returned NULL") }
    defer { bfx_simulation_free(freshSim) }
    try applyConfig(freshSim, seekConfigJson, "fresh-seek config")
    bfx_seek(freshSim, forwardExpected.seekTimes[forwardExpected.seekTimes.count - 1])
    try assertBufferMatches(
        freshSim, count: forwardCount, stride: forwardExpected.particleFloats,
        buffer: forwardFloats, tolerance: 0)
    print("  ok  forward seeks reproduce the Rust buffer and match a fresh seek exactly")

    // --- frame protocol --------------------------------------------------------
    let frameConfigJson = try text("ffi-frame.config.json")
    let frameExpected: FrameExpectation = try decode("ffi-frame.expected.json")
    let frameBytes = [UInt8](try Data(contentsOf: fixtures.appendingPathComponent(frameExpected.rgbaFile)))
    guard frameExpected.nonzeroPixels > 500 else { throw Failure("frame fixture is vacuous") }

    guard let frameSim = bfx_simulation_new(frameExpected.seed) else { throw Failure("bfx_simulation_new returned NULL") }
    defer { bfx_simulation_free(frameSim) }
    try applyConfig(frameSim, frameConfigJson, "frame config")

    guard let viewportPtr = bfx_set_viewport(frameSim, frameExpected.width, frameExpected.height, frameExpected.scale) else {
        throw Failure("bfx_set_viewport returned NULL")
    }
    let viewportEnvelope = String(cString: viewportPtr)
    bfx_string_free(viewportPtr)
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

    // --- error handling -------------------------------------------------------
    let badPtr = "{ not json".withCString { bfx_set_config(sim, $0) }
    guard let badPtr else { throw Failure("bfx_set_config returned NULL for bad input") }
    let badEnvelope = String(cString: badPtr)
    bfx_string_free(badPtr)
    guard badEnvelope.contains("\"ok\":false") else { throw Failure("bad JSON was accepted: \(badEnvelope)") }
    print("  ok  an invalid config is rejected with a message, not a crash")

    // --- null tolerance -------------------------------------------------------
    guard bfx_particle_count(nil) == 0, bfx_buffer_ptr(nil) == nil else {
        throw Failure("null handle was not tolerated")
    }
    bfx_advance(nil, 0.016)
    bfx_seek(nil, 1.0)
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
