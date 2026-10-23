// SPDX-License-Identifier: MIT
// Original bounded CoreCLR class-initialization smoke control, no vendor code.
using System;
using System.Diagnostics;
using System.Runtime.CompilerServices;
using System.Threading;

internal static class BeforeMain
{
    [ModuleInitializer]
    internal static void Mark()
    {
        Console.WriteLine("MARKER before_main module_initializer");
        Console.Out.Flush();
    }
}

internal static class Counters
{
    internal static int SharedRuns;
    internal static int GenericRuns;
    internal static int LazyGcRuns;
    internal static int LazyNonGcRuns;
    internal static readonly ManualResetEventSlim CctorEntered = new ManualResetEventSlim(false);
    internal static readonly ManualResetEventSlim CompleteCctor = new ManualResetEventSlim(false);
}

internal static class Shared
{
    internal static readonly int Value;
    static Shared()
    {
        Interlocked.Increment(ref Counters.SharedRuns);
        Console.WriteLine("MARKER shared_cctor_enter");
        Counters.CctorEntered.Set();
        if (!Counters.CompleteCctor.Wait(5000)) throw new TimeoutException("shared ctor gate");
        Value = 0x1234567;
        Console.WriteLine("MARKER shared_cctor_exit");
    }
}

internal sealed class Tag { }
internal static class LazyGc
{
    internal static readonly byte[] Payload;
    static LazyGc()
    {
        Interlocked.Increment(ref Counters.LazyGcRuns);
        Payload = new byte[512];
        Payload[0] = 0x42;
        Payload[511] = 0x64;
        Console.WriteLine("MARKER lazy_gc_cctor");
    }
}
internal static class LazyNonGc
{
    internal static readonly long Number;
    static LazyNonGc()
    {
        Interlocked.Increment(ref Counters.LazyNonGcRuns);
        Number = 0x102030405060708L;
        Console.WriteLine("MARKER lazy_non_gc_cctor");
    }
}
internal static class GenericCell<T>
{
    internal static readonly byte[] Payload;
    internal static readonly long Number;
    internal static int Runs;
    static GenericCell()
    {
        Interlocked.Increment(ref Runs);
        Interlocked.Increment(ref Counters.GenericRuns);
        Payload = new byte[2048];
        Payload[0] = 0x31;
        Payload[2047] = 0x73;
        Number = 0x1122334455667788L;
        Console.WriteLine("MARKER generic_cctor type=" + typeof(T).Name);
    }
}

internal static class Program
{
    private static int checks;
    private static int failures;
    private static void Check(bool condition, string name)
    {
        checks++;
        if (!condition) { failures++; Console.WriteLine("FAIL " + name); }
    }

    // Plain ldsfld access first, not RunClassConstructor, exercises lazy
    // class-init/static-base paths closer to the observed InitClassHelper.
    // Explicit cctors ensure these types are not beforefieldinit.
    [MethodImpl(MethodImplOptions.NoInlining)]
    private static byte[] ReadLazyGc() { return LazyGc.Payload; }
    [MethodImpl(MethodImplOptions.NoInlining)]
    private static long ReadLazyNonGc() { return LazyNonGc.Number; }
    [MethodImpl(MethodImplOptions.NoInlining)]
    private static byte[] ReadGenericGc<T>() { return GenericCell<T>.Payload; }
    [MethodImpl(MethodImplOptions.NoInlining)]
    private static long ReadGenericNonGc<T>() { return GenericCell<T>.Number; }

    private static void Generic<T>()
    {
        RuntimeHelpers.RunClassConstructor(typeof(GenericCell<T>).TypeHandle);
        Check(GenericCell<T>.Runs == 1, "generic once");
        Check(GenericCell<T>.Payload.Length == 2048 && GenericCell<T>.Payload[0] == 0x31 &&
              GenericCell<T>.Payload[2047] == 0x73, "generic GC static payload");
        Check(GenericCell<T>.Number == 0x1122334455667788L, "generic non-GC static number");
    }

