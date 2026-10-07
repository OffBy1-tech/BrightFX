// C# smoke harness for the BrightFX C ABI.
// Replays the shared driving protocol and compares against the Rust fixture.
//
// The library name is resolved without an extension so the same P/Invoke
// declarations work against libbrightfx_ffi.dylib here and brightfx_ffi.dll
// on Windows.

using System.Diagnostics.CodeAnalysis;
using System.Runtime.InteropServices;
using System.Text.Json;
using System.Text.Json.Serialization;

internal static class Native
{
    private const string Lib = "brightfx_ffi";

    [DllImport(Lib, CallingConvention = CallingConvention.Cdecl)] internal static extern IntPtr bfx_simulation_new(ulong seed);
    [DllImport(Lib, CallingConvention = CallingConvention.Cdecl)] internal static extern void bfx_simulation_free(IntPtr sim);
    [DllImport(Lib, CallingConvention = CallingConvention.Cdecl)] internal static extern IntPtr bfx_set_config(IntPtr sim, IntPtr json);
    [DllImport(Lib, CallingConvention = CallingConvention.Cdecl)] internal static extern void bfx_string_free(IntPtr text);
    [DllImport(Lib, CallingConvention = CallingConvention.Cdecl)] internal static extern void bfx_set_emitter(
        IntPtr sim, float x, float y, float vx, float vy,
        [MarshalAs(UnmanagedType.I1)] bool active);
    [DllImport(Lib, CallingConvention = CallingConvention.Cdecl)] internal static extern void bfx_set_bounds(IntPtr sim, float width, float height);
    [DllImport(Lib, CallingConvention = CallingConvention.Cdecl)] internal static extern void bfx_trigger_burst(IntPtr sim);
    [DllImport(Lib, CallingConvention = CallingConvention.Cdecl)] internal static extern void bfx_advance(IntPtr sim, float dt);
    [DllImport(Lib, CallingConvention = CallingConvention.Cdecl)] internal static extern void bfx_seek(IntPtr sim, float time);
    [DllImport(Lib, CallingConvention = CallingConvention.Cdecl)] internal static extern IntPtr bfx_buffer_ptr(IntPtr sim);
    [DllImport(Lib, CallingConvention = CallingConvention.Cdecl)] internal static extern uint bfx_particle_count(IntPtr sim);
    [DllImport(Lib, CallingConvention = CallingConvention.Cdecl)] internal static extern uint bfx_particle_floats();
    [DllImport(Lib, CallingConvention = CallingConvention.Cdecl)] internal static extern IntPtr bfx_set_viewport(IntPtr sim, uint width, uint height, float scale);
    [DllImport(Lib, CallingConvention = CallingConvention.Cdecl)] internal static extern void bfx_render(IntPtr sim);
    [DllImport(Lib, CallingConvention = CallingConvention.Cdecl)] internal static extern IntPtr bfx_frame_ptr(IntPtr sim);
    [DllImport(Lib, CallingConvention = CallingConvention.Cdecl)] internal static extern uint bfx_frame_len(IntPtr sim);
    [DllImport(Lib, CallingConvention = CallingConvention.Cdecl)] internal static extern uint bfx_frame_width(IntPtr sim);
    [DllImport(Lib, CallingConvention = CallingConvention.Cdecl)] internal static extern uint bfx_frame_height(IntPtr sim);

    internal static string SetViewport(IntPtr sim, uint width, uint height, float scale)
    {
        IntPtr result = bfx_set_viewport(sim, width, height, scale);
        if (result == IntPtr.Zero) throw new Exception("bfx_set_viewport returned NULL");
        try { return Marshal.PtrToStringUTF8(result) ?? ""; }
        finally { bfx_string_free(result); }
    }

    /// Calls bfx_set_config with a UTF-8 string and returns the envelope,
    /// releasing the Rust-owned string. Rust owns the returned buffer, so it
    /// must be freed with bfx_string_free rather than Marshal.FreeHGlobal.
    internal static string SetConfig(IntPtr sim, string json)
    {
        IntPtr utf8 = Marshal.StringToCoTaskMemUTF8(json);
        try
        {
            IntPtr result = bfx_set_config(sim, utf8);
            if (result == IntPtr.Zero) throw new Exception("bfx_set_config returned NULL");
            try { return Marshal.PtrToStringUTF8(result) ?? ""; }
            finally { bfx_string_free(result); }
        }
        finally { Marshal.FreeCoTaskMem(utf8); }
    }

