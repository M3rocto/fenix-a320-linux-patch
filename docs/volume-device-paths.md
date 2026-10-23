# NT volume device paths

This experimental preview.10 change fixes the x64 Unix `ntdll.so` in each
pinned runner. A path obtained with
`GetFinalPathNameByHandleW(..., VOLUME_NAME_NT)` identifies its real NT volume,
for example `\Device\HarddiskVolumeN\folder\file.exe`. Applications can reopen
that file through the Win32 `\\?\GLOBALROOT` namespace.

The affected Wine versions rejected that alias with `ERROR_INVALID_NAME`
(123). Their NT-to-DOS conversion recognized an unprefixed `\Device\...`
name but missed the same device behind `\??\GLOBALROOT`. The patch recognizes
the complete namespace prefix and uses the existing live DOS-device symlink
lookup. It requires the complete device name to match, so a truncated name
cannot select another volume. It does not map a fixed volume number to C.

The same conversion also inserted a separator before a suffix already
starting with one. An input with two root separators consequently acquired
three and failed Wine's lower path parser, while the equivalent extended-DOS
path with two separators worked. The device branch now consumes the first
existing separator before adding its replacement. It preserves the caller's
separator count, leaves the lower parser unchanged and keeps triple root
separators rejected. The separate SystemRoot conversion is unchanged.
Root paths returned by `GetFinalPathNameByHandleW` retain their trailing
separator. Canonicalization of a reopened handle's `FILE_NAME_NORMALIZED`
result remains a separate API behavior; this patch does not change that
function.

## Regression coverage

`fenix-tools smoke` runs two original MIT probes in a fresh disposable prefix.
That prefix has distinct C and D scratch roots; the probes discover both NT
device identities with `QueryDosDeviceW`.

* `volume-roundtrip-probe.c` creates and reads ordinary files, compares
  reopened root identities, opens returned NT paths directly, and rejects
  truncated, nonexistent and similarly named invalid devices/namespaces.
* `volume-separators-probe.c` compares ordinary DOS, extended DOS, direct NT
  and GLOBALROOT file opens with one, two and three root separators. It also
  checks unchanged SystemRoot behavior using an ordinary file it creates and
  removes in the scratch Windows directory.

The focused baseline/candidate runs passed on Xodus, Proton Experimental 11,
CachyOS Proton 10 sunset and CachyOS Proton 11. The latter three used their
pinned Steam containers (Runtime 4, sniper and Runtime 4 respectively).
Each original overlay reproduced the failure; each matching rebuilt module
passed both probes. Existing SystemRoot controls and the 32-bit server ABI
check also passed, with no new ELF dependencies. The complete pinned source
archives and ordered patches reproduce the exact affected build input with
`--fuzz=0` on all four variants.

These checks cover the tested x64 drive-letter-backed volumes. They do not
establish support for the full NT namespace, volumes without DOS drive
letters, network paths or a 32-bit filesystem fix.

## Fenix observation

During the local aircraft **3.0.0.7454** investigation for
[issue #7](https://github.com/marselnenaj/fenix-a320-linux-patch/issues/7), the
reported invalid-name error occurred while the application reopened volume
roots and child executable paths before process creation. The diagnostic
path candidate cleared those failures: all three executable preflight checks
passed and CDU/Display processes started. Other compatibility candidates
were also present in that investigation; this is one resolved filesystem
blocker, not a standalone v3 compatibility result.

The integrated form restricts separator handling to the device branch. Its
native probes and a subsequent real Xodus retry also pass all three image
preflight checks. With the separate BCrypt candidate, authorization
preparation and System process launch completed. The other preview.10 fixes
then enabled System startup and user-confirmed displays on Xodus. A Wine
startup assertion and full-flight stability remain open; see
[the combined v3 result](fenix-v3.md).
