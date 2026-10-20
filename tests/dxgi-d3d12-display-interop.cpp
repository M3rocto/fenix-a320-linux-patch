/* SPDX-License-Identifier: MIT
 * A synthetic display transport, independent of Fenix/MSFS. A D3D12 producer
 * shares a texture and timeline fence with a separate D3D11 process. The
 * consumer discovers the actual owner adapter and verifies every frame.
 *
 * x86_64-w64-mingw32-g++ -std=c++17 -O2 -static -o display-interop.exe \
 *   tests/dxgi-d3d12-display-interop.cpp -ld3d12 -ld3d11 -ldxgi -ldxguid
 * display-interop.exe [--adapter N] [--frames N] [--known-owner]
 * --known-owner is a diagnostic control that skips the owner query, allowing
 * the remaining transport to be tested on a baseline without that API.
 */
#include <windows.h>
#include <d3d11_4.h>
#include <d3d12.h>
#include <dxgi1_4.h>
#include <algorithm>
#include <cstdint>
#include <cstdio>
#include <cstdlib>
#include <stdexcept>
#include <string>
#include <vector>

template<typename T> struct Com {
    T *p = nullptr;
    ~Com() { if (p) p->Release(); }
    T **out() { return &p; }
    void **vout() { return reinterpret_cast<void **>(&p); }
    T *operator->() const { return p; }
    operator T *() const { return p; }
    Com() = default;
    Com(const Com &) = delete;
    Com &operator=(const Com &) = delete;
};

struct Handle {
    HANDLE h = nullptr;
    ~Handle() { if (h && h != INVALID_HANDLE_VALUE) CloseHandle(h); }
    Handle() = default;
    explicit Handle(HANDLE value) : h(value) {}
    Handle(const Handle &) = delete;
    Handle &operator=(const Handle &) = delete;
};

// The only process this probe may terminate is its own directly created child.
struct Child {
    PROCESS_INFORMATION pi = {};
    ~Child() {
        if (pi.hProcess) {
            if (WaitForSingleObject(pi.hProcess, 0) == WAIT_TIMEOUT) {
                TerminateProcess(pi.hProcess, 3);
                WaitForSingleObject(pi.hProcess, 5000);
            }
            CloseHandle(pi.hProcess);
            CloseHandle(pi.hThread);
        }
    }
};

static void check(HRESULT hr, const char *what) {
    if (FAILED(hr)) {
        std::fprintf(stderr, "FAIL %s: HRESULT %08lx\n", what, hr);
        throw std::runtime_error(what);
    }
}

static void require(bool ok, const char *what) {
    if (!ok) {
        std::fprintf(stderr, "FAIL %s: Windows error %lu\n", what, GetLastError());
        throw std::runtime_error(what);
    }
}

static HANDLE inherited_copy(HANDLE source) {
    HANDLE copy;
    require(DuplicateHandle(GetCurrentProcess(), source, GetCurrentProcess(),
            &copy, 0, TRUE, DUPLICATE_SAME_ACCESS), "duplicate inheritable handle");
    return copy;
}

static constexpr UINT width = 64, height = 64;
static constexpr DWORD timeout_ms = 10000;

static UINT number(const char *value) {
    char *end;
    const auto n = std::strtoul(value, &end, 10);
    require(*value && !*end && n < 1000000, "numeric argument");
    return static_cast<UINT>(n);
}

static void color(UINT frame, BYTE out[4]) {
    out[0] = static_cast<BYTE>(frame * 101); // BGRA byte order
    out[1] = static_cast<BYTE>(frame * 37);
    out[2] = static_cast<BYTE>(frame);
    out[3] = 255;
}

