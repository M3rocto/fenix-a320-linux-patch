# Preparing 0.1.0

The patch release is **0.1.0-preview.11**. A regular **0.1.0** remains in
preparation with an unpublished GitHub draft. That retained draft's archives
precede the Gateway correction and must be refreshed before publication.
There is no published regular 0.1.0 release yet.

The intended release carries the existing matched native overlays, the v3
startup fixes, explicit server-synchronization settings and clearer setup
documentation. Its supported configurations and known limitations must match
the available evidence.

## Open work

| Item | Current state |
| --- | --- |
| v3 Display/Gateway report, [#7](https://github.com/marselnenaj/fenix-a320-linux-patch/issues/7) | The reporter confirms GPU displays with Display Safe Mode off and Gateway/EFB after the loader-name fix in PR #15. Preview.11 rebuilds all four PE NTDLLs; 64 independent process-name comparisons pass. See [startup diagnostics](display-startup.md). |
| FenixSystem memory growth, [#12](https://github.com/marselnenaj/fenix-a320-linux-patch/issues/12) | Supplied v2 CSVs confirm private/anonymous growth: stationary linear fit 29.41 MiB/min. The reporter withdrew the large OutputReport estimate; repeated signing-certificate data is the main new lead. A bounded own-code query control does not reproduce sustained growth when both returned handles are closed. The Fenix caller and platform attribution remain unverified; no System-memory fix is claimed. See [measurements and limits](system-memory.md). |
| v3 stability | User-confirmed local Xodus cockpit and bounded production restarts; full-flight stability and the earlier Wine assertion remain unresolved. |
| Packaging and upgrades | Preview.10 snapshots retained. Only four PE NTDLL payload pins change for preview.11; native exports, source replay and loader controls pass. The regular draft still requires current source/archive verification. |
| Flightdeck | 0.2.19 adopts the exact preview.11 archive and aligns synchronization settings. A switch to regular 0.1.0 requires its final public archive and checksum. The old prepared stable manifest is superseded by the new native payloads. |

## Publication sequence

1. Resolve the reported failures or explicitly agree a narrower release scope
   supported by runtime evidence.
2. Complete candidate installer, upgrade/restore, source and native checks.
3. Build and verify ZIP, corresponding source and checksum manifest; publish
   a new `v0.1.0` release without rewriting earlier releases.
4. Update Flightdeck to the public manifest, validate real archive upgrades on
   all four pinned runners and publish the matching Flightdeck version.
5. Reply to issues with exact fixed versions and validation limits.

Flightdeck follows preview.11 while regular-release preparation continues.
