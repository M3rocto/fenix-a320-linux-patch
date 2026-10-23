// SPDX-License-Identifier: MIT
// Own synthetic, account-free D3D11/D2D initialization diagnostic.
// Never opens another process, simulator data, IPC, or vendor application.
#define WIN32_LEAN_AND_MEAN
#include <windows.h>
#include <d3d11_4.h>
#include <dxgi1_2.h>
#include <d2d1_2.h>
#include <stdint.h>
#include <stdio.h>
#include <stdlib.h>
#include <string.h>

static unsigned stage_failures;
static unsigned created_11_1;
static unsigned created_11_0;
static ULONGLONG started;

template<typename T> static void release(T *&object)
{
    if (object) object->Release();
    object = NULL;
}

static void utf8(const wchar_t *source, char *destination, unsigned size)
{
    destination[0] = 0;
    if (!WideCharToMultiByte(CP_UTF8, 0, source, -1, destination, size, NULL, NULL))
        strcpy(destination, "<conversion-failed>");
}

static bool result(unsigned index, D3D_FEATURE_LEVEL level, const char *phase, HRESULT hr, const void *output)
{
    printf("CASE adapter=%u requested=0x%04x ms=%llu phase=%s hr=0x%08lx nonnull=%u\n",
        index, (unsigned)level, (unsigned long long)(GetTickCount64() - started), phase,
        (unsigned long)hr, output != NULL);
    if (FAILED(hr) || !output) { stage_failures++; return false; }
    return true;
}

static void module_identity(const wchar_t *name)
{
    HMODULE module = GetModuleHandleW(name);
    wchar_t path[1024] = {};
    char name_utf8[256], path_utf8[4096];
    utf8(name, name_utf8, sizeof(name_utf8));
    if (!module) { printf("MODULE name=%s loaded=0\n", name_utf8); return; }
    GetModuleFileNameW(module, path, sizeof(path) / sizeof(path[0]));
    utf8(path, path_utf8, sizeof(path_utf8));
    const IMAGE_DOS_HEADER *dos = (const IMAGE_DOS_HEADER *)module;
    unsigned image_size = 0, timestamp = 0;
    bool builtin = false;
    if (dos->e_magic == IMAGE_DOS_SIGNATURE && dos->e_lfanew >= 64 && dos->e_lfanew < 4096)
    {
        const IMAGE_NT_HEADERS *nt = (const IMAGE_NT_HEADERS *)((const BYTE *)module + dos->e_lfanew);
        if (nt->Signature == IMAGE_NT_SIGNATURE)
        {
            image_size = nt->OptionalHeader.SizeOfImage;
            timestamp = nt->FileHeader.TimeDateStamp;
        }
        const char marker[] = "Wine builtin DLL";
        for (LONG offset = sizeof(IMAGE_DOS_HEADER); offset + (LONG)sizeof(marker) <= dos->e_lfanew; offset++)
            if (!memcmp((const BYTE *)module + offset, marker, sizeof(marker))) { builtin = true; break; }
    }
    printf("MODULE name=%s loaded=1 path=%s image_size=0x%x timestamp=0x%x wine_builtin_marker=%u\n",
        name_utf8, path_utf8, image_size, timestamp, builtin);
}

