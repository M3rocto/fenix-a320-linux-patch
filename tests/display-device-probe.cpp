// SPDX-License-Identifier: MIT
// Own, bounded D3D11 initialization diagnostic. No vendor execution or file access.
#define WIN32_LEAN_AND_MEAN
#include <windows.h>
#include <d3d11_4.h>
#include <dxgi1_4.h>
#include <stdio.h>
#include <stdlib.h>
#include <string.h>

struct Trial {
    IDXGIAdapter1 *adapter;
    HANDLE start, created, release;
    ID3D11Device *device;
    ID3D11DeviceContext *context;
    HRESULT hr, qi;
    D3D_FEATURE_LEVEL obtained;
    ULONGLONG elapsed;
    LONGLONG entry_ticks, exit_ticks;
    DWORD thread_id;
};

static DWORD WINAPI create_device(void *arg)
{
    Trial *trial = (Trial *)arg;
    if (WaitForSingleObject(trial->start, 30000) != WAIT_OBJECT_0) {
        trial->hr = trial->qi = E_ABORT;
        SetEvent(trial->created);
        return 1;
    }
    ULONGLONG started = GetTickCount64();
    trial->thread_id = GetCurrentThreadId();
    LARGE_INTEGER stamp = {};
    QueryPerformanceCounter(&stamp);
    trial->entry_ticks = stamp.QuadPart;
    D3D_FEATURE_LEVEL level = D3D_FEATURE_LEVEL_11_1;
    trial->hr = D3D11CreateDevice(trial->adapter, D3D_DRIVER_TYPE_UNKNOWN, NULL,
        D3D11_CREATE_DEVICE_BGRA_SUPPORT, &level, 1, D3D11_SDK_VERSION,
        &trial->device, &trial->obtained, &trial->context);
    QueryPerformanceCounter(&stamp);
    trial->exit_ticks = stamp.QuadPart;
    trial->elapsed = GetTickCount64() - started;
    trial->qi = E_FAIL;
    if (trial->device) {
        ID3D11Device5 *device5 = NULL;
        trial->qi = trial->device->QueryInterface(__uuidof(ID3D11Device5), (void **)&device5);
        if (device5) device5->Release();
    }
    SetEvent(trial->created);
    WaitForSingleObject(trial->release, 30000);
    if (trial->context) trial->context->Release();
    if (trial->device) trial->device->Release();
    return 0;
}

static void budget(IDXGIAdapter1 *adapter, const char *phase)
{
    IDXGIAdapter3 *adapter3 = NULL;
    HRESULT hr = adapter->QueryInterface(__uuidof(IDXGIAdapter3), (void **)&adapter3);
    DXGI_QUERY_VIDEO_MEMORY_INFO info = {};
    if (SUCCEEDED(hr)) {
        hr = adapter3->QueryVideoMemoryInfo(0, DXGI_MEMORY_SEGMENT_GROUP_LOCAL, &info);
        adapter3->Release();
    }
    printf("BUDGET phase=%s hr=0x%08lx budget_bytes=%llu current_usage_bytes=%llu\n",
        phase, (unsigned long)hr, (unsigned long long)info.Budget, (unsigned long long)info.CurrentUsage);
}

