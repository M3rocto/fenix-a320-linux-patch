// SPDX-License-Identifier: MIT
// Original bounded control: signature queries on an explicitly selected file.
#define WIN32_LEAN_AND_MEAN
#include <windows.h>
#include <wincrypt.h>
#include <psapi.h>
#include <cstdio>
#include <cwchar>

static unsigned query_count, close_errors;
static ULONGLONG started;
struct Handles { HCERTSTORE store; HCRYPTMSG message; };

static bool close_handles(Handles &h) {
    bool ok = true;
    if (h.message && !CryptMsgClose(h.message)) { ++close_errors; ok = false; }
    h.message = nullptr;
    if (h.store && !CertCloseStore(h.store, 0)) { ++close_errors; ok = false; }
    h.store = nullptr;
    return ok;
}

static bool query(const wchar_t *file, Handles &h) {
    DWORD encoding = 0, content = 0, format = 0;
    if (GetTickCount64() - started > 20000) {
        std::fprintf(stderr, "ERROR internal deadline\n");
        return false;
    }
    if (!CryptQueryObject(CERT_QUERY_OBJECT_FILE, file,
            CERT_QUERY_CONTENT_FLAG_PKCS7_SIGNED_EMBED,
            CERT_QUERY_FORMAT_FLAG_BINARY, 0, &encoding, &content, &format,
            &h.store, &h.message, nullptr)) {
        std::fprintf(stderr, "ERROR CryptQueryObject=%lu at query=%u\n",
                     GetLastError(), query_count);
        close_handles(h);
        return false;
    }
    ++query_count;
    if (!h.store || !h.message || content != CERT_QUERY_CONTENT_PKCS7_SIGNED_EMBED
            || format != CERT_QUERY_FORMAT_BINARY) {
        std::fprintf(stderr, "ERROR unexpected query outputs\n");
        close_handles(h);
        return false;
    }
    return true;
}

static bool sample(const char *phase, unsigned retained) {
    PROCESS_MEMORY_COUNTERS_EX counters{};
    counters.cb = sizeof(counters);
    if (!GetProcessMemoryInfo(GetCurrentProcess(),
            reinterpret_cast<PROCESS_MEMORY_COUNTERS *>(&counters), sizeof(counters)))
        return false;
    std::printf("SAMPLE,%s,%u,%u,%llu,%llu,%llu\n", phase, query_count, retained,
        static_cast<unsigned long long>(GetTickCount64() - started),
        static_cast<unsigned long long>(counters.WorkingSetSize),
        static_cast<unsigned long long>(counters.PrivateUsage));
    std::fflush(stdout);
    return true;
}

static bool closed_queries(const wchar_t *file, unsigned count, const char *phase) {
    for (unsigned i = 0; i < count; ++i) {
        Handles h{};
        if (!query(file, h)) return false;
        if (!close_handles(h)) return false;
        if ((i + 1) % 500 == 0 && !sample(phase, 0)) return false;
    }
    return true;
}

int wmain(int argc, wchar_t **argv) {
    if (argc != 3 || (std::wcscmp(argv[2], L"smoke") &&
            std::wcscmp(argv[2], L"bounded"))) {
        std::fprintf(stderr, "Usage: certificate-query-probe.exe FILE smoke|bounded\n");
        return 2;
    }
    started = GetTickCount64();
    char module[MAX_PATH]{};
    GetModuleFileNameA(GetModuleHandleW(L"crypt32.dll"), module, MAX_PATH);
    std::printf("MODULE,crypt32,%s\n", module);
    std::printf("COLUMNS,phase,total_queries,retained_handles_pairs,elapsed_ms,working_set_bytes,private_bytes\n");
    if (!sample("initial", 0)) return 1;
    if (!std::wcscmp(argv[2], L"smoke")) {
        if (!closed_queries(argv[1], 1, "smoke") || !sample("smoke-closed", 0)) return 1;
        std::printf("SUMMARY,queries=%u,close_errors=%u,passed=1\n", query_count, close_errors);
        return 0;
    }
    if (!closed_queries(argv[1], 128, "warmup") || !sample("warm", 0)) return 1;
    if (!closed_queries(argv[1], 4000, "closed-1")) return 1;
    Handles retained[256]{};
    bool ok = true;
    unsigned held = 0;
    for (; held < 256; ++held) {
        if (!query(argv[1], retained[held])) { ok = false; break; }
        if ((held + 1) % 64 == 0 && !sample("retained-control", held + 1)) {
            ++held; ok = false; break;
        }
    }
    for (unsigned i = 0; i < held; ++i) ok = close_handles(retained[i]) && ok;
    ok = sample("released-control", 0) && ok;
    if (ok) ok = closed_queries(argv[1], 4000, "closed-2");
    std::printf("SUMMARY,queries=%u,close_errors=%u,passed=%u\n", query_count,
                close_errors, static_cast<unsigned>(ok && !close_errors));
    return ok && !close_errors ? 0 : 1;
}
