# Fenix v3 compatibility and validation

The experimental **0.1.0-preview.10** installs aircraft
**3.0.0.7454** through official Fenix Installer **1.0.359**, completes login
and System startup, and reaches **user-confirmed working cockpit displays**.
The real test used MSFS 2024 **1.8.16.0**, Xodus Wine **11.0 / 7c0b4354**,
GPU displays and Legacy readouts. It was an isolated installation with the
original Fenix executables and ordinary authentication.

An experimental download is provided for further testing. One startup hit
Wine's memory-view assertion, whose cause remains unknown. The issue remains
[open](https://github.com/marselnenaj/fenix-a320-linux-patch/issues/7#issuecomment-6350550099).

A later Steam/MSFS 2020 report with pinned CachyOS 11 and NVIDIA/PRIME
confirms installation, login, System and CDU, but encounters a Display
initialization exception and an early Gateway spin. These are separate from
earlier installer/path/crypto failures. See [the follow-up](display-startup.md).
The 0.1.0 source candidate corrects a synchronization configuration mismatch;
the reported failures are not yet confirmed fixed.

## Compatibility changes

| Observed failure | Wine change |
| --- | --- |
| Installer SQLite initialization throws at `ApplicationData.LocalFolder` | [Return the no-package error](winrt-storage.md), allowing the existing desktop fallback |
| Main launcher exits at the SIP stub | [Bounded PE SHA1/SHA256 indirect data](pe-indirect-data.md) |
| Brief invalid-name error before child image validation | [Resolve GLOBALROOT through live volume identities](volume-device-paths.md) |
| Authorization preparation rejects PBKDF2 handle `0x331` | [Map the documented pseudo-handle](pbkdf2-pseudo-handle.md) |
| System link rejects ephemeral `ECDH_P256` | [Scoped NCrypt key/secret operations and BCrypt validation](ncrypt-ecdh.md) |
| Relative engine-model and service content-root lookups use bare GLOBALROOT paths | [Store an absolute current directory](current-directory.md) |

All four pinned runners have matching source/binary builds and pass the
independent native controls for these changes. Their complete archive and
ordered patch replays match the built sources. These API checks do not
establish a working v3 cockpit on every runner.

## Actual runtime observations

The official installer completes all four update plans and the aircraft
installation. The three unchanged child executables pass image validation.
After the working-directory fix, the engine-model and content-root errors
disappear; System boots, connects through SimConnect and starts its gateway.
A read-only 30-second SimConnect observation records simulator frames and
changing engine values, separately from the visual confirmation.

One complete production-payload start then hits
`ntdll/unix/virtual.c:create_view: view->protect & VPROT_SYSTEM`, followed by
System exit status `0x80000101`. No Linux core dump or OOM event was found.
An unchanged production repeat boots and runs for about five minutes without
the assertion. The next run adds only address/protection/caller logging to
Unix NTDLL, retains the assertion and also avoids it. During that run the
tester reports that the displays work, then closes MSFS. This final visual
initial confirmation therefore included diagnostic logging. The follow-up
below checks the production payload separately.

The diagnostic module is excluded from the payload and was restored to the
production module after the test. Independent allocation/placeholder controls
pass 10,753 checks with write-copy tracking both disabled and enabled. Those
controls do not reproduce or explain the application assertion. No speculative
memory-manager change is included.

On 23 October, a fresh isolated MSFS launch and all twenty production module
hashes were checked, with no diagnostic module or new graphics override.
The tester confirmed the displays again, and System, Display, CDU and the
gateway were running with System reporting Connected. Five additional warm
Fenix backend restarts all connected and remained alive for at least sixty
seconds after connection. No assertion or invalid-stack-frame signal appeared
in these six starts. The simulator stayed running between backend restarts;
these are not six simulator cold starts. Read-only SimConnect observations
also confirm aircraft/frame data and changing engine values.

Additional independent NativeAOT-shaped controls pass 6,465 GC
reserve/partial-commit/reset/decommit/write-watch and RX/RW checks, plus
4,815 checks involving three disjoint sparse eight-GiB arenas and persistent
RX/RW data pools. They do not reproduce the assertion. The primary .NET
10.0.13 source is a control/reference; the actual application's 10.0.12-dev
runtime and allocation schedule are not established by it. Although its
banner says NativeAOT, inspection finds CIL/ReadyToRun payloads and a native
single-file host consistent with statically linked CoreCLR. The banner alone
does not support a pure NativeAOT classification.

The repeated DXVK `Cannot relocate image` messages were also checked with
an independent shader-draw/shared-copy probe using the same D3D11 revision.
On the tested AMD/RADV setup, 24 matching messages accompany 147,456 exact
pixel comparisons with no mismatches or timeouts, including a second-device
import. Source inspection identifies a rejected optional inline optimization
followed by the normal copy. This bounded result does not establish every
application transport case; it provides no reason to change graphics defaults
or suppress errors. No additional graphics setting is included.

## Remaining limits

The startup assertion's cause and recurrence rate remain unknown. A full
flight, long-session stability, individual display/mode coverage, Steam and
MSFS 2020/GE behavior, other runners in a real v3 cockpit, NVIDIA and weather
radar remain unverified. The short display confirmation does not close the
reported freeze/stall issue.

The published release is experimental and the draft PR remains open.
Flightdeck 0.2.18 pins preview.10; future changes require a separate manifest
update. No general startup or long-flight stability guarantee follows
from the bounded successful repeats.

The bounded SIP output representation is not fully compared with native
Windows, and NCrypt support covers only the documented subset linked above.
The .NET 10 prerequisite preparation from preview.9 is retained; it does not
explain the .NET 8 installer's separate SQLite failure. No general .NET/CNG
compatibility or need for an extra global runtime is inferred from these tests.
