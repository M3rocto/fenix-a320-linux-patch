# Rebuild and publish

The binary ZIP includes complete, unchanged upstream source archives in
`sources/`, their pinned checksums in `bundle.json`, and all modifications in
`patches/`. Patches apply in manifest order. Retain upstream per-file notices.

The initial build used x86_64 Arch Linux, GCC/MinGW-w64, autoconf, GNU make,
Python 3 and Perl for Wine's own build steps, and Rust 1.98 for this project's
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
  rust:1.98-alpine cargo build --release --locked --bin fenix-patch
cargo run --locked -p fenix-tools -- release --installer build/installer/target/release/fenix-patch
```

The installer is `installer/` (program `fenix-patch`), the maintainer tool is
`tools/` (`cargo run --locked -p fenix-tools -- --help`). The release ships the
installer as one statically linked program, so that it runs on any x86_64
distribution without a runtime; the Alpine container above builds it against
musl. `release` refuses an installer that is not statically linked or reports
another version than `bundle.json`. `./install.sh` in a source checkout builds
and runs the installer with the host toolchain instead.

Use a fresh `--build-dir`; existing build trees are never overwritten. `--prepare-only` verifies and applies all patches
without compiling. The build verifies both source archives, populates the
xgameruntime submodule, generates spec/Vulkan headers from the included XML,
and configures Wine with `--enable-win64 --disable-tests --enable-silent-rules
--without-ffmpeg --without-unwind`. Without the last option `configure` links
`ntdll.so` against the build host's libunwind, which the pinned runners and the
Steam Linux Runtime containers do not have. It builds the eleven targets listed in `tools/src/build.rs` and
the independent MIT window guard, geometry setup and MCDU refresh helpers. Debug sections are stripped from release files.
Compiler prefix maps replace local checkout paths in both Unix and Windows
modules with `/usr/src/fenix-a320-linux-patch`.

The mapped-image and protocol-handler patches include matching generated
protocol 931 headers, trace and handlers; do not
regenerate them with a bumped protocol. Existing 32-bit runner clients must
remain compatible. Rebuild hashes can differ with compiler/platform/timestamps;
`--record-build` explicitly records a new binary set for review. Rebuild and
repin the Flightdeck manifest/release together after validating a changed build.

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
runner's graphics DLLs are not part of the overlay. CachyOS retains its existing
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
