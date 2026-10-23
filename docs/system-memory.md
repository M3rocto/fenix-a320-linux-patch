# FenixSystem memory investigation

Issue [#12](https://github.com/marselnenaj/fenix-a320-linux-patch/issues/12)
tracks separate `FenixSystem.exe` growth in Fenix **2.4.0.4720**, MSFS 2024
through Steam Flatpak, preview.9, CachyOS 11, Fedora Kinoite 44 and RX 9070.
The [23 October follow-up](https://github.com/marselnenaj/fenix-a320-linux-patch/issues/12#issuecomment-6364948274)
supplies four aggregate CSVs and an application-object retention hypothesis.
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

## OutputReport hypothesis and remaining evidence

The reporter describes two object-count snapshots **1020 seconds** apart:
`ProsimCore.OutputReport` and each of seven dictionary families increase by
**10,200** instances. The named report fields are `numerical`, `gate`,
`indicators`, `globalNumerical`, `globalGauge`, `globalGates` and
`globalIndicators`. They report that the dictionaries and their arrays are
owned by the reports, which are retained in an undrained hardware-output queue.
These type counts, ownership paths and queue behavior are **reporter findings**;
the attached CSVs contain aggregate process counters, not object/root evidence.

A net increase of ten report graphs per second at approximately **50 KiB**
each would account for **29.30 MiB/min**, close to the measured stationary
rate. This makes retained report graphs a useful lead. Two counts establish
net growth, not that no instances were released or that a consumer never
started. Heap census counts also need a distinction between enumerated and
GC-rooted live objects. Microsoft's [managed-leak investigation guide](https://learn.microsoft.com/en-us/dotnet/core/diagnostics/debug-memory-leak)
explains the separate roles of type statistics and reference-root analysis.

The next useful material is any **already retained, aggregate text** from that
analysis: tool/runtime versions and snapshot times; per-type before/after
counts and byte totals; whether those are live/rooted objects; queue counts;
and a root-to-queue-to-report path containing only type and field names.
Count shared backing objects once when estimating retained bytes. No new
flight or memory-dump upload is needed to provide existing summaries.

A Fenix support report can include the exact setup, the supplied CSVs, the
private/anonymous growth and the qualified OutputReport lead. The lack of a
consumer and an upstream cause remain unconfirmed; no matching Windows
control was supplied. Namespace and interface names alone do not establish
the owning failure or platform independence. Keep #12 open while that is
investigated. No System-memory fix or Fenix v3 qualification is claimed.
The working Direct2D Display correction remains in place.

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
