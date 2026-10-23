<p align="center">
  <img src="docs/assets/mark.svg" width="76" alt="Fenix A320 Linux Patch community project mark">
</p>
<h1 align="center">Fenix A320 Linux Patch</h1>
<p align="center">
  Wine compatibility and guided setup for the Fenix A320 on Linux.
</p>
<p align="center">
  <a href="#get-started"><strong>Get started</strong></a> &nbsp;·&nbsp;
  <a href="docs/index.md">Documentation</a> &nbsp;·&nbsp;
  <a href="#compatibility">Compatibility</a> &nbsp;·&nbsp;
  <a href="docs/release-notes.md">Changes</a> &nbsp;·&nbsp;
  <a href="docs/README.de.md">Deutsch</a>
</p>

![Original A320-family aircraft artwork above the Alps at sunset](docs/assets/fenix-panorama.png)

Bring your **purchased Fenix A320** to your Linux simulator setup with matching
Wine/Proton fixes, GPU display sharing and a guided installer. Use the built-in
integration in [Flightdeck](https://github.com/marselnenaj/flightdeck-msfs-2024-linux-xbox)
for its Xbox PC editions, or the standalone setup for **Steam MSFS 2020 and 2024**.
Installation, sign-in and activation use the official Fenix software.

**Current public release: [0.1.0-preview.10](https://github.com/marselnenaj/fenix-a320-linux-patch/releases/tag/v0.1.0-preview.10).**
A regular **0.1.0** release is in preparation and remains unpublished.

**New in preview.10:** Compatibility fixes for official Installer 1.0.359 and
Fenix v3 startup, retaining the GPU display transport and Direct2D memory-leak
correction. Fenix 3.0.0.7454 reached working cockpit displays on the tested
Xodus/MSFS 2024 setup. Other reported v3 startup failures remain open;
see [compatibility](#compatibility) and [release readiness](docs/release-readiness.md).

## Get started

### With Flightdeck

1. Update to **Flightdeck 0.2.18 or later**, then close MSFS and Fenix.
2. Open **Mods → Fenix A320** and follow the highlighted setup step.
3. Run the official installer from your [Fenix account](https://fenixsim.com/dashboard/).
   Sign in, install the A320 for your simulator and wait for installation to finish.
4. Open Fenix, sign in and apply the display settings. Start MSFS through Flightdeck.

Flightdeck includes the checksum-pinned preview.10 integration. Use its built-in
panel; newer patch versions require a matching Flightdeck integration update.

### With Steam

1. Start your Steam edition of MSFS once, then close it.
2. Download the **Linux installer ZIP** from the
   [preview.10 release](https://github.com/marselnenaj/fenix-a320-linux-patch/releases/tag/v0.1.0-preview.10)
   and extract it.
3. Open **Fenix A320 Linux Patch** or run `./install.sh`.
4. Follow the guided setup to install the patch, select **Proton Fenix A320**,
   run the official Fenix installer, sign in and configure the displays.

In the official installer, **install the A320 for the selected simulator** and
wait for completion before closing it. Opening the installer alone does not
install the aircraft.

The guided setup runs locally in your browser and needs no root privileges.
It creates a separate Proton tool from an exact supported build. With Steam
closed, it can select that tool automatically; otherwise select it under
**Properties → Compatibility**. `./install.sh --text` offers the same setup
in a terminal. [Steam setup guide](docs/steam.md)

## What the patch provides

| Area | What it provides |
| --- | --- |
| **Guided setup** | Matching runner selection, verified prerequisites, official-installer launch and setup status. [Steam guide](docs/steam.md) |
| **Cockpit displays** | GPU display sharing on supported Wine 11 builds, with CPU fallback and Legacy readouts. [Display transport](docs/gpu-displays.md) |
| **Compatibility fixes** | Wine fixes used by the installer and v3 startup, plus the Direct2D geometry lifetime correction. [Fenix v3](docs/fenix-v3.md) · [Memory fix](docs/d2d-lifetime.md) |
| **Updates and restore** | Verified upgrade paths that retain the aircraft, profile, renderer preference and original restore point. [Recovery](docs/mcdu-restart.md) |

## Requirements and runners

You need **x86_64 Linux with glibc 2.38+**, GNU `cp`, a desktop session, a web
browser, a working simulator installation and a purchased Fenix A320 license.
Internet access is required for official prerequisites and the geometry dependency.

| Exact supported runner | Default display renderer |
| --- | --- |
| Xodus Wine 11.0 / 7c0b4354 | GPU |
| Proton Experimental 11.0 / 20260924 | GPU |
| CachyOS Proton 11.0 / 20261005 | GPU |
| CachyOS Proton 10.0 sunset | CPU |

The installer is self-contained. It prepares checksum-verified Microsoft
prerequisites, including .NET Framework 4.8 and the .NET 10 Desktop Runtime
where needed. The official Fenix installer handles its own version-specific
prerequisites. Other runner versions require a matching overlay; exact pins
are recorded in [bundle.json](bundle.json).

## Compatibility

**Experimental.** Native regression checks cover all four pinned runners;
complete simulator and GPU-driver compatibility requires runtime testing.

| Fenix | Tested setup | Observed behavior |
| --- | --- | --- |
| 2.4.0.4720 | MSFS 2024 1.8.16.0 · Xodus 11.0 | PFD, ND, ECAM, MCDU and cockpit readouts checked locally. |
| 2.4.0.4720 | MSFS 2024 · CachyOS 11 | Community flight report with stable Display memory and no frozen displays; the supplied trace spans nearly two hours. Separate System growth is confirmed in the supplied counters; its cause remains open. |
| 3.0.0.7454 | MSFS 2024 1.8.16.0 · Xodus 11.0 | Official Installer 1.0.359, login, System connection and working cockpit displays checked; six bounded production starts passed. |
| 3.0.0.7454 | Steam MSFS 2020 · CachyOS 11 · NVIDIA/PRIME | Installation, login, System and CDU work according to the report. CPU rendering restores the displays; GPU initialization and Gateway startup remain unresolved. |

Fenix v3 full-flight stability remains unvalidated, and an earlier Wine startup
assertion is unexplained. [Issue #7](https://github.com/marselnenaj/fenix-a320-linux-patch/issues/7)
tracks the CPU display workaround and unresolved GPU/Gateway failures;
[#12](https://github.com/marselnenaj/fenix-a320-linux-patch/issues/12) tracks
separate FenixSystem memory growth. See the
[current findings and focused diagnostics](docs/display-startup.md).

GPU-capable overlays use **GPU displays** and **Legacy readouts**. An explicit
CPU preference survives updates. Weather radar is unavailable in CPU mode;
GPU weather radar remains unverified. Brightness and flight modes are normal
cockpit controls. [Display settings](docs/gpu-displays.md)

## Updates and recovery

Close MSFS and Fenix before updating. Flightdeck applies supported patch
updates before opening Fenix apps; the standalone installer recognizes verified
earlier previews. Updates retain the aircraft, Windows profile, settings and
original restore point.

**Restore** returns the pre-patch profile and keeps the newer profile in a
separate local directory. Community content outside that profile is retained.
Flightdeck's separate Xbox-save storage is outside this operation.

<details>
<summary>Terminal commands</summary>

```sh
./install.sh targets
./install.sh status --steam msfs2024
./install.sh install --steam msfs2024
./install.sh configure --steam msfs2024 --renderer cpu
./install.sh configure --steam msfs2024 --renderer gpu
./install.sh restore --steam msfs2024
```

Use `--steam msfs2020` for the Steam 2020 edition. Older Flightdeck Python
runtimes through 0.1.21 accept `--runtime /path/to/runtime`; current Flightdeck
releases use their built-in setup panel.

</details>

## Documentation and support

Start with the [documentation index](docs/index.md) for setup, display settings,
recovery and technical references. Report the patch, Fenix, simulator and runner
versions, your GPU/driver and the failing step in a
[GitHub issue](https://github.com/marselnenaj/fenix-a320-linux-patch/issues).
Review logs and remove account, activation and license information before sharing.
An optional [aggregate-memory sampler](docs/system-memory.md) helps investigate
System memory growth without collecting memory contents.

[BUILDING.md](BUILDING.md) covers the build and test workflow. Releases include
the installer, helper tools, replacement modules, Wine/DXVK patches and complete
corresponding source archives. Official redistributables are downloaded from
their publishers and checked against pinned hashes.

## License

Installer, helpers and project artwork: [MIT](LICENSE). Wine changes and derived
modules: **LGPL-2.1-or-later**. DXVK: **zlib/libpng**. Upstream notices are retained.
[Third-party notices](THIRD_PARTY.md)

This is an independent community project, not endorsed by Fenix Simulations,
Microsoft or the upstream projects. It distributes no Fenix/Microsoft programs,
aircraft, fonts, account data or private profiles. The cover is original project
artwork, not a simulator screenshot. [Artwork provenance](docs/artwork.md)
