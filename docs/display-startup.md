# Fenix v3 display and Gateway startup

Issue [#7](https://github.com/marselnenaj/fenix-a320-linux-patch/issues/7#issuecomment-6363596629)
reports two remaining failures with Fenix 3.0.0.7454, preview.10, Steam MSFS
2020 and the pinned CachyOS 11 runner on NVIDIA/PRIME. Installation, login,
System connection and CDU work in the reported later sessions. Earlier path
and PBKDF2 errors in the same attachments belong to older sessions.

## Synchronization configuration

The native checks and successful local cockpit starts used server
synchronization. Disabling esync and fsync alone does not disable ntsync.
The exact Wine 11 sources used by Xodus, Experimental 11 and CachyOS 11
read `PROTON_NO_NTSYNC`; CachyOS 10 reads `WINE_DISABLE_FAST_SYNC`.

The 0.1.0 source candidate sets the appropriate switches in the standalone
installer and legacy Flightdeck launch script. Upgrading a Steam tool adds
the switches while retaining its other Python settings. The corresponding
[Flightdeck 0.2.19 source change](https://github.com/marselnenaj/flightdeck-msfs-2024-linux-xbox/pull/6)
is also on main; public Flightdeck remains 0.2.18. These changes make the
configuration consistent with the tested backend; they are **not a confirmed
fix** for the reporter's Display or Gateway failure.

The reporter has now repeated the preview.10 test with `PROTON_NO_NTSYNC=1`,
confirmed the process environment and absence of ntsync handles, and found
**both failures unchanged**. Disabling ntsync is not a fix for this report.
When comparing synchronization backends, close the simulator and its Fenix
helpers first: a running wineserver retains the backend selected at startup.

## Display: graphics initialization

The new Display DXVK log reports **`VK_ERROR_INITIALIZATION_FAILED` from
Vulkan device creation**, including the retry in safe mode. The matching DXVK
source then returns a failed D3D11 result with a NULL device. Feature level
11.1 is supported on this adapter; the successful initialization entries
report a maximum of 12.1. Those feature-level messages appear after Vulkan
device creation and do not count the failed attempts.

The matching 3.0.0.7454 Display initialization method creates a D3D11 device,
shared texture and Direct2D target. One selection path requests only feature
level **11.1**, with BGRA support and an explicit adapter, then discards the
device-creation HRESULT. A failed creation can leave a NULL device for the
next call. Other objects in the same method can also be NULL; the supplied
exception has no IL/native offset identifying the exact receiver.

An independent account-free probe with the exact CachyOS 11 graphics DLLs:

- Passes device creation, Device5, shared-texture and Direct2D target setup on
  the tested AMD and Intel adapters, at requested levels 11.1 and 11.0.
- With an intentional `d3d11.maxFeatureLevel = 11_0` cap, returns
  `0x80070057` and NULL for 11.1 while the 11.0 comparison succeeds.
- With WineD3D, reaches Direct2D setup on the tested AMD adapter but rejects
  the keyed-mutex interface. That is a different checked-error path.

These controls validate a possible failure mechanism. They do not reproduce
the NVIDIA/PRIME failure. A further own-code control retained twelve devices
at once: all sixty serial or parallel creations across the tested AMD and
Intel configurations passed. These adapters do not reproduce the reporter's
NVIDIA driver or memory-budget conditions. Do not force a feature-level downgrade as a fix:
the application explicitly requests 11.1. Microsoft documents the
[device-creation contract](https://learn.microsoft.com/en-us/windows/win32/api/d3d11/nf-d3d11-d3d11createdevice).

### NVIDIA follow-up: both comparisons still fail

The latest attachments confirm these results on NVIDIA driver 615.78.8:

| Configuration | Effective binding model | Result |
| --- | --- | --- |
| Display-only `dxvk.enableDescriptorHeap = False` | Descriptor buffer: heap 0, buffer 1 | 11 of 13 primary device requests fail; 2 succeed. |
| Default graphics settings, separate Gateway ReadyToRun test | Descriptor heap: heap 1, buffer 0 | 11 of 13 primary device requests fail; 2 succeed. |

Each log contains 24 Vulkan attempts: 13 primary attempts and 11 safe-mode
retries. The safe-mode retry removes NVIDIA CUDA-interop extensions, but keeps
the selected descriptor model. Disabling descriptor heaps did not resolve
the failure; it **did not test legacy descriptor sets**, because descriptor
buffers became active.

Device-creation log messages interleave, showing overlapping outer
initialization work. They do not time the driver calls or establish that
concurrency caused the failures. The latest device-local budget estimates
fall from 94/100 MiB to around 30 MiB against a 4 GiB heap. A budget is a
guideline for the process, not a direct free-VRAM measurement or proof of OOM.

The next focused comparison uses an
[own-code retained-device diagnostic](device-creation-diagnostic.md), first
with MSFS and Fenix closed. It creates the same number of devices serially
and in parallel, retaining successful devices in both modes. This can isolate
concurrency without mixing it with device-count or simulator-load changes.
Graphics defaults remain unchanged pending a reproducer.

One remaining application comparison can test legacy descriptor sets by
disabling **both** descriptor models for Display only:

```sh
DXVK_CONFIG='[FenixDisplay.exe]; dxvk.enableDescriptorHeap = False; dxvk.enableDescriptorBuffer = False'
```

Confirm heap 0, buffer 0 and the Legacy binding model in the Display log.
This is a diagnostic comparison, not a confirmed workaround. If it still
fails, the standalone serial/parallel probe above can produce a smaller case.

### Own-code graphics diagnostic

Source: [display-initialization-probe.cpp](../tests/display-initialization-probe.cpp).
Build with a MinGW-w64 C++ compiler and its DirectX headers:

```sh
x86_64-w64-mingw32-g++ -std=c++17 -O2 -Wall -Wextra -Werror \
  -fno-exceptions -fno-rtti -static-libgcc -static-libstdc++ \
  tests/display-initialization-probe.cpp -o display-initialization-probe.exe \
  -ld3d11 -ldxgi -ld2d1 -lole32 -luuid
```

Run `display-initialization-probe.exe --interop` through the affected
installation's exact Wine/Proton environment, with the simulator closed.
Use the same D3D11/DXGI loading and Steam Runtime as the application; a
different host Wine or substituted graphics DLLs do not test that setup.
`--adapter 0` limits device tests to one enumerated adapter. The probe tests
at most eight and uses synthetic 32×32 textures. It does not start Fenix,
read its account files or send simulator input.

`MODULE`, `ADAPTER` and `CASE` records identify backend, adapter, HRESULT and
output pointer. **Exit 0 means at least one 11.1 device was created**; inspect
every stage. The optional mutex query can fail even when device creation
succeeds. The probe does not test cross-process transport or a complete cockpit.

## Gateway: class initialization remains unresolved

The supplied native stack is structurally consistent with CoreCLR class
initialization entering deadlock-aware lock handling; its other normal thread
matches finalizer waiting. A paused stack does not identify the CPU loop,
lock owner or initialized type. No speculative lock/memory-manager patch is included.

The inspected single-file bundle contains CIL/ReadyToRun payloads and a native
host consistent with statically linked CoreCLR. The product's NativeAOT banner
alone does not establish a pure NativeAOT runtime. The .NET 10.0.13 reference
source is not the exact 10.0.12-dev product build.

Fresh account-free controls used unchanged official Gateway files, the exact
CachyOS 11 overlay, server synchronization and a watchdog. Both ReadyToRun
default and disabled returned before reaching a listener or application log.
The reported service arguments and DOS/`\\.\GLOBALROOT`/`\\?\GLOBALROOT` image
paths also returned early with a live own-code parent. These controls neither
reproduce nor refute the sustained spin.

The reporter has now confirmed `DOTNET_ReadyToRun=1` in the actual Gateway
environment. It still has two threads, a main thread using around 97% of a
core, an idle finalizer and no log or listener. Neither this change nor
disabling ntsync resolves the reported spin. The latest attachments contain
no fresh Gateway registers or stack.

The next useful evidence is **three brief samples of the same hot Gateway
Windows thread**, about two seconds apart. Attach using the existing matched
runner/prefix and the current Gateway Windows PID, then run:

```text
thread 0x<CURRENT_HOT_WINDOWS_TID>
info regs
bt
detach
```

Let the process run between samples and reattach to that same PID. Include
Windows PID/TID, sample times and any context-read errors; Linux TIDs are not
interchangeable with Windows thread IDs. The raw registers must precede the
backtrace. Include the Gateway executable's SHA-256 to confirm image identity.
No memory dump, lock-memory read or further runtime flag is needed at this stage.

Keep Gateway write-copy tracking at its published default; the extra override
did not help. No Gateway fix or full v3 stability claim follows from these results.