int main(int argc, char **argv)
{
    unsigned count = 1, selected = 0;
    bool serial = false;
    for (int i = 1; i < argc; ++i) {
        if (!strcmp(argv[i], "--help")) {
            puts("display-device-probe.exe [--count 1..16] [--adapter 0..7] [--serial]");
            return 0;
        } else if (!strcmp(argv[i], "--serial")) serial = true;
        else if ((!strcmp(argv[i], "--count") || !strcmp(argv[i], "--adapter")) && i + 1 < argc) {
            bool is_count = !strcmp(argv[i], "--count");
            char *end = NULL;
            const char *input = argv[++i];
            long value = strtol(input, &end, 10);
            if (!end || end == input || *end || value < (is_count ? 1 : 0) || value > (is_count ? 16 : 7)) return 2;
            if (is_count) count = (unsigned)value; else selected = (unsigned)value;
        } else return 2;
    }
    setvbuf(stdout, NULL, _IONBF, 0);
    LARGE_INTEGER frequency = {};
    if (!QueryPerformanceFrequency(&frequency) || frequency.QuadPart <= 0) return 4;
    printf("CLOCK qpc_hz=%lld\n", (long long)frequency.QuadPart);
    IDXGIFactory1 *factory = NULL;
    IDXGIAdapter1 *adapter = NULL;
    HRESULT hr = CreateDXGIFactory1(__uuidof(IDXGIFactory1), (void **)&factory);
    if (SUCCEEDED(hr)) hr = factory->EnumAdapters1(selected, &adapter);
    if (factory) factory->Release();
    if (FAILED(hr) || !adapter) { printf("ADAPTER failed=0x%08lx\n", (unsigned long)hr); return 3; }
    DXGI_ADAPTER_DESC1 desc = {};
    adapter->GetDesc1(&desc);
    char description[512] = {};
    WideCharToMultiByte(CP_UTF8, 0, desc.Description, -1, description,
        sizeof(description), NULL, NULL);
    printf("ADAPTER index=%u name=%s vendor=0x%x device=0x%x\n",
        selected, description, desc.VendorId, desc.DeviceId);
    printf("PROBE count=%u serial=%u adapter=%u vendor=0x%x device=0x%x requested=0xb100 flags=0x20\n",
        count, serial, selected, desc.VendorId, desc.DeviceId);
    budget(adapter, "before");
    HANDLE start = CreateEventW(NULL, TRUE, FALSE, NULL);
    HANDLE release = CreateEventW(NULL, TRUE, FALSE, NULL);
    if (!start || !release) {
        printf("ERROR stage=events win32=%lu\n", GetLastError());
        return 4;
    }
    // One deadline for all creations, including serial mode.
    const ULONGLONG deadline = GetTickCount64() + 30000;
    Trial trials[16] = {};
    HANDLE threads[16] = {}, created[16] = {};
    unsigned made = 0, failures = 0;
    if (serial) SetEvent(start);
    for (; made < count; made++) {
        trials[made].adapter = adapter;
        trials[made].start = start;
        trials[made].release = release;
        created[made] = trials[made].created = CreateEventW(NULL, TRUE, FALSE, NULL);
        if (!created[made]) {
            printf("ERROR stage=created-event win32=%lu\n", GetLastError());
            return 4;
        }
        threads[made] = CreateThread(NULL, 0, create_device, &trials[made], 0, NULL);
        if (!threads[made]) {
            printf("ERROR stage=thread win32=%lu\n", GetLastError());
            return 4;
        }
        if (serial) {
            ULONGLONG now = GetTickCount64();
            DWORD remaining = now < deadline ? (DWORD)(deadline - now) : 0;
            DWORD wait = WaitForSingleObject(created[made], remaining);
            if (wait != WAIT_OBJECT_0) {
                printf("WATCHDOG stage=serial-create index=%u wait=%lu\n", made, wait);
                return 5;
            }
        }
    }
    SetEvent(start);
    ULONGLONG now = GetTickCount64();
    DWORD remaining = now < deadline ? (DWORD)(deadline - now) : 0;
    DWORD wait = WaitForMultipleObjects(count, created, TRUE, remaining);
    if (wait != WAIT_OBJECT_0) { printf("WATCHDOG wait=%lu\n", wait); return 5; }
    budget(adapter, "all_devices_retained");
    for (unsigned i = 0; i < count; i++) {
        printf("DEVICE index=%u hr=0x%08lx nonnull=%u obtained=0x%x device5_hr=0x%08lx elapsed_ms=%llu thread=0x%lx entry_ticks=%lld exit_ticks=%lld\n",
            i, (unsigned long)trials[i].hr, trials[i].device != NULL,
            (unsigned)trials[i].obtained, (unsigned long)trials[i].qi,
            (unsigned long long)trials[i].elapsed, (unsigned long)trials[i].thread_id,
            (long long)trials[i].entry_ticks, (long long)trials[i].exit_ticks);
        if (FAILED(trials[i].hr) || !trials[i].device || FAILED(trials[i].qi)) failures++;
    }
    SetEvent(release);
    wait = WaitForMultipleObjects(count, threads, TRUE, 10000);
    if (wait != WAIT_OBJECT_0) {
        printf("WATCHDOG stage=release wait=%lu\n", wait);
        return 5;
    }
    for (unsigned i = 0; i < count; i++) { CloseHandle(threads[i]); CloseHandle(created[i]); }
    CloseHandle(start); CloseHandle(release);
    budget(adapter, "after_release");
    adapter->Release();
    printf("SUMMARY devices=%u failures=%u\n", count, failures);
    return failures ? 1 : 0;
}