    /// A span over Rust-owned memory, not a copy: the same zero-copy read
    /// that Node's Float32Array view and Swift's UnsafeBufferPointer perform.
    /// Valid only as long as the Rust side says the pointer is — for the
    /// particle buffer, until the next advance/seek/trigger_burst; for the
    /// frame, until the next render/set_viewport.
    internal static unsafe ReadOnlySpan<T> View<T>(IntPtr ptr, int count) where T : unmanaged
        => new((T*)ptr, count);
}

/// A failed check. Thrown rather than calling Environment.Exit at the point
/// of failure so that every `using var` simulation is freed on the way out;
/// Exit does not run finally blocks or disposers. Same reasoning as the Swift
/// harness's thrown `Failure`.
internal sealed class HarnessFailure(string message) : Exception(message);

/// Owns a simulation for a `using` scope, so it is freed on every exit path.
/// The C# twin of the Swift harness's `defer { bfx_simulation_free(sim) }`.
internal sealed class Simulation : IDisposable
{
    internal IntPtr Ptr { get; }

    internal Simulation(ulong seed)
    {
        Ptr = Native.bfx_simulation_new(seed);
        if (Ptr == IntPtr.Zero) throw new HarnessFailure("bfx_simulation_new returned NULL");
    }

    public void Dispose() => Native.bfx_simulation_free(Ptr);
}

internal sealed class EmitterFrame
{
    [JsonPropertyName("x")] public float X { get; set; }
    [JsonPropertyName("y")] public float Y { get; set; }
    [JsonPropertyName("vx")] public float Vx { get; set; }
    [JsonPropertyName("vy")] public float Vy { get; set; }
}

internal sealed class Expectation
{
    [JsonPropertyName("seed")] public ulong Seed { get; set; }
    [JsonPropertyName("frames")] public int Frames { get; set; }
    [JsonPropertyName("dt")] public float Dt { get; set; }
    [JsonPropertyName("burstFrame")] public int BurstFrame { get; set; }
    [JsonPropertyName("emitterFrames")] public EmitterFrame[] EmitterFrames { get; set; } = [];
    [JsonPropertyName("particleFloats")] public uint ParticleFloats { get; set; }
    [JsonPropertyName("tolerance")] public float Tolerance { get; set; }
    [JsonPropertyName("particleCount")] public uint ParticleCount { get; set; }
    [JsonPropertyName("buffer")] public float[] Buffer { get; set; } = [];
}

internal sealed class SeekExpectation
{
    [JsonPropertyName("seed")] public ulong Seed { get; set; }
    [JsonPropertyName("seekTimes")] public float[] SeekTimes { get; set; } = [];
    [JsonPropertyName("particleFloats")] public uint ParticleFloats { get; set; }
    [JsonPropertyName("tolerance")] public float Tolerance { get; set; }
    [JsonPropertyName("particleCount")] public uint ParticleCount { get; set; }
    [JsonPropertyName("buffer")] public float[] Buffer { get; set; } = [];
}

internal sealed class FrameExpectation
{
    [JsonPropertyName("seed")] public ulong Seed { get; set; }
    [JsonPropertyName("frames")] public int Frames { get; set; }
    [JsonPropertyName("dt")] public float Dt { get; set; }
    [JsonPropertyName("burstFrame")] public int BurstFrame { get; set; }
    [JsonPropertyName("emitterFrames")] public EmitterFrame[] EmitterFrames { get; set; } = [];
    [JsonPropertyName("width")] public uint Width { get; set; }
    [JsonPropertyName("height")] public uint Height { get; set; }
    [JsonPropertyName("scale")] public float Scale { get; set; }
    [JsonPropertyName("channelTolerance")] public int ChannelTolerance { get; set; }
    [JsonPropertyName("maxDifferingPixels")] public int MaxDifferingPixels { get; set; }
    [JsonPropertyName("nonzeroPixels")] public int NonzeroPixels { get; set; }
    [JsonPropertyName("rgbaFile")] public string RgbaFile { get; set; } = "";
}

internal static class Program
{
    [DoesNotReturn]
    private static void Fail(string message) => throw new HarnessFailure(message);

