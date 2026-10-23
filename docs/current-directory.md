# GLOBALROOT current-directory conversion

This experimental preview.10 change corrects an absolute working-directory
defect in the pinned x64 Wine runners. Opening a GLOBALROOT directory could
succeed while `RtlSetCurrentDirectory_U` stored bare
`GLOBALROOT\Device\HarddiskVolumeN\...` as a relative current directory.
Relative application files were then searched beneath that malformed path.

Microsoft specifies that [GetCurrentDirectory returns an absolute path](https://learn.microsoft.com/en-us/windows/win32/api/winbase/nf-winbase-getcurrentdirectory)
and [SetCurrentDirectory calculates and stores the full path](https://learn.microsoft.com/en-us/windows/win32/api/winbase/nf-winbase-setcurrentdirectory).
Own native and .NET 8/10 programs reproduce this Wine defect independently.
Exact native Windows spelling for this alias remains unmeasured.

## Scope

For the exact `\??\GLOBALROOT\` namespace, the patch queries the directory
handle that was already opened successfully. Its live DOS drive-rooted or
UNC object identity feeds the existing conversion before the prefix is
removed. No volume number or drive letter is mapped by a constant. The
query happens before the existing removable-media rule can close the handle.

The stored trailing separator and NUL must fit the current-directory buffer
before its old path or handle is replaced. A failed query or capacity check
closes only the new handle and preserves the previous directory. The older
CachyOS 10 function has a separate source port retaining its existing full
PEB lock and same-directory behavior.

Only PE `ntdll.dll` changes; the
[Unix volume-path module](volume-device-paths.md), server protocol,
KernelBase and APC behavior are retained. Components and separators are
copied without a new normalization policy. General long-directory support,
all NT devices, live SMB and removable-media runtime behavior are outside
the verified scope.

## Verification

All four matching release builds pass 194 parent and 128 inherited-child
checks, plus ten negative/state-preservation checks. They cover distinct
C/D volumes discovered through handles, single/doubled GLOBALROOT aliases,
ordinary/extended DOS, a private filesystem UNC mapping, A/W buffer queries,
relative reopening, `.`, `..`, rooted and drive-relative paths. The private
UNC mapping establishes namespace handling, not live SMB correctness.

A mapped DOS directory of 258 characters fits the tested buffer; 259/260
characters reject and preserve the old state. These are measured Wine
capacity/error cases, not complete Windows status-precedence claims. The
unchanged baseline skips the near-limit copy rather than exercising its
missing bound.

The existing unchanged path suites pass on original and candidate: 1,011
tests for Xodus/Experimental 11/CachyOS 11, 989 for CachyOS 10, with no
failures, todo or skips. The matching Steam containers are used. Complete
archive/fuzz0 replay matches each built path source; PE exports are retained.
Smoke prepares only its new prefix's C/D/UNC fixtures and compiles the own
probe with that explicitly owned prefix, then runs its child and negative
controls. A focused real Xodus run also checks that integration setup.

Literal trailing dots/spaces remain an existing broader limitation: stored
components are preserved, but reopening `.` can still normalize them. This
patch does not establish complete verbatim relative-path behavior.

## Fenix observation

The separate diagnostic Xodus retry corrected the child application folders,
removed the engine-model/Kestrel path failures, let Systems boot and reach
SimConnect Connected, and kept children/Gateway alive. A subsequent trial
with the final combined payloads reached link setup/System boot and then
hit a Wine memory-view assertion. Its cause is being investigated
independently; that occurrence does not establish a cwd or crypto regression.
An unchanged production repeat ran for about five minutes without that
assertion. A later run with address-only Unix NTDLL logging also avoided it,
and the tester confirmed working displays before closing MSFS. The diagnostic
logging is excluded from the payload. Stable v3 operation and a full flight
remain unverified. A later fresh production run and five warm backend
restarts also avoided the assertion, with user-confirmed displays on the
fresh run; see [the complete v3 result](fenix-v3.md).
