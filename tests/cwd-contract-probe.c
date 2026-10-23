/* SPDX-License-Identifier: MIT. Synthetic current-directory contract controls. */
#define WIN32_LEAN_AND_MEAN
#define _WIN32_WINNT 0x0a00
#include <windows.h>
#include <stdio.h>
#include <wchar.h>

#ifndef CWD_PROBE_PREFIX
#error "Define CWD_PROBE_PREFIX as the explicitly owned test prefix"
#endif

static unsigned int checks, failures;
#define CHECK(test, label) do { ++checks; if (!(test)) { ++failures; \
    wprintf(L"FAIL %ls error=%lu\n", label, GetLastError()); } } while (0)

static BOOL same_file(const WCHAR *a, const WCHAR *b, BOOL directory)
{
    BY_HANDLE_FILE_INFORMATION ia, ib;
    HANDLE ha, hb;
    BOOL ok = FALSE;
    DWORD flags = directory ? FILE_FLAG_BACKUP_SEMANTICS : 0;
    ha = CreateFileW(a, FILE_READ_ATTRIBUTES, FILE_SHARE_READ | FILE_SHARE_WRITE | FILE_SHARE_DELETE,
                     NULL, OPEN_EXISTING, flags, NULL);
    hb = CreateFileW(b, FILE_READ_ATTRIBUTES, FILE_SHARE_READ | FILE_SHARE_WRITE | FILE_SHARE_DELETE,
                     NULL, OPEN_EXISTING, flags, NULL);
    if (ha != INVALID_HANDLE_VALUE && hb != INVALID_HANDLE_VALUE &&
        GetFileInformationByHandle(ha, &ia) && GetFileInformationByHandle(hb, &ib))
        ok = ia.dwVolumeSerialNumber == ib.dwVolumeSerialNumber &&
             ia.nFileIndexHigh == ib.nFileIndexHigh && ia.nFileIndexLow == ib.nFileIndexLow;
    if (ha != INVALID_HANDLE_VALUE) CloseHandle(ha);
    if (hb != INVALID_HANDLE_VALUE) CloseHandle(hb);
    return ok;
}

static void check_paths(const WCHAR *base)
{
    WCHAR cwd[1024], full[1024], expected[1024], parent[1024], root[1024], relative[32];
    char narrow[1024];
    DWORD length;
    WCHAR *last;
    length = GetCurrentDirectoryW(ARRAYSIZE(cwd), cwd);
    wprintf(L"  cwd=%ls\n", length ? cwd : L"FAIL");
    CHECK(length && ((cwd[1] == ':' && cwd[2] == '\\') || (cwd[0] == '\\' && cwd[1] == '\\')), L"cwd absolute");
    CHECK(length && same_file(cwd, base, TRUE), L"cwd identity");
    length = GetCurrentDirectoryA(ARRAYSIZE(narrow), narrow);
    if (length) MultiByteToWideChar(CP_ACP, 0, narrow, -1, full, ARRAYSIZE(full));
    CHECK(length && !wcscmp(cwd, full), L"cwd A/W identity");
    length = GetCurrentDirectoryW(0, NULL);
    CHECK(length == wcslen(cwd) + 1, L"cwd size query");
    full[0] = 0x1234;
    CHECK(GetCurrentDirectoryW(1, full) == length && full[0] == 0x1234, L"cwd short buffer untouched");
    swprintf(expected, ARRAYSIZE(expected), L"%ls\\own-control.txt", base);
    length = GetFullPathNameW(L"own-control.txt", ARRAYSIZE(full), full, NULL);
    CHECK(length && !wcscmp(full, expected), L"relative full path identity");
    CHECK(same_file(full, expected, FALSE), L"full relative reopen identity");
    CHECK(same_file(L"own-control.txt", expected, FALSE), L"native relative reopen identity");
    length = GetFullPathNameW(L".", ARRAYSIZE(full), full, NULL);
    CHECK(length && same_file(full, base, TRUE), L"dot current identity");
    wcscpy(parent, base);
    last = wcsrchr(parent, '\\');
    if (last && last == parent + 2) last[1] = 0;
    else if (last) *last = 0;
    length = GetFullPathNameW(L"..", ARRAYSIZE(full), full, NULL);
    CHECK(length && same_file(full, parent, TRUE), L"parent traversal identity");
    if (base[1] == ':') {
        swprintf(root, ARRAYSIZE(root), L"%lc:\\cwd-root-control.txt", base[0]);
        swprintf(relative, ARRAYSIZE(relative), L"%lc:own-control.txt", base[0]);
        length = GetFullPathNameW(relative, ARRAYSIZE(full), full, NULL);
        CHECK(length && !wcscmp(full, expected), L"same drive relative path identity");
    } else {
        wcscpy(root, base);
        last = wcsrchr(root, '\\');
        if (last) wcscpy(last, L"\\cwd-root-control.txt");
    }
    length = GetFullPathNameW(L"\\cwd-root-control.txt", ARRAYSIZE(full), full, NULL);
    CHECK(length && !wcscmp(full, root), L"rooted path identity");
    CHECK(same_file(full, root, FALSE), L"rooted reopen identity");
}

