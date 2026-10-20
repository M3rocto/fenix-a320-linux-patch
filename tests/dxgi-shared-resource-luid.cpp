// SPDX-License-Identifier: MIT
// Build with x86_64-w64-mingw32-g++ -std=c++17 -O2 -static,
// then link dxgi-shared-resource-luid.cpp with -ld3d11 -ldxgi -ldxguid.
// Run with native DXVK dxgi/d3d11 and a dedicated Wine prefix.
// This does not load the simulator, Fenix, or vendor renderer code.
#define WIN32_LEAN_AND_MEAN
#define NOMINMAX
#include <windows.h>
#include <d3d11_1.h>
#include <dxgi1_2.h>
#include <cstdio>
#include <cstdlib>
#include <cstdint>
#include <cstring>

static unsigned checks, failures;

static void check(bool condition, const char *message, HRESULT result = S_OK) {
    ++checks;
    if (!condition) {
        ++failures;
        std::printf("FAIL %s hr=%08lx\n", message, static_cast<unsigned long>(result));
    }
}

static bool equal(LUID a, LUID b) {
    return a.LowPart == b.LowPart && a.HighPart == b.HighPart;
}

static bool query(IDXGIFactory2 *factory, HANDLE handle, LUID expected, const char *name) {
    LUID actual = {};
    HRESULT hr = factory->GetSharedResourceAdapterLuid(handle, &actual);
    std::printf("%s handle=%p hr=%08lx owner=%08lx:%08lx expected=%08lx:%08lx\n",
        name, handle, static_cast<unsigned long>(hr), static_cast<unsigned long>(actual.HighPart),
        static_cast<unsigned long>(actual.LowPart), static_cast<unsigned long>(expected.HighPart),
        static_cast<unsigned long>(expected.LowPart));
    check(hr == S_OK && equal(actual, expected), name, hr);
    return hr == S_OK && equal(actual, expected);
}

static void child_check(HANDLE handle, LUID expected, bool nt) {
    IDXGIFactory2 *factory = nullptr;
    HRESULT hr = CreateDXGIFactory1(__uuidof(IDXGIFactory2), reinterpret_cast<void **>(&factory));
    check(SUCCEEDED(hr), "child factory", hr);
    if (!factory) return;
    query(factory, handle, expected, "cross-process owner");
    if (nt) {
        HANDLE duplicate = nullptr;
        check(DuplicateHandle(GetCurrentProcess(), handle, GetCurrentProcess(), &duplicate,
            0, FALSE, DUPLICATE_SAME_ACCESS), "child duplicate handle");
        if (duplicate) {
            query(factory, duplicate, expected, "child duplicated owner");
            CloseHandle(duplicate);
        }
    }
    factory->Release();
}

static void cross_process(HANDLE handle, LUID expected, bool nt) {
    char executable[MAX_PATH], command[1024];
    GetModuleFileNameA(nullptr, executable, MAX_PATH);
    std::snprintf(command, sizeof(command), "\"%s\" --child %llx %lx %lx %u", executable,
        static_cast<unsigned long long>(reinterpret_cast<uintptr_t>(handle)),
        static_cast<unsigned long>(expected.HighPart), static_cast<unsigned long>(expected.LowPart), nt);
    STARTUPINFOA startup = {};
    PROCESS_INFORMATION process = {};
    startup.cb = sizeof(startup);
    BOOL created = CreateProcessA(executable, command, nullptr, nullptr, TRUE, 0, nullptr, nullptr, &startup, &process);
    check(created, "create child");
    if (!created) return;
    DWORD wait = WaitForSingleObject(process.hProcess, 30000);
    check(wait == WAIT_OBJECT_0, "child completed");
    DWORD exit_code = 1;
    GetExitCodeProcess(process.hProcess, &exit_code);
    check(exit_code == 0, "child passed");
    if (wait != WAIT_OBJECT_0) TerminateProcess(process.hProcess, 2);
    CloseHandle(process.hThread);
    CloseHandle(process.hProcess);
}