static int consumer(HANDLE texture_handle, HANDLE fence_handle,
        LUID expected, UINT frames, bool query_owner) {
    Handle texture_owner(texture_handle), fence_owner(fence_handle);
    Com<IDXGIFactory4> factory;
    check(CreateDXGIFactory1(__uuidof(IDXGIFactory4), factory.vout()), "consumer factory");
    LUID actual = expected;
    if (query_owner) {
        check(factory->GetSharedResourceAdapterLuid(texture_handle, &actual),
                "GetSharedResourceAdapterLuid on D3D12 texture in child");
    } else {
        std::puts("CONTROL: using producer-provided adapter; owner query is untested");
    }
    require(actual.LowPart == expected.LowPart && actual.HighPart == expected.HighPart,
            "shared texture reports its actual owner adapter");
    Com<IDXGIAdapter1> adapter;
    check(factory->EnumAdapterByLuid(actual, __uuidof(IDXGIAdapter1), adapter.vout()),
            "EnumAdapterByLuid");
    Com<ID3D11Device> device;
    Com<ID3D11DeviceContext> context;
    check(D3D11CreateDevice(adapter, D3D_DRIVER_TYPE_UNKNOWN, nullptr,
            D3D11_CREATE_DEVICE_BGRA_SUPPORT, nullptr, 0, D3D11_SDK_VERSION,
            device.out(), nullptr, context.out()), "consumer D3D11 device");
    Com<ID3D11Device5> device5;
    Com<ID3D11DeviceContext4> context4;
    check(device->QueryInterface(__uuidof(ID3D11Device5), device5.vout()), "D3D11 device5");
    check(context->QueryInterface(__uuidof(ID3D11DeviceContext4), context4.vout()), "D3D11 context4");
    Com<ID3D11Texture2D> shared, staging;
    Com<ID3D11Fence> fence;
    check(device5->OpenSharedResource1(texture_handle, __uuidof(ID3D11Texture2D),
            shared.vout()), "D3D11 import D3D12 texture");
    check(device5->OpenSharedFence(fence_handle, __uuidof(ID3D11Fence), fence.vout()),
            "D3D11 import D3D12 fence");
    D3D11_TEXTURE2D_DESC desc = {};
    shared->GetDesc(&desc);
    require(desc.Width == width && desc.Height == height
            && desc.Format == DXGI_FORMAT_B8G8R8A8_UNORM, "imported texture description");
    desc.BindFlags = desc.MiscFlags = 0;
    desc.Usage = D3D11_USAGE_STAGING;
    desc.CPUAccessFlags = D3D11_CPU_ACCESS_READ;
    check(device->CreateTexture2D(&desc, nullptr, staging.out()), "consumer readback");
    Handle ready(CreateEventW(nullptr, FALSE, FALSE, nullptr));
    require(ready.h != nullptr, "consumer completion event");
    UINT64 pixels = 0;
    for (UINT frame = 0; frame < frames; ++frame) {
        // Bound the wait before queuing a GPU wait, so producer failure cannot
        // leave a staging Map blocked indefinitely on an unsignaled fence.
        const UINT64 value = UINT64(frame) * 2 + 1;
        check(fence->SetEventOnCompletion(value, ready.h), "consumer fence event");
        require(WaitForSingleObject(ready.h, timeout_ms) == WAIT_OBJECT_0,
                "consumer fence completion timeout");
        check(context4->Wait(fence, value), "D3D11 GPU wait");
        context->CopyResource(staging, shared);
        D3D11_MAPPED_SUBRESOURCE map = {};
        check(context->Map(staging, 0, D3D11_MAP_READ, 0, &map), "frame readback");
        BYTE expected_color[4];
        color(frame, expected_color);
        bool correct = true;
        for (UINT y = 0; y < height; ++y) {
            const auto *row = static_cast<const BYTE *>(map.pData) + y * map.RowPitch;
            for (UINT x = 0; x < width; ++x)
                for (UINT c = 0; c < 4; ++c)
                    correct &= std::abs(int(row[x * 4 + c]) - int(expected_color[c])) <= 1;
        }
        context->Unmap(staging, 0);
        if (!correct) {
            std::fprintf(stderr, "FAIL stale/corrupt texture at frame %u\n", frame);
            return 1;
        }
        pixels += UINT64(width) * height;
        check(context4->Signal(fence, value + 1), "D3D11 GPU acknowledgement");
        context->Flush();
    }
    std::printf("PASS D3D12 -> D3D11: %u ordered frames, %llu verified pixels, owner %08lx:%08lx\n",
            frames, static_cast<unsigned long long>(pixels), actual.HighPart, actual.LowPart);
    return 0;
}