static void inherit(const WCHAR *base)
{
    WCHAR command[2048];
    STARTUPINFOW startup = {sizeof(startup)};
    PROCESS_INFORMATION process;
    DWORD code = 99;
    BOOL ok;
    swprintf(command, ARRAYSIZE(command), L"C:\\probe\\cwd-contract-probe.exe --child \"%ls\"", base);
    startup.dwFlags = STARTF_USESTDHANDLES;
    startup.hStdInput = GetStdHandle(STD_INPUT_HANDLE);
    startup.hStdOutput = GetStdHandle(STD_OUTPUT_HANDLE);
    startup.hStdError = GetStdHandle(STD_ERROR_HANDLE);
    ok = CreateProcessW(NULL, command, NULL, NULL, TRUE, CREATE_NO_WINDOW, NULL, NULL, &startup, &process);
    CHECK(ok, L"inherited-cwd child creation");
    if (!ok) return;
    if (WaitForSingleObject(process.hProcess, 5000) == WAIT_OBJECT_0)
        GetExitCodeProcess(process.hProcess, &code);
    else { TerminateProcess(process.hProcess, 99); WaitForSingleObject(process.hProcess, 1000); }
    CHECK(code == 0, L"inherited-cwd child contract");
    CloseHandle(process.hThread);
    CloseHandle(process.hProcess);
}

static void one(const WCHAR *input, const WCHAR *expected)
{
    BOOL ok;
    CHECK(SetCurrentDirectoryW(L"C:\\probe"), L"reset cwd");
    SetLastError(0);
    ok = SetCurrentDirectoryW(input);
    wprintf(L"CASE input=%ls expected=%ls success=%d error=%lu\n", input, expected, ok, ok ? 0 : GetLastError());
    CHECK(ok, L"set fixture cwd");
    if (!ok) return;
    check_paths(expected);
    fflush(stdout);
    inherit(expected);
}

static void global_alias(const WCHAR *dos, WCHAR *global, DWORD capacity, BOOL doubled)
{
    HANDLE h;
    WCHAR nt[1024], *separator;
    DWORD n;
    h = CreateFileW(dos, FILE_READ_ATTRIBUTES, FILE_SHARE_READ | FILE_SHARE_WRITE | FILE_SHARE_DELETE,
                     NULL, OPEN_EXISTING, FILE_FLAG_BACKUP_SEMANTICS, NULL);
    CHECK(h != INVALID_HANDLE_VALUE, L"volume fixture open");
    if (h == INVALID_HANDLE_VALUE) { global[0] = 0; return; }
    n = GetFinalPathNameByHandleW(h, nt, ARRAYSIZE(nt), VOLUME_NAME_NT);
    CloseHandle(h);
    CHECK(n && !wcsncmp(nt, L"\\Device\\HarddiskVolume", 22), L"live NT volume name");
    if (!n) { global[0] = 0; return; }
    if (doubled && (separator = wcschr(nt + 22, '\\'))) {
        swprintf(global, capacity, L"\\\\?\\GLOBALROOT%.*ls\\%ls", (int)(separator - nt), nt, separator);
    } else swprintf(global, capacity, L"\\\\?\\GLOBALROOT%ls", nt);
    wprintf(L"ALIAS DOS=%ls NT=%ls global=%ls\n", dos, nt, global);
}

