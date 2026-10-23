#!/usr/bin/env python3
# SPDX-License-Identifier: MIT
"""Sample aggregate Linux memory counters for one explicitly selected PID."""
import argparse
import csv
import datetime
import math
from pathlib import Path
import sys
import time


def identity(proc):
    # Fields after the final ')' begin with stat field 3; starttime is field 22.
    return (proc / 'stat').read_text().rsplit(')', 1)[1].split()[19]


def counters(path):
    result = {}
    for line in path.read_text().splitlines():
        key, separator, value = line.partition(':')
        words = value.split()
        if separator and len(words) == 2 and words[1] == 'kB':
            result[key] = int(words[0])
    return result


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--pid', required=True, type=int)
    parser.add_argument('--output', required=True, type=Path)
    parser.add_argument('--interval', type=float, default=15)
    parser.add_argument('--samples', type=int, default=120)
    args = parser.parse_args()
    if args.pid <= 0 or args.samples <= 0 or not math.isfinite(args.interval) or args.interval <= 0:
        parser.error('PID, interval and sample count must be positive finite values')
    proc = Path('/proc') / str(args.pid)
    try:
        initial = identity(proc)
        name = (proc / 'comm').read_text().strip()
        columns = ['timestamp_utc', 'elapsed_s', 'pid', 'rss_kib', 'pss_kib',
                   'rss_anon_kib', 'rss_file_kib', 'rss_shmem_kib',
                   'private_clean_kib', 'private_dirty_kib', 'swap_kib']
        # Never read command lines, environment, mapped file names or memory contents.
        started = time.monotonic()
        with args.output.open('x', newline='') as output:
            writer = csv.writer(output)
            writer.writerow(columns)
            for sample in range(args.samples):
                if identity(proc) != initial:
                    raise RuntimeError('PID was reused; sampling stopped')
                status = counters(proc / 'status')
                rollup = counters(proc / 'smaps_rollup')
                if identity(proc) != initial:
                    raise RuntimeError('PID changed while reading counters')
                writer.writerow([
                    datetime.datetime.now(datetime.timezone.utc).isoformat(),
                    round(time.monotonic() - started, 3), args.pid,
                    rollup.get('Rss', ''), rollup.get('Pss', ''),
                    status.get('RssAnon', ''), status.get('RssFile', ''),
                    status.get('RssShmem', ''), rollup.get('Private_Clean', ''),
                    rollup.get('Private_Dirty', ''), rollup.get('Swap', ''),
                ])
                output.flush()
                if sample + 1 < args.samples:
                    time.sleep(max(0, started + (sample + 1) * args.interval - time.monotonic()))
        print(f'{name}: wrote {args.samples} aggregate samples to {args.output}')
        return 0
    except (OSError, ValueError, IndexError, RuntimeError) as error:
        print(f'Sampling stopped: {error}. Any partial CSV is retained.', file=sys.stderr)
        return 1


if __name__ == '__main__':
    sys.exit(main())