static int producer(UINT adapter_index, UINT frames, bool query_owner) {
    Com<IDXGIFactory4> factory;
    check(CreateDXGIFactory1(__uuidof(IDXGIFactory4), factory.vout()), "producer factory");
    Com<IDXGIAdapter1> adapter;
    check(factory->EnumAdapters1(adapter_index, adapter.out()), "producer adapter");
    DXGI_ADAPTER_DESC1 adapter_desc;
    check(adapter->GetDesc1(&adapter_desc), "producer adapter description");
    require(!(adapter_desc.Flags & DXGI_ADAPTER_FLAG_SOFTWARE), "hardware adapter");
    Com<ID3D12Device> device;
    check(D3D12CreateDevice(adapter, D3D_FEATURE_LEVEL_11_0, __uuidof(ID3D12Device),
            device.vout()), "producer D3D12 device");
    Com<ID3D12CommandQueue> queue;
    D3D12_COMMAND_QUEUE_DESC queue_desc = {};
    queue_desc.Type = D3D12_COMMAND_LIST_TYPE_DIRECT;
    check(device->CreateCommandQueue(&queue_desc, __uuidof(ID3D12CommandQueue), queue.vout()),
            "producer queue");
    D3D12_HEAP_PROPERTIES heap = {};
    heap.Type = D3D12_HEAP_TYPE_DEFAULT;
    heap.CreationNodeMask = heap.VisibleNodeMask = 1;
    D3D12_RESOURCE_DESC desc = {};
    desc.Dimension = D3D12_RESOURCE_DIMENSION_TEXTURE2D;
    desc.Width = width;
    desc.Height = height;
    desc.DepthOrArraySize = desc.MipLevels = desc.SampleDesc.Count = 1;
    desc.Format = DXGI_FORMAT_B8G8R8A8_UNORM;
    desc.Flags = D3D12_RESOURCE_FLAG_ALLOW_RENDER_TARGET
            | D3D12_RESOURCE_FLAG_ALLOW_SIMULTANEOUS_ACCESS;
    Com<ID3D12Resource> texture;
    check(device->CreateCommittedResource(&heap, D3D12_HEAP_FLAG_SHARED, &desc,
            D3D12_RESOURCE_STATE_COMMON, nullptr, __uuidof(ID3D12Resource), texture.vout()),
            "producer shared texture");
    Com<ID3D12Fence> fence;
    check(device->CreateFence(0, D3D12_FENCE_FLAG_SHARED, __uuidof(ID3D12Fence), fence.vout()),
            "producer shared fence");
    Handle texture_handle, fence_handle;
    check(device->CreateSharedHandle(texture, nullptr, GENERIC_ALL, nullptr, &texture_handle.h),
            "texture NT handle");
    check(device->CreateSharedHandle(fence, nullptr, GENERIC_ALL, nullptr, &fence_handle.h),
            "fence NT handle");
    Handle child_texture(inherited_copy(texture_handle.h));
    Handle child_fence(inherited_copy(fence_handle.h));
    Com<ID3D12DescriptorHeap> rtv_heap;
    D3D12_DESCRIPTOR_HEAP_DESC rtv_desc = {};
    rtv_desc.Type = D3D12_DESCRIPTOR_HEAP_TYPE_RTV;
    rtv_desc.NumDescriptors = 1;
    check(device->CreateDescriptorHeap(&rtv_desc, __uuidof(ID3D12DescriptorHeap),
            rtv_heap.vout()), "RTV heap");
    const auto rtv = rtv_heap->GetCPUDescriptorHandleForHeapStart();
    device->CreateRenderTargetView(texture, nullptr, rtv);
    Com<ID3D12CommandAllocator> allocator;
    check(device->CreateCommandAllocator(D3D12_COMMAND_LIST_TYPE_DIRECT,
            __uuidof(ID3D12CommandAllocator), allocator.vout()), "command allocator");
    Com<ID3D12GraphicsCommandList> list;
    check(device->CreateCommandList(0, D3D12_COMMAND_LIST_TYPE_DIRECT, allocator,
            nullptr, __uuidof(ID3D12GraphicsCommandList), list.vout()), "command list");
    check(list->Close(), "initial command close");
    Handle complete(CreateEventW(nullptr, FALSE, FALSE, nullptr));
    require(complete.h != nullptr, "producer completion event");
    char executable[32768];
    const DWORD length = GetModuleFileNameA(nullptr, executable, sizeof(executable));
    require(length && length < sizeof(executable), "probe executable path");
    const LUID owner = device->GetAdapterLuid();
    std::string command = std::string("\"") + executable + "\" --consume "
            + std::to_string(reinterpret_cast<uintptr_t>(child_texture.h)) + " "
            + std::to_string(reinterpret_cast<uintptr_t>(child_fence.h)) + " "
            + std::to_string(owner.LowPart) + " "
            + std::to_string(static_cast<DWORD>(owner.HighPart)) + " "
            + std::to_string(frames) + " " + (query_owner ? "1" : "0");
    STARTUPINFOA startup = {};
    startup.cb = sizeof(startup);
    Handle child_output(inherited_copy(GetStdHandle(STD_OUTPUT_HANDLE)));
    startup.dwFlags = STARTF_USESTDHANDLES;
    startup.hStdOutput = startup.hStdError = child_output.h;
    startup.hStdInput = GetStdHandle(STD_INPUT_HANDLE);
    Child child;
    require(CreateProcessA(nullptr, command.data(), nullptr, nullptr, TRUE,
            CREATE_NO_WINDOW, nullptr, nullptr, &startup, &child.pi), "consumer process");
    LARGE_INTEGER frequency;
    require(QueryPerformanceFrequency(&frequency), "performance counter frequency");
    std::vector<double> times;
    for (UINT frame = 0; frame < frames; ++frame) {
        LARGE_INTEGER begin, end;
        QueryPerformanceCounter(&begin);
        check(allocator->Reset(), "allocator reset");
        check(list->Reset(allocator, nullptr), "command list reset");
        D3D12_RESOURCE_BARRIER barrier = {};
        barrier.Type = D3D12_RESOURCE_BARRIER_TYPE_TRANSITION;
        barrier.Transition.pResource = texture;
        barrier.Transition.Subresource = D3D12_RESOURCE_BARRIER_ALL_SUBRESOURCES;
        barrier.Transition.StateBefore = D3D12_RESOURCE_STATE_COMMON;
        barrier.Transition.StateAfter = D3D12_RESOURCE_STATE_RENDER_TARGET;
        list->ResourceBarrier(1, &barrier);
        BYTE bytes[4];
        color(frame, bytes);
        const float clear[] = {bytes[2] / 255.f, bytes[1] / 255.f, bytes[0] / 255.f, 1.f};
        list->ClearRenderTargetView(rtv, clear, 0, nullptr);
        std::swap(barrier.Transition.StateBefore, barrier.Transition.StateAfter);
        list->ResourceBarrier(1, &barrier);
        check(list->Close(), "command list close");
        ID3D12CommandList *commands[] = {list};
        queue->ExecuteCommandLists(1, commands);
        const UINT64 value = UINT64(frame) * 2 + 1;
        check(queue->Signal(fence, value), "producer GPU signal");
        check(fence->SetEventOnCompletion(value + 1, complete.h), "producer fence event");
        HANDLE waits[] = {complete.h, child.pi.hProcess};
        require(WaitForMultipleObjects(2, waits, FALSE, timeout_ms) == WAIT_OBJECT_0,
                "consumer exited or frame acknowledgement timeout");
        QueryPerformanceCounter(&end);
        if (frame >= std::min(30u, frames / 4))
            times.push_back(1000.0 * (end.QuadPart - begin.QuadPart) / frequency.QuadPart);
    }
    require(WaitForSingleObject(child.pi.hProcess, timeout_ms) == WAIT_OBJECT_0,
            "consumer exit timeout");
    DWORD status = 1;
    require(GetExitCodeProcess(child.pi.hProcess, &status), "consumer exit code");
    require(status == 0, "consumer success");
    std::sort(times.begin(), times.end());
    const auto percentile = [&](double p) { return times[size_t((times.size() - 1) * p)]; };
    std::printf("PASS adapter=%u frames=%u synthetic transfer + pixel readback ms: "
            "p50=%.3f p95=%.3f p99=%.3f max=%.3f\n", adapter_index, frames,
            percentile(.50), percentile(.95), percentile(.99), times.back());
    return 0;
}

