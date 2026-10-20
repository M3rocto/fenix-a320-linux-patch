/* SPDX-License-Identifier: MIT
 * Cross-process D3D12 -> D3D11 shared-fence value contract, independent of Fenix.
 * Build with x86_64-w64-mingw32-g++ -std=c++17 -O2 -static, linking
 * -ld3d12 -ld3d11 -ldxgi -ldxguid. Run with dense, strided, rewind, initial, initial-high, lifetime, creator-exit.
 * Uses bounded CPU events for test coordination; no unsupported GPU waits.
 */
#define WIN32_LEAN_AND_MEAN
#define NOMINMAX
#include <windows.h>
#include <d3d11_4.h>
#include <d3d12.h>
#include <dxgi1_4.h>
#include <array>
#include <cstdio>
#include <cstdlib>
#include <cstdint>
#include <cstring>
#include <stdexcept>
#include <string>

template<class T> struct Com {
    T *p = nullptr;
    ~Com() { if (p) p->Release(); }
    void reset() { if (p) p->Release(); p = nullptr; }
    T **out() { return &p; }
    void **vout() { return reinterpret_cast<void **>(&p); }
    T *operator->() { return p; }
};
struct Handle {
    HANDLE p = nullptr;
    Handle() = default;
    explicit Handle(HANDLE h) : p(h) {}
    ~Handle() { reset(); }
    void reset() { if (p && p != INVALID_HANDLE_VALUE) CloseHandle(p); p = nullptr; }
};
struct Child {
    PROCESS_INFORMATION p = {};
    ~Child() {
        if (p.hProcess) {
            if (WaitForSingleObject(p.hProcess, 0) == WAIT_TIMEOUT) {
                TerminateProcess(p.hProcess, 3);
                WaitForSingleObject(p.hProcess, 3000);
            }
            CloseHandle(p.hProcess);
            CloseHandle(p.hThread);
        }
    }
};
static void need(bool ok, const char *message) {
    if (!ok) throw std::runtime_error(message);
}
static void check(HRESULT hr, const char *message) {
    if (FAILED(hr)) {
        std::fprintf(stderr, "%s: %08lx\n", message, static_cast<unsigned long>(hr));
        throw std::runtime_error(message);
    }
}
static HANDLE inherit(HANDLE handle) {
    HANDLE result = nullptr;
    need(DuplicateHandle(GetCurrentProcess(), handle, GetCurrentProcess(), &result,
        0, TRUE, DUPLICATE_SAME_ACCESS), "inheritable handle");
    return result;
}
static uintptr_t value(HANDLE handle) { return reinterpret_cast<uintptr_t>(handle); }
static HANDLE handle(const char *number) {
    return reinterpret_cast<HANDLE>(static_cast<uintptr_t>(std::strtoull(number, nullptr, 10)));
}
static std::array<UINT64, 4> sequence(const std::string &mode) {
    if (mode == "dense") return {1, 2, 3, 4};
    if (mode == "strided") return {5, 12, 30, 97};
    if (mode == "rewind" || mode == "initial" || mode == "lifetime") return {9, 3, 15, 2};
    if (mode == "initial-high") return {4, UINT64_MAX - 2, 9, 2};
    throw std::runtime_error("unknown fence sequence mode");
}
static UINT64 initial(const std::string &mode) {
    if (mode == "initial") return 5;
    if (mode == "initial-high") return UINT64_MAX - 1;
    return 0;
}
static void signal_imported(ID3D11Device *device, ID3D11Fence *fence, UINT64 next) {
    Com<ID3D11DeviceContext> context;
    device->GetImmediateContext(context.out());
    Com<ID3D11DeviceContext4> context4;
    check(context->QueryInterface(__uuidof(ID3D11DeviceContext4), context4.vout()), "context4");
    Handle event(CreateEventW(nullptr, FALSE, FALSE, nullptr));
    need(event.p != nullptr, "completion event");
    check(context4->Signal(fence, next), "imported Signal");
    context->Flush();
    check(fence->SetEventOnCompletion(next, event.p), "imported completion");
    need(WaitForSingleObject(event.p, 3000) == WAIT_OBJECT_0, "imported signal completion timeout");
    need(fence->GetCompletedValue() == next, "imported signal completed value");
}
static int consume(HANDLE shared, HANDLE advance, HANDLE ack, LUID luid, const std::string &mode) {
    Com<IDXGIFactory4> factory;
    check(CreateDXGIFactory1(__uuidof(IDXGIFactory4), factory.vout()), "child factory");
    Com<IDXGIAdapter1> adapter;
    check(factory->EnumAdapterByLuid(luid, __uuidof(IDXGIAdapter1), adapter.vout()), "child adapter");
    Com<ID3D11Device> device;
    check(D3D11CreateDevice(adapter.p, D3D_DRIVER_TYPE_UNKNOWN, nullptr, 0, nullptr, 0,
        D3D11_SDK_VERSION, device.out(), nullptr, nullptr), "child device");
    Com<ID3D11Device5> device5;
    check(device->QueryInterface(__uuidof(ID3D11Device5), device5.vout()), "child device5");
    Com<ID3D11Fence> fence;
    check(device5->OpenSharedFence(shared, __uuidof(ID3D11Fence), fence.vout()), "child OpenSharedFence");
    CloseHandle(shared); // The local import must retain its own synchronization context.
    const UINT64 imported_initial = fence->GetCompletedValue();
    unsigned failures = imported_initial != initial(mode);
    std::printf("mode=%s initial=%llu imported=%llu\n", mode.c_str(),
        static_cast<unsigned long long>(initial(mode)), static_cast<unsigned long long>(imported_initial));
    SetEvent(ack);
    for (UINT64 expected : sequence(mode)) {
        need(WaitForSingleObject(advance, 3000) == WAIT_OBJECT_0, "child advance timeout");
        const UINT64 actual = fence->GetCompletedValue();
        std::printf("mode=%s expected=%llu imported=%llu\n", mode.c_str(),
            static_cast<unsigned long long>(expected), static_cast<unsigned long long>(actual));
        failures += actual != expected;
        SetEvent(ack);
    }
    if (mode == "lifetime") {
        signal_imported(device.p, fence.p, 97);
        std::printf("lifetime consumer signal=97 after creator released all fence/device/NT references\n");
    }
    std::printf("fence checks=%u failures=%u\n", mode == "lifetime" ? 6u : 5u, failures);
    return failures ? 1 : 0;
}
static int produce(const std::string &mode) {
    sequence(mode);
    Com<IDXGIFactory4> factory;
    check(CreateDXGIFactory1(__uuidof(IDXGIFactory4), factory.vout()), "factory");
    Com<IDXGIAdapter1> adapter;
    check(factory->EnumAdapters1(0, adapter.out()), "adapter");
    Com<ID3D12Device> device;
    check(D3D12CreateDevice(adapter.p, D3D_FEATURE_LEVEL_11_0, __uuidof(ID3D12Device), device.vout()), "device");
    Com<ID3D12Fence> fence;
    check(device->CreateFence(initial(mode), D3D12_FENCE_FLAG_SHARED, __uuidof(ID3D12Fence), fence.vout()), "shared fence");
    Handle shared;
    check(device->CreateSharedHandle(fence.p, nullptr, GENERIC_ALL, nullptr, &shared.p), "fence handle");
    Handle inherited_shared(inherit(shared.p));
    SECURITY_ATTRIBUTES attr = {sizeof(attr), nullptr, TRUE};
    Handle advance(CreateEventW(&attr, FALSE, FALSE, nullptr));
    Handle ack(CreateEventW(&attr, FALSE, FALSE, nullptr));
    need(advance.p && ack.p, "coordination events");
    char executable[32768];
    need(GetModuleFileNameA(nullptr, executable, sizeof(executable)), "executable path");
    const LUID luid = device->GetAdapterLuid();
    std::string command = std::string("\"") + executable + "\" --consume "
        + std::to_string(value(inherited_shared.p)) + " " + std::to_string(value(advance.p)) + " "
        + std::to_string(value(ack.p)) + " " + std::to_string(luid.LowPart) + " "
        + std::to_string(static_cast<DWORD>(luid.HighPart)) + " " + mode;
    STARTUPINFOA startup = {};
    startup.cb = sizeof(startup);
    startup.dwFlags = STARTF_USESTDHANDLES;
    Handle output(inherit(GetStdHandle(STD_OUTPUT_HANDLE)));
    Handle error(inherit(GetStdHandle(STD_ERROR_HANDLE)));
    startup.hStdOutput = output.p;
    startup.hStdError = error.p;
    Child child;
    need(CreateProcessA(executable, command.data(), nullptr, nullptr, TRUE, 0,
        nullptr, nullptr, &startup, &child.p), "create child");
    need(WaitForSingleObject(ack.p, 5000) == WAIT_OBJECT_0, "child import timeout");
    const auto values = sequence(mode);
    for (size_t index = 0; index != values.size(); ++index) {
        const UINT64 expected = values[index];
        check(fence->Signal(expected), "producer Signal");
        need(fence->GetCompletedValue() == expected, "producer local fence value");
        if (mode == "lifetime" && index + 1 == values.size()) {
            shared.reset();
            inherited_shared.reset();
            fence.reset();
            device.reset();
        }
        SetEvent(advance.p);
        need(WaitForSingleObject(ack.p, 3000) == WAIT_OBJECT_0, "child value timeout");
    }
    need(WaitForSingleObject(child.p.hProcess, 5000) == WAIT_OBJECT_0, "child exit timeout");
    DWORD result = 3;
    need(GetExitCodeProcess(child.p.hProcess, &result), "child result");
    return static_cast<int>(result);
}
struct Export {
    uintptr_t shared;
    LUID luid;
};
static int creator(HANDLE pipe, HANDLE release) {
    Com<IDXGIFactory4> factory;
    check(CreateDXGIFactory1(__uuidof(IDXGIFactory4), factory.vout()), "creator factory");
    Com<IDXGIAdapter1> adapter;
    check(factory->EnumAdapters1(0, adapter.out()), "creator adapter");
    Com<ID3D12Device> device;
    check(D3D12CreateDevice(adapter.p, D3D_FEATURE_LEVEL_11_0, __uuidof(ID3D12Device), device.vout()), "creator device");
    Com<ID3D12Fence> fence;
    check(device->CreateFence(5, D3D12_FENCE_FLAG_SHARED, __uuidof(ID3D12Fence), fence.vout()), "creator fence");
    Handle shared;
    check(device->CreateSharedHandle(fence.p, nullptr, GENERIC_ALL, nullptr, &shared.p), "creator shared handle");
    check(fence->Signal(97), "creator signal");
    const Export data = {value(shared.p), device->GetAdapterLuid()};
    DWORD written = 0;
    need(WriteFile(pipe, &data, sizeof(data), &written, nullptr) && written == sizeof(data), "write export");
    need(WaitForSingleObject(release, 5000) == WAIT_OBJECT_0, "creator release timeout");
    return 0;
}
static int creator_exit() {
    SECURITY_ATTRIBUTES attr = {sizeof(attr), nullptr, TRUE};
    Handle read, write;
    need(CreatePipe(&read.p, &write.p, &attr, 0), "creator pipe");
    Handle release(CreateEventW(&attr, FALSE, FALSE, nullptr));
    need(release.p != nullptr, "creator release event");
    char executable[32768];
    need(GetModuleFileNameA(nullptr, executable, sizeof(executable)), "executable path");
    std::string command = std::string("\"") + executable + "\" --creator "
        + std::to_string(value(write.p)) + " " + std::to_string(value(release.p));
    STARTUPINFOA startup = {};
    startup.cb = sizeof(startup);
    Child child;
    need(CreateProcessA(executable, command.data(), nullptr, nullptr, TRUE, 0,
        nullptr, nullptr, &startup, &child.p), "create fence creator");
    Export data = {};
    DWORD received = 0;
    const ULONGLONG deadline = GetTickCount64() + 5000;
    DWORD available = 0;
    while (available < sizeof(data) && GetTickCount64() < deadline) {
        need(PeekNamedPipe(read.p, nullptr, 0, nullptr, &available, nullptr), "peek export");
        if (available < sizeof(data)) Sleep(1);
    }
    need(available >= sizeof(data), "creator export timeout");
    need(ReadFile(read.p, &data, sizeof(data), &received, nullptr) && received == sizeof(data), "read export");
    Handle shared;
    need(DuplicateHandle(child.p.hProcess, reinterpret_cast<HANDLE>(data.shared), GetCurrentProcess(),
        &shared.p, 0, FALSE, DUPLICATE_SAME_ACCESS), "duplicate creator NT handle");
    SetEvent(release.p);
    need(WaitForSingleObject(child.p.hProcess, 5000) == WAIT_OBJECT_0, "creator exit timeout");
    DWORD result = 3;
    need(GetExitCodeProcess(child.p.hProcess, &result) && result == 0, "creator successful exit");
    Com<IDXGIFactory4> factory;
    check(CreateDXGIFactory1(__uuidof(IDXGIFactory4), factory.vout()), "consumer factory");
    Com<IDXGIAdapter1> adapter;
    check(factory->EnumAdapterByLuid(data.luid, __uuidof(IDXGIAdapter1), adapter.vout()), "consumer adapter");
    Com<ID3D11Device> device;
    check(D3D11CreateDevice(adapter.p, D3D_DRIVER_TYPE_UNKNOWN, nullptr, 0, nullptr, 0,
        D3D11_SDK_VERSION, device.out(), nullptr, nullptr), "consumer device");
    Com<ID3D11Device5> device5;
    check(device->QueryInterface(__uuidof(ID3D11Device5), device5.vout()), "consumer device5");
    Com<ID3D11Fence> fence;
    check(device5->OpenSharedFence(shared.p, __uuidof(ID3D11Fence), fence.vout()), "import after creator exit");
    shared.reset();
    need(fence->GetCompletedValue() == 97, "value after creator exit");
    signal_imported(device.p, fence.p, 123);
    std::printf("creator-exit imported=97 subsequent consumer signal=123 PASS\n");
    return 0;
}

