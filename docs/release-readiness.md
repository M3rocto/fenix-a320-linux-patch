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
| v3 Display/Gateway failures, [#7](https://github.com/marselnenaj/fenix-a320-linux-patch/issues/7) | Disabling ntsync did not help. Display Vulkan device creation returns `VK_ERROR_INITIALIZATION_FAILED`; a Display-only descriptor-heap comparison is pending. Gateway's sustained spin remains unresolved; a separate ReadyToRun comparison is pending. See [startup diagnostics](display-startup.md). |
| FenixSystem memory growth, [#12](https://github.com/marselnenaj/fenix-a320-linux-patch/issues/12) | Separate v2 report; aggregate measurements requested. Cause and fix unknown. The Display geometry correction is not a System-memory fix. |
| v3 stability | User-confirmed local Xodus cockpit and bounded production restarts; full-flight stability and the earlier Wine assertion remain unresolved. |
| Packaging and upgrades | Preview.10 hashes retained as upgrade snapshots; native runner/module pins unchanged. Local static-installer packaging, source/hash verification and real-archive upgrade checks on all four pinned runners pass. Recheck the final artifacts before publication. |
| Flightdeck | Public 0.2.18 pins preview.10. [Draft 0.2.19 integration](https://github.com/marselnenaj/flightdeck-msfs-2024-linux-xbox/pull/6) prepares the synchronization changes and a private candidate pin; it must use the exact final public archive and checksum before merging or shipping. |

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
