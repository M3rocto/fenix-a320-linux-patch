# Fenix A320 Linux Patch

Wine compatibility fixes and a guided installer for the Fenix A320 on Linux,
with integration for [Flightdeck](https://github.com/marselnenaj/flightdeck-msfs-2024-linux-xbox)
and the Steam editions of Microsoft Flight Simulator 2020 and 2024.

[Download preview.10](https://github.com/marselnenaj/fenix-a320-linux-patch/releases/tag/v0.1.0-preview.10)
· [Release notes](docs/release-notes.md) · [Report an issue](https://github.com/marselnenaj/fenix-a320-linux-patch/issues)
· [Deutsch](docs/README.de.md)

This is an independent community project. You need your own simulator and
Fenix license. Sign-in, installation and activation use the official Fenix software.

## Current status

**The published build is 0.1.0-preview.10.** Version **0.1.0** is being prepared;
it has not been published as a regular release. Fenix v3 still has unresolved
Display and Gateway failures on the reported NVIDIA/Steam setup. A separate
System-memory report is under investigation. See [release readiness](docs/release-readiness.md).

| Configuration | Evidence and remaining limits |
| --- | --- |
| Fenix 2.4.0.4720 · MSFS 2024 1.8.16.0 · Xodus 11.0 | PFD, ND, ECAM, MCDU and cockpit readouts checked locally; complete-flight validation remains limited. |
| Fenix 2.4.0.4720 · MSFS 2024 · CachyOS 11 | Community-reported 1 h 53 min flight with stable Display memory and no frozen displays; [System memory growth remains open](https://github.com/marselnenaj/fenix-a320-linux-patch/issues/12). |
| Fenix 3.0.0.7454 · MSFS 2024 1.8.16.0 · Xodus 11.0 | Official Installer 1.0.359, login, System connection and working cockpit displays checked; six bounded production starts passed. An earlier Wine assertion remains unexplained; no full-flight qualification. |
| Fenix 3.0.0.7454 · Steam MSFS 2020 · CachyOS 11 · NVIDIA/PRIME | Reporter confirms installation, login, System and CDU. [Display initialization and Gateway startup still fail](https://github.com/marselnenaj/fenix-a320-linux-patch/issues/7). |

Native regression checks cover all four exact runners listed in `bundle.json`.
They do not establish complete simulator compatibility on every runner or GPU.
Detailed results: [Fenix v3](docs/fenix-v3.md), [GPU displays](docs/gpu-displays.md),
[Direct2D memory fix](docs/d2d-lifetime.md).

## Install with Flightdeck

1. Update to **Flightdeck 0.2.18 or later** and close MSFS and Fenix.
2. Open **Mods → Fenix A320** and follow the highlighted setup step.
3. Run the official Fenix Installer from your [Fenix account](https://fenixsim.com/dashboard/).
   Sign in, install the A320 for your simulator and wait for installation to finish before closing it.
4. Open Fenix, sign in and apply the display settings. Then start MSFS through Flightdeck.

Flightdeck 0.2.18 includes the checksum-pinned preview.10 integration. Future
patch releases require an explicit Flightdeck pin update. Use Flightdeck's
built-in panel; the standalone installer only recognizes the launch scripts
of Flightdeck's older Python releases through 0.1.21.

## Install with Steam

1. Start your Steam edition of MSFS once, then close it.
2. Download and extract the **Linux installer ZIP** linked above.
3. Run `./install.sh` or open **Fenix A320 Linux Patch**. The guided setup runs
   locally in your browser; no root privileges are required.
4. Follow the steps to install the patch, select **Proton Fenix A320**, run the
   official Fenix Installer, sign in and configure the displays.

Inside the official installer, **install the A320 for the selected simulator**
and wait for it to finish. Closing the installer before installing the aircraft
does not complete this step.

The setup creates a separate Proton tool from an exact supported build.
With Steam closed it can select that tool automatically; otherwise choose it
under **Properties → Compatibility**. `./install.sh --text` provides the same
setup in a terminal. See [Steam setup, updates and limitations](docs/steam.md).

## Requirements and supported runners

- x86_64 Linux, glibc **2.38+**, GNU `cp`, a desktop session and a web browser.
- A working simulator installation and a purchased Fenix A320 license.
- Internet access for official prerequisites and the geometry dependency.
- One of the exact Wine/Proton builds pinned in `bundle.json`.

| Runner | Default display renderer |
| --- | --- |
| Xodus Wine 11.0 / 7c0b4354 | GPU |
| Proton Experimental 11.0 / 20260924 | GPU |
| CachyOS Proton 11.0 / 20261005 | GPU |
| CachyOS Proton 10.0 sunset | CPU |

The installer is self-contained. It prepares checksum-verified Microsoft
prerequisites, including .NET Framework 4.8 and the .NET 10 Desktop Runtime
where required. The official Fenix installer handles its own version-specific
prerequisites. Other runner versions need a matching overlay.

## Updates, display settings and restore

Updates retain the aircraft, Windows profile, settings and original restore
point. Flightdeck applies supported patch updates before opening Fenix apps.
The standalone installer recognizes verified earlier previews.

GPU-capable overlays use **GPU displays** and **Legacy readouts**. An explicit
CPU preference survives updates. Weather radar is unavailable in CPU mode;
GPU weather radar remains unverified. Brightness and flight modes remain
normal cockpit controls. See [GPU transport](docs/gpu-displays.md),
[display restart recovery](docs/mcdu-restart.md) and [window handling](docs/hyprland.md).

```sh
./install.sh targets
./install.sh status --steam msfs2024
./install.sh install --steam msfs2024
./install.sh configure --steam msfs2024 --renderer cpu
./install.sh configure --steam msfs2024 --renderer gpu
./install.sh restore --steam msfs2024
```

Use `--steam msfs2020` for the Steam 2020 edition. Older supported Flightdeck
Python runtimes accept `--runtime /path/to/runtime` instead.

**Restore** returns the pre-patch profile and retains the newer profile in a
separate local directory. Community content outside the profile is not removed.
Flightdeck's separate Xbox-save storage is outside this restore operation.

## Troubleshooting and bug reports

For v3 startup or black/white displays, start with the
[current findings and next diagnostics](docs/display-startup.md). Issue
[#7](https://github.com/marselnenaj/fenix-a320-linux-patch/issues/7) tracks the
remaining v3 failures; [#12](https://github.com/marselnenaj/fenix-a320-linux-patch/issues/12)
tracks the separate FenixSystem memory observation. An optional
[aggregate-memory sampler](docs/system-memory.md) helps collect comparable measurements.

Include patch, Fenix, simulator and runner versions, distribution, desktop,
GPU/driver and the failing step. Review logs before sharing them; omit account,
activation and license data. Wine profiles and process-memory dumps are not
needed for an initial report.

## Source and licenses

Releases include the installer, helper tools, Wine/DXVK patches, replacement
modules, complete corresponding source archives and build instructions.
Fenix/Microsoft binaries, aircraft, fonts, account data and private profiles
are not distributed. Official redistributables are downloaded from their
publishers and checked against pinned hashes.

Installer and helpers: **MIT**. Wine changes and derived modules:
**LGPL-2.1-or-later**. DXVK: **zlib/libpng**. Upstream notices are retained.
See [BUILDING.md](BUILDING.md) and [THIRD_PARTY.md](THIRD_PARTY.md).
