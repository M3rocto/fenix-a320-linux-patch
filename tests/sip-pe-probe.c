/* SPDX-License-Identifier: MIT. Own bounded SIP contract/mutation probe.
 * The included public Wine PE fixture retains its separate LGPL notices.
 * No fixture executable is run and no certificate trust verdict is tested. */
#define WIN32_LEAN_AND_MEAN
#include <windows.h>
#include <wincrypt.h>
#include <wintrust.h>
#include <mssip.h>
#include <stdint.h>
#include <stdio.h>
#include <stdlib.h>
#include <string.h>
#include "sip-pe-fixture.h"

static GUID pe = {0xc689aab8,0x8e78,0x11d0,{0x8c,0x47,0,0xc0,0x4f,0xc2,0x95,0xee}};
static unsigned checks, failures, rejected;
static const char sha1[] = "8ad545533d67df2f78e0550ae0d97a283ebf452b";
static const char sha256[] = "278fdc9ae9432a46767a1ff9b1b857df0422d52770ecb4e9f9934f6c69093e13";

static void check(const char *label, BOOL ok)
{
    ++checks;
    if (!ok) ++failures;
    printf("%s %s\n", ok ? "PASS" : "FAIL", label);
}

static BOOL inside(const void *allocation, DWORD size, const void *value, DWORD length)
{
    uintptr_t base = (uintptr_t)allocation, address = (uintptr_t)value;
    return address >= base && address - base <= size && length <= size - (address - base);
}

static BOOL digest(SIP_SUBJECTINFO *subject, const char *oid, const char *expected)
{
    SIP_INDIRECT_DATA *data;
    DWORD required = 0, size, i, length = strlen(expected) / 2;
    BOOL ok, untouched;
    char actual[129];
    subject->DigestAlgorithm.pszObjId = (char *)oid;
    ok = CryptSIPCreateIndirectData(subject, &required, NULL);
    check("SIP size query", ok && required >= sizeof(*data) && required < 1024);
    if (!ok || required < sizeof(*data) || required >= 1024) return FALSE;
    data = malloc(required + 16);
    if (!data) { check("allocate SIP result", FALSE); return FALSE; }
    memset(data, 0xa5, required + 16);
    size = required - 1;
    SetLastError(0);
    ok = CryptSIPCreateIndirectData(subject, &size, data);
    untouched = TRUE;
    for (i = 0; i < required + 16; ++i) if (((BYTE *)data)[i] != 0xa5) untouched = FALSE;
    check("SIP short buffer unchanged", !ok && GetLastError() == ERROR_INSUFFICIENT_BUFFER && size == required && untouched);
    size = required + 16;
    ok = CryptSIPCreateIndirectData(subject, &size, data);
    check("SIP full buffer", ok);
    if (ok)
    {
        check("SIP digest bounded", data->Digest.cbData == length && inside(data, required, data->Digest.pbData, length));
        if (data->Digest.cbData == length && inside(data, required, data->Digest.pbData, length))
        {
            for (i = 0; i < length; ++i) sprintf(actual + 2*i, "%02x", data->Digest.pbData[i]);
            check("SIP independent digest reference", !strcmp(actual, expected));
        }
        check("SIP data OID", inside(data, required, data->Data.pszObjId, sizeof(SPC_PE_IMAGE_DATA_OBJID)) &&
              !memcmp(data->Data.pszObjId, SPC_PE_IMAGE_DATA_OBJID, sizeof(SPC_PE_IMAGE_DATA_OBJID)));
        check("SIP algorithm OID", inside(data, required, data->DigestAlgorithm.pszObjId, strlen(oid)+1) &&
              !memcmp(data->DigestAlgorithm.pszObjId, oid, strlen(oid)+1));
        check("SIP encoded value bounded", inside(data, required, data->Data.Value.pbData, data->Data.Value.cbData));
        untouched = TRUE;
        for (i = required; i < required+16; ++i) if (((BYTE *)data)[i] != 0xa5) untouched = FALSE;
        check("SIP output canary", untouched);
        /* Exact native Data.Value and total required count are unmeasured. */
    }
    free(data);
    return ok;
}

static BOOL write_fixture(HANDLE file, const BYTE *data, DWORD length)
{
    LARGE_INTEGER zero = {{0}};
    DWORD written;
    BOOL ok = SetFilePointerEx(file, zero, NULL, FILE_BEGIN) &&
              WriteFile(file, data, length, &written, NULL) && written == length && SetEndOfFile(file);
    if (!ok) check("write test fixture", FALSE);
    return ok;
}

static void set16(BYTE *data, DWORD offset, WORD value) { memcpy(data+offset, &value, sizeof(value)); }
static void set32(BYTE *data, DWORD offset, DWORD value) { memcpy(data+offset, &value, sizeof(value)); }

