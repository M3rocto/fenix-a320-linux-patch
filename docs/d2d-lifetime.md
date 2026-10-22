# Direct2D geometry lifetime and termination

Preview.9 addresses the per-figure leak in
[issue #8](https://github.com/marselnenaj/fenix-a320-linux-patch/issues/8)
on the pinned Xodus, Proton Experimental 11, CachyOS 10 and CachyOS 11 sources.
The three patches are applied after the existing ports in every manifest list.
Only `d2d1.dll` is rebuilt; the other Wine modules and DXGI remain unchanged.

## Memory ownership

`wine-d2d1-free-vertex-types.patch` backports Wine
[`4ae80e26e5`](https://github.com/wine-mirror/wine/commit/4ae80e26e5bd6bc30e0e7e7fe3b1a60895f896de)
by Giang Nguyen, contributed by M3rocto in
[PR #10](https://github.com/marselnenaj/fenix-a320-linux-patch/pull/10).
`d2d_figure_cleanup()` now frees the separately allocated
`vertex_types` array, including when a geometry is released after `Close()` fails.

The own-code regression warms the allocator, then creates and releases 300,000
geometries. It measures live allocations with `HeapWalk`, rather than relying
on the process working set dropping immediately. The original CachyOS 11 DLL
retained 15,641,640 bytes. The patched DLL retained 0 bytes, both for normal
closure and for an incorrect sink state followed by release.

## Degenerate curves

These synthetic failures are separate from the leak. M3rocto withdrew the
simulator-freeze diagnosis from issue #8 on 22 October 2026: the subdivision
loop exists, but further testing did not identify it as the cause of the
PFD/ND freeze in MSFS. Preview.9 does not claim to fix that simulator freeze.

`wine-d2d1-bezier-split-limit.patch` adapts the control-triangle area threshold
from M3rocto's withdrawn [PR #9](https://github.com/marselnenaj/fenix-a320-linux-patch/pull/9).
Subdivision stops when both areas are below `1e-5`, or either
area is non-finite. The latter condition matters when just one area is NaN:
an ordinary greater-than comparison could keep selecting that curve forever.
A line-intersection parameter must also be inside `[0, 1]`; this rejects NaN
from a zero-length line before it can introduce invalid path vertices.

The area threshold is absolute in path coordinates, before the world
transform. Very large transforms can magnify an overlap below that threshold.
Passing the rendering comparisons does not establish equivalence for every
possible transform or path.

`wine-d2d1-triangulation-cycle.patch` handles two additional failures found by
the new deterministic stress test: an edge cut revisiting five edges (case
2629), and segment insertion recurring with the same origin (case 7577).
Both walks are bounded by the number of edges. An impossible walk returns a
triangulation error through the existing cleanup path, instead of overflowing
the stack. These failures are distinct from the originally reported subdivision
loop.

## Regression checks

`tests/d2d-lifetime-probe.cpp` runs as part of `fenix-tools smoke`. Each of its
20,000 paths has a separate fixed seed and includes duplicate points, tiny
segments and control points close to their endpoints. A watchdog reports the
case number if `Close()` does not return within five seconds. Rerun a single
case with `d2d-lifetime-probe.exe --case 2629` under the tested runner.

The unpatched DLL hangs at case 67. With all three patches, the CachyOS 11
test completes 20,000 cases with a maximum `Close()` time of 32 ms and three
triangulation errors. The errors are recorded separately from termination;
they are not presented as successful renders. The existing reference geometry
and pixel comparisons remain separate checks in the full smoke test.

The final full smoke runs passed on all four pinned runners, including the
Steam container start for each Proton variant. All retained 0 live heap bytes
in both 300,000-figure modes, completed 20,000 paths, and reported the same
three triangulation errors. Observed maximum `Close()` times were:

| Runner | Maximum `Close()` |
| --- | ---: |
| Xodus | 33 ms |
| Proton Experimental 11 | 32 ms |
| CachyOS 10 | 32 ms |
| CachyOS 11 | 17 ms |

These timings are measurements from those runs, not execution-time guarantees.

These are synthetic regressions. A full Fenix flight, long-term process memory
behaviour and Fenix v3 flight compatibility have not been independently verified.
