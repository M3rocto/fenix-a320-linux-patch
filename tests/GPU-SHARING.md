# Shared GPU display regression tests

These programs contain only original test code. They exercise the shared texture,
owner adapter, and fence contracts used by a D3D12 producer and a separate D3D11
consumer. They do not load Fenix or MSFS. Use a dedicated Wine prefix and the
runner built for that exact variant.

## Build

The MinGW compiler needs the normal Direct3D headers and import libraries. The
Vulkan test additionally uses the pinned Vulkan-Headers included in the complete
DXVK source archive (`include/vulkan/include`). It loads `vulkan-1.dll` dynamically.

```sh
FENIX_TEST_OUT=/absolute/path/to/own-test-directory
FENIX_VULKAN_HEADERS=/absolute/path/to/extracted-dxvk/include/vulkan/include
mkdir -p "$FENIX_TEST_OUT"
x86_64-w64-mingw32-g++ -std=c++17 -O2 -static -Wall -Wextra -Werror \
  tests/dxgi-shared-resource-luid.cpp -o "$FENIX_TEST_OUT/owner.exe" \
  -ld3d11 -ldxgi -ldxguid
x86_64-w64-mingw32-g++ -std=c++17 -O2 -static -Wall -Wextra -Werror \
  tests/dxgi-d3d12-display-interop.cpp -o "$FENIX_TEST_OUT/display-interop.exe" \
  -ld3d12 -ld3d11 -ldxgi -ldxguid
x86_64-w64-mingw32-g++ -std=c++17 -O2 -static -Wall -Wextra -Werror \
  tests/dxgi-shared-fence-values.cpp -o "$FENIX_TEST_OUT/shared-fence-values.exe" \
  -ld3d12 -ld3d11 -ldxgi -ldxguid
x86_64-w64-mingw32-g++ -std=c++17 -O2 -static -Wall -Wextra -Werror \
  -I "$FENIX_VULKAN_HEADERS" tests/vulkan-shared-fence-context.cpp \
  -o "$FENIX_TEST_OUT/vulkan-shared-fence-context.exe"
```

Place the runner's original `d3d11.dll`, `d3d12.dll` and `d3d12core.dll`, plus its
matching patched `dxgi.dll`, beside the test executables. This keeps each test on
the exact DXVK/VKD3D versions shipped by its runner. Run from that directory:

```sh
export WINEPREFIX=/absolute/path/to/own-test-prefix
export WINEESYNC=0 WINEFSYNC=0 WINEDEBUG=-all
export WINEDLLOVERRIDES='dxgi,d3d11,d3d12,d3d12core=n;winemenubuilder.exe=d'
FENIX_TEST_WINE=/absolute/path/to/staged-runner/files/bin/wine
timeout 40 "$FENIX_TEST_WINE" ./owner.exe
timeout 40 "$FENIX_TEST_WINE" ./display-interop.exe --adapter 0 --frames 1000
timeout 40 "$FENIX_TEST_WINE" ./display-interop.exe --adapter 1 --frames 300
```

Adapter index 1 requires a second Vulkan-capable GPU. The owner test verifies
actual creating-adapter LUIDs for NT and KMT handles, cross-process imports,
duplicates, lifetime, and invalid/unrelated handles. The interop test verifies
every BGRA pixel in every ordered 64×64 frame and the bidirectional fence
handshake. Its timing describes synthetic transfer/readback, not simulator FPS.

## Wine 11 virtual fence contract

Run the following on Experimental 11 or CachyOS 11 with
`wine-shared-fence-context.patch`. The older Xodus physical timeline backend
supports the monotonic tests; its existing rewind limitation remains unchanged.

```sh
for FENIX_FENCE_MODE in dense strided rewind initial initial-high lifetime creator-exit legacy-reject; do
  timeout 25 "$FENIX_TEST_WINE" ./shared-fence-values.exe "$FENIX_FENCE_MODE" || break
done
for FENIX_VULKAN_MODE in values batch-peak reset-history; do
  timeout 25 "$FENIX_TEST_WINE" ./vulkan-shared-fence-context.exe "$FENIX_VULKAN_MODE" || break
done
```