static void test_adapter(IDXGIAdapter1 *adapter, unsigned index, D3D_FEATURE_LEVEL level, bool interop)
{
    ID3D11Device *device = NULL;
    ID3D11DeviceContext *immediate = NULL;
    ID3D11Device5 *device5 = NULL;
    ID3D11Texture2D *texture = NULL;
    IDXGISurface *surface = NULL;
    IDXGIDevice *dxgi_device = NULL;
    ID2D1Factory1 *factory = NULL;
    ID2D1Device *d2d_device = NULL;
    ID2D1Device1 *d2d_device1 = NULL;
    ID2D1DeviceContext1 *context1 = NULL;
    ID2D1Bitmap1 *bitmap = NULL;
    IDXGIResource1 *resource1 = NULL;
    IDXGIKeyedMutex *mutex = NULL;
    D3D_FEATURE_LEVEL obtained = (D3D_FEATURE_LEVEL)0;
    D3D11_TEXTURE2D_DESC texture_description = {};
    D2D1_FACTORY_OPTIONS factory_options = {};
    D2D1_BITMAP_PROPERTIES1 bitmap_properties = {};
    HRESULT hr = D3D11CreateDevice(adapter, D3D_DRIVER_TYPE_UNKNOWN, NULL,
        D3D11_CREATE_DEVICE_BGRA_SUPPORT, &level, 1, D3D11_SDK_VERSION,
        &device, &obtained, &immediate);
    bool valid = result(index, level, "D3D11CreateDevice", hr, device);
    printf("DEVICE adapter=%u requested=0x%04x obtained=0x%04x immediate_nonnull=%u flags=0x%08x count=1 driver_type=0\n",
        index, (unsigned)level, (unsigned)obtained, immediate != NULL, D3D11_CREATE_DEVICE_BGRA_SUPPORT);
    if (!valid) goto cleanup;
    if (level == D3D_FEATURE_LEVEL_11_1) created_11_1++; else created_11_0++;
    hr = device->QueryInterface(__uuidof(ID3D11Device5), (void **)&device5);
    if (!result(index, level, "QI_ID3D11Device5", hr, device5)) goto cleanup;
    if (!interop) goto cleanup;

    // Descriptor values decoded from the exact top crash method, synthetic 32x32 size.
    texture_description.Width = 32;
    texture_description.Height = 32;
    texture_description.MipLevels = 1;
    texture_description.ArraySize = 1;
    texture_description.Format = DXGI_FORMAT_R8G8B8A8_UNORM;
    texture_description.SampleDesc.Count = 1;
    texture_description.Usage = D3D11_USAGE_DEFAULT;
    texture_description.BindFlags = D3D11_BIND_SHADER_RESOURCE | D3D11_BIND_RENDER_TARGET;
    texture_description.MiscFlags = D3D11_RESOURCE_MISC_SHARED_KEYEDMUTEX | D3D11_RESOURCE_MISC_SHARED_NTHANDLE;
    hr = device5->CreateTexture2D(&texture_description, NULL, &texture);
    if (!result(index, level, "CreateTexture2D_RGBA8_bind28_misc900", hr, texture)) goto cleanup;
    hr = texture->QueryInterface(__uuidof(IDXGISurface), (void **)&surface);
    if (!result(index, level, "QI_IDXGISurface", hr, surface)) goto cleanup;
    hr = device5->QueryInterface(__uuidof(IDXGIDevice), (void **)&dxgi_device);
    if (!result(index, level, "QI_IDXGIDevice", hr, dxgi_device)) goto cleanup;
    hr = D2D1CreateFactory(D2D1_FACTORY_TYPE_SINGLE_THREADED, __uuidof(ID2D1Factory1), &factory_options, (void **)&factory);
    if (!result(index, level, "D2D1CreateFactory_SINGLE_THREADED_Factory1", hr, factory)) goto cleanup;
    hr = factory->CreateDevice(dxgi_device, &d2d_device);
    if (!result(index, level, "Factory1_CreateDevice", hr, d2d_device)) goto cleanup;
    hr = d2d_device->QueryInterface(__uuidof(ID2D1Device1), (void **)&d2d_device1);
    if (!result(index, level, "QI_ID2D1Device1", hr, d2d_device1)) goto cleanup;
    hr = d2d_device1->CreateDeviceContext(D2D1_DEVICE_CONTEXT_OPTIONS_NONE, &context1);
    if (!result(index, level, "Device1_CreateDeviceContext1", hr, context1)) goto cleanup;
    bitmap_properties.pixelFormat.format = DXGI_FORMAT_R8G8B8A8_UNORM;
    bitmap_properties.pixelFormat.alphaMode = D2D1_ALPHA_MODE_PREMULTIPLIED;
    bitmap_properties.dpiX = bitmap_properties.dpiY = 96.0f;
    bitmap_properties.bitmapOptions = D2D1_BITMAP_OPTIONS_TARGET | D2D1_BITMAP_OPTIONS_CANNOT_DRAW;
    hr = context1->CreateBitmapFromDxgiSurface(surface, &bitmap_properties, &bitmap);
    if (!result(index, level, "CreateBitmapFromDxgiSurface_RGBA8_opts3", hr, bitmap)) goto cleanup;
    context1->SetTarget(bitmap);
    printf("CASE adapter=%u requested=0x%04x phase=SetTarget completed=1\n", index, (unsigned)level);
    // Query only. No handle export, IPC, mutex acquisition, rendering or infinite wait.
    hr = texture->QueryInterface(__uuidof(IDXGIResource1), (void **)&resource1);
    result(index, level, "QI_IDXGIResource1", hr, resource1);
    hr = texture->QueryInterface(__uuidof(IDXGIKeyedMutex), (void **)&mutex);
    result(index, level, "QI_IDXGIKeyedMutex", hr, mutex);
cleanup:
    if (context1) context1->SetTarget(NULL);
    release(mutex); release(resource1); release(bitmap); release(context1);
    release(d2d_device1); release(d2d_device); release(factory); release(dxgi_device);
    release(surface); release(texture); release(device5); release(immediate); release(device);
}

