/* SPDX-License-Identifier: MIT
 * Repeated geometry destruction and near-degenerate paths (GitHub issue #8).
 * A watchdog turns a stuck GeometrySink::Close into a reproducible test failure.
 */
#include <windows.h>
#include <d2d1.h>
#include <psapi.h>
#include <cstdio>
#include <cstdlib>
#include <cstdint>
#include <cmath>
#include <cstring>
#include <initializer_list>

static volatile LONG active_case = -1, started, finished;

static void check(HRESULT hr, const char *step)
{
    if (FAILED(hr)) { printf("FAIL %s: %08lx\n", step, hr); exit(2); }
}

static DWORD WINAPI watchdog(void *)
{
    while (!InterlockedCompareExchange(&finished, 0, 0)) {
        LONG current = InterlockedCompareExchange(&active_case, 0, 0);
        DWORD tick = (DWORD)InterlockedCompareExchange(&started, 0, 0);
        if (current >= 0 && GetTickCount() - tick > 5000) {
            printf("FAIL Close timeout at deterministic case %ld\n", current);
            TerminateProcess(GetCurrentProcess(), 3);
        }
        Sleep(25);
    }
    return 0;
}

static SIZE_T heap_bytes()
{
    HANDLE heap = GetProcessHeap();
    PROCESS_HEAP_ENTRY entry = {};
    SIZE_T bytes = 0;
    if (!HeapLock(heap)) { puts("FAIL HeapLock"); exit(2); }
    while (HeapWalk(heap, &entry))
        if (entry.wFlags & PROCESS_HEAP_ENTRY_BUSY) bytes += entry.cbData;
    DWORD error = GetLastError();
    HeapUnlock(heap);
    if (error != ERROR_NO_MORE_ITEMS) { printf("FAIL HeapWalk: %lu\n", error); exit(2); }
    return bytes;
}

static void figures(ID2D1Factory *factory, unsigned count, bool wrong_state)
{
    for (unsigned i = 0; i < count; ++i) {
        ID2D1PathGeometry *geometry;
        ID2D1GeometrySink *sink;
        check(factory->CreatePathGeometry(&geometry), "create");
        check(geometry->Open(&sink), "open");
        sink->BeginFigure(D2D1::Point2F(0, 0), D2D1_FIGURE_BEGIN_FILLED);
        const D2D1_POINT_2F points[] = {{10, 0}, {10, 10}, {0, 10}, {0, 0}};
        sink->AddLines(points, 4);
        if (!wrong_state) sink->EndFigure(D2D1_FIGURE_END_CLOSED);
        HRESULT hr = sink->Close();
        if (wrong_state) {
            if (hr != D2DERR_WRONG_STATE) { puts("FAIL missing Close state error"); exit(2); }
        } else check(hr, "close");
        sink->Release();
        geometry->Release();
    }
}

static void lifetime(ID2D1Factory *factory)
{
    for (bool wrong_state : {false, true}) {
        figures(factory, 10000, wrong_state);
        SIZE_T before = heap_bytes();
        figures(factory, 300000, wrong_state);
        SIZE_T after = heap_bytes();
        long long growth = (long long)after - before;
        printf("geometry release (%s): live heap growth=%lld bytes / 300000 figures\n",
               wrong_state ? "Close error" : "closed", growth);
        // The original leaks about 16 MB; allow unrelated Wine bookkeeping.
        if (growth > 1024 * 1024) { puts("FAIL geometry heap leak"); exit(2); }
    }
}

static uint32_t random_bits(uint32_t &state)
{
    state ^= state << 13; state ^= state >> 17; state ^= state << 5;
    return state;
}

static D2D1_POINT_2F point(uint32_t &state, D2D1_POINT_2F previous)
{
    switch (random_bits(state) % 5) {
    case 0: return previous;
    case 1: return D2D1::Point2F(previous.x + ((int)(random_bits(state) % 3) - 1) * .00001f,
                               previous.y + ((int)(random_bits(state) % 3) - 1) * .00001f);
    default: return D2D1::Point2F((random_bits(state) % 1000) * .01f,
                                 (random_bits(state) % 1000) * .01f);
    }
}

static void stress(ID2D1Factory *factory, unsigned count, unsigned first = 0)
{
    HANDLE worker = CreateThread(nullptr, 0, watchdog, nullptr, 0, nullptr);
    if (!worker) { puts("FAIL watchdog creation"); exit(2); }
    unsigned errors = 0;
    DWORD longest = 0;
    for (unsigned i = first; i < first + count; ++i) {
        // Every case has its own seed, so a reported index can be rerun directly.
        uint32_t seed = 0x8f3a291dU ^ (i + 1) * 0x9e3779b9U;
        ID2D1PathGeometry *geometry;
        ID2D1GeometrySink *sink;
        check(factory->CreatePathGeometry(&geometry), "stress create");
        check(geometry->Open(&sink), "stress open");
        D2D1_POINT_2F p = point(seed, D2D1::Point2F(0, 0));
        sink->BeginFigure(p, D2D1_FIGURE_BEGIN_FILLED);
        for (unsigned j = 0; j < 4; ++j) {
            D2D1_POINT_2F control = point(seed, p), next = point(seed, control);
            sink->AddQuadraticBezier(D2D1::QuadraticBezierSegment(control, next));
            p = point(seed, next);
            sink->AddLine(p);
        }
        sink->EndFigure(D2D1_FIGURE_END_CLOSED);
        DWORD tick = GetTickCount();
        InterlockedExchange(&started, (LONG)tick);
        InterlockedExchange(&active_case, i);
        HRESULT hr = sink->Close();
        InterlockedExchange(&active_case, -1);
        DWORD elapsed = GetTickCount() - tick;
        if (elapsed > longest) longest = elapsed;
        // Degenerate/self-intersecting paths may legitimately fail triangulation;
        // this regression checks termination, separately from rendering checks.
        if (FAILED(hr)) ++errors;
        sink->Release();
        geometry->Release();
    }
    InterlockedExchange(&finished, 1);
    WaitForSingleObject(worker, INFINITE);
    CloseHandle(worker);
    printf("degenerate paths: %u completed, %u triangulation errors, max Close=%lu ms\n",
           count, errors, longest);
}

int main(int argc, char **argv)
{
    setvbuf(stdout, nullptr, _IONBF, 0);
    ID2D1Factory *factory;
    check(D2D1CreateFactory(D2D1_FACTORY_TYPE_SINGLE_THREADED, &factory), "factory");
    if (argc == 1 || !strcmp(argv[1], "--lifetime")) lifetime(factory);
    if (argc == 1 || !strcmp(argv[1], "--stress")) stress(factory, 20000);
    if (argc == 3 && !strcmp(argv[1], "--case")) stress(factory, 1, strtoul(argv[2], nullptr, 10));
    factory->Release();
    puts("Direct2D lifetime/termination checks passed");
    return 0;
}
