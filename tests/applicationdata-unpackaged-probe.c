/* SPDX-License-Identifier: MIT
 * An ordinary desktop executable has no Windows package storage. Check the
 * native WinRT contract on which Microsoft.Data.Sqlite's desktop fallback
 * relies: ApplicationData.Current must fail, not expose a partial object.
 */
#define COBJMACROS
#define WIN32_LEAN_AND_MEAN
#include <windows.h>
#include <appmodel.h>
#include <roapi.h>
#include <winstring.h>
#include <stdio.h>
#include <initguid.h>
#define WIDL_using_Windows_Foundation
#define WIDL_using_Windows_Foundation_Collections
#include <windows.foundation.h>
#define WIDL_using_Windows_Storage
#include <windows.storage.h>

int main(void)
{
    static const WCHAR name[] = L"Windows.Storage.ApplicationData";
    IApplicationDataStatics *factory = NULL;
    HSTRING classname = NULL;
    UINT32 length = 0;
    HRESULT hr;
    LONG status;
    unsigned int call;
    int result = 1;

    setvbuf(stdout, NULL, _IONBF, 0);
    status = GetCurrentPackageFullName(&length, NULL);
    printf("package identity: %ld\n", status);
    if (status != APPMODEL_ERROR_NO_PACKAGE) return 1;
    hr = RoInitialize(RO_INIT_MULTITHREADED);
    if (FAILED(hr)) return 1;
    hr = WindowsCreateString(name, ARRAYSIZE(name) - 1, &classname);
    if (FAILED(hr)) goto done;
    hr = RoGetActivationFactory(classname, &IID_IApplicationDataStatics, (void **)&factory);
    printf("activation factory: %#lx\n", (ULONG)hr);
    if (FAILED(hr)) goto done;
    hr = IApplicationDataStatics_get_Current(factory, NULL);
    printf("Current(NULL): %#lx\n", (ULONG)hr);
    if (hr != E_INVALIDARG) goto done;
    for (call = 0; call < 3; ++call)
    {
        IApplicationData *sentinel = (IApplicationData *)(ULONG_PTR)0xdeadbeef;
        IApplicationData *data = sentinel;

        hr = IApplicationDataStatics_get_Current(factory, &data);
        printf("Current call %u: %#lx, cleared=%u\n", call + 1, (ULONG)hr, data == NULL);
        if (hr != HRESULT_FROM_WIN32(APPMODEL_ERROR_NO_PACKAGE) || data != NULL)
        {
            if (data && data != sentinel) IApplicationData_Release(data);
            goto done;
        }
    }
    puts("Unpackaged ApplicationData native semantics: PASS");
    result = 0;
done:
    if (factory) IApplicationDataStatics_Release(factory);
    if (classname) WindowsDeleteString(classname);
    RoUninitialize();
    return result;
}
