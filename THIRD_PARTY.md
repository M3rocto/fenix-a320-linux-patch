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
* All modifications to these Wine sources are in `patches/`, in manifest order.
  The Wine patches are LGPL-2.1-or-later. See `licenses/LGPL-2.1.txt`.
* `tests/sip-pe-fixture.h` retains the public synthetic PE fixture and notices
  from Wine's `dlls/wintrust/tests/softpub.c`, under LGPL-2.1-or-later. The
  own SIP probe creates this fixture as data and never executes it.
* DXVK: https://github.com/doitsujin/dxvk at
  `6227b633e8d528952873c4c146773663964fcb31` for Xodus,
  `25ca63f17f34bdc05a39873ee63907d3cbbfa030` for the pinned Proton Experimental,
  and `685301564ea3486a7098bd37194020da84299617` for CachyOS Wine 11;
  zlib/libpng licence, reproduced
  in `licenses/DXVK.txt`. The matching overlay redistributes a modified
  `dxgi.dll` with the changes in `patches/dxvk-shared-resource-adapter.patch`
  under the same licence. `sources/dxvk-6227b633e8d5289.tar.gz`,
  `sources/dxvk-25ca63f17f34bdc.tar.gz` and
  `sources/dxvk-685301564ea3486.tar.gz` contain the
  original committed sources and all six original recursively pinned submodules,
  with their copyright and licence notices intact:
  * `include/native/directx`: Joshua-Ashton/mingw-directx-headers at
    `9df86f2341616ef1888ae59919feaa6d4fad693d`.
  * `include/spirv`: KhronosGroup/SPIRV-Headers at
    `04f10f650d514df88b76d25e83db360142c7b174`.
  * `include/vulkan`: KhronosGroup/Vulkan-Headers at
    `8864cdc896bbc2a9b6eb36b3218fc9ef57908d77`.
  * `subprojects/dxbc-spirv`: doitsujin/dxbc-spirv at
    `c5c1a5b9b61773d59592f628faa39be9add26282` for Xodus,
    `bf14419e5fa7eacb817b7b632f03cb61d61bbad7` for Proton Experimental, and
    `c7f069701227dcb800a01bd93e42f8001b7543e7` for CachyOS Wine 11, MIT.
  * `subprojects/dxbc-spirv/submodules/spirv_headers`: KhronosGroup/SPIRV-Headers at
    `c8ad050fcb29e42a2f57d9f59e97488f465c436d`.
  * `subprojects/libdisplay-info`: doitsujin/libdisplay-info at
    `275e6459c7ab1ddd4b125f28d0440716e4888078`, MIT.
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
The runner itself, its fonts, D3D11 and vkd3d binaries are supplied by the user's
Flightdeck or Proton installation. This project redistributes the modified
same-revision DXGI only for overlays whose manifest includes its source and
output pins; the runner's other graphics modules are retained.
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

The Microsoft .NET 10.0.13 x64 Desktop Runtime is downloaded from Microsoft at
setup time and installed in the selected Windows profile. Its SHA-256 is pinned
in `installer/src/desktop.rs` after verification against Microsoft's published
SHA-512 release metadata. The Microsoft installer and runtime are not included
in this project's archives. This modern runtime is separate from .NET Framework
4.8, which setup retains for existing Fenix components.

The MCDU refresh helper calls the user’s existing `SimConnect_internal.dll` from
the installed MSFS 2024 game directory. No SimConnect library or SDK is bundled
or downloaded. `tests/mcdu-simconnect-fixture.c` is an original synthetic endpoint
for boundary checks and contains no Microsoft implementation.