int main(int argc, char **argv) {
    std::setvbuf(stdout, nullptr, _IONBF, 0);
    try {
        if (argc == 8 && std::string(argv[1]) == "--consume") {
            const auto handle = [](const char *v) {
                char *end;
                const auto n = std::strtoull(v, &end, 10);
                require(*v && !*end && n && n <= UINTPTR_MAX, "inherited handle argument");
                return reinterpret_cast<HANDLE>(static_cast<uintptr_t>(n));
            };
            // LUID halves are full DWORD values, not the bounded CLI counts.
            const LUID owner = {static_cast<DWORD>(std::strtoul(argv[4], nullptr, 10)),
                    static_cast<LONG>(std::strtoul(argv[5], nullptr, 10))};
            return consumer(handle(argv[2]), handle(argv[3]), owner, number(argv[6]),
                    number(argv[7]) != 0);
        }
        UINT adapter = 0, frames = 300;
        bool query_owner = true;
        for (int i = 1; i < argc; ++i) {
            const std::string option = argv[i];
            if (option == "--known-owner") query_owner = false;
            else {
                require(i + 1 < argc, "option value missing");
                if (option == "--adapter") adapter = number(argv[++i]);
                else if (option == "--frames") frames = number(argv[++i]);
                else throw std::runtime_error("unknown option");
            }
        }
        require(frames > 0 && frames <= 100000, "frame count must be 1..100000");
        return producer(adapter, frames, query_owner);
    } catch (const std::exception &error) {
        std::fprintf(stderr, "Interop probe stopped: %s\n", error.what());
        return 2;
    }
}
