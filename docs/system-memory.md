# FenixSystem memory investigation

Issue [#12](https://github.com/marselnenaj/fenix-a320-linux-patch/issues/12)
tracks a separate observation from Fenix **2.4.0.4720**, MSFS 2024, preview.9,
CachyOS 11, Fedora Kinoite 44 and RX 9070. During a reported 1 h 53 min flight,
System RSS rose from about 710 MB to 4.5 GB, approximately 32 MB/min. Display
memory remained around 640–668 MB after warm-up, without frozen displays.

This has not been reproduced independently. RSS alone cannot distinguish a
leak, retained application data, runtime heaps or resident file mappings.
The Direct2D Display leak fix does not establish a fix for System memory.

## Useful measurements

Keep the working patch in place. Compare a stationary session and a flight
with the same version/settings; note start, aircraft reload and simulator exit
times. The next evidence is timestamped **System** RSS, anonymous/file-backed
RSS and private memory. Existing measurements are useful; a new flight is not
required just to share them.

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
