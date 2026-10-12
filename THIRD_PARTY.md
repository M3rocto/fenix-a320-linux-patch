# Components and source

* Wine: https://github.com/xodus-gaming/wine at
  `b1dd32734a34472a28eb5be9922df06e07ac0834`; LGPL-2.1-or-later and upstream
  per-file notices. The unchanged archive is included as `sources/wine-source.tar.gz`.
* Wine's xgameruntime submodule: https://github.com/xodus-gaming/xgameruntime at
  `64aebcabb8c66121eae25d3bf0ace4b582ebb0da`; included as
  `sources/xgameruntime-source.tar.gz`, with original notices.
* Proton Experimental Wine: https://github.com/ValveSoftware/wine at
  `adea6cbac9d00bb22910145354d00c463fcf3afc`; unchanged source archive
  `sources/experimental-11-wine.tar.gz`.
* CachyOS Wine: https://github.com/CachyOS/wine-cachyos at
  `603c2335ab4b26e57512ce19aca7c8ae95485daf`; unchanged source archive
  `sources/cachyos-10-wine.tar.gz`.
* CachyOS Wine 11: the same repository at
  `2ce6b44caa66a56ec99c7826ae564c088abc1b9f`; unchanged source archive
  `sources/cachyos-11-wine.tar.gz`. All three retain Wine’s LGPL-2.1-or-later
  licensing and original per-file notices.
* All modifications to these sources are in `patches/`, in manifest order.
  The patches are LGPL-2.1-or-later. See `licenses/LGPL-2.1.txt`.
* The selected-cursor sharing fix is backported from GloriousEggroll's
  [GE-Proton patch](https://github.com/GloriousEggroll/proton-ge-custom/commit/16a62e92).
  Its authorship and Wine's per-file notices are retained.
* The installer, integration wrappers, window guard and MCDU refresh helper are MIT. The integration
  scripts originate from the MIT-licensed Flightdeck launcher, with its notices retained.
* The installer program is linked statically with Rust crates under MIT,
  Apache-2.0, ISC, BSD-3-Clause, Zlib, Unicode-3.0 and CDLA-Permissive-2.0
  terms, among them rustls, ring and the Mozilla root certificates of
  webpki-roots. Each release contains their licence texts and an inventory
  with names, versions and licence expressions under `licenses/rust/`; the
  release tool collects them from the exact dependency set in `Cargo.lock`
  and refuses a crate without a licence text.

The complete sources are in the same binary ZIP and in a separate corresponding
source archive in each release. Build and relinking instructions are in BUILDING.md.
The runner itself, its fonts, DXVK and vkd3d binaries are supplied by the user's
Flightdeck or Proton installation and are not redistributed by this project.
For Steam, the installer can download the unmodified CachyOS Proton build from
its [GitHub release](https://github.com/CachyOS/proton-cachyos/releases); the
URL and SHA-256 pin are in `installer/src/steam.rs`.

The Direct2D geometry provider is downloaded separately from Microsoft's
[Platform Update for Windows 7 (KB2670838)](https://www.microsoft.com/en-us/download/details.aspx?id=36805).
The installer extracts only the x64 geometry library as `d2d1_geometry.dll`,
using a separately downloaded `msdelta.dll` from Microsoft's public symbol
server to decode the update. Neither DLL nor the update is redistributed.
URLs and SHA-256 pins are in `installer/src/core.rs`; package, delta-basis and
output checksums are checked before committing the staging profile.

`WINE_D2D1_GEOMETRY_PROVIDER=FenixDisplay.exe` limits this dependency to Fenix's
geometry calculations. Wine owns public Direct2D objects and rendering. The
original system `d2d1.dll` is not replaced with Microsoft's implementation.
The extraction helper is original MIT code in `native/geometry-setup.c`;
its binary and complete source are included.

Fenix and Microsoft software is installed separately through the respective
official installers. Their names are used to identify compatibility. This project
does not grant licenses for them or modify their licensing/activation checks.

The MCDU refresh helper calls the user’s existing `SimConnect_internal.dll` from
the installed MSFS 2024 game directory. No SimConnect library or SDK is bundled
or downloaded. `tests/mcdu-simconnect-fixture.c` is an original synthetic endpoint
for boundary checks and contains no Microsoft implementation.