    /// Replays the emitter states the fixture recorded, one advance per
    /// frame, with the burst on the recorded frame.
    private static void Drive(IntPtr sim, EmitterFrame[] frames, int burstFrame, float dt)
    {
        for (int frame = 0; frame < frames.Length; frame++)
        {
            EmitterFrame e = frames[frame];
            Native.bfx_set_emitter(sim, e.X, e.Y, e.Vx, e.Vy, true);
            if (frame == burstFrame) Native.bfx_trigger_burst(sim);
            Native.bfx_advance(sim, dt);
        }
    }

    /// Applies a config and fails unless it was accepted.
    private static string ApplyConfig(IntPtr sim, string json, string label)
    {
        string envelope = Native.SetConfig(sim, json);
        if (!envelope.Contains("\"ok\":true")) Fail($"{label} rejected: {envelope}");
        return envelope;
    }

    /// Compares the live particle buffer against a recorded one within tolerance.
    /// A non-finite float on either side fails outright: `Math.Abs(a - b) >
    /// tolerance` is false whenever either operand is NaN. At tolerance 0 the
    /// property is "bit-identical", so the bit patterns are compared -- that
    /// is the only way -0.0 and +0.0 differ.
    private static void AssertBufferMatches(IntPtr sim, uint expectedCount, uint stride, float[] expected, float tolerance)
    {
        uint count = Native.bfx_particle_count(sim);
        if (count != expectedCount) Fail($"particle count diverged: {count} vs {expectedCount}");

        IntPtr basePtr = Native.bfx_buffer_ptr(sim);
        if (basePtr == IntPtr.Zero) Fail("bfx_buffer_ptr returned NULL");
        ReadOnlySpan<float> actual = Native.View<float>(basePtr, (int)(count * stride));

        if (actual.Length != expected.Length) Fail($"buffer length diverged: {actual.Length} vs {expected.Length}");
        for (int index = 0; index < actual.Length; index++)
        {
            float got = actual[index], wanted = expected[index];
            if (!float.IsFinite(got) || !float.IsFinite(wanted))
                Fail($"float {index} is not finite: got {got}, expected {wanted}");
            bool differs = tolerance == 0f
                ? BitConverter.SingleToInt32Bits(got) != BitConverter.SingleToInt32Bits(wanted)
                : Math.Abs(got - wanted) > tolerance;
            if (differs) Fail($"float {index} drifted: got {got}, expected {wanted}");
        }
    }

    private static int Main()
    {
        try
        {
            Run();
            return 0;
        }
        catch (HarnessFailure failure)
        {
            Console.Error.WriteLine($"csharp harness FAILED: {failure.Message}");
            return 1;
        }
    }

