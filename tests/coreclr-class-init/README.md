# CoreCLR class-initialization control

This original, optional maintainer probe exercises class initialization near
the `InitClassHelper` path identified in the Gateway investigation. It reads
only its own synthetic state and uses no Fenix assemblies, account data,
simulator input or process attachment.

It checks:

- Two workers contending for one explicit static constructor: it must run
  once and publish the same value to both workers.
- Repeated explicit `RunClassConstructor` calls.
- First access to GC and non-GC static fields in types with explicit
  constructors, followed by repeated reads.
- Three closed generic types with distinct GC/non-GC static storage.

The direct-field cases are important: the inspected Gateway CoreLib maps
lazy static access to `InitHelpers.InitClassSlow` and `QCall!InitClassHelper`.
`RunClassConstructor` instead imports `ReflectionInvocation_RunClassConstructor`.
API use alone does not prove which native instructions execute in a test.

## Build and run

With a .NET 10 SDK:

```sh
dotnet build tests/coreclr-class-init/ClassInitProbe.csproj \
  -c Release -o build/class-init-control
```

The project produces a framework-dependent **CIL** program, without R2R or
a native apphost. Run `ClassInitProbe.dll` using an explicit public Windows
.NET 10 x64 `dotnet.exe` through the selected patched Wine/Proton and a fresh
disposable prefix. Running it with Linux's dotnet does not test Wine.
The SDK recipe was not executed on the investigation host, which has no SDK;
the tested build used cached public Roslyn and Windows .NET runtime references.

`MARKER` records separate module startup, main, constructor entry/exit and
the test groups. A successful run ends with `SUMMARY checks=3607 failures=0`
and exit 0. Nonzero exits indicate a failed check, exception or watchdog.
The 30-second internal watchdog starts in Main; use a separate external
watchdog to bound runtime/module startup as well.

## Recorded control and limits

The source passed all **3,607 checks** using public Windows .NET **10.0.13**,
the exact patched CachyOS 11 **20261005** runner, Steam Linux Runtime 4 and a
fresh prefix. The two class-initialization wrapper intervals overlapped by
**50.36 ms**; the shared constructor ran once.

The culture-free test required **invariant globalization** and Wine's builtin
**mscoree** for the CIL launch. An initial missing-ICU-function failure and a
second failure caused by disabling mscoree both occurred before the tested
cases; neither is a class-initialization result. These settings belong to this
control, not the production Gateway launch.

This is a smoke control on Microsoft's public runtime. It neither reproduces
nor qualifies Fenix's custom bundled 10.0.12-dev CoreCLR, protected initializer,
R2R execution, simulator-load conditions or full service startup. No Gateway
fix follows from a passing control. See the [investigation](../../docs/display-startup.md).