| Mode | Assertion |
| --- | --- |
| `dense` | Separate D3D11 importer reads 1, 2, 3, 4. |
| `strided` | Importer reads 5, 12, 30, 97 rather than physical update counts. |
| `rewind` | Importer and local producer read 9, 3, 15, 2. |
| `initial` | Nonzero initial value 5 survives import before the first signal. |
| `initial-high` | Initial `UINT64_MAX-1` survives, followed by low and high values. |
| `lifetime` | Consumer reads and signals after all creator fence/device/NT references are released. |
| `creator-exit` | Import occurs after the separate creator process has exited; the consumer can then signal. |
| `legacy-reject` | Unaware legacy open returns `STATUS_NOT_SUPPORTED`; query-only and unrelated event handles fail; a valid import still succeeds. |
| Vulkan `values` | Satisfied waits succeed at timeout 0; unsatisfied waits respect their deadlines; reimport works and failed temporary/type-0 imports preserve the prior payload. Ordinary opaque semaphores remain usable. |
| Vulkan `batch-peak` | 64 GPU batches signal 20→10→15; a previously registered wait on 20 completes and the final counter is 15. |
| Vulkan `reset-history` | A wait for never-reached 1000 survives more than 16 rewinds without false completion. Lost required history returns `VK_ERROR_DEVICE_LOST`, while an unrelated fence still signals 7. |

The fixed-size rewind history intentionally fails closed when a pending wait can
no longer be proven satisfied. These tests do not claim unlimited rewind history
or defined ordering for application races between independent GPU queues.

## Mutex owner death

`shared-fence-owner-death-hook.c` is a test-only Linux fault injector. It is not
part of the Wine patch, installer payload, or runtime. It acts only in the test's
`--fence-crash-creator` process and only after an explicit arm file exists. It
exits that process immediately after acquiring its own robust process-shared
mutex, deliberately exercising the kernel's `EOWNERDEAD` path.

```sh
cc -shared -fPIC -O2 -Wall -Wextra -Werror \
  tests/shared-fence-owner-death-hook.c \
  -o "$FENIX_TEST_OUT/shared-fence-owner-death-hook.so" -ldl -pthread
FENIX_ARM_FILE="$FENIX_TEST_OUT/fence-owner-death.arm"
FENIX_ARM_WINDOWS=$(printf 'Z:%s' "$FENIX_ARM_FILE" | sed 's,/,\\,g')
env LD_PRELOAD="$FENIX_TEST_OUT/shared-fence-owner-death-hook.so" \
  FENIX_TEST_FENCE_CRASH_ARM="$FENIX_ARM_FILE" \
  timeout 25 "$FENIX_TEST_WINE" ./vulkan-shared-fence-context.exe \
  owner-death "$FENIX_ARM_WINDOWS"
```

Expected: the hook logs its acquired robust mutex and Linux exit 86; Wine may
report a different Windows process exit code. An already running infinite host
wait returns `VK_ERROR_DEVICE_LOST` within the test's 3-second bound. No missing
signal is fabricated. An unrelated fence on the same device still signals 7.
A 32-bit Wine launcher may log that it ignored the 64-bit preload library; the
64-bit test creator must emit the hook's explicit owner-death line. This tests
host wait/error and wrapper lifetime handling, not recovery of native GPU queues
that the application has left waiting for an unsignalled value.

## Mixed server versions

The optional direct Vulkan mode `old-server-reject` is a negative control for a
separate test runner whose Wine client modules are patched but whose `wineserver`
is the original binary from the same protocol-compatible base runner. Give this
control its own fresh prefix; ordinary full patched runners must fail this test
because they correctly support creating the new fence context.

```sh
timeout 40 /absolute/path/to/own-old-server-control/files/bin/wine \
  ./vulkan-shared-fence-context.exe old-server-reject
```

Expected: creating a virtual shared fence fails before export, while an ordinary
opaque semaphore still has its initial value 5. This exercises the capability
readback that prevents type-2 metadata reaching old unaware importers through an
old server that ignores the new context field.
