# Fenix v3 display and Gateway startup

Issue [#7](https://github.com/marselnenaj/fenix-a320-linux-patch/issues/7)
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

The first two Display comparisons report **`VK_ERROR_INITIALIZATION_FAILED`
from Vulkan device creation**, including the retry in safe mode. The Legacy
comparison below instead reports **`VK_ERROR_OUT_OF_HOST_MEMORY`**. The matching DXVK
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

### NVIDIA follow-up: all three binding models fail in Display

The latest attachments confirm these results on NVIDIA driver 615.78.8:

| Configuration | Effective binding model | Result |
| --- | --- | --- |
| Display-only `dxvk.enableDescriptorHeap = False` | Descriptor buffer: heap 0, buffer 1 | 11 of 13 primary requests fail with `VK_ERROR_INITIALIZATION_FAILED`; 2 succeed. |
| Default graphics settings, separate Gateway ReadyToRun test | Descriptor heap: heap 1, buffer 0 | 11 of 13 primary requests fail with `VK_ERROR_INITIALIZATION_FAILED`; 2 succeed. |
| Display-only heap **and** buffer disabled, [run 3](https://github.com/marselnenaj/fenix-a320-linux-patch/issues/7#issuecomment-6364740742) | Legacy: heap 0, buffer 0 | 11 of 13 primary requests fail with `VK_ERROR_OUT_OF_HOST_MEMORY`; 2 succeed. |

Each log contains 24 Vulkan attempts: 13 primary attempts and 11 safe-mode
retries. The safe-mode retry removes NVIDIA CUDA-interop extensions, but keeps
the selected descriptor model. Disabling descriptor heaps did not resolve
the failure; it **did not test legacy descriptor sets**, because descriptor
buffers became active. Run 3 subsequently tested Legacy and still produced
black displays and eleven reported NullReferenceExceptions. All 22 Vulkan
errors in that log, including the eleven safe-mode retries, are host-memory
allocation errors. Legacy is not a workaround for this report.

Device-creation log messages interleave, showing overlapping outer
initialization work. They do not time the driver calls or establish that
concurrency caused the failures. The latest device-local budget estimates
reach around 30 MiB against a 4 GiB heap in the first two comparisons. Run 3
has estimates ranging **41–105 MiB**, with later increases as well as drops.
A budget is a guideline for the process, not a direct free-VRAM measurement.
The Vulkan [result-code contract](https://docs.vulkan.org/refpages/latest/refpages/source/VkResult.html)
describes a failed host allocation; it does not by itself identify system-wide
RAM exhaustion. Neither local video-memory budgets nor the reported Linux
MemAvailable value settle that cause.

### The affected NVIDIA GPU passes the closed-simulator probe

The [retained-device follow-up](https://github.com/marselnenaj/fenix-a320-linux-patch/issues/7#issuecomment-6364896282)
uses the published own-code diagnostic, with MSFS/Fenix closed, the same
reported runner/prefix, Steam Runtime and NVIDIA/PRIME settings. The reported
EXE SHA-256 matches the verified artifact. The supplied stdout and DXVK logs
show an **RTX 3050 Laptop GPU**, driver **615.78.8**, descriptor heaps,
feature level **11.1**, and **27 successful device/Device5 queries**:
one adapter check, thirteen serial devices and thirteen parallel devices.
The QPC intervals confirm at most one API call in serial mode and thirteen
overlapping calls in parallel mode.

Both batches retain all devices until completion. Each reports **186.875 MiB**
of process-local estimated usage while retained, returning to zero after
release. The local budgets are several GiB, unlike the low estimates in the
application logs. These are DXVK-adjusted process estimates, not raw global
driver usage.

The successful parallel batch shows that thirteen overlapping creations alone
do not reproduce the failure in the closed-simulator case. Application work
and simulator load still differ: the probe does not render, allocate the
application's shared textures or initialize Direct2D. A load-only cause or
application-specific race is not established or excluded by this comparison.

The next bounded comparison keeps a stationary loaded cockpit and its Display
state constant: **one probe device first**, then thirteen serial devices if
that passes, and thirteen parallel devices only if the serial batch passes.
Stop after a failed smaller/serial case and preserve its first error. See the
[exact comparison and launch notes](device-creation-diagnostic.md#next-comparison-stationary-cockpit-loaded).
Graphics defaults remain unchanged pending a reproducer.

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

The reporter confirmed `DOTNET_ReadyToRun=1` in the actual Gateway
environment. It still has two threads, a main thread using around 97% of a
core, an idle finalizer and no log or listener. Neither this change nor
disabling ntsync resolves the reported spin.

### Fresh samples match the inspected executable

The [run-3 attachment](https://github.com/marselnenaj/fenix-a320-linux-patch/issues/7#issuecomment-6364740742)
now supplies three raw-register/backtrace samples of the same Windows thread,
with clean attaches and no reported context-read errors. The reported full EXE
SHA-256 is **6e1ad073786c5c8d3591b7e093385bc0f60a1a42006ee177af00ef39d84a2bbb**,
matching the locally inspected image. Register values precede the backtraces.

| Sampled RVA | Bounded native/source interpretation |
| --- | --- |
| `+0x124c79` | Initial flags test in the structurally matched `CrstBase::Enter`, before `mov rdi,rcx`. The Crst argument is in **RCX**; **RDI still belongs to the caller**. |
| `+0x10f9bc` | Entry of the structurally matched `MethodTable::EnsureStaticDataAllocated`, called by the class-initialization wrapper. The sampled instruction is not one of the helper's retry loops. |
| `+0x10f465` | Class-initialization entry-holder cleanup call, not a sampled wait or deadlock-graph traversal. |

All three backtraces share the return at `+0x12abd9` from **InitClassHelper**
(entry `+0x12aba0`); their raw contexts are consistent with the same candidate
MethodTable. This strengthens the class-initialization boundary, but does not
identify the initialized type, its caller or a causal busy loop. InitClassHelper
is named by an actual native registration entry in the inspected image;
the other function names remain structural source matches rather than symbols
from a matching product PDB.

The exact bundled `System.Private.CoreLib` also confirms the managed route:
`InitHelpers.InitClassSlow` calls `QCall!InitClassHelper`; the lazy GC/non-GC
static-base helpers and generic initialization can reach it. In contrast,
`RuntimeHelpers.RunClassConstructor` imports
`ReflectionInvocation_RunClassConstructor`. This distinction comes from the
selected CoreLib's CIL/import metadata, not just a neighboring reference build.
Its SHA-256 is **3440c3d45d1301dda4cb4f4058a7cb15ca26fc0d2f4d056b46e0d7dd3bb7cb61**,
MVID **5bf3e5bc-4e65-415b-a722-c3448d57f321**, assembly version **10.0.0.0**,
with CIL and R2R payloads. The public [InitHelpers source](https://github.com/dotnet/runtime/blob/6aa72daa24f66c01c3d7b6a32519e9d21396deec/src/coreclr/System.Private.CoreLib/src/System/Runtime/CompilerServices/InitHelpers.cs)
is a reference for that route, not proof of exact native-build equivalence.

An [original class-initialization control](../tests/coreclr-class-init/README.md)
passed **3,607 checks** with public Windows .NET 10.0.13, the patched CachyOS 11
runner and a fresh prefix. It includes direct lazy GC/non-GC and generic
static-field access as well as two contending constructor calls. It uses
invariant globalization and builtin mscoree for its own CIL launch. It does
not test the custom Gateway runtime, protected type, R2R path or simulator load.

In particular, interpreting **RDI as a Crst** in the first sample would be
wrong: the instruction that changes it has not executed. A one-off lock-owner
read at the supplied pre-acquisition/cleanup sites would not distinguish the
cause. No further generic runtime flags, repeated stack request or lock-memory
read follows from these samples. A targeted next step needs the actual
repeating call/initializer or a smaller reproducer.

Keep Gateway write-copy tracking at its published default; the extra override
did not help. No Gateway fix or full v3 stability claim follows from these results.