static int legacy_reject() {
    Com<ID3D12Device> device;
    check(D3D12CreateDevice(nullptr, D3D_FEATURE_LEVEL_11_0, __uuidof(ID3D12Device), device.vout()), "legacy device");
    Com<ID3D12Fence> fence;
    check(device->CreateFence(5, D3D12_FENCE_FLAG_SHARED, __uuidof(ID3D12Fence), fence.vout()), "legacy fence");
    Handle shared;
    check(device->CreateSharedHandle(fence.p, nullptr, GENERIC_ALL, nullptr, &shared.p), "legacy shared handle");
    struct LegacyOpen { HANDLE shared; UINT sync; } params = {shared.p, 0};
    using Open = LONG (WINAPI *)(LegacyOpen *);
    const FARPROC symbol = GetProcAddress(LoadLibraryW(L"gdi32.dll"), "D3DKMTOpenSyncObjectFromNtHandle");
    need(symbol != nullptr, "legacy open export");
    Open open = nullptr;
    static_assert(sizeof(open) == sizeof(symbol));
    std::memcpy(&open, &symbol, sizeof(open));
    const LONG status = open(&params);
    need(static_cast<ULONG>(status) == 0xc00000bbu && params.sync == 0,
        "unaware legacy importer must return STATUS_NOT_SUPPORTED without opening a pointer context");
    Com<ID3D11Device> consumer;
    check(D3D11CreateDevice(nullptr, D3D_DRIVER_TYPE_HARDWARE, nullptr, 0, nullptr, 0,
        D3D11_SDK_VERSION, consumer.out(), nullptr, nullptr), "negative consumer device");
    Com<ID3D11Device5> device5;
    check(consumer->QueryInterface(__uuidof(ID3D11Device5), device5.vout()), "negative device5");
    Com<ID3D11Fence> imported;
    Handle query;
    need(DuplicateHandle(GetCurrentProcess(), shared.p, GetCurrentProcess(), &query.p,
        0x0001, FALSE, 0), "query-only duplicate");
    need(FAILED(device5->OpenSharedFence(query.p, __uuidof(ID3D11Fence), imported.vout())) && !imported.p,
        "query-only sync handle must not yield a writable fence context");
    Handle event(CreateEventW(nullptr, FALSE, FALSE, nullptr));
    need(FAILED(device5->OpenSharedFence(event.p, __uuidof(ID3D11Fence), imported.vout())) && !imported.p,
        "unrelated event must not import as fence");
    check(device5->OpenSharedFence(shared.p, __uuidof(ID3D11Fence), imported.vout()), "aware valid import after failures");
    need(imported->GetCompletedValue() == 5, "negative paths preserve valid context");
    std::printf("legacy-reject unaware=NOT_SUPPORTED query-only=denied event=denied valid=5 PASS\n");
    return 0;
}
int main(int argc, char **argv) {
    std::setvbuf(stdout, nullptr, _IONBF, 0);
    try {
        if (argc == 4 && std::string(argv[1]) == "--creator") return creator(handle(argv[2]), handle(argv[3]));
        if (argc == 2 && std::string(argv[1]) == "legacy-reject") return legacy_reject();
        if (argc == 2 && std::string(argv[1]) == "creator-exit") return creator_exit();
        if (argc == 8 && std::string(argv[1]) == "--consume") {
            LUID luid = {static_cast<DWORD>(std::strtoul(argv[5], nullptr, 10)),
                         static_cast<LONG>(std::strtoul(argv[6], nullptr, 10))};
            return consume(handle(argv[2]), handle(argv[3]), handle(argv[4]), luid, argv[7]);
        }
        need(argc == 2, "usage: shared-fence-values.exe dense|strided|rewind|initial|initial-high|lifetime|creator-exit|legacy-reject");
        return produce(argv[1]);
    } catch (const std::exception &error) {
        std::fprintf(stderr, "FAIL %s\n", error.what());
        return 2;
    }
}