    private static void SharedConcurrent()
    {
        Console.WriteLine("MARKER before_classinit shared concurrent_workers=2");
        // Initialize the own coordination state before touching the tested type.
        Check(Counters.SharedRuns == 0, "shared initially uninitialized");
        using (var ready = new CountdownEvent(2))
        using (var callsStarted = new CountdownEvent(2))
        using (var start = new ManualResetEventSlim(false))
        {
            var errors = new Exception[2];
            var entries = new long[2];
            var exits = new long[2];
            var observed = new int[2];
            var threads = new Thread[2];
            for (int i = 0; i < 2; i++)
            {
                int worker = i;
                threads[i] = new Thread(() => {
                    try
                    {
                        ready.Signal();
                        if (!start.Wait(5000)) throw new TimeoutException("worker start");
                        entries[worker] = Stopwatch.GetTimestamp();
                        callsStarted.Signal();
                        RuntimeHelpers.RunClassConstructor(typeof(Shared).TypeHandle);
                        observed[worker] = Shared.Value;
                        exits[worker] = Stopwatch.GetTimestamp();
                    }
                    catch (Exception error) { errors[worker] = error; }
                });
                threads[i].IsBackground = true;
                threads[i].Start();
            }
            try
            {
                Check(ready.Wait(5000), "two workers ready");
                start.Set();
                Check(callsStarted.Wait(5000), "two classinit calls dispatched");
                Check(Counters.CctorEntered.Wait(5000), "shared ctor entered");
                // The ctor gate keeps the first call pending while the other
                // worker gets scheduled. This is own-code contention only.
                Thread.Sleep(50);
            }
            finally { Counters.CompleteCctor.Set(); }
            for (int i = 0; i < 2; i++)
            {
                Check(threads[i].Join(5000), "worker completed");
                Check(errors[i] == null, "worker no exception");
                if (errors[i] != null) Console.WriteLine("WORKER_EXCEPTION " + errors[i].GetType().FullName);
                Check(observed[i] == 0x1234567, "published shared value");
            }
            Check(Counters.SharedRuns == 1, "shared ctor exactly once");
            Check(entries[0] > 0 && entries[1] > 0 && exits[0] > entries[0] && exits[1] > entries[1], "valid own timing");
            long overlap = Math.Min(exits[0], exits[1]) - Math.Max(entries[0], entries[1]);
            Check(overlap > 0, "overlapping classinit wrapper intervals");
            Console.WriteLine("MARKER after_classinit shared runs=" + Counters.SharedRuns +
                " overlap_ticks=" + overlap + " stopwatch_hz=" + Stopwatch.Frequency);
        }
    }

    private static int Run()
    {
        Console.WriteLine("MARKER main_enter runtime=" + Environment.Version + " pointer_bytes=" + IntPtr.Size);
        SharedConcurrent();
        Console.WriteLine("MARKER before_classinit serial_repeat iterations=1024");
        for (int i = 0; i < 1024; i++)
        {
            RuntimeHelpers.RunClassConstructor(typeof(Shared).TypeHandle);
            Check(Shared.Value == 0x1234567 && Counters.SharedRuns == 1, "serial repeat stable");
        }
        Console.WriteLine("MARKER after_classinit serial_repeat");
        Console.WriteLine("MARKER before_classinit lazy_direct_fields");
        Check(Counters.LazyGcRuns == 0 && Counters.LazyNonGcRuns == 0, "lazy types initially uninitialized");
        byte[] lazy = ReadLazyGc();
        Check(lazy.Length == 512 && lazy[0] == 0x42 && lazy[511] == 0x64, "plain lazy GC static access");
        Check(ReadLazyNonGc() == 0x102030405060708L, "plain lazy non-GC static access");
        for (int i = 0; i < 256; i++)
            Check(Object.ReferenceEquals(lazy, ReadLazyGc()) && ReadLazyNonGc() == 0x102030405060708L,
                  "plain field repeat stable");
        Check(Counters.LazyGcRuns == 1 && Counters.LazyNonGcRuns == 1, "lazy direct cctors once");
        Console.WriteLine("MARKER after_classinit lazy_direct_fields");
        Console.WriteLine("MARKER before_classinit generic direct_fields_first types=3");
        Check(Counters.GenericRuns == 0, "generic types initially uninitialized");
        Check(ReadGenericGc<int>()[0] == 0x31 && ReadGenericNonGc<int>() == 0x1122334455667788L, "generic GC-first direct access");
        Check(ReadGenericNonGc<string>() == 0x1122334455667788L && ReadGenericGc<string>()[2047] == 0x73, "generic non-GC-first direct access");
        Check(ReadGenericGc<Tag>()[2047] == 0x73 && ReadGenericNonGc<Tag>() == 0x1122334455667788L, "generic own-reference-type access");
        Console.WriteLine("MARKER before_classinit generic explicit_repeat rounds=256 types=3");
        for (int i = 0; i < 256; i++) { Generic<int>(); Generic<string>(); Generic<Tag>(); }
        Check(Counters.GenericRuns == 3, "one init per closed generic type");
        Check(!Object.ReferenceEquals(GenericCell<int>.Payload, GenericCell<string>.Payload) &&
              !Object.ReferenceEquals(GenericCell<string>.Payload, GenericCell<Tag>.Payload), "distinct generic GC statics");
        Console.WriteLine("MARKER after_classinit generic total_runs=" + Counters.GenericRuns);
        Console.WriteLine("SUMMARY checks=" + checks + " failures=" + failures);
        return failures == 0 ? 0 : 1;
    }

    private static int Main()
    {
        using (var done = new ManualResetEventSlim(false))
        {
            var watchdog = new Thread(() => {
                if (!done.Wait(30000))
                {
                    Console.Error.WriteLine("WATCHDOG internal30s");
                    Environment.Exit(124);
                }
            });
            watchdog.IsBackground = true;
            watchdog.Start();
            try { return Run(); }
            catch (Exception error)
            {
                Console.WriteLine("EXCEPTION " + error.GetType().FullName + ": " + error.Message);
                return 2;
            }
            finally { done.Set(); watchdog.Join(1000); }
        }
    }
}
