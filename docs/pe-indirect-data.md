# Bounded PE indirect data

This experimental preview.10 change implements the PE image-digest route of
`CryptSIPCreateIndirectData`, which the pinned Wine Wintrust module leaves as
a stub. It is not a complete native SIP implementation.

In the isolated Fenix 3.0.0.7454 investigation, replacing that stub allowed
the launcher to stay running and the three child images to pass preflight.
The final V4 production module also passes the three child image checks in
the real application and has the own-code checks below. One combined retry
reached System boot, then hit a Wine memory-view assertion. Later starts
avoided it, and the tester confirmed working cockpit displays. Stable
operation and a complete flight remain unverified; see [the v3 result](fenix-v3.md).

## Supported input

The route accepts the PE subject GUID
`c689aab8-8e78-11d0-8c47-00c04fc295ee`, SHA1/SHA256, flags0/union0 and files
smaller than `MAXDWORD`. It checks complete PE32/PE32+ header bounds, ordered
contiguous raw sections, and an absent certificate table or a table ending
at EOF. Other layouts fail explicitly. This bound is narrower than all
valid PE layouts.

The digest omits the checksum, security-directory entry and certificate
table according to the
[Authenticode PE format](https://download.microsoft.com/download/9/c/5/9c5b2167-8017-4bae-9fde-d599bac8184a/authenticode_pe.docx).
Unsigned files receive the existing Wine eight-byte padding behavior. One
explicitly sized read-only mapping stays open; copied DOS/COFF/optional and
section metadata determine bounded hash ranges. The catalog helper is
unchanged. The code does not promise a consistent digest if file contents
change during hashing; concurrent truncation remains untested.

## Native representation gap

The candidate retains Wine's encoded empty `Data.Value` sequence `30 00`.
Modern Windows' field values, DER representation and total required-count
behavior have not been measured for the same request. The public Windows
SHA1 reference validates the **image digest**, not all fields of this API's
indirect-data output. The historical Authenticode document and a signed
image's encoding do not establish today's native API buffer contract.

No certificate-chain trust verdict or activation check is changed or
inferred from a matching image digest. Treat this as a bounded compatibility
implementation with that explicit conformance gap.

## Validation

Each of the four matching x64 Wine modules passes 97 native own-code checks:
the public Wine PE fixture's known Windows SHA1 digest; independent SHA256
and fixed PE32+ references; four mutations separating included content from
excluded checksum/certificate bytes; 22 malformed/unsupported PE controls,
including the sparse 4-GiB boundary; size-query/short-buffer/output canaries;
filename mode; and preservation of a caller-supplied crypto provider.
The fixture is never executed and retains Wine's LGPL notices.

Experimental 11/CachyOS 11 run in Steam Linux Runtime 4, CachyOS 10 in sniper,
and Xodus on the host. Original matching modules fail the initial digest
query; all four new modules pass. The complete checksum-pinned source and
ordered fuzz0 patch replay matches each built `crypt.c` exactly.

The separate Xodus diagnostic also passed 2,000 calls during same-size edits
of a disposable public fixture's headers. Calls either succeeded or
rejected the current metadata; that stress does not establish content
consistency or truncation behavior. Four unchanged shipped-image digests
matched their independently extracted embedded image digests in that
separate investigation. These observations are distinct from a native
Windows API comparison and a certificate trust verdict.
