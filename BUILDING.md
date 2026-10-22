# Rebuild and publish

The binary ZIP includes complete original upstream source archives in
`sources/`, their pinned checksums in `bundle.json`, and all modifications in
`patches/`. Patches apply in manifest order. Retain upstream per-file notices.

The initial build used x86_64 Arch Linux, GCC/MinGW-w64, autoconf, GNU make,
Python 3 and Perl for Wine's own build steps, Meson 1.0 or newer, Ninja and
glslang for DXVK, and Rust 1.98 for this project's
installer and maintainer tool. Install Wine's normal development dependencies, including
FreeType, fontconfig, X11, ALSA/PulseAudio, Vulkan headers and MinGW-w64.
The shipped ELF modules require glibc 2.38 or newer. An older-distribution build
must be tested and published as a separately pinned artifact.

```sh
cargo run --locked -p fenix-tools -- build --build-dir build/release --record-build
cargo test --locked --workspace
cargo run --locked -p fenix-tools -- smoke --runner /path/to/pinned-runner --work build/release-smoke
docker run --rm --user "$(id -u):$(id -g)" -v "$PWD":/src:ro -v "$PWD/build/installer":/out \
  -e CARGO_TARGET_DIR=/out/target -e CARGO_HOME=/out/cargo -e HOME=/out -w /src \
  rust:1.98-alpine cargo build --release --locked -p fenix-patch --bin fenix-patch
cargo run --locked -p fenix-tools -- release --installer build/installer/target/release/fenix-patch
```

The installer is `installer/` (program `fenix-patch`), the maintainer tool is
`tools/` (`cargo run --locked -p fenix-tools -- --help`). The release ships the
installer as one statically linked program, so that it runs on any x86_64
distribution without a runtime; the Alpine container above builds it against
musl. `release` refuses an installer that is not statically linked or reports
another version than `bundle.json`. `./install.sh` in a source checkout builds
and runs the installer with the host toolchain instead.

Three files under `integration/` are Python or contain it:
`fenix-display-refresh.py`, `xodus-wine-launch` and `launch-msfs.sh`. They are
the launch scripts of Flightdeck's Python releases (0.1.x) with the Fenix
additions, shipped unchanged and pinned by hash in `bundle.json`; the installer
only copies them. `tests/test_display_refresh.py` checks the first of them and
needs that launcher's interpreter. Nothing else in this project is Python.

Use a fresh `--build-dir`; existing build trees are never overwritten. `--prepare-only` verifies and applies all patches
without compiling. The build verifies the Wine and xgameruntime source archives, populates the
xgameruntime submodule, generates spec/Vulkan headers from the included XML,
and configures Wine with `--enable-win64 --disable-tests --enable-silent-rules
--without-ffmpeg --without-unwind`. Without the last option `configure` links
`ntdll.so` against the build host's libunwind, which the pinned runners and the
Steam Linux Runtime containers do not have. It builds the eleven original targets listed in `tools/src/build.rs` and
the independent MIT window guard, geometry setup and MCDU refresh helpers. Debug sections are stripped from release files.
Compiler prefix maps replace local checkout paths in both Unix and Windows
modules with `/usr/src/fenix-a320-linux-patch`.

The mapped-image and protocol-handler patches include matching generated
protocol 931 headers, trace and handlers; do not
regenerate them with a bumped protocol. Existing 32-bit runner clients must
remain compatible. Rebuild hashes can differ with compiler/platform/timestamps;
`--record-build` explicitly records a new binary set for review. Rebuild and
repin the Flightdeck manifest/release together after validating a changed build.

An overlay with a `dxvk` block also builds `gdi32.dll`, `win32u.dll` and
`wow64win.dll`, and the already included `win32u.so` and wineserver carry the
shared-resource owner-adapter changes. `make_specfiles` generates the matching
win32u syscall table from the patched spec file; that generated file is absent
from the original source archive. The corresponding DXVK revision must
match the pinned runner's original `d3d11.dll`; rebuilding a different DXVK
revision can break its private DXGI interface. Only `dxgi.dll` is replaced.

The `dxvk` block records `revision`, `source`, `source_root`, recursive
`submodules` (path to commit), and ordered `patches`. Its archive checksum belongs
in the overlay's `sources` table. The included
`sources/dxvk-6227b633e8d5289.tar.gz` contains the original committed DXVK
`6227b633e8d528952873c4c146773663964fcb31` tree and all six pinned submodules;
the pinned Proton Experimental uses `sources/dxvk-25ca63f17f34bdc.tar.gz` at
`25ca63f17f34bdc05a39873ee63907d3cbbfa030`, and CachyOS Wine 11 uses
`sources/dxvk-685301564ea3486.tar.gz` at
`685301564ea3486a7098bd37194020da84299617`, each with its own recursive pins.
The modifications are a separate patch. A codeload tarball alone has empty
submodule directories and cannot replace this complete archive. The build
checks the archive digest, every recursive submodule declaration and populated
directory, and each DXVK patch digest before compiling. Extract the complete
archive from the matching release if it is absent from a source checkout.

Meson uses the included `build-win64.txt` cross file, release optimization,
`--wrap-mode=nodownload`, the included libdisplay-info, and disables D3D8/9/10/11
targets. Ninja builds only `src/dxgi/dxgi.dll`. Prefix maps and
`--no-insert-timestamp` and `SOURCE_DATE_EPOCH=0` for DXGI compilation and
stripping avoid local paths and changing PE timestamps in that output. Put the selected Meson installation on `PATH`; no global installation
is required. The source archive retains upstream licence texts, including
DXVK's zlib/libpng licence and its subprojects' MIT notices.