static void reject(SIP_SUBJECTINFO *subject, const char *label, DWORD expected)
{
    DWORD size = 0xfeedface;
    BOOL ok;
    subject->DigestAlgorithm.pszObjId = (char *)szOID_NIST_sha256;
    SetLastError(0);
    ok = CryptSIPCreateIndirectData(subject, &size, NULL);
    check(label, !ok && GetLastError() == expected && size == 0xfeedface);
    ++rejected;
}

int main(void)
{
    enum { PE_OPTIONAL = FILE_PE_START + 24, PE_DIRECTORY = PE_OPTIONAL + 96,
           PE_SECURITY = PE_DIRECTORY + 4*8, PE_SECTION = PE_OPTIONAL + sizeof(IMAGE_OPTIONAL_HEADER32) };
    SIP_SUBJECTINFO subject = {0};
    SIP_DISPATCH_INFO dispatch = {0};
    HCRYPTPROV provider = 0;
    HCRYPTHASH hash = 0;
    HMODULE module;
    HRESULT (WINAPI *register_server)(void);
    HANDLE file = INVALID_HANDLE_VALUE;
    BYTE data[FILE_TOTAL+32];
    IMAGE_OPTIONAL_HEADER64 optional64 = {0};
    WCHAR temporary[MAX_PATH], path[MAX_PATH];
    LARGE_INTEGER large;
    DWORD i;

    _Static_assert(sizeof(bin) == FILE_TOTAL, "public PE fixture layout changed");
    setvbuf(stdout, NULL, _IONBF, 0);
    if (!GetTempPathW(MAX_PATH, temporary) || !GetTempFileNameW(temporary, L"sip", 0, path)) return 2;
    file = CreateFileW(path, GENERIC_READ|GENERIC_WRITE, FILE_SHARE_READ|FILE_SHARE_WRITE|FILE_SHARE_DELETE,
                       NULL, OPEN_EXISTING, FILE_ATTRIBUTE_TEMPORARY, NULL);
    if (file == INVALID_HANDLE_VALUE) { DeleteFileW(path); return 2; }
    module = LoadLibraryW(L"wintrust.dll");
    register_server = module ? (void *)GetProcAddress(module, "DllRegisterServer") : NULL;
    if (!register_server || FAILED(register_server())) { check("register SIP provider", FALSE); goto done; }
    dispatch.cbSize = sizeof(dispatch);
    check("load registered PE SIP provider", CryptSIPLoad(&pe, 0, &dispatch));
    subject.cbSize = sizeof(subject);
    subject.pgSubjectType = &pe;
    subject.hFile = file;
    subject.dwEncodingType = X509_ASN_ENCODING|PKCS_7_ASN_ENCODING;
    if (!write_fixture(file, (const BYTE *)&bin, sizeof(bin))) { check("write public fixture", FALSE); goto done; }
    if (!digest(&subject, szOID_OIWSEC_sha1, sha1)) goto done;
    digest(&subject, szOID_NIST_sha256, sha256);

    check("acquire caller provider", CryptAcquireContextW(&provider, NULL, MS_ENH_RSA_AES_PROV_W, PROV_RSA_AES, CRYPT_VERIFYCONTEXT));
    subject.hProv = provider;
    subject.hFile = INVALID_HANDLE_VALUE;
    subject.pwsFileName = path;
    digest(&subject, szOID_NIST_sha256, sha256);
    check("caller provider retained", CryptCreateHash(provider, CALG_SHA_256, 0, 0, &hash));
    if (hash) { CryptDestroyHash(hash); hash = 0; }
    subject.hProv = 0;
    subject.hFile = file;

    /* Independent fixed PE32+ layout; never loaded as a process image. */
    memcpy(data, &bin, FILE_TOTAL);
    optional64.Magic = IMAGE_NT_OPTIONAL_HDR64_MAGIC;
    optional64.AddressOfEntryPoint = optional64.BaseOfCode = RVA_TEXT;
    optional64.ImageBase = VA_START;
    optional64.SectionAlignment = 0x1000;
    optional64.FileAlignment = FILE_TEXT;
    optional64.SizeOfImage = RVA_TOTAL;
    optional64.SizeOfHeaders = FILE_TEXT;
    optional64.Subsystem = IMAGE_SUBSYSTEM_WINDOWS_GUI;
    optional64.NumberOfRvaAndSizes = 16;
    memcpy(data+PE_OPTIONAL, &optional64, sizeof(optional64));
    memcpy(data+PE_OPTIONAL+sizeof(optional64), bin.sections, sizeof(bin.sections));
    set16(data, FILE_PE_START+4, IMAGE_FILE_MACHINE_AMD64);
    set16(data, FILE_PE_START+20, sizeof(optional64));
    set16(data, FILE_PE_START+22, IMAGE_FILE_EXECUTABLE_IMAGE|IMAGE_FILE_LARGE_ADDRESS_AWARE);
    write_fixture(file, data, FILE_TOTAL);
    digest(&subject, szOID_NIST_sha256, "546ca98c01be8a6ae2d3543be2526b97c98f9391deee7b426322057869e216c0");

    /* Excluded checksum, included content, and excluded certificate mutations. */
    memcpy(data, &bin, FILE_TOTAL);
    for (i=0; i<4; ++i) data[PE_OPTIONAL+64+i] ^= 0xa5;
    write_fixture(file, data, FILE_TOTAL);
    digest(&subject, szOID_NIST_sha256, sha256);
    memcpy(data, &bin, FILE_TOTAL); data[FILE_TEXT] ^= 1;
    write_fixture(file, data, FILE_TOTAL);
    digest(&subject, szOID_NIST_sha256, "9e7369486dd22db07a7482373843f7080b3ad31e2f0d7d6b5e5c5cbf725a00e6");
    memcpy(data, &bin, FILE_TOTAL); memset(data+FILE_TOTAL, 0, 16);
    set32(data, PE_SECURITY, FILE_TOTAL); set32(data, PE_SECURITY+4, 16);
    write_fixture(file, data, FILE_TOTAL+16);
    digest(&subject, szOID_NIST_sha256, sha256);
    data[FILE_TOTAL+15] = 0xa5;
    write_fixture(file, data, FILE_TOTAL+16);
    digest(&subject, szOID_NIST_sha256, sha256);

    for (i=0; i<22; ++i)
    {
        DWORD length = FILE_TOTAL, error = ERROR_BAD_FORMAT;
        char label[64];
        memcpy(data, &bin, FILE_TOTAL); memset(data+FILE_TOTAL, 0, 32);
        switch (i)
        {
        case 0: length=0; break;
        case 1: length=63; break;
        case 2: set16(data, 0, 0); break;
        case 3: set32(data, 0x3c, 0xffffffff); break;
        case 4: set32(data, 0x3c, FILE_TOTAL-2); break;
        case 5: set32(data, FILE_PE_START, 0); break;
        case 6: set16(data, FILE_PE_START+20, 2); break;
        case 7: set16(data, FILE_PE_START+20, 0xffff); break;
        case 8: set16(data, PE_OPTIONAL, 0); break;
        case 9: set32(data, PE_DIRECTORY-4, 4); break;
        case 10: set32(data, PE_DIRECTORY-4, 0xffffffff); break;
        case 11: set16(data, FILE_PE_START+6, 0xffff); break;
        case 12: set32(data, PE_OPTIONAL+60, 1); break;
        case 13: set32(data, PE_OPTIONAL+60, FILE_TOTAL+1); break;
        case 14: set32(data, PE_SECTION+16, 0xffffffff); break;
        case 15: set32(data, PE_SECTION+20, 0xffffffff); break;
        case 16: set32(data, PE_SECTION+20, FILE_TEXT+8); error=ERROR_NOT_SUPPORTED; break;
        case 17: set32(data, PE_SECURITY, FILE_TOTAL); break;
        case 18: set32(data, PE_SECURITY, FILE_TOTAL+8); set32(data, PE_SECURITY+4, 16); break;
        case 19: length+=24; set32(data, PE_SECURITY, FILE_TOTAL); set32(data, PE_SECURITY+4, 16); error=ERROR_NOT_SUPPORTED; break;
        case 20: length+=17; set32(data, PE_SECURITY, FILE_TOTAL+1); set32(data, PE_SECURITY+4, 16); error=ERROR_NOT_SUPPORTED; break;
        case 21: error=ERROR_NOT_SUPPORTED; break;
        }
        if (!write_fixture(file, data, length)) { check("write malformed fixture", FALSE); continue; }
        if (i==21)
        {
            large.QuadPart = 0x100000000ULL;
            if (!SetFilePointerEx(file, large, NULL, FILE_BEGIN) || !SetEndOfFile(file))
            { check("create sparse size boundary", FALSE); continue; }
        }
        sprintf(label, "reject malformed/unsupported PE %02lu", i);
        reject(&subject, label, error);
    }
done:
    if (hash) CryptDestroyHash(hash);
    if (provider) CryptReleaseContext(provider, 0);
    if (file != INVALID_HANDLE_VALUE) CloseHandle(file);
    DeleteFileW(path);
    printf("SUMMARY checks=%u rejected_pe=%u failures=%u\n", checks, rejected, failures);
    return failures ? 1 : 0;
}
