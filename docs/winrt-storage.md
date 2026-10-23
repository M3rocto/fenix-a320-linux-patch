# Unpackaged WinRT storage

This compatibility change in experimental preview.10 changes Wine's
`windows.storage.applicationdata.dll`. An ordinary desktop process without a
Windows package identity must receive `0x80073d54`
(`HRESULT_FROM_WIN32(APPMODEL_ERROR_NO_PACKAGE)`) from
`ApplicationData.Current`, with a NULL output object. The pinned Wine runners
instead returned an object whose `LocalFolder` property was unimplemented.

The patch checks package identity before creating that object and promotes
Wine's existing native Windows expectations to regression assertions. It
preserves the existing packaged-process path and invalid-argument behavior.
It does not invent a storage folder or change an application's database path.

## Installer observation

[Issue #7](https://github.com/marselnenaj/fenix-a320-linux-patch/issues/7#issuecomment-6349155866)
reports `System.NotImplementedException` from `ApplicationData.LocalFolder`
during SQLite initialization. The inspected official Fenix Installer
**1.0.359** targets .NET 8 and contains Microsoft.Data.Sqlite **9.0.10**.
Its own bootstrap also declares .NET 8.
Its desktop fallback handles the no-package error from `Current`; receiving
an object causes it to reach the unimplemented folder property instead.

The preserved Installer **1.0.286** already targets .NET 8 and uses SQLite
9.0.10. Installer 1.0.359 adds `Microsoft.Windows.SDK.NET.dll` and
`WinRT.Runtime.dll`, making the WinRT projection available to SQLite. A
controlled own-code probe using the same new SQLite assembly succeeds under
unchanged Wine when those two projection libraries are absent, reproduces the
storage failure when they are present, and passes with both present after the
Wine fix. The candidate keeps the vendor libraries intact. See the
[issue investigation update](https://github.com/marselnenaj/fenix-a320-linux-patch/issues/7#issuecomment-6350550099).

With the candidate Xodus module, the real installer completed four update
plans for aircraft **3.0.0.7454**, compared with none using the original module.
A separate .NET 8 probe using the shipped SQLite dependency also completed
`SELECT sqlite_version()` with SQLite **3.46.1**. These observations establish
database initialization and update planning. The official installer later
completed installation of aircraft **3.0.0.7454** on the tested Xodus profile.
Aircraft startup needed additional compatibility fixes. Later tests reached
System startup and user-confirmed cockpit displays on Xodus; a Wine startup
assertion and full-flight stability remain open. See [the v3 result](fenix-v3.md).

The previously added .NET 10 prerequisite support remains unchanged. Installer
1.0.359's .NET 8 storage failure is a separate compatibility problem.

## Native regression coverage

`tests/applicationdata-unpackaged-probe.c` is original MIT code integrated into
`fenix-tools smoke`. It verifies that the executable has no package identity,
the WinRT factory can be activated, a NULL argument returns `E_INVALIDARG`,
and three calls to `Current` return the no-package error while clearing an
initially non-NULL output pointer.

| Pinned runner | Original module | Candidate module | Execution environment |
| --- | --- | --- | --- |
| Xodus | Incorrect object returned | Pass | Host |
| Proton Experimental 11 | Incorrect object returned | Pass | Steam Linux Runtime 4 |
| CachyOS Proton 10 sunset | Incorrect object returned | Pass | Steam Linux Runtime sniper |
| CachyOS Proton 11 | Incorrect object returned | Pass | Steam Linux Runtime 4 |

Only the affected module was rebuilt for this check. The builds used private
copies of the corresponding pinned Wine sources; the original sources and
runners remained unchanged. The normal build now includes the module in each
overlay, with original-runner and candidate-payload hashes in `bundle.json`.
Preview.9's file sets are retained as previous-release snapshots for an
in-place upgrade. This change is included in experimental preview.10.
