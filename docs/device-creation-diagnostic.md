# Isolating Display device creation

The [#7 Display comparisons](display-startup.md#nvidia-follow-up-all-three-binding-models-fail-in-display)
fail with descriptor heaps, descriptor buffers and Legacy descriptor sets.
Legacy changes the Vulkan error to `VK_ERROR_OUT_OF_HOST_MEMORY`, without
restoring the displays. The [subsequent closed-simulator probe](https://github.com/marselnenaj/fenix-a320-linux-patch/issues/7#issuecomment-6364896282)
passes on the affected NVIDIA GPU in both serial and parallel modes.
This narrows the conditions for a reproducer; load dependence and the
application's extra graphics work still need to be separated.

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
do not reproduce the affected NVIDIA system. The reporter's exact-identity
artifact then passed **1 + 13 serial + 13 parallel** device/Device5 queries
on the RTX 3050 Laptop GPU, driver 615.78.8, with MSFS/Fenix closed. Its QPC
records confirm thirteen overlapping API calls in the parallel case.

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

Each run is a separate probe process. Let it exit and release its devices
before starting the next one. Retain its stdout, exit status and DXVK
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

### Launch notes from the affected setup

The successful reporter runs used the same patched Proton through Steam Linux
Runtime, with the selected NVIDIA adapter and native D3D11/DXGI loading.
These differences were recorded for the diagnostic launch:

- The MSFS 2020 Protonfix appends `-FastLaunch`, which is not a probe argument.
  `PROTONFIXES_DISABLE=1` was set **for that probe launch only**.
- `DXVK_LOG_LEVEL=info` recorded the graphics logs. Console stdout was redirected
  through Wine's `cmd /c` into a separate output file, including the probe's exit
  code; the Proton wrapper's exit status alone is not the probe result.
- The container could not open the original accented download path. A copy in
  an accessible ASCII-named directory ran successfully. This is a recorded
  launch-context difference, not proof of a general non-ASCII-path defect.

Keep those probe-only choices the same in later comparisons. They are not
changes to the production Fenix/MSFS launch defaults.

## Next comparison: stationary cockpit loaded

The closed-simulator control has already passed on the affected machine.
A stationary cockpit comparison can now test whether the **same small probe**
fails under load; no flight is needed. Keep the runner, prefix, Steam Runtime,
NVIDIA/PRIME environment and probe build unchanged. Leave diagnostic
`DXVK_CONFIG` unset for the probe, as in the successful closed case.

Keep the simulator scene and Fenix Display state constant across the runs;
note whether Display itself uses its default or Legacy configuration and
whether it is running with black screens. A probe process cannot remove an
override from an already-running Display process.

1. Run `display-device-probe.exe --count 1 --adapter 0`. If it fails, stop and
   retain that smallest case.
2. If it passes, run `display-device-probe.exe --count 13 --adapter 0 --serial`.
   If that fails, stop: a concurrent batch is not needed to demonstrate this
   serial failure.
3. Only if all thirteen serial devices pass, run
   `display-device-probe.exe --count 13 --adapter 0` once.

Keep each process's stdout, probe exit code and DXVK d3d11/dxgi logs separate,
including all BUDGET and first-failing DEVICE records. Note the cockpit/Display
state and the Linux available-memory figure near the run. These measurements
describe the tested context; neither a budget nor available RAM identifies
the failing allocation by itself.

## What the comparison can establish

- Serial succeeds and parallel fails: concurrent initialization is isolated in
  this small case, without changing device count or involving Fenix rendering.
- Both fail with MSFS closed: the failure can be investigated independently of
  the simulator. Compare the first failing device, HRESULT and DXVK log.
- Both pass with the simulator closed: the reporter has now obtained this
  result. Use the loaded comparison above to separate simulator context from
  the application's own additional work. If it also passes, that small probe
  still does not reproduce the full application path.

If a minimal failing run is obtained, a **separate** legacy-binding comparison
can disable both descriptor models for this probe only:

```sh
DXVK_CONFIG='[display-device-probe.exe]; dxvk.enableDescriptorHeap = False; dxvk.enableDescriptorBuffer = False'
```

Confirm both effective features are 0 in that probe's DXVK log. This remains a
diagnostic, not a default change or confirmed workaround. Do not combine it
with the first serial/parallel comparison.
