# Bounded certificate-query control

This original native control investigates the signature-query lead in
[#12](https://github.com/marselnenaj/fenix-a320-linux-patch/issues/12).
It opens one explicitly selected file through `CryptQueryObject`, requesting
an embedded binary PKCS#7 signature and both returned store/message handles.
It never executes the signature fixture or starts Fenix/MSFS.

The caller closes **both** `HCRYPTMSG` and `HCERTSTORE`. The Wine message-backed
store holds an additional message reference, so closing just one handle is
not equivalent. No certificate context is requested. This is a query control,
not a trust-verification or managed-wrapper reproducer.

## Build and run

Build from the repository root with MinGW-w64:

```sh
x86_64-w64-mingw32-g++ -std=c++17 -O2 -Wall -Wextra -Werror \
  -fno-exceptions -fno-rtti -static-libgcc -static-libstdc++ -municode \
  tests/certificate-query/certificate-query-probe.cpp \
  -o certificate-query-probe.exe -lcrypt32 -lpsapi
```

Run through the runner being investigated in a **new disposable prefix**, with
an explicitly selected signed PE file. These are executable arguments; they
do not select a host Wine installation or an existing simulator prefix:

```text
certificate-query-probe.exe C:\path\signed-file.exe smoke
certificate-query-probe.exe C:\path\signed-file.exe bounded
```

`smoke` performs one query and closes both handles. Only continue after it
passes. `bounded` performs 128 warmup queries, 4,000 close-both queries,
retains at most 256 store/message pairs as a measurement control, closes
those pairs, then performs another 4,000 close-both queries. A failed query
stops the run. The internal deadline is 20 seconds between API calls; use an
external 30-second watchdog for a call that does not return.

CSV `SAMPLE` records contain phase, completed queries, retained pairs,
elapsed milliseconds, working set and API-reported private usage. Exit 0 and
`passed=1` mean the requested calls and closes succeeded; exit 1 is a query,
close, memory-counter or internal-deadline failure, and exit 2 is invalid input.
This does not demonstrate the absence of all leaks. In the tested Wine build,
`PrivateUsage` is always zero and is **not a usable private-memory measurement**.

## Recorded control

The tested source SHA-256 is
`99b3dab841804bc89067f145646a0599ba78ccb7c177e408e39a4221cd8bc386`.
The selected public Microsoft Windows .NET **10.0.13** `dotnet.exe` fixture is
167,208 bytes, SHA-256
`abce6623a6729059cf39bb0f728c4a7804f625895099c88c17e6e704661e09eb`.
It was queried read-only, not executed or redistributed here.

The exact production CachyOS 11 runner and its pinned overlays were copied
into an analysis directory and checked against all 27 bundle paths. Each run
used its own fresh prefix through Steam Linux Runtime 4; all source-runner,
copied-runner and fixture hashes were unchanged afterward.

A second copy substitutes only the matching stock CachyOS 11 `crypt32.dll`
and `wintrust.dll`, retaining every other pinned file and using another fresh
prefix. Both prefix copies were checked against their selected DLL hashes:

| PE module | Production overlay SHA-256 | Matching stock SHA-256 |
| --- | --- | --- |
| crypt32.dll | `a40827bf153753b7c1e51279e62d6c07de119d4afe17bf8f31bac76f9c098d7b` | `eafb9bdef88f074c2208c783a37f4b7b54bac44909bc627b8dc4117e5d684bb7` |
| wintrust.dll | `99134ff02e50dd24e833d65e071eb2d0ec4c38c63e7e6a1ab4f92f3fd74b0436` | `f10a47ade0f6c7cfc3d5f368efe0e1975c0ea243d54f4a89a3949d97603ce085` |

The patched run completes **8,384 queries**, zero query/close failures, in
**1.525 seconds** inside the probe. After warmup the first 4,000-query series
adds **48 KiB** of working set. Holding 256 handle pairs adds **9.63 MiB**;
after release the second 4,000-query series is flat. The retained high working
set after release is consistent with allocator reuse; release need not return
pages immediately to the OS.

The matching-stock run also completes **8,384 queries** with zero failures,
in **1.649 seconds** inside the probe. Its first closed series adds **36 KiB**,
the retained-pair control adds **9.77 MiB**, and the second closed series is
flat. No material difference between stock and overlay appears in this case;
other signature inputs and trust-verification paths remain outside the test.

These bounded native observations do not reproduce Fenix 2's signing chain,
.NET Framework 4.8 wrappers, other trust APIs or long-running process. No
Fenix-side cause, Windows equivalence or production fix follows from them.
Store-only, message-only and no-output queries use different internal cleanup
branches. Certificate contexts obtained later by enumeration/find/duplication
also have separate release obligations; this control requests none. The actual
Fenix wrapper/API/output parameters have not been established.
The measured counters are retained in [overlay.csv](overlay.csv) and
[matching-stock.csv](matching-stock.csv); the private-usage column is explicitly
unavailable.
See [the investigation](../../docs/system-memory.md) for the reporter's
separate process measurements and their limits.
