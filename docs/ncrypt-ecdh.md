# Ephemeral NCrypt ECDH compatibility

This is an experimental preview.10 compatibility change, tested on 23 October 2026.
The pinned Wine provider returns `NTE_NOT_SUPPORTED` for unnamed
`ECDH_P256` creation. An isolated Fenix 3.0.0.7454 retry with the diagnostic
Xodus modules completed Systems link setup in 8,894 ms, kept FenixSystem
running and spawned its gateway. The separate application-folder defect is
addressed by the [current-directory change](current-directory.md). A final
combined retry reached System boot, then hit a Wine memory-view assertion
under separate investigation. Later starts avoided that assertion, and the
tester confirmed working cockpit displays. Stable operation and a complete
flight remain unverified; see [the v3 result](fenix-v3.md).

## Supported scope

The patch implements the ephemeral P256 route for the Microsoft Software
Key Storage Provider through Wine's existing BCrypt cryptography. It follows
the [.NET key-creation](https://github.com/dotnet/runtime/blob/v10.0.13/src/libraries/System.Security.Cryptography/src/System/Security/Cryptography/CngKey.Create.cs),
[public-key import](https://github.com/dotnet/runtime/blob/v10.0.13/src/libraries/System.Security.Cryptography/src/System/Security/Cryptography/CngKey.Import.cs)
and [ECDH derivation](https://github.com/dotnet/runtime/blob/v10.0.13/src/libraries/System.Security.Cryptography/src/System/Security/Cryptography/ECDiffieHellmanCng.Derive.cs)
calls:

- Fixed 256-bit creation/finalization and exact P256 public/private blobs.
- Public export and policy-controlled plaintext private export, with size
  queries and short-buffer rejection before writing.
- Secret agreement, basic HASH with SHA1/default, SHA256, SHA384 or SHA512,
  and little-endian raw output. Short HASH/raw buffers receive the requested
  prefix, following [NCryptDeriveKey](https://learn.microsoft.com/en-us/windows/win32/api/ncrypt/nf-ncrypt-ncryptderivekey).
- Owning provider handles, separate provider opens, public-only key checks,
  and secret lifetime after source keys/providers are freed.

Named persistence, other providers/curves, one-time archiving behavior, HMAC,
TLS and prepend/append KDF buffers are outside this implementation. Exact
Windows error precedence and every export-policy edge remain unmeasured.

The companion BCrypt Unix module validates imported P256 points, scalars
and private/public consistency with GnuTLS's existing
[parameter validators](https://www.gnutls.org/reference/gnutls-abstract.html).
Missing validator symbols fail as unsupported. Other algorithm imports are
unchanged. Both new modules must be paired with their matching runner;
the existing [PBKDF2 Windows module](pbkdf2-pseudo-handle.md) is preserved.

## Validation

All four matching x64 runner builds pass the own native probe: 127 checks,
fixed independent SHA1/256/384/512 and raw-secret references, malformed
point/scalar rejection, output canaries and 1,000 disposal cycles. The
NCrypt object census balances 5,014 allocations and destructions per runner.
Experimental 11 and CachyOS 11 run inside Steam Linux Runtime 4; CachyOS 10
uses sniper. The vector bytes also match fresh Python/OpenSSL CLI references.

CachyOS 10 needs its older header's ECDH group constant and explicitly
positioned checks at its actual ECC import functions. A generic-context
patch initially matched other imports despite fuzz0; the malformed-key
controls caught that port error before commit. The final full source replay
matches the built files and verifies ECC-only placement.

The separate Xodus investigation passes focused managed vectors on .NET
8.0.31 and 10.0.13 and its existing Wine suites. Its broader managed CNG
control retains the same ECDSA and RSA failures as baseline; only the scoped
ECDH case advances. This is not general .NET CNG compatibility.

The additional CachyOS 10 NCrypt suite passes 436 tests with 176 todo on both
baseline and candidate. Its original BCrypt test source needs a missing
closing brace repaired solely in the private test fixture. The identical
repaired executable reports 23,353 tests and the same ten baseline failure
events on both modules, with no new events. Those existing failures are
retained limitations, not a clean-suite claim. Production source replay
preserves the pinned test source exactly.

No server protocol, KernelBase normalizer or APC experiment is included.
The source/build/smoke/archive checks cover the new module pair and original
synthetic probe; prior payload files and previous-release hashes are retained.
