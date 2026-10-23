# Isolating Display device creation

The latest [#7 comparison](https://github.com/marselnenaj/fenix-a320-linux-patch/issues/7#issuecomment-6363596629)
still fails with descriptor heaps disabled. That run uses **descriptor buffers**,
so it does not test the older descriptor-set path. The default run also fails.
Concurrent initialization is present in the application logs, but it is not yet
established as the cause rather than a resource/driver condition.

The own-code `display-device-probe.exe` separates serial from parallel device
creation. Both modes retain all successful devices until the entire batch has
completed, then release them. This keeps device count comparable rather than
creating and immediately releasing one device per serial iteration.

It requests the reported D3D11 feature level 11.1, BGRA support and an explicit
adapter, and queries Device5. It performs no rendering, Fenix startup, account
access, file writes or simulator input. It defaults to one device, permits at
most sixteen and bounds creation to thirty seconds plus ten seconds for cleanup.

The final source passed 65 device creations with the exact CachyOS 11 overlay
on local AMD/Intel adapters, including both modes and an AMD legacy-binding
control. Its timestamps show thirteen overlapping API calls in parallel mode
and at most one in serial mode. These controls validate the diagnostic; they
do not reproduce the affected NVIDIA system.

## Obtain the diagnostic

The **graphics-diagnostics** job in the repository's
[Actions runs](https://github.com/marselnenaj/fenix-a320-linux-patch/actions)
publishes `fenix-graphics-diagnostics`: the retained-device probe, its source,
license, build/commit information and `SHA256SUMS`. Use the artifact from the
linked issue response so the source and executable identities match.

The artifact includes exact source copies in `source/`. Or build
[the source](https://github.com/marselnenaj/fenix-a320-linux-patch/blob/main/tests/display-device-probe.cpp)
from the repository root:

```sh
x86_64-w64-mingw32-g++ -std=c++17 -O2 -Wall -Wextra -Werror \
  -fno-exceptions -fno-rtti -static-libgcc -static-libstdc++ \
  tests/display-device-probe.cpp -o display-device-probe.exe \
  -ld3d11 -ldxgi -lole32 -luuid
```

## First comparison: simulator closed

Close MSFS and Fenix. Run the probe through the **same patched Proton/Wine,
prefix, Steam Runtime and NVIDIA/PRIME environment** as the affected Display
process, preserving its native D3D11/DXGI loading. A different host Wine or
substituted graphics DLL does not test the reported setup. The commands below
show the probe arguments; they are not instructions to invoke an unrelated Wine.

1. Run `display-device-probe.exe --count 1 --adapter 0` to verify the selected
   adapter. Check the `ADAPTER` record names the affected NVIDIA GPU; use its
   adapter index if different. The DXVK log must identify the expected backend.
2. With other settings unchanged and no diagnostic `DXVK_CONFIG`, run:

   ```text
   display-device-probe.exe --count 13 --adapter 0 --serial
   display-device-probe.exe --count 13 --adapter 0
   ```

Each run is a separate probe process. Retain its stdout, exit status and DXVK
d3d11/dxgi logs separately. `DEVICE` records include HRESULT, returned pointer,
feature level, Device5 result, elapsed time, Windows thread ID and monotonic
entry/exit ticks around the D3D11 call. `CLOCK qpc_hz` gives their frequency.
These intervals cover the API call, not the individual calls inside the driver.
`SUMMARY failures=0` with exit 0
means every requested device and Device5 query succeeded. Exit 1 records a
device/interface failure; 2 is invalid input, 3 adapter setup, 4 process setup
and 5 a watchdog timeout.

`BUDGET` records before creation, with devices retained and after release are
process budget/usage estimates. Interpret them only if their HRESULT succeeds;
zero fields after a failed query are not measurements. CurrentUsage is DXVK's
adjusted estimate, not raw driver-wide usage. Neither field directly measures
free VRAM.

## What the comparison can establish

- Serial succeeds and parallel fails: concurrent initialization is isolated in
  this small case, without changing device count or involving Fenix rendering.
- Both fail with MSFS closed: the failure can be investigated independently of
  the simulator. Compare the first failing device, HRESULT and DXVK log.
- Both pass: this does not reproduce the application failure. A later comparison
  with simulator load can distinguish load dependence; it is not required for
  the initial report.

If a minimal failing run is obtained, a **separate** legacy-binding comparison
can disable both descriptor models for this probe only:

```sh
DXVK_CONFIG='[display-device-probe.exe]; dxvk.enableDescriptorHeap = False; dxvk.enableDescriptorBuffer = False'
```

Confirm both effective features are 0 in that probe's DXVK log. This remains a
diagnostic, not a default change or confirmed workaround. Do not combine it
with the first serial/parallel comparison.