int main(int argc, char **argv)
{
    int selected = -1;
    bool interop = false;
    for (int arg = 1; arg < argc; arg++)
    {
        if (!strcmp(argv[arg], "--interop")) interop = true;
        else if (!strcmp(argv[arg], "--adapter") && arg + 1 < argc)
        {
            char *end = NULL;
            long value = strtol(argv[++arg], &end, 10);
            if (!end || *end || value < 0 || value > 7) return 2;
            selected = (int)value;
        }
        else { fprintf(stderr, "usage: display-graphics-probe.exe [--adapter 0..7] [--interop]\n"); return 2; }
    }
    setvbuf(stdout, NULL, _IONBF, 0);
    started = GetTickCount64();
    printf("PROBE version=1 own_code=1 vendor_execution=0 max_adapters=8 selected=%d interop=%u\n", selected, interop);
    printf("NOTE exit0 means at_least_one_11_1_device; individual HRESULTs remain authoritative\n");
    HRESULT initialized = CoInitializeEx(NULL, COINIT_MULTITHREADED);
    printf("COM hr=0x%08lx\n", (unsigned long)initialized);
    module_identity(L"d3d11.dll"); module_identity(L"dxgi.dll"); module_identity(L"d2d1.dll");
    IDXGIFactory1 *factory = NULL;
    HRESULT hr = CreateDXGIFactory1(__uuidof(IDXGIFactory1), (void **)&factory);
    printf("FACTORY hr=0x%08lx nonnull=%u\n", (unsigned long)hr, factory != NULL);
    unsigned enumerated = 0, tested = 0;
    if (SUCCEEDED(hr) && factory)
    {
        for (unsigned index = 0; index < 8; index++)
        {
            IDXGIAdapter1 *adapter = NULL;
            hr = factory->EnumAdapters1(index, &adapter);
            if (hr == DXGI_ERROR_NOT_FOUND) break;
            printf("ENUM index=%u hr=0x%08lx nonnull=%u\n", index, (unsigned long)hr, adapter != NULL);
            if (FAILED(hr) || !adapter) break;
            enumerated++;
            DXGI_ADAPTER_DESC1 description = {};
            hr = adapter->GetDesc1(&description);
            char name[1024]; utf8(description.Description, name, sizeof(name));
            printf("ADAPTER index=%u hr=0x%08lx name=%s vendor=0x%04x device=0x%04x luid=%08lx:%08lx flags=0x%x\n",
                index, (unsigned long)hr, name, description.VendorId, description.DeviceId,
                (unsigned long)description.AdapterLuid.HighPart, (unsigned long)description.AdapterLuid.LowPart, description.Flags);
            // Diagnostic marker from pinned DXVK's public source. E_NOINTERFACE is inconclusive.
            static const GUID dxvk_adapter = {0x907bf281,0xea3c,0x43b4,{0xa8,0xe4,0x9f,0x23,0x11,0x07,0xb4,0xff}};
            IUnknown *dxvk_marker = NULL;
            HRESULT marker_hr = adapter->QueryInterface(dxvk_adapter, (void **)&dxvk_marker);
            printf("BACKEND adapter=%u dxvk_adapter_iid_hr=0x%08lx nonnull=%u negative_is_inconclusive=1\n",
                index, (unsigned long)marker_hr, dxvk_marker != NULL);
            release(dxvk_marker);
            if (selected < 0 || selected == (int)index)
            {
                test_adapter(adapter, index, D3D_FEATURE_LEVEL_11_1, interop);
                test_adapter(adapter, index, D3D_FEATURE_LEVEL_11_0, interop);
                tested++;
            }
            release(adapter);
        }
    }
    release(factory);
    if (SUCCEEDED(initialized)) CoUninitialize();
    printf("SUMMARY enumerated=%u tested=%u created_11_1=%u created_11_0=%u stage_failures=%u elapsed_ms=%llu\n",
        enumerated, tested, created_11_1, created_11_0, stage_failures, (unsigned long long)(GetTickCount64() - started));
    return !tested ? 3 : !created_11_1 ? 4 : 0;
}
