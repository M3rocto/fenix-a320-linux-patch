# Unreleased

* The guided setup shows what a running step has finished, what it is doing
  now and for how long: an activity bar, the completed sub-steps and the
  elapsed time, instead of only the latest message. How long the rest takes is
  not known, so the bar shows activity, not a percentage.

# 0.1.0-preview.7 — 13 October 2026

The installer is now one self-contained program instead of a Python package.
The Wine overlays, helpers and patches are the preview.6 files, byte for byte;
nothing changes in the simulator. Installations of preview.1 to preview.6 are
updated as before, without reinstalling Fenix.

* `./install.sh` starts the program `fenix-patch` that the ZIP contains. It is
  statically linked and needs no Python, no other runtime and no system
  library. Commands, options, messages and exit codes are those of the Python
  installer. The guided setup in the browser is unchanged.
* The host still needs glibc 2.38 or newer and GNU `cp`, because the patched
  Wine modules and the profile copy do; the installer checks this as before.
* Compared with the Python installer on the same inputs: a complete
  installation and restoration in a Flightdeck runtime with the pinned Xodus
  runner and real Wine give the same output, state and files, apart from what
  Wine and Microsoft's setup write differently on every run. Settings files
  keep their namespace declarations, which the Python installer dropped.
* A failure that the Python installer reported as a traceback is now an
  ordinary message. Downloads verify against the bundled Mozilla root
  certificates instead of the system's store; the pinned checksums are checked
  as before.
* The maintainer scripts are the tool `fenix-tools` (`build`, `build-proton`,
  `smoke`, `release`); see BUILDING.md. The smoke check passes on the Xodus
  runner, CachyOS Proton 10 and CachyOS Proton 11 with the unchanged overlays.
* The release contains the licence texts of the Rust crates linked into the
  installer under `licenses/rust/`.
* Three launch scripts for Flightdeck's Python releases 0.1.x stay in the
  bundle unchanged; Flightdeck verifies two of them by hash.

Not verified: the Steam path with a real Steam simulator, as before.

# 0.1.0-preview.6 — 12 October 2026

Fixes the Steam start failure of preview.5. In every overlay exactly one file
changes, `ntdll.so`; all other modules and helpers are the preview.5 files.
Verified preview.5 installations are updated in place, on Steam and in
Flightdeck, without reinstalling Fenix.