## Proton overlays

`bundle.json` records each exact runner version and the hashes of its Wine ABI
partners, original source archive, port and output modules. Build all overlays
before packaging a release:

```sh
cargo run --locked -p fenix-tools -- build-proton experimental-11 --build-dir build/experimental --record-build
cargo run --locked -p fenix-tools -- build-proton cachyos-10 --build-dir build/cachyos --record-build
cargo run --locked -p fenix-tools -- build-proton cachyos-11 --build-dir build/cachyos-11 --record-build
cargo run --locked -p fenix-tools -- smoke --variant experimental-11 --runner /path/to/Proton-Experimental --work build/experimental-smoke --steam-runtime /path/to/SteamLinuxRuntime_4
cargo run --locked -p fenix-tools -- smoke --variant cachyos-10 --runner /path/to/Proton-CachyOS --work build/cachyos-smoke --steam-runtime /path/to/SteamLinuxRuntime_sniper
cargo run --locked -p fenix-tools -- smoke --variant cachyos-11 --runner /path/to/proton-cachyos-11.0-20261005-slr-x86_64 --work build/cachyos-11-smoke --steam-runtime /path/to/SteamLinuxRuntime_4
```

Steam starts a Proton inside the runtime named by `require_tool_appid` in its
`toolmanifest.vdf` (1628350 is sniper, 4183110 is Steam Linux Runtime 4). That
container has only its own libraries, so a module that loads on the host can
still fail there. `--steam-runtime` starts a Windows process with the patched
runner inside the container; the smoke check also refuses an overlay module that
needs a library its unmodified counterpart does not. The runtimes are free
Steam tools and need no simulator licence.

The Windows modules carry a link timestamp and checksum, so a rebuild without
source changes differs in a few header bytes. Keep the published files in that
case and record only modules whose code changed.

`cachyos-11` is the default base of the Steam installation. Its port combines
the Wine 11 changes of `experimental-11` with the Direct2D changes of
`cachyos-10`, because CachyOS carries the same Direct2D code in both versions.

These ports regenerate their own Wine protocol headers (931, 865 and 933 respectively)
and keep the existing 32-bit clients. They do not import Xodus's mapped-image
protocol extension: Flightdeck provides the portable Store loader. The selected
runner's graphics DLLs are preserved except the same-revision DXGI in overlays
with their own `dxvk` block. An overlay without that block keeps the original
eleven Wine targets and does not inherit the base overlay's DXVK build. CachyOS retains its existing
arc/group rendering; exact path streaming avoids its rotated-arc simplification
error when computing metrics. Its remote-thread activation-context fix belongs
in ntdll, while Experimental's implementation is in kernelbase.

For local modifications, change the corresponding source patch and its hash,
then rebuild. The installer validates its own manifest, so modified binaries
can be installed by updating that manifest and building your own release.
Do not overwrite binaries in an active Wine session.

The smoke checks use an isolated runner and prefix, require X11/Xwayland (or
Xvfb), and download pinned Microsoft geometry dependencies. They compare route
metrics and rasterized stroke contours with an independent geometry reference,
check actual X11 mapping as well as Win32 visibility, and run the existing
loader, cursor, write-copy, IOCP and audio regressions.
The Direct2D lifetime probe checks live heap growth after 300,000 geometries
on both normal and error paths, and 20,000 deterministic near-degenerate paths
with a per-case watchdog. See [lifetime and termination](docs/d2d-lifetime.md).
An overlay with its own `dxvk` block also tests the native DXGI/D3D11 and
VKD3D-Proton libraries: shared-resource ownership on every enumerated adapter,
1,000 cross-process texture frames with pixel verification and GPU fence
acknowledgements, and exact shared-fence values. The Proton 11 overlays also
exercise rewinds, nonzero/extreme initial values and creator lifetime. The
original Xodus physical fence backend retains its pre-existing inability to
rewind a local fence; the sharing checks there use increasing values.

## Release files

The `release` command exports only allowlisted project files. It requires all
source archives and validates binary hashes. It adds the licence texts of the
Rust crates linked into the installer under `licenses/rust/`. It creates:

* `*-linux-x86_64.zip`: standalone installer program, Wine overlay, complete sources.
* `*-source.tar.gz`: complete corresponding source, including upstream archives.
* `SHA256SUMS`: checksums for both downloads.
* `flightdeck-release.json`: the pinned URL/hash used by Flightdeck.

Create a GitHub **prerelease** tagged `v` plus the version in `bundle.json` and attach the two
archives plus SHA256SUMS. The tag must contain this project source and
`bundle.json`; do not commit payload binaries or private prefixes. Use
`docs/release-notes.md` as the release body. This GitHub release is Flightdeck's
patch download source; no separate flightsim.to listing is maintained for it.
Make the repository public so Flightdeck can download the pinned asset without
a GitHub login.

In Flightdeck, run `cargo run --locked -p flightdeck-tools -- probe sync-fenix
/path/to/this/repository` after building the release. This copies the two
checksum manifests and the licence. Flightdeck installs with its own code and
only extracts the fixed payload files from the verified ZIP.

The ZIP is suitable for distribution only with its source archives and notices
intact. Microsoft redistributables and Fenix software are separate downloads;
never add them, fonts, logs, settings, account data or a Wine prefix to a release.
