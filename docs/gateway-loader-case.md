# Gateway loader-name compatibility

Preview.11 includes [PR #15](https://github.com/marselnenaj/fenix-a320-linux-patch/pull/15)
by @malik05051. It adds `Fenix.GqlGateway.exe` to the existing process-name
allowlist for PEB loader-entry spelling. The four matching x64 PE `ntdll.dll`
files are rebuilt; other native payloads and runner pins are retained.

The reporter traced Gateway startup to a case-sensitive search for a
12-character name beginning with uppercase `KERN`. Wine's `kernel32.dll`
does not match. The existing Fenix compatibility rule already exposes
`KERNEL32.DLL` and `KERNELBASE.dll` to the other four Fenix processes; Gateway
was missing. This change extends that same rule to its exact executable name.

On the reported Fenix **3.0.0.7454**, Steam MSFS 2020, CachyOS 11 and RTX 3050
Laptop setup, the rebuilt Gateway starts, listens on **8083**, writes its log
and serves the EFB. That is a community runtime result; the maintainer's
independent controls below execute only original native test code.

## Verification

All four source patch chains apply against the pinned archives with fuzz zero.
The built NTDLL sources and include files match a fresh replay, including
the normal generated headers. PE export names, ordinals and forwarders match
the preceding release modules. Only the four PE NTDLL payload hashes change.

Each runner is checked before and after the fix in separate fresh prefixes,
through its matching Steam Runtime where applicable. The unchanged original
[loader probe](../tests/loader-case-probe.c) is copied under eight names:

| Executable name | Before | Preview.11 |
| --- | --- | --- |
| Fenix.exe, FenixSystem.exe, FenixDisplay.exe, FenixCDU.exe | Fenix spelling | Fenix spelling |
| Fenix.GqlGateway.exe, fenix.gqlgateway.exe | Wine spelling | Fenix spelling |
| LoaderCaseControl.exe, Fenix.GqlGateway.exe.extra | Wine spelling | Wine spelling |

All **64 named comparisons pass**. The new automated smoke checks cover the
Gateway, case-insensitive process-name matching, an unrelated name and a
suffix near-match; a failure stops smoke and cleans up its own wineserver.
No vendor executable or existing simulator prefix is used in these controls.

The earlier class-initialization samples in [#7](display-startup.md) did not
locate the busy loop by themselves. The loader-name finding and the reporter's
working application result are the subsequent discriminator. This fix does
not establish full-flight v3 stability or correct the separate
[System-memory growth](system-memory.md).