* Build the patched `ntdll.so` without libunwind. On the build host, Wine's
  `configure` found libunwind and linked `libunwind.so.8`, which neither the
  unmodified runners nor Steam Linux Runtime 3 and 4 provide. Started from
  Steam, the simulator therefore exited after about a second with `could not
  load ntdll.so: libunwind.so.8`. All previews since preview.1 were affected;
  outside the container the host library hid it. The module now needs
  `libgcc_s.so.1` and `libc.so.6`, like the runners it replaces.
  ([#3](https://github.com/marselnenaj/fenix-a320-linux-patch/issues/3))
* Update an installed Steam release in place: **Update patch** in the guided
  setup, or `./install.sh install --steam msfs2024`, replaces the overlay in the
  *Proton Fenix A320* tool. The Windows profile, Fenix, its sign-in and
  settings stay as they are. A tool shared by MSFS 2020 and 2024 is updated
  once; both simulators have to be closed.
* The guided setup no longer waits at **Open Fenix and sign in** for
  `persistancy.xml`. **Apply settings** writes CPU rendering and the autostart
  entry as soon as `fenixConfig.xml` exists. When Fenix has not created
  `persistancy.xml` yet, a further step asks to load the aircraft once and then
  sets the Legacy FCU readouts, instead of reporting an error.
* The guided setup offers an available update for Flightdeck runtimes as well.
* `scripts/smoke.py` fails when an overlay module needs a library its runner
  does not, and with `--steam-runtime` starts a Windows process with the
  patched runner inside the Steam Linux Runtime container.

Checked for this release: the preview.5 `cachyos-11` module fails inside Steam
Linux Runtime 4 with the reported message and the new one starts a Windows
process there; `cachyos-10` does the same in Steam Linux Runtime 3 (sniper).
The regression probes pass again for the Xodus, `cachyos-10` and `cachyos-11`
overlays. The pinned Proton Experimental build is no longer distributed by
Steam; its overlay was probed on a runner assembled from that build's untouched
files and the overlay. A rebuild of the Windows modules differs from preview.5
only in the PE timestamp and checksum, so the published files are kept.

The Steam path was confirmed by one user with MSFS 2024, the `cachyos-11` base
and preview.5 plus a manually added libunwind. **The maintainer has still not
run a real Steam simulator**; MSFS 2020 with Fenix is untested. Whether Fenix
creates `persistancy.xml` only on the first aircraft load is taken from that
report.

# 0.1.0-preview.5 — 10 October 2026

One release for Steam and Flightdeck. The Flightdeck overlay (Xodus Wine) and
the two earlier Proton overlays are byte-identical to preview.4; verified
preview.4 installations are recognised and updated without reinstalling Fenix.

* New overlay for CachyOS Proton `cachyos-11.0-20261005-slr` (Wine 11), the
  default base of the Steam installation. Loader, memory, audio, cursor,
  helper-window and Direct2D regression probes pass on the unmodified build:
  3,564 geometry reference cases, 800 concurrent metric checks and 768 stroke
  comparisons with zero differing pixels; 32-bit clients keep working with the
  rebuilt server.

* Standalone installation for the Steam editions of MSFS 2020 and 2024
  (`./install.sh install --steam msfs2024`). The installer creates a separate
  compatibility tool, *Proton Fenix A320*, from the pinned CachyOS Proton and
  downloads that build when it is not installed. Proton Experimental and other
  tools are left unchanged. See [steam.md](steam.md).
* The Windows profile is exchanged transactionally; the simulator's `Packages`
  folder is moved instead of copied. Restore returns the earlier profile.
* Proton's shared Wine Mono registration and linked framework placeholders are
  removed before Microsoft .NET setup, which otherwise installs nothing.
* New guided setup in the web browser, served only to the local computer, in
  place of the Tk window: one step at a time, finished steps ticked, resumable.
  No additional packages are needed; `--text` offers the same in a terminal.
* With Steam closed, the installer enters the Fenix Proton for the simulator
  in Steam's configuration and restore removes it again.
* Flightdeck runtimes keep working with the same commands.

Verified in a reproduced Steam library whose profile was created by Proton
Experimental, with both CachyOS bases: pinned Proton download, .NET Framework
4.8 (32- and 64-bit CLR through the tool's launcher), geometry dependency,
overlay, Proton's own profile refresh, restore and repeat installation. **Not yet run with a real
Steam simulator or Fenix on Steam**; MSFS 2020 with Fenix is untested.

# 0.1.0-preview.4 — 3 October 2026

* Add separate, source-matched Fenix overlays for Proton Experimental
  `experimental-11.0-20260924-x86_64` and CachyOS `cachyos-10.0-sunset-slr`.
  Preserve each runner's graphics stack and verify its exact Wine ABI partners.
* Support the classic Wine64 entry point in setup, GSX/.NET helpers and MCDU
  refresh. Keep 32-bit clients compatible with the rebuilt Wine server.
* Retain Proton selection when installing/updating Fenix and restore its original
  selection when explicitly restoring the pre-patch profile.
* Provide unified launch scripts with both the portable Store loader and Fenix
  geometry, helper-window and MCDU refresh support.

Fresh-source builds and native regression probes pass for both Proton overlays,
including 3,564 geometry reference cases, 800 concurrent metric checks and 768
stroke comparisons. Fresh .NET/Fenix dependency installation, 32-/64-bit CLR
execution, official FSDT preparation and Flightdeck profile round trips were
checked in disposable environments. Simulator/Fenix flight behavior and NVIDIA black-view reports
still require hardware/in-game validation. No Fenix, FSDT or Microsoft installer
or proprietary dependency is redistributed.

# 0.1.0-preview.3 — 24 September 2026

* Restore route path metrics instead of returning `E_NOTIMPL`.
* Respect stroke joins, caps, dashes and miter limits, removing long lines at
  acute route joins. Cache widened contours in geometry realizations.
* Download and verify the required geometry dependency from Microsoft during
  both fresh setup and updates; scope it to FenixDisplay.exe. No Microsoft DLL
  is bundled in this release.
* Keep matching X11/Xwayland helper windows off the desktop while preserving
  their internal visibility and startup events. Keep sign-in and other programs
  interactive; retain the portable guard for other Wine display drivers.
* Automatically refresh stale MCDU startup images after Fenix Display restarts.
  Restore the display preference and brightness without changing MCDU pages.
  The helper uses the simulator’s installed SimConnect library; no additional
  Microsoft binary is distributed.
* Update verified preview.1/preview.2 profiles while retaining aircraft,
  settings and the original restore point.

Built from clean, pinned source archives. Installer transaction checks and the
fresh-prefix loader, cursor, IOCP, audio and window-mapping probes pass. Graphics
checks pass 3,564 reference and 800 concurrent metrics cases, plus 768 stroke
comparisons with zero differing pixels.
Route behavior was also checked in the live cockpit before this clean rebuild.
The automatic MCDU refresh passed the live official display-restart check with
unchanged pages, retained brightness and unchanged simulator/System/CDU process
identities. See [mcdu-restart.md](mcdu-restart.md) for behavior and limits.

## 0.1.0-preview.2

* Fix invisible mouse pointers in the official Fenix Installer and livery
  manager by sharing selected Wine cursors between the WebView processes.
* Repair missing 64-bit UI-font registrations before opening Fenix, preventing
  affected installer startup crashes. Existing fonts come from the user's runner.
* Update verified preview.1 profiles without reinstalling Fenix or .NET, keeping
  aircraft, settings and the original restore point.
* Keep manual Fenix windows accessible and suppress Wine's separate fallback
  notification-area window.

The Wine overlay now contains ten modules. Complete corresponding source and
the upstream cursor-patch attribution are included. The pinned runner and
platform requirements are unchanged. This remains a community preview.

## 0.1.0-preview.1

First packaged version of the locally verified MSFS 2024 / Fenix fixes.

* App-scoped loader names and actual first-write tracking for copy-on-write pages.
* NT image names preserved for Xodus's inherited file descriptors; simulator
  basename/resource aliases preserved without writing its decrypted image to disk.
* RFC 3161 content decoding, in-process audio session notifications and correct
  remote-thread activation-context handling.
* Direct2D display effects, geometry groups/realizations, masks, layers and arcs.
* App-scoped unhinted DirectWrite outlines for complete FCU/radio digits.
* Wine server completion-wait lifetime fix.
* CPU rendering, Legacy readouts, standard font registration and normal autostart.
* Transactional setup, checksum/runner checks, restore and Flightdeck UI integration.

Known limits: pinned Xodus runner only; glibc 2.38+; MSFS 2024 only; no weather
radar in CPU mode. Ground cockpit verified on Hyprland; full flight, other desktops
and future Fenix/MSFS versions need further testing. Account activation uses the
official Fenix workflow.

Packaging checks: patches reproduce all 24 modified Wine source files. Synthetic
installer transactions cover corruption, wrong runners, custom scripts, locks,
active profiles and interrupted commit/restore. The stripped release binaries
passed loader-name, copy-on-write, 30,000-message IOCP and audio callback probes
in a separate fresh Wine prefix. Native .NET Framework 4.8 installation and font
registration completed in a separate test runtime. The window guard hides a
matching helper and leaves a different executable with the same title visible.
The Flightdeck flow was tested in Chromium with synthetic API jobs on desktop
and mobile; no account login or vendor aircraft was part of that automated test.
