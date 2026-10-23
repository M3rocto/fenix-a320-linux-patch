# Preparing 0.1.0

The published patch is **0.1.0-preview.10**. The [candidate in PR #11](https://github.com/marselnenaj/fenix-a320-linux-patch/pull/11) prepares
**0.1.0**; there is no published regular 0.1.0 release yet.

The intended release carries the existing matched native overlays, the v3
startup fixes, explicit server-synchronization settings and clearer setup
documentation. Its supported configurations and known limitations must match
the available evidence.

## Open work

| Item | Current state |
| --- | --- |
| v3 Display/Gateway failures, [#7](https://github.com/marselnenaj/fenix-a320-linux-patch/issues/7) | Graphics initialization narrowed down; effective NVIDIA HRESULT unknown. Gateway spin not reproduced. Source configuration corrected; reporter asked to test the exact ntsync switch. |
| FenixSystem memory growth, [#12](https://github.com/marselnenaj/fenix-a320-linux-patch/issues/12) | Separate v2 report; aggregate measurements requested. Cause and fix unknown. The Display geometry correction is not a System-memory fix. |
| v3 stability | User-confirmed local Xodus cockpit and bounded production restarts; full-flight stability and the earlier Wine assertion remain unresolved. |
| Packaging and upgrades | Preview.10 hashes retained as upgrade snapshots; native runner/module pins unchanged. Candidate checks must pass before publication. |
| Flightdeck | 0.2.18 pins preview.10. Prepare matching synchronization changes; switch the patch pin after the final public archive and checksum exist. |

## Publication sequence

1. Resolve the reported failures or explicitly agree a narrower release scope
   supported by runtime evidence.
2. Complete candidate installer, upgrade/restore, source and native checks.
3. Build and verify ZIP, corresponding source and checksum manifest; publish
   a new `v0.1.0` release without rewriting earlier releases.
4. Update Flightdeck to the public manifest, validate real archive upgrades on
   all four pinned runners and publish the matching Flightdeck version.
5. Reply to issues with exact fixed versions and validation limits.

Flightdeck's production patch pin is unchanged during this preparation.
