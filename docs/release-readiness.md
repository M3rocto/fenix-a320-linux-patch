# Preparing 0.1.0

The published patch is **0.1.0-preview.10**. The current source prepares
**0.1.0**; its GitHub release is an unpublished draft. There is no published
regular 0.1.0 release yet.

The intended release carries the existing matched native overlays, the v3
startup fixes, explicit server-synchronization settings and clearer setup
documentation. Its supported configurations and known limitations must match
the available evidence.

## Open work

| Item | Current state |
| --- | --- |
| v3 Display/Gateway failures, [#7](https://github.com/marselnenaj/fenix-a320-linux-patch/issues/7) | Neither disabling ntsync/descriptor heaps nor enabling ReadyToRun resolves the report. Display Vulkan device creation fails with both descriptor-heap and descriptor-buffer models. A serial/parallel retained-device diagnostic and current Gateway register/backtrace samples are the next evidence. See [startup diagnostics](display-startup.md). |
| FenixSystem memory growth, [#12](https://github.com/marselnenaj/fenix-a320-linux-patch/issues/12) | Supplied v2 CSVs confirm private/anonymous growth: stationary linear fit 29.41 MiB/min. Reporter-identified OutputReport retention is a lead; object/root summaries and platform attribution remain unverified. No System-memory fix is claimed. See [measurements and limits](system-memory.md). |
| v3 stability | User-confirmed local Xodus cockpit and bounded production restarts; full-flight stability and the earlier Wine assertion remain unresolved. |
| Packaging and upgrades | Preview.10 hashes retained as upgrade snapshots; native runner/module pins unchanged. Local static-installer packaging, source/hash verification and real-archive upgrade checks on all four pinned runners pass. Recheck the final artifacts before publication. |
| Flightdeck | Public 0.2.18 pins preview.10. The 0.2.19 source aligns synchronization settings and retains the public preview.10 pin. A switch to 0.1.0 requires the exact final public archive and checksum. The prepared manifest is retained with the unpublished Fenix release draft. |

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
