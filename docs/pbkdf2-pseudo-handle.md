# PBKDF2 pseudo-handle compatibility

This preview.10 change adds the missing PBKDF2 predefined-handle entry to each
pinned Wine runner's `bcrypt.dll`. Microsoft documents
[`BCRYPT_PBKDF2_ALG_HANDLE` as `0x331`](https://learn.microsoft.com/en-us/windows/win32/seccng/cng-algorithm-pseudo-handles).
Wine already implements PBKDF2 through an opened provider, but the pinned
predefined-handle table ends before this entry. Creating a key through the
documented handle therefore returns `STATUS_INVALID_HANDLE` (`0xc0000008`).

The two-entry backport adds the intervening unsupported CAPI KDF placeholder
and maps PBKDF2 to the existing algorithm. It matches the corresponding
[Wine upstream table entries](https://github.com/wine-mirror/wine/blob/master/dlls/bcrypt/bcrypt_main.c)
and reuses the existing key generation and derivation
implementation. The included Wine regression checks predefined-handle key
generation, derivation, expected bytes and destruction.

Both the official [.NET 8.0.31](https://github.com/dotnet/runtime/blob/v8.0.31/src/libraries/System.Security.Cryptography/src/System/Security/Cryptography/Pbkdf2Implementation.Windows.cs)
and [.NET 10.0.13](https://github.com/dotnet/runtime/blob/v10.0.13/src/libraries/System.Security.Cryptography/src/System/Security/Cryptography/Pbkdf2Implementation.Windows.cs)
Windows implementations use this handle where predefined handles are
supported. The [.NET exception mapping](https://github.com/dotnet/runtime/blob/v10.0.13/src/libraries/Common/src/Interop/Windows/BCrypt/Interop.CreateCryptographicException.cs)
converts the failing status to
`CryptographicException.HResult` `0xc1000008`. This API path already exists in
the inspected .NET 8 version.

## Regression coverage

`tests/pbkdf2-probe.c` is original MIT code integrated into `fenix-tools
smoke`. It compares five independent reference vectors through an opened
PBKDF2 provider, the predefined handle and the legacy HMAC-provider API:
fifteen vector/API combinations. The vectors include the public RFC 6070
SHA1 case, SHA256/384/512, one to 4,096 iterations, a 200-byte password, empty
password/salt and an 80-byte output spanning multiple digest blocks.
Expected values were independently checked with Python hashlib/OpenSSL.
All inputs are fixed synthetic test data; no application credentials or
vendor assemblies are used.

Each original runner passes the ten opened-provider/legacy combinations and
fails the five predefined-handle generations. Every matching candidate
passes all fifteen. Xodus runs on the host, Experimental 11 and CachyOS 11
use Steam Linux Runtime 4, and CachyOS 10 sunset uses sniper. The probe also
observes the predefined handle's object-length property, preserves rejection
of the unsupported CAPI handle and checks that predefined handles cannot be
closed like ordinary provider handles.

The four modules were compiled from private copies of their matching pinned
sources. Complete archive and ordered `--fuzz=0` patch replay reproduces both
affected source and Wine regression inputs. Existing payload modules,
including storage and volume-path fixes, and previous-release snapshots are
unchanged. BCrypt is included in the normal Wine build targets.

Separate Xodus tests with own-code .NET 8 and .NET 10 programs also pass
their five reference vectors; their original-runner controls reproduce the
same HRESULT. The existing Wine bcrypt suite with the new regression passes
on the candidate, while an unchanged rebuilt baseline fails the newly added
predefined-handle assertion.

## Fenix observation and limits

The real aircraft **3.0.0.7454** investigation observed the same missing
handle during System authorization preparation. A diagnostic BCrypt build
cleared that API failure and allowed authorization preparation and the
System process launch to complete. Later System initialization requires
separate compatibility work. This fixes the API contract and preserves the
application's own cryptographic inputs and checks.

The integrated production payloads pass the four-runner native tests and
the real application completes authorization preparation. Subsequent System
startup/display observations use the other preview.10 fixes too; see
[the combined v3 result](fenix-v3.md). SHA3, performance, the full CNG surface
and full-flight behavior are not established by these checks.
