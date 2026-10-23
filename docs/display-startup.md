# Fenix v3 display and Gateway startup

Issue [#7](https://github.com/marselnenaj/fenix-a320-linux-patch/issues/7#issuecomment-6362400086)
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
[Flightdeck 0.2.19 change](https://github.com/marselnenaj/flightdeck-msfs-2024-linux-xbox/pull/6)
is prepared separately. These changes make the
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

### Targeted NVIDIA comparison

The exact pinned CachyOS 11 DXVK source enables descriptor heaps on the
reported NVIDIA driver. Both normal and safe-mode attempts in the supplied
log retain this feature; safe mode only removes the NVIDIA CUDA-interop
extensions. One diagnostic comparison is therefore to add this per-application
setting to the existing Steam launch options for one run:

```sh
DXVK_CONFIG='[FenixDisplay.exe]; dxvk.enableDescriptorHeap = False'
```

The section matches `FenixDisplay.exe` and leaves the simulator outside that
section. Keep the other settings unchanged. Confirm effective configuration
and `descriptorHeap : 0` in the new **Display process's** DXVK log, then report
whether Vulkan device creation and the displays succeed. This comparison is
pending on the affected setup; it is not a confirmed workaround or a patch
default. Revert the diagnostic setting after the comparison.

The log also reports a small device-local budget of 108–216 MiB. A budget is
a guideline for the current process, not a direct measurement of free VRAM;
it does not by itself prove an out-of-memory failure.

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

Disabling ntsync did not resolve the reporter's sustained spin. Another
configuration difference remains: the successful local production starts
used ReadyToRun defaults, whereas the Steam tool sets `DOTNET_ReadyToRun=0`.
In a **separate comparison**, add only `DOTNET_ReadyToRun=1` as the next change
and confirm that value in the Gateway process environment. The pinned Proton
script preserves an existing environment value over the tool's default.
Record whether Gateway writes its log, listens on port 8083 or retains the
two-thread CPU spin. This comparison is pending; the fresh-profile early
returns above did not reach the hanging service phase.

If it still hangs, useful evidence is a short timestamped CPU/thread summary
and brief repeated stacks from the same hot thread. Keep Gateway write-copy
tracking at the published default; the reported extra override did not help.
No Gateway fix or full v3 stability claim follows from the current observations.