int wmain(int argc, WCHAR **argv)
{
    WCHAR prefix[1024], global[2048], path[1024], old[1024], current[1024], suffix[300];
    unsigned int length;
    BOOL ok;
    if (!GetEnvironmentVariableW(L"WINEPREFIX", prefix, ARRAYSIZE(prefix)) ||
        wcscmp(prefix, CWD_PROBE_PREFIX)) return 2;
    if (argc > 2 && !wcscmp(argv[1], L"--child")) {
        wprintf(L"INHERITED expected=%ls\n", argv[2]);
        check_paths(argv[2]);
        wprintf(L"CHILD checks=%u failures=%u\n", checks, failures);
        return failures ? 1 : 0;
    }
    if (argc > 1 && !wcscmp(argv[1], L"--negative")) {
        const WCHAR *bad[] = {
            L"\\\\?\\GLOBALROOT\\Device\\HarddiskVolume99999\\probe",
            L"\\\\?\\GLOBALROOT_SUFFIX\\Device\\HarddiskVolume1\\probe",
            L"\\\\?\\GLOBALROOT\\Device\\Null",
            L"\\\\?\\GLOBALROOT\\Device\\HarddiskVolume1\\probe\\own-control.txt"
        };
        unsigned int i;
        CHECK(SetCurrentDirectoryW(L"C:\\probe"), L"negative reset");
        GetCurrentDirectoryW(ARRAYSIZE(old), old);
        for (i = 0; i < ARRAYSIZE(bad); ++i) {
            ok = SetCurrentDirectoryW(bad[i]);
            wprintf(L"NEGATIVE path=%ls success=%d error=%lu\n", bad[i], ok, ok ? 0 : GetLastError());
            CHECK(!ok, L"wrong volume/namespace/object rejects");
            GetCurrentDirectoryW(ARRAYSIZE(current), current);
            CHECK(!wcscmp(old, current), L"negative preserves cwd");
        }
        CHECK(!same_file(L"C:\\probe\\own-control.txt", L"D:\\probe\\own-control.txt", FALSE), L"C/D fixtures distinct");
        wprintf(L"NEGATIVE RESULT checks=%u failures=%u\n", checks, failures);
        return failures ? 1 : 0;
    }
    one(L"C:\\probe", L"C:\\probe");
    one(L"\\\\?\\C:\\probe", L"C:\\probe");
    global_alias(L"C:\\probe", global, ARRAYSIZE(global), FALSE);
    if (*global) one(global, L"C:\\probe");
    global_alias(L"C:\\probe", global, ARRAYSIZE(global), TRUE);
    if (*global) one(global, L"C:\\probe");
    one(L"D:\\probe", L"D:\\probe");
    one(L"\\\\?\\D:\\probe", L"D:\\probe");
    global_alias(L"D:\\probe", global, ARRAYSIZE(global), FALSE);
    if (*global) one(global, L"D:\\probe");
    global_alias(L"D:\\probe", global, ARRAYSIZE(global), TRUE);
    if (*global) one(global, L"D:\\probe");
    /* A private filesystem UNC mapping is a namespace control, not live SMB. */
    one(L"\\\\cwd-own-test\\share\\probe", L"\\\\cwd-own-test\\share\\probe");
    one(L"\\\\?\\UNC\\cwd-own-test\\share\\probe", L"\\\\cwd-own-test\\share\\probe");
    CHECK(SetCurrentDirectoryW(L"C:\\probe"), L"reset before rejection");
    GetCurrentDirectoryW(ARRAYSIZE(old), old);
    ok = SetCurrentDirectoryW(L"\\\\?\\GLOBALROOT\\Device\\HarddiskVolume1\\definitely-own-missing-cwd");
    CHECK(!ok, L"missing cwd rejection");
    GetCurrentDirectoryW(ARRAYSIZE(current), current);
    CHECK(!wcscmp(old, current), L"rejected missing cwd preserves state");
    if (!(argc > 1 && !wcscmp(argv[1], L"--skip-boundary"))) {
        for (length = 258; length <= 260; ++length) {
            unsigned int count = length - (unsigned int)wcslen(L"C:\\probe\\boundary\\");
            wmemset(suffix, L'a', count); suffix[count] = 0;
            swprintf(path, ARRAYSIZE(path), L"C:\\probe\\boundary\\%ls", suffix);
            global_alias(path, global, ARRAYSIZE(global), FALSE);
            CHECK(SetCurrentDirectoryW(L"C:\\probe"), L"boundary reset");
            GetCurrentDirectoryW(ARRAYSIZE(old), old);
            ok = SetCurrentDirectoryW(global);
            wprintf(L"BOUNDARY DOS-length=%u success=%d error=%lu\n", length, ok, ok ? 0 : GetLastError());
            CHECK(ok == (length == 258), L"trailing slash plus NUL capacity");
            GetCurrentDirectoryW(ARRAYSIZE(current), current);
            if (ok) CHECK(wcslen(current) == length && same_file(current, path, TRUE), L"boundary cwd identity");
            else CHECK(!wcscmp(old, current), L"over-capacity rejection preserves cwd");
        }
    }
    SetCurrentDirectoryW(L"C:\\probe");
    wprintf(L"RESULT checks=%u failures=%u\n", checks, failures);
    return failures ? 1 : 0;
}
