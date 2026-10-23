# GPU displays in preview.8

The pinned Xodus, Proton Experimental 11 and CachyOS Proton 11 overlays support
Fenix's GPU display transport. CachyOS Proton 10 keeps the existing CPU path.
Use the complete matching overlay; changing `displayMode` alone leaves the
missing graphics interfaces unresolved.

## What changes

Fenix's CPU mode still draws through Direct2D. It reads the rendered pixels
back to CPU memory for transport to the simulator. The GPU path shares textures
and synchronization objects between the simulator's D3D12 renderer and Fenix's
D3D11 renderer, avoiding that CPU readback for display transport. It still needs
CPU time to calculate and draw the instruments.

Two compatibility gaps prevented this path from working:

* DXVK returned `E_NOTIMPL` from
  `IDXGIFactory2::GetSharedResourceAdapterLuid`. Wine now retains the creating
  GPU's LUID with the shared allocation, independently of its replaceable
  graphics metadata. DXGI queries that owner. It does not guess the first
  adapter, and invalid handles still fail. NT handles, KMT handles, duplication,
  closing the original handle and cross-process queries are covered.
* The pinned Proton 11 semaphore backend kept virtual D3D12 fence state in
  process-local memory. An importer could dereference a foreign heap pointer;
  discarding the pointer instead exposed physical Vulkan ticks as incorrect
  D3D12 values. Its shared fence context now lives in a mapped NT section with
  process-shared synchronization and server-owned handle lifetime. Imports and
  signals use the same virtual values, including nonzero initial values and
  rewinds. An unusable context fails instead of fabricating completion.

The fixed Wine server request/reply layouts and protocol numbers stay unchanged.
Old importers are refused access to the new fence-context metadata. Existing
32-bit clients are retained. The installer requires Wine, server, GDI, win32u,
WoW64 and DXGI components as a complete set; it checks the underlying runner
before installation and refuses to change an active Wine session.

Only the matching DXGI DLL changes in DXVK. D3D11 and VKD3D-Proton remain those
of the pinned runner. The three DXVK sources are separate immutable revisions:

| Overlay | DXVK revision |
| --- | --- |
| Xodus | `6227b633e8d528952873c4c146773663964fcb31` |
| Proton Experimental 11, 20260924 | `25ca63f17f34bdc05a39873ee63907d3cbbfa030` |
| CachyOS Proton 11, 20261005 | `685301564ea3486a7098bd37194020da84299617` |

The archives include their original sources and all recursively pinned
submodules. See [BUILDING.md](../BUILDING.md) and
[THIRD_PARTY.md](../THIRD_PARTY.md).

## Settings and upgrades

GPU-capable overlays explicitly declare `display_renderer: GPU` and their own
DXVK source and binary pins in `bundle.json`. The configuration is
`displayMode=GPU`, `newRender=true`, `preferCPU=false`, `multithread=true`.
Legacy FCU readouts remain selected.

Older overlays without this capability remain CPU-only. Selecting a variant
does not inherit the base overlay's GPU declaration. In particular, CachyOS
Proton 10 is unchanged.

An upgrade changes a previously automatic CPU setting to the new supported
renderer. Explicit choices are saved separately and survive configuration and
later upgrades:

```sh
./install.sh configure --steam msfs2024 --renderer cpu
./install.sh configure --steam msfs2024 --renderer gpu
```

For an older supported Flightdeck runtime, use `--runtime /path/to/runtime`
instead of `--steam msfs2024`. Current Flightdeck installs through its own
Add-ons panel and needs updated embedded release pins and renderer support;
downloading this source checkout does not update that installed application.

The CPU option remains a fallback. The GPU option is rejected on a CPU-only
overlay. Both keep the original restore point and preserve the other settings.

## Validation and limits

### NVIDIA Display Safe Mode

For the [#7 Fenix 3.0.0.7454 / CachyOS 11 / NVIDIA-PRIME report](https://github.com/marselnenaj/fenix-a320-linux-patch/issues/7#issuecomment-6368573752),
turn **USE DISPLAY SAFE MODE off** in the Fenix application before comparing
GPU displays. The reporter records `ST D2D: False`, two D3D11 device creations,
zero failures and working GPU displays. With Safe Mode on, thirteen devices
were requested and most failed under simulator load. CPU rendering remains
an available fallback. The patch does not force this user preference.

This result does not establish physical RAM/VRAM exhaustion, a driver-wide
concurrency defect or compatibility across all NVIDIA systems. The historical
diagnostic sequence remains in [startup notes](display-startup.md).

### Sharing controls

The Xodus path was checked with Fenix 2.4.0.4720, MSFS 2024 1.8.16.0 and an
AMD RX 6900 XT under Xwayland. PFD, ND and ECAM rendered in the live cockpit.
A controlled SimConnect airspeed sweep exercised the moving PFD tape from
standstill to over 100 knots; the test restored the original speed afterwards.
This is a display test, not a validated complete flight or a guarantee of
particular cockpit frame rates.

The standalone own-code probes use actual shared GPU resources, separate
Windows processes, bounded waits and pixel checks:

* The unpatched owner query fails; the patched owner probe passes on both AMD
  RX 6900 XT and Intel RPL-S, including invalid-handle controls.
* The final Xodus payload passes 1,000 ordered D3D12-to-D3D11 texture frames
  with fence acknowledgements in the opposite direction on AMD, and 300 on
  Intel: 5,324,800 checked pixels in total. Transfer timing includes synthetic
  readback and is not a PFD performance measurement.
* Proton 11 additionally checks dense, strided and decreasing values, nonzero
  and near-maximum initial values, releasing the creating objects and importing
  after the creator exits. Its original mixed-process implementation fails the
  frame/value tests; the shared-context implementation passes.
* The existing loader, memory, completion-wait, audio, cursor, helper-window,
  path metric and rasterized stroke regression probes remain part of `smoke`.

The existing Xodus physical timeline backend cannot rewind even a local fence;
that limitation is retained. Its monotonic shared-frame transport is tested.
The virtual-value/rewind repair belongs to the two Proton 11 overlays.

NVIDIA, GPU weather radar, MSFS 2020 with Fenix, a real Steam simulator session
and complete flights remain unvalidated by the maintainer. The Proton 11
graphics probes establish the sharing contract, not full cockpit performance
on those runners. GPU support removes the CPU display transport requirement;
other simulator or instrument workload can still cause stutters.