static void resource_check(IDXGIFactory2 *factory, IDXGIAdapter *adapter, LUID expected, bool nt) {
    ID3D11Device *device = nullptr;
    HRESULT hr = D3D11CreateDevice(adapter, D3D_DRIVER_TYPE_UNKNOWN, nullptr, 0, nullptr, 0,
        D3D11_SDK_VERSION, &device, nullptr, nullptr);
    check(SUCCEEDED(hr), "D3D11 device", hr);
    if (!device) return;
    D3D11_TEXTURE2D_DESC desc = {};
    desc.Width = desc.Height = 8;
    desc.MipLevels = desc.ArraySize = desc.SampleDesc.Count = 1;
    desc.Format = DXGI_FORMAT_R8G8B8A8_UNORM;
    desc.BindFlags = D3D11_BIND_SHADER_RESOURCE | D3D11_BIND_RENDER_TARGET;
    desc.MiscFlags = nt ? D3D11_RESOURCE_MISC_SHARED_NTHANDLE | D3D11_RESOURCE_MISC_SHARED_KEYEDMUTEX
                        : D3D11_RESOURCE_MISC_SHARED;
    ID3D11Texture2D *texture = nullptr;
    hr = device->CreateTexture2D(&desc, nullptr, &texture);
    check(SUCCEEDED(hr), "shared texture", hr);
    if (!texture) { device->Release(); return; }
    IDXGIResource1 *resource = nullptr;
    hr = texture->QueryInterface(__uuidof(IDXGIResource1), reinterpret_cast<void **>(&resource));
    check(SUCCEEDED(hr), "DXGI resource", hr);
    HANDLE handle = nullptr;
    if (resource) {
        SECURITY_ATTRIBUTES attr = {sizeof(attr), nullptr, TRUE};
        hr = nt ? resource->CreateSharedHandle(&attr, DXGI_SHARED_RESOURCE_READ | DXGI_SHARED_RESOURCE_WRITE, nullptr, &handle)
                : resource->GetSharedHandle(&handle);
        check(SUCCEEDED(hr), "shared handle", hr);
        if (SUCCEEDED(hr)) {
            query(factory, handle, expected, nt ? "NT owner" : "KMT owner");
            cross_process(handle, expected, nt);
            if (nt) {
                HANDLE duplicate = nullptr;
                check(DuplicateHandle(GetCurrentProcess(), handle, GetCurrentProcess(), &duplicate,
                    0, FALSE, DUPLICATE_SAME_ACCESS), "duplicate resource handle");
                CloseHandle(handle);
                if (duplicate) {
                    query(factory, duplicate, expected, "owner after original handle closes");
                    CloseHandle(duplicate);
                }
                LUID output = {};
                hr = factory->GetSharedResourceAdapterLuid(handle, &output);
                check(hr == DXGI_ERROR_INVALID_CALL, "closed handle rejected", hr);
            }
        }
        resource->Release();
    }
    texture->Release();
    device->Release();
}

int main(int argc, char **argv) {
    std::setvbuf(stdout, nullptr, _IONBF, 0);
    if (argc == 6 && std::strcmp(argv[1], "--child") == 0) {
        HANDLE handle = reinterpret_cast<HANDLE>(static_cast<uintptr_t>(std::strtoull(argv[2], nullptr, 16)));
        LUID expected = {static_cast<DWORD>(std::strtoul(argv[4], nullptr, 16)),
                         static_cast<LONG>(std::strtoul(argv[3], nullptr, 16))};
        child_check(handle, expected, std::strtoul(argv[5], nullptr, 10));
    } else {
        IDXGIFactory2 *factory = nullptr;
        HRESULT hr = CreateDXGIFactory1(__uuidof(IDXGIFactory2), reinterpret_cast<void **>(&factory));
        check(SUCCEEDED(hr), "factory", hr);
        if (!factory) return 1;
        LUID actual = {};
        HANDLE event = CreateEventW(nullptr, FALSE, FALSE, nullptr);
        hr = factory->GetSharedResourceAdapterLuid(event, &actual);
        check(hr == DXGI_ERROR_INVALID_CALL, "unrelated event rejected", hr);
        CloseHandle(event);
        hr = factory->GetSharedResourceAdapterLuid(nullptr, &actual);
        check(hr == DXGI_ERROR_INVALID_CALL, "null handle rejected", hr);
        hr = factory->GetSharedResourceAdapterLuid(INVALID_HANDLE_VALUE, &actual);
        check(hr == DXGI_ERROR_INVALID_CALL, "invalid handle rejected", hr);
        hr = factory->GetSharedResourceAdapterLuid(nullptr, nullptr);
        check(hr == DXGI_ERROR_INVALID_CALL, "null output rejected", hr);
        unsigned adapters = 0;
        for (UINT index = 0;; ++index) {
            IDXGIAdapter *adapter = nullptr;
            if (factory->EnumAdapters(index, &adapter) == DXGI_ERROR_NOT_FOUND) break;
            if (!adapter) { check(false, "enumerate adapter"); break; }
            DXGI_ADAPTER_DESC desc;
            adapter->GetDesc(&desc);
            std::printf("adapter[%u] %ls LUID=%08lx:%08lx\n", index, desc.Description,
                static_cast<unsigned long>(desc.AdapterLuid.HighPart), static_cast<unsigned long>(desc.AdapterLuid.LowPart));
            resource_check(factory, adapter, desc.AdapterLuid, true);
            resource_check(factory, adapter, desc.AdapterLuid, false);
            ++adapters;
            adapter->Release();
        }
        check(adapters > 0, "at least one hardware adapter");
        factory->Release();
    }
    std::printf("checks=%u failures=%u\n", checks, failures);
    return failures ? 1 : 0;
}
