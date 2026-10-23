# 0.1.0 — in preparation, not published

The regular release remains an unpublished draft. Refresh its retained
archives with the preview.11 Gateway correction before publication. The
separate System-memory investigation [#12](system-memory.md), full-flight v3
qualification and the earlier Wine assertion remain open.

See [release readiness](release-readiness.md).

# 0.1.0-preview.11 — experimental, 23 October 2026

* Include @malik05051's [PR #15](https://github.com/marselnenaj/fenix-a320-linux-patch/pull/15):
  add `Fenix.GqlGateway.exe` to the existing loader-name allowlist. Rebuild
  matching x64 PE NTDLL for all four runners and repin the tested payloads.
  The reporter confirms Gateway logging, port 8083 and a working EFB on
  Fenix 3.0.0.7454 / Steam MSFS 2020 / CachyOS 11 / NVIDIA-PRIME.
* Document working GPU displays on that setup with **USE DISPLAY SAFE MODE
  off**. This setting creates two devices instead of thirteen in the supplied
  comparison. CPU mode remains available; no new graphics override is forced.
* Preserve existing Fenix loader behavior and unrelated executable names.
  All **64** own-code baseline/candidate name comparisons pass across four
  runners. Smoke now checks Gateway names and negative near-matches.

* Explicitly select server synchronization for Fenix: `PROTON_NO_NTSYNC=1`
  for pinned Wine 11 and `WINE_DISABLE_FAST_SYNC=1` for CachyOS 10. Steam
  updates retain other Python settings while adding these switches; the
  standalone application's environment and legacy launch script agree.
  Synchronization alone did not fix #7; the Gateway correction is the separate
  loader-name change above.
* Retain exact preview.10 snapshots for upgrades; native overlay modules
  except for the four rebuilt PE NTDLL files; runner pins stay unchanged.
* Rewrite English/German READMEs around setup, evidence, restore and
  troubleshooting. Add an own-code graphics-initialization diagnostic and
  document the corrected #7 findings and remaining #12 work. The earlier
  OutputReport-size estimate is withdrawn; certificate-related accumulation
  is the main qualified System-memory lead. No System-memory fix is claimed.

See [release readiness](release-readiness.md) and the
[Display/Gateway investigation](display-startup.md). The published download
is preview.11; this section does not announce a regular release.

# 0.1.0-preview.10 — experimental, 23 October 2026

This test release reaches user-confirmed cockpit displays with Fenix
**3.0.0.7454**, official Installer **1.0.359**, MSFS 2024 **1.8.16.0** and
Xodus **11.0 / 7c0b4354**. **Known issue:** one startup hit Wine's
`create_view` assertion after System boot. A later fresh production run and
five warm backend restarts did not reproduce it; its cause remains unknown.
No full flight or MSFS 2020/GE regression check has been completed.
See [the test sequence and limits](fenix-v3.md).

* Return the native no-package error from WinRT `ApplicationData.Current`
  for ordinary desktop processes. The new installer's existing SQLite
  fallback then works, and the official aircraft installation completes.
* Implement a bounded SHA1/SHA256 PE indirect-data route for
  `CryptSIPCreateIndirectData`, replacing the startup stub. Complete native
  SIP representation and certificate trust conformance are not established.
* Reopen `GLOBALROOT` aliases through their actual volume mapping, preserve
  separator counts, and store a rooted current directory. This clears the
  brief invalid-name failure and later engine/content-root lookup failures.
* Map the missing PBKDF2 pseudo-handle to Wine's existing implementation.
* Support unnamed ephemeral NCrypt `ECDH_P256` keys with scoped BCrypt
  parameter validation, provider lifetime and HASH/raw derivation handling.
  Other curves, named keys and the full CNG API remain outside this change.
* Include matching builds and independent native controls for all four
  pinned runners. Real v3 cockpit checks cover Xodus only. Verified preview.9
  installations can update without reinstalling the aircraft or resetting
  renderer preferences and the original restore point.

The vendor executables and their authentication remain unchanged. Diagnostic
memory logging, APC and KernelBase experiments are excluded from the payload.
The issue remains open for stability investigation. Flightdeck's
separate patch pin was subsequently updated in Flightdeck **0.2.18**. A later
Steam/MSFS 2020 NVIDIA report confirms installation/login/System but still
fails Display and Gateway startup; see [the follow-up](display-startup.md).

# 0.1.0-preview.9 — 22 October 2026

* All four overlays free Direct2D figure vertex types, backporting Wine
  `4ae80e26e5` from M3rocto's PR #10. This fixes the per-geometry leak
  reported in issue #8. A later community 1 h 53 min v2 flight reported stable
  Display memory and no frozen displays. Its separate System RSS growth
  remains open in [#12](https://github.com/marselnenaj/fenix-a320-linux-patch/issues/12).
* Nearly coincident Bézier control triangles have a subdivision limit;
  non-finite areas and invalid line-intersection parameters are rejected.
  The new stress test also found two cyclic triangulation walks, which now
  return an error through the normal cleanup path. Only `d2d1.dll` changes in
  each Wine overlay. These failures reproduce with synthetic paths; they are
  not a confirmed cause or fix for displays freezing in the simulator. See
  [geometry lifetime and termination](d2d-lifetime.md)
  for the reproducible checks and the absolute path-unit tolerance.
* The standalone installer prepares the pinned Microsoft .NET 10.0.13 x64
  Desktop Runtime for Fenix builds reporting that missing prerequisite
  (issue #7), alongside .NET Framework 4.8. The runtime is also prepared when updating an existing
  Steam installation and checked before opening an official Fenix application;
  an incomplete installation prevents that launch. Downloads are checksum-verified and not included
  in the release. Runtime installation, host discovery and an own-code WPF
  window were checked in a separate CachyOS 11 profile; an actual v3 flight
  remains unvalidated.
* Setup explicitly asks to sign in and install the A320 for the selected
  simulator inside the official installer, wait for aircraft installation to
  finish, then close it (issue #6).
* Verified preview.8 installations are recognized as update sources. Existing
  aircraft, renderer preferences, sign-in and the original restore point are
  retained by the existing update flow.

# 0.1.0-preview.8 — 20 October 2026

* Preview.8 adds GPU display transport for the pinned Xodus and Proton 11
  overlays. Wine records the shared texture's real adapter, and DXGI implements
  the owner query that previously returned `E_NOTIMPL`. Proton 11 also shares
  virtual fence state safely between processes instead of dereferencing a
  foreign pointer or returning physical Vulkan ticks.
* Each GPU overlay carries fourteen matching Wine modules and DXGI built from
  its own runner's exact DXVK revision. D3D11 and VKD3D are retained. Complete
  source archives, recursive submodule pins and the DXVK licence are included.
  CachyOS Proton 10 retains the CPU path.
* Existing automatic CPU settings migrate to GPU on capable overlays.
  `configure --renderer cpu|gpu` records a persistent preference, retained
  across later configuration and upgrades. Older and CPU-only variants cannot
  inherit or force-enable GPU support. Native DXGI is updated in an existing
  Windows profile as well as in the runner.
* The Xodus GPU path has been checked in a running Fenix cockpit and with a
  controlled moving airspeed tape. Own-code resource and fence tests also cover
  multiple GPUs, separate processes, pixel transfer, handle lifetime and
  Proton 11's virtual values. Full flights, NVIDIA and GPU weather radar remain
  unvalidated. See [GPU displays](gpu-displays.md) for the precise scope.

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
