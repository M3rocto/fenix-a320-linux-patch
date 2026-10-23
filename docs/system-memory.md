# FenixSystem memory investigation

Issue [#12](https://github.com/marselnenaj/fenix-a320-linux-patch/issues/12)
tracks separate `FenixSystem.exe` growth in Fenix **2.4.0.4720**, MSFS 2024
through Steam Flatpak, preview.9, CachyOS 11, Fedora Kinoite 44 and RX 9070.
The [23 October follow-up](https://github.com/marselnenaj/fenix-a320-linux-patch/issues/12#issuecomment-6364948274)
supplies four aggregate CSVs. The [subsequent correction](https://github.com/marselnenaj/fenix-a320-linux-patch/issues/12#issuecomment-6367478956)
withdraws the large OutputReport estimate and identifies accumulating signing-
certificate data as the main new lead.
We have independently recalculated the supplied counters; we have **not**
independently reproduced the Fenix runtime behavior.

## What the measurements establish

Process sizes below are converted to **MiB**, and rates use **MiB/min**.
A linear fit is ordinary least squares against recorded elapsed time;
an endpoint rate is the first-to-last change
divided by that interval. Startup, reloads and instrumentation can make these
estimates differ.

| Evidence | Recalculated result |
| --- | --- |
| [Unmodified sampler, stationary](https://github.com/user-attachments/files/33870748/fenix-system-memory-stationary-20261023.csv) | 120 samples, one PID, **29 min 45 s**. RSS **684.56 → 1569.23 MiB**: **29.41 MiB/min** linear fit, **29.74 MiB/min** endpoint rate. |
| Same stationary series, memory categories | RSS **+884.66 MiB**, anonymous RSS **+884.70 MiB**, private dirty **+884.66 MiB**. File RSS stays **43.32 MiB**, shared RSS **27.96 MiB**, swap **0**. |
| [Flight time series](https://github.com/user-attachments/files/33870750/flight-20261023-rss.csv) | 469 samples covering **1 h 57 min 11 s**, including ground/paused samples. Takeoff through the last rollout sample spans **1 h 52 min 55 s**, consistent with the reported 1 h 53 min flight. System **710 → 4540 MiB**, **32.68 MiB/min** endpoint rate across the full CSV. Display stays **636–668 MiB** after the first eight minutes. |
| [Stationary comparisons](https://github.com/user-attachments/files/33870751/stationary-tests-smaps-20261023.csv) | Growth continues in paused and loaded ground segments. The no-web-client and connected-EFB segments have linear-fit rates **31.22** and **30.90 MiB/min**. After the telemetry-off restart settles, its recorded interval has a **30.01 MiB/min** linear-fit rate. |
| [Earlier preview.8 flights](https://github.com/user-attachments/files/33870749/earlier-flights-rss.csv) | System endpoint rates are **28.20**, **32.88** and **30.95 MiB/min** in the three separately labelled runs, including CPU display mode and a local Direct2D-fix test. |

The strongest new result is sustained **private/anonymous System growth**,
rather than increasing resident file mappings. The stationary sampler uses
15-second intervals: 120 points span 119 intervals, not a full 30 minutes.
Its reported 29.4 MiB/min agrees with the linear fit.

The other CSVs need their state and PID boundaries preserved. The stationary
comparison file contains startup drops, process restarts, a segment labelled
as a tool-induced simulator hang and a snapshot-associated **45 MiB** file-PSS
increase. Those are not clean steady-state intervals. The telemetry-off fit above uses the
55 samples from **10:50:10–11:03:42 UTC**, after the startup drop; the whole
new-PID segment instead has a **24.58 MiB/min** endpoint rate. Paused segments
in this CSV are shorter than the 20-minute comparison described in the
comment. These observations support continuing growth; they do not establish
identical rates in every state or rule out every EFB/telemetry interaction.

## Correction: OutputReport is a secondary finding

The reporter explicitly withdrew the earlier **~50 KB per report** estimate:
it was back-calculated from total growth, rather than measured. Our earlier
numerical comparison using 50 KiB must therefore not be treated as evidence
that reports explain the main growth.

The corrected graph has eight dictionaries, usually two populated, and
approximately **1.4 KB** of owned data per report: roughly **0.8–0.85 MiB/min**
at ten reports/s. Shared keys are counted once. Three same-PID snapshots at
**13:48:01, 13:58:02 and 14:08:03 UTC** have 601-second gaps. Counts rise
from **4,900 to 16,921** over 1,202 seconds, consistent with ten/s net growth.
Reported identified managed objects grow **13.0** and **7.2 MiB** per interval.

The reported path is `ConcurrentQueue<T>` → 529 segments → 529 item arrays
→ 16,916 wrappers → 16,916 of 16,921 reports. No root above the queue was
traced. The .NET Framework **4.8.4110** DAC did not initialize; the reporter
instead scanned aligned MethodTable headers and parent chains in an
unsuspended process. This is a non-atomic object census, **not a GC-rooted
live-object walk**. Net growth and the supplied chain do not establish zero
releases, a never-started consumer, or all live ownership paths.

## Main new lead: repeated signing-certificate data

The same follow-up attributes **293** and **266 MiB** per interval to
anonymous regions without recognized managed-object headers, containing
copies of the executable's Authenticode X.509/PKCS#7 signing chain. Over
601 seconds these correspond to **29.25** and **26.56 MiB/min**, or **27.90**
combined. One CA-name count rises **35,810 → 86,645 → 137,497**: about
**84.6 occurrences/s**. A string occurrence is not an API-call count or an
independent allocation; the native/managed classification remains the
reporter's inference from that census.
Freed heap blocks can retain certificate strings and nonzero bytes, so their
presence alone does not establish live allocations or leaked handles.

The [next observation](https://github.com/marselnenaj/fenix-a320-linux-patch/issues/12#issuecomment-6367877773)
reports executable opens at ten/s beginning about 70 seconds after restart,
near the onset of growth. At 30 MiB/min that would be approximately **51.2 KiB
per open**, if each open maps to one leaking operation. The fanotify opening
PID/TID and actual API/close counts were not supplied. File opens alone do not
identify `CryptQueryObject`, a caller, or a release imbalance. The later
certificate snapshots do not overlap the attached CSVs, so their attribution
cannot be independently recalculated from those counters.

The existing reload CSV only partly supports a common 60–90-second flat
startup phase: the first five samples of new PID 40808 span 60 seconds and
rise 5 MiB, but new PID 22034 instead rises **427 → 687 MiB in 46 seconds**.
Both have a later 66–67 MiB downward step around 106 seconds. Fixed
120–300-second windows have **29.40** and **30.74 MiB/min** linear-fit rates.
These windows are timed from the first new-PID sample, not a recorded process
creation time.
These startup allocations and drops do not independently establish identical
onset timing or a constant 32–36 MiB/min in both runs. The fanotify observation
is a useful separate reporter finding, not yet an attributed API trace.

Microsoft's [CryptQueryObject contract](https://learn.microsoft.com/en-us/windows/win32/api/wincrypt/nf-wincrypt-cryptqueryobject)
assigns the returned message and certificate-store handles to the caller.
The matching Wine path duplicates the message for its message-backed store;
both [CryptMsgClose](https://learn.microsoft.com/en-us/windows/win32/api/wincrypt/nf-wincrypt-cryptmsgclose)
and [CertCloseStore](https://learn.microsoft.com/en-us/windows/win32/api/wincrypt/nf-wincrypt-certclosestore)
matter. This contract does not show which API Fenix uses or whether it closes
every handle/context. The pinned crypt32 includes our RFC3161 content patch;
the pinned wintrust includes the PE indirect-data patch. Reviewing Wine main
alone cannot rule out Wine, the overlay, or managed-wrapper behavior.
The reporter's actual preview.9 loaded-module hashes have not been verified;
the own-code comparison below uses the current pinned candidate.

## Bounded own-code control

The original [certificate query probe](../tests/certificate-query/README.md)
opens a public signed Windows .NET **10.0.13** `dotnet.exe` as a read-only
signature fixture; it never executes that file or starts Fenix. A fresh-prefix
run with the exact patched CachyOS 11 runner completes **8,384 queries** with
zero API/close failures. After 128 warmup queries, a 4,000-query close-both
series adds only **48 KiB** of working set. Retaining 256 handle pairs adds
about **9.63 MiB**; after closing them, another 4,000-query series is flat.
The working set stays high after release, consistent with allocator reuse;
returning pages to the OS is not required for release. Wine reports zero for
`PrivateUsage` throughout this test, so that field is not used as evidence.

A second fresh-prefix run substitutes only the matching stock `crypt32.dll`
and `wintrust.dll`. It also completes **8,384 queries** without failures:
**36 KiB** working-set increase in the first closed series, **9.77 MiB** with
256 retained pairs, and a flat second closed series. Prefix-module hashes
confirm the selected stock/overlay DLLs. No material contrast appears for
this fixture and query path; this does not test other trust-verification APIs.

This short native control does not reproduce the large growth on the tested
signature/query path. It does not reproduce Fenix 2's certificate, .NET
Framework wrapper, other trust APIs, or long-running process, and does not
establish Windows behavior. Existing aggregate API/close counts and the file-
opening actor, if already retained, are the useful next evidence. No new
flight, process-memory collection or dump upload is requested.

The reporter is submitting the certificate lead to Fenix support. Keep #12
open: no System-memory fix, platform attribution or Fenix v3 qualification is
claimed. The working Direct2D Display correction remains in place.

## Optional aggregate sampling

The requested stationary and flight counters have been supplied. Keep the
working patch in place; a further flight is not required merely to repeat them.
For a separate comparison, record version/settings and start, aircraft reload
and simulator exit times, using a new output filename for each process.

The optional Python 3 helper
[sample-system-memory.py](../tools/sample-system-memory.py) reads only selected
aggregate counters from `/proc/<pid>/status` and `smaps_rollup`. Select the
Linux PID of the intended `FenixSystem.exe` process, then run:

```sh
python3 tools/sample-system-memory.py --pid 12345 \
  --interval 15 --samples 120 --output fenix-system-memory.csv
```

Replace `12345` with the actual PID. This records about thirty minutes; choose
another sample count for an existing test. Use a new output filename each time.
The helper stops on process exit/PID reuse or inaccessible counters and retains
partial output. Missing kernel counters remain blank, rather than becoming zero.

The CSV contains UTC time, elapsed time, PID, RSS/PSS, anonymous/file/shared RSS,
private clean/dirty memory and swap, in KiB. It does not read process memory,
command lines, environments, mapped filenames or account files. Share the CSV
and version/settings context in #12. There is no System-memory fix or v3
qualification implied by this diagnostic.