    private static void Run()
    {
        string fixtures = Path.GetFullPath(
            Path.Combine(AppContext.BaseDirectory, "fixtures"));
        string configJson = File.ReadAllText(Path.Combine(fixtures, "ffi-smoke.config.json"));
        Expectation expected = JsonSerializer.Deserialize<Expectation>(
            File.ReadAllText(Path.Combine(fixtures, "ffi-smoke.expected.json")))!;

        if (Native.bfx_particle_floats() != expected.ParticleFloats)
            Fail($"stride mismatch: {Native.bfx_particle_floats()} vs {expected.ParticleFloats}");
        Console.WriteLine("  ok  stride matches the Rust constant");

        if (expected.ParticleCount == 0) Fail("fixture is vacuous — no particles to compare");
        if (expected.EmitterFrames.Length != expected.Frames) Fail("emitter frames do not cover every frame");
        Console.WriteLine("  ok  fixture is not vacuous");

        using var sim = new Simulation(expected.Seed);

        string smokeEnvelope = ApplyConfig(sim.Ptr, configJson, "config");
        if (!smokeEnvelope.Contains("particle pool"))
            Fail($"the smoke config's worst case is over the pool, but set_config did not warn: {smokeEnvelope}");

        Drive(sim.Ptr, expected.EmitterFrames, expected.BurstFrame, expected.Dt);
        AssertBufferMatches(sim.Ptr, expected.ParticleCount, expected.ParticleFloats, expected.Buffer, expected.Tolerance);
        Console.WriteLine("  ok  the driving protocol reproduces the Rust buffer");

        string seekConfigJson = File.ReadAllText(Path.Combine(fixtures, "ffi-seek.config.json"));
        SeekExpectation seekExpected = JsonSerializer.Deserialize<SeekExpectation>(
            File.ReadAllText(Path.Combine(fixtures, "ffi-seek.expected.json")))!;
        if (seekExpected.ParticleCount == 0 || seekExpected.SeekTimes.Length == 0) Fail("seek fixture is vacuous");
        Console.WriteLine("  ok  the seek fixture is not vacuous");

        using var seekSim = new Simulation(seekExpected.Seed);
        ApplyConfig(seekSim.Ptr, seekConfigJson, "seek config");
        foreach (float time in seekExpected.SeekTimes) Native.bfx_seek(seekSim.Ptr, time);
        AssertBufferMatches(seekSim.Ptr, seekExpected.ParticleCount, seekExpected.ParticleFloats, seekExpected.Buffer, seekExpected.Tolerance);
        Console.WriteLine("  ok  seek reproduces the Rust buffer from a baked track");

        SeekExpectation forwardExpected = JsonSerializer.Deserialize<SeekExpectation>(
            File.ReadAllText(Path.Combine(fixtures, "ffi-seek-forward.expected.json")))!;
        if (forwardExpected.ParticleCount == 0) Fail("forward-seek fixture is vacuous");

        using var forwardSim = new Simulation(forwardExpected.Seed);
        ApplyConfig(forwardSim.Ptr, seekConfigJson, "forward-seek config");
        foreach (float time in forwardExpected.SeekTimes) Native.bfx_seek(forwardSim.Ptr, time);
        AssertBufferMatches(forwardSim.Ptr, forwardExpected.ParticleCount, forwardExpected.ParticleFloats, forwardExpected.Buffer, forwardExpected.Tolerance);

        // The forward handle's own buffer becomes the expectation the fresh
        // seek has to reproduce -- at tolerance 0, because "bit-identical"
        // is the whole property.
        uint forwardCount = Native.bfx_particle_count(forwardSim.Ptr);
        IntPtr forwardBase = Native.bfx_buffer_ptr(forwardSim.Ptr);
        if (forwardBase == IntPtr.Zero) Fail("bfx_buffer_ptr returned NULL");
        float[] forwardFloats =
            Native.View<float>(forwardBase, (int)(forwardCount * forwardExpected.ParticleFloats)).ToArray();

        using var freshSim = new Simulation(forwardExpected.Seed);
        ApplyConfig(freshSim.Ptr, seekConfigJson, "fresh-seek config");
        Native.bfx_seek(freshSim.Ptr, forwardExpected.SeekTimes[^1]);
        AssertBufferMatches(freshSim.Ptr, forwardCount, forwardExpected.ParticleFloats, forwardFloats, 0f);
        Console.WriteLine("  ok  forward seeks reproduce the Rust buffer and match a fresh seek exactly");

        string frameConfigJson = File.ReadAllText(Path.Combine(fixtures, "ffi-frame.config.json"));
        FrameExpectation frameExpected = JsonSerializer.Deserialize<FrameExpectation>(
            File.ReadAllText(Path.Combine(fixtures, "ffi-frame.expected.json")))!;
        byte[] frameBytes = File.ReadAllBytes(Path.Combine(fixtures, frameExpected.RgbaFile));
        if (frameExpected.NonzeroPixels <= 500) Fail("frame fixture is vacuous");

        using var frameSim = new Simulation(frameExpected.Seed);
        ApplyConfig(frameSim.Ptr, frameConfigJson, "frame config");
        string viewportEnvelope = Native.SetViewport(frameSim.Ptr, frameExpected.Width, frameExpected.Height, frameExpected.Scale);
        if (!viewportEnvelope.Contains("\"ok\":true")) Fail($"viewport rejected: {viewportEnvelope}");

        Drive(frameSim.Ptr, frameExpected.EmitterFrames, frameExpected.BurstFrame, frameExpected.Dt);
        Native.bfx_render(frameSim.Ptr);

        if (Native.bfx_frame_width(frameSim.Ptr) != frameExpected.Width || Native.bfx_frame_height(frameSim.Ptr) != frameExpected.Height)
            Fail("frame dimensions diverged");
        int frameLen = (int)Native.bfx_frame_len(frameSim.Ptr);
        if (frameLen != frameBytes.Length) Fail($"frame length diverged: {frameLen} vs {frameBytes.Length}");
        IntPtr framePtr = Native.bfx_frame_ptr(frameSim.Ptr);
        if (framePtr == IntPtr.Zero) Fail("bfx_frame_ptr returned NULL");
        ReadOnlySpan<byte> actualFrame = Native.View<byte>(framePtr, frameLen);

        int differing = 0;
        for (int px = 0; px < frameLen; px += 4)
        {
            for (int c = 0; c < 4; c++)
            {
                if (Math.Abs(actualFrame[px + c] - frameBytes[px + c]) > frameExpected.ChannelTolerance)
                {
                    differing++;
                    break;
                }
            }
        }
        if (differing > frameExpected.MaxDifferingPixels)
            Fail($"{differing} pixels drifted beyond {frameExpected.ChannelTolerance} per channel");
        Console.WriteLine("  ok  the render protocol reproduces the Rust frame");

        // --- culling and warnings ---
        // No viewport here (sprite mode): bfx_set_bounds is the only way to
        // give cullMargin a frame. The config also overflows the particle
        // pool, which set_config reports as a warning without rejecting it.
        string cullConfigJson = File.ReadAllText(Path.Combine(fixtures, "ffi-cull.config.json"));
        using var cullSim = new Simulation(7);
        string cullEnvelope = ApplyConfig(cullSim.Ptr, cullConfigJson, "cull config");
        if (!cullEnvelope.Contains("particle pool"))
            Fail($"a config that overflows the pool carried no warning: {cullEnvelope}");
        Console.WriteLine("  ok  set_config warns about a pool overflow and still accepts the config");

        Native.bfx_set_bounds(cullSim.Ptr, 200f, 200f);
        Native.bfx_set_emitter(cullSim.Ptr, 50f, 50f, 0f, 0f, true);
        for (int i = 0; i < 5; i++) Native.bfx_advance(cullSim.Ptr, expected.Dt);
        if (Native.bfx_particle_count(cullSim.Ptr) == 0) Fail("cull check is vacuous: nothing spawned");
        Native.bfx_set_bounds(cullSim.Ptr, 1f, 1f);
        uint left = Native.bfx_particle_count(cullSim.Ptr);
        if (left != 0) Fail($"bfx_set_bounds did not cull the live particles: {left} left");
        Console.WriteLine("  ok  bfx_set_bounds culls particles outside the new frame");

        // set_config re-culls too. A 1000 margin around a 1 x 1 frame holds
        // the particles (so this also shows bfx_set_bounds took effect:
        // without bounds the later cull could not happen); margin 0 then
        // drops them at once.
        using var reCullSim = new Simulation(7);
        ApplyConfig(reCullSim.Ptr, cullConfigJson.Replace("\"cullMargin\": 0.0", "\"cullMargin\": 1000.0"), "wide-margin config");
        Native.bfx_set_bounds(reCullSim.Ptr, 1f, 1f);
        Native.bfx_set_emitter(reCullSim.Ptr, 50f, 50f, 0f, 0f, true);
        for (int i = 0; i < 5; i++) Native.bfx_advance(reCullSim.Ptr, expected.Dt);
        if (Native.bfx_particle_count(reCullSim.Ptr) == 0) Fail("set_config cull check is vacuous: nothing held");
        ApplyConfig(reCullSim.Ptr, cullConfigJson, "narrow-margin config");
        uint reLeft = Native.bfx_particle_count(reCullSim.Ptr);
        if (reLeft != 0) Fail($"set_config did not cull the live particles: {reLeft} left");
        Console.WriteLine("  ok  set_config with a smaller cullMargin culls at once");

        string bad = Native.SetConfig(sim.Ptr, "{ not json");
        if (!bad.Contains("\"ok\":false")) Fail($"bad JSON was accepted: {bad}");
        Console.WriteLine("  ok  an invalid config is rejected with a message, not a crash");

        if (Native.bfx_particle_count(IntPtr.Zero) != 0) Fail("null handle was not tolerated");
        if (Native.bfx_buffer_ptr(IntPtr.Zero) != IntPtr.Zero) Fail("null handle was not tolerated");
        Native.bfx_advance(IntPtr.Zero, 0.016f);
        Native.bfx_seek(IntPtr.Zero, 1.0f);
        Native.bfx_set_bounds(IntPtr.Zero, 100f, 100f);
        Native.bfx_simulation_free(IntPtr.Zero);
        Native.bfx_string_free(IntPtr.Zero);
        if (Native.bfx_frame_len(IntPtr.Zero) != 0) Fail("null handle was not tolerated by bfx_frame_len");
        if (Native.bfx_frame_ptr(IntPtr.Zero) != IntPtr.Zero) Fail("null handle was not tolerated by bfx_frame_ptr");
        Native.bfx_render(IntPtr.Zero);
        Console.WriteLine("  ok  null handles are tolerated");

        Console.WriteLine("csharp harness: all checks passed");
    }
}
