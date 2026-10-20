/* SPDX-License-Identifier: MIT
 * Direct Vulkan tests for Wine's virtual D3D12 fence contract. No Fenix code.
 * Build: MinGW C++17 -O2 -static -Wall -Wextra -Werror with Vulkan-Headers.
 * Modes: values, batch-peak, reset-history, owner-death <Windows arm-file path>.
 * owner-death additionally needs the narrowly scoped Linux LD_PRELOAD test hook.
 */
#define WIN32_LEAN_AND_MEAN
#define NOMINMAX
#define VK_USE_PLATFORM_WIN32_KHR
#define VK_NO_PROTOTYPES
#include <windows.h>
#include <vulkan/vulkan.h>
#include <algorithm>
#include <atomic>
#include <cstdio>
#include <cstdlib>
#include <cstring>
#include <stdexcept>
#include <string>
#include <vector>

static void need(bool condition, const char *message) {
    if (!condition) throw std::runtime_error(message);
}
static void check(VkResult result, const char *message) {
    if (result != VK_SUCCESS) {
        std::fprintf(stderr, "%s: VkResult=%d\n", message, result);
        throw std::runtime_error(message);
    }
}
struct Handle {
    HANDLE h = nullptr;
    ~Handle() { if (h && h != INVALID_HANDLE_VALUE) CloseHandle(h); }
};
struct Device {
    VkInstance instance = VK_NULL_HANDLE;
    VkPhysicalDevice physical = VK_NULL_HANDLE;
    VkDevice device = VK_NULL_HANDLE;
    VkQueue queue = VK_NULL_HANDLE;
    PFN_vkGetInstanceProcAddr getInstanceProc = nullptr;
    PFN_vkGetDeviceProcAddr getDeviceProc = nullptr;
    PFN_vkDestroyInstance destroyInstance = nullptr;
    PFN_vkDestroyDevice destroyDevice = nullptr;
    PFN_vkCreateSemaphore createSemaphore = nullptr;
    PFN_vkDestroySemaphore destroySemaphore = nullptr;
    PFN_vkSignalSemaphore signalSemaphore = nullptr;
    PFN_vkWaitSemaphores waitSemaphores = nullptr;
    PFN_vkGetSemaphoreCounterValue getCounter = nullptr;
    PFN_vkGetSemaphoreWin32HandleKHR exportSemaphore = nullptr;
    PFN_vkImportSemaphoreWin32HandleKHR importSemaphore = nullptr;
    PFN_vkQueueSubmit queueSubmit = nullptr;
    PFN_vkQueueWaitIdle queueWaitIdle = nullptr;
    template<class T> static T function(PFN_vkVoidFunction symbol) {
        need(symbol != nullptr, "missing Vulkan function");
        T result = nullptr;
        static_assert(sizeof(result) == sizeof(symbol));
        std::memcpy(&result, &symbol, sizeof(result));
        return result;
    }
    template<class T> T instance_function(const char *name) {
        return function<T>(getInstanceProc(instance, name));
    }
    template<class T> T device_function(const char *name) {
        return function<T>(getDeviceProc(device, name));
    }
    Device() {
        HMODULE library = LoadLibraryW(L"vulkan-1.dll");
        need(library != nullptr, "load vulkan-1.dll");
        const FARPROC symbol = GetProcAddress(library, "vkGetInstanceProcAddr");
        static_assert(sizeof(getInstanceProc) == sizeof(symbol));
        std::memcpy(&getInstanceProc, &symbol, sizeof(symbol));
        need(getInstanceProc != nullptr, "vkGetInstanceProcAddr");
        const auto create = instance_function<PFN_vkCreateInstance>("vkCreateInstance");
        VkApplicationInfo application = {};
        application.sType = VK_STRUCTURE_TYPE_APPLICATION_INFO;
        application.pApplicationName = "Own shared-fence regression";
        application.apiVersion = VK_API_VERSION_1_2;
        VkInstanceCreateInfo info = {};
        info.sType = VK_STRUCTURE_TYPE_INSTANCE_CREATE_INFO;
        info.pApplicationInfo = &application;
        check(create(&info, nullptr, &instance), "create instance");
        destroyInstance = instance_function<PFN_vkDestroyInstance>("vkDestroyInstance");
        const auto enumerate = instance_function<PFN_vkEnumeratePhysicalDevices>("vkEnumeratePhysicalDevices");
        uint32_t count = 0;
        check(enumerate(instance, &count, nullptr), "physical device count");
        need(count != 0, "physical GPU required");
        std::vector<VkPhysicalDevice> devices(count);
        check(enumerate(instance, &count, devices.data()), "physical devices");
        physical = devices[0];
        const auto properties = instance_function<PFN_vkGetPhysicalDeviceProperties>("vkGetPhysicalDeviceProperties");
        VkPhysicalDeviceProperties props = {};
        properties(physical, &props);
        std::printf("Vulkan device: %s\n", props.deviceName);
        VkPhysicalDeviceTimelineSemaphoreFeatures timeline = {};
        timeline.sType = VK_STRUCTURE_TYPE_PHYSICAL_DEVICE_TIMELINE_SEMAPHORE_FEATURES;
        timeline.timelineSemaphore = VK_TRUE;
        float priority = 1.0f;
        VkDeviceQueueCreateInfo queue_info = {};
        queue_info.sType = VK_STRUCTURE_TYPE_DEVICE_QUEUE_CREATE_INFO;
        queue_info.queueFamilyIndex = 0;
        queue_info.queueCount = 1;
        queue_info.pQueuePriorities = &priority;
        const char *extensions[] = {VK_KHR_EXTERNAL_SEMAPHORE_WIN32_EXTENSION_NAME};
        VkDeviceCreateInfo device_info = {};
        device_info.sType = VK_STRUCTURE_TYPE_DEVICE_CREATE_INFO;
        device_info.pNext = &timeline;
        device_info.queueCreateInfoCount = 1;
        device_info.pQueueCreateInfos = &queue_info;
        device_info.enabledExtensionCount = 1;
        device_info.ppEnabledExtensionNames = extensions;
        const auto create_device = instance_function<PFN_vkCreateDevice>("vkCreateDevice");
        check(create_device(physical, &device_info, nullptr, &device), "create device");
        getDeviceProc = instance_function<PFN_vkGetDeviceProcAddr>("vkGetDeviceProcAddr");
#define LOAD(member, name) member = device_function<PFN_##name>(#name)
        LOAD(destroyDevice, vkDestroyDevice);
        LOAD(createSemaphore, vkCreateSemaphore);
        LOAD(destroySemaphore, vkDestroySemaphore);
        LOAD(signalSemaphore, vkSignalSemaphore);
        LOAD(waitSemaphores, vkWaitSemaphores);
        LOAD(getCounter, vkGetSemaphoreCounterValue);
        LOAD(exportSemaphore, vkGetSemaphoreWin32HandleKHR);
        LOAD(importSemaphore, vkImportSemaphoreWin32HandleKHR);
        LOAD(queueSubmit, vkQueueSubmit);
        LOAD(queueWaitIdle, vkQueueWaitIdle);
#undef LOAD
        const auto get_queue = device_function<PFN_vkGetDeviceQueue>("vkGetDeviceQueue");
        get_queue(device, 0, 0, &queue);
    }
    ~Device() {
        if (device) destroyDevice(device, nullptr);
        if (instance) destroyInstance(instance, nullptr);
    }
    VkResult signal(VkSemaphore semaphore, uint64_t value) {
        VkSemaphoreSignalInfo info = {};
        info.sType = VK_STRUCTURE_TYPE_SEMAPHORE_SIGNAL_INFO;
        info.semaphore = semaphore;
        info.value = value;
        return signalSemaphore(device, &info);
    }
    VkResult wait(VkSemaphore semaphore, uint64_t value, uint64_t timeout) {
        VkSemaphoreWaitInfo info = {};
        info.sType = VK_STRUCTURE_TYPE_SEMAPHORE_WAIT_INFO;
        info.semaphoreCount = 1;
        info.pSemaphores = &semaphore;
        info.pValues = &value;
        return waitSemaphores(device, &info, timeout);
    }
    uint64_t counter(VkSemaphore semaphore) {
        uint64_t value = 0;
        check(getCounter(device, semaphore, &value), "counter");
        return value;
    }
    VkResult import(VkSemaphore semaphore, HANDLE handle,
                    VkExternalSemaphoreHandleTypeFlagBits type = VK_EXTERNAL_SEMAPHORE_HANDLE_TYPE_D3D12_FENCE_BIT,
                    VkSemaphoreImportFlags flags = 0) {
        VkImportSemaphoreWin32HandleInfoKHR info = {};
        info.sType = VK_STRUCTURE_TYPE_IMPORT_SEMAPHORE_WIN32_HANDLE_INFO_KHR;
        info.semaphore = semaphore;
        info.handleType = type;
        info.handle = handle;
        info.flags = flags;
        return importSemaphore(device, &info);
    }
};
struct Semaphore {
    Device &d;
    VkSemaphore semaphore = VK_NULL_HANDLE;
    Semaphore(Device &device, uint64_t initial = 0,
              VkExternalSemaphoreHandleTypeFlagBits type = VK_EXTERNAL_SEMAPHORE_HANDLE_TYPE_D3D12_FENCE_BIT) : d(device) {
        VkExportSemaphoreCreateInfo export_info = {};
        export_info.sType = VK_STRUCTURE_TYPE_EXPORT_SEMAPHORE_CREATE_INFO;
        export_info.handleTypes = type;
        VkSemaphoreTypeCreateInfo timeline = {};
        timeline.sType = VK_STRUCTURE_TYPE_SEMAPHORE_TYPE_CREATE_INFO;
        timeline.pNext = type ? &export_info : nullptr;
        timeline.semaphoreType = VK_SEMAPHORE_TYPE_TIMELINE;
        timeline.initialValue = initial;
        VkSemaphoreCreateInfo info = {};
        info.sType = VK_STRUCTURE_TYPE_SEMAPHORE_CREATE_INFO;
        info.pNext = &timeline;
        check(d.createSemaphore(d.device, &info, nullptr, &semaphore), "create shared semaphore");
    }
    ~Semaphore() { if (semaphore) d.destroySemaphore(d.device, semaphore, nullptr); }
    HANDLE share(VkExternalSemaphoreHandleTypeFlagBits type = VK_EXTERNAL_SEMAPHORE_HANDLE_TYPE_D3D12_FENCE_BIT) {
        VkSemaphoreGetWin32HandleInfoKHR info = {};
        info.sType = VK_STRUCTURE_TYPE_SEMAPHORE_GET_WIN32_HANDLE_INFO_KHR;
        info.semaphore = semaphore;
        info.handleType = type;
        HANDLE result = nullptr;
        check(d.exportSemaphore(d.device, &info, &result), "export shared semaphore");
        need(result != nullptr, "export valid handle");
        return result;
    }
};
struct Waiter {
    Device &d;
    VkSemaphore semaphore;
    uint64_t value;
    Handle started, thread;
    VkResult result = VK_NOT_READY;
    explicit Waiter(Device &device, VkSemaphore sem, uint64_t target) : d(device), semaphore(sem), value(target) {
        started.h = CreateEventW(nullptr, FALSE, FALSE, nullptr);
        thread.h = CreateThread(nullptr, 0, run, this, 0, nullptr);
        need(started.h && thread.h, "host waiter thread");
        need(WaitForSingleObject(started.h, 1000) == WAIT_OBJECT_0, "waiter start timeout");
        Sleep(30); // Let the independent host-wait thread enter the runtime.
    }
    static DWORD WINAPI run(void *pointer) {
        auto &self = *static_cast<Waiter *>(pointer);
        SetEvent(self.started.h);
        self.result = self.d.wait(self.semaphore, self.value, UINT64_MAX);
        return 0;
    }
    VkResult finish() {
        if (WaitForSingleObject(thread.h, 3000) != WAIT_OBJECT_0) {
            std::fprintf(stderr, "FAIL unbounded host wait after peer error\n");
            TerminateProcess(GetCurrentProcess(), 3);
        }
        return result;
    }
    ~Waiter() {
        if (thread.h && WaitForSingleObject(thread.h, 0) == WAIT_TIMEOUT) {
            std::fprintf(stderr, "FAIL test left a host waiter running\n");
            TerminateProcess(GetCurrentProcess(), 3);
        }
    }
};
static int values() {
    Device d;
    Semaphore original(d, UINT64_MAX - 1);
    need(d.counter(original.semaphore) == UINT64_MAX - 1, "initial high virtual value");
    check(d.wait(original.semaphore, UINT64_MAX - 1, 0), "already satisfied timeout-zero wait");
    check(d.signal(original.semaphore, 5), "rewind from high initial value");
    need(d.counter(original.semaphore) == 5, "rewind counter");
    check(d.wait(original.semaphore, 5, 0), "satisfied timeout-zero wait");
    need(d.wait(original.semaphore, 6, 0) == VK_TIMEOUT, "unsatisfied zero timeout");
    const ULONGLONG started = GetTickCount64();
    need(d.wait(original.semaphore, 6, 5000000) == VK_TIMEOUT, "unsatisfied finite timeout");
    need(GetTickCount64() - started < 200, "finite wait keeps total deadline");
    Handle first;
    first.h = original.share();
    Semaphore imported(d, 0, static_cast<VkExternalSemaphoreHandleTypeFlagBits>(0));
    check(d.import(imported.semaphore, first.h), "first import");
    need(d.counter(imported.semaphore) == 5, "import virtual value");
    check(d.signal(imported.semaphore, 12), "import signal");
    need(d.counter(original.semaphore) == 12, "shared virtual signal");
    Semaphore other(d, 97);
    Handle second;
    second.h = other.share();
    check(d.import(imported.semaphore, second.h), "reimport");
    need(d.counter(imported.semaphore) == 97, "reimport value");
    need(d.import(imported.semaphore, first.h, VK_EXTERNAL_SEMAPHORE_HANDLE_TYPE_D3D12_FENCE_BIT,
        VK_SEMAPHORE_IMPORT_TEMPORARY_BIT) == VK_ERROR_INVALID_EXTERNAL_HANDLE, "temporary timeline import rejected");
    need(d.counter(imported.semaphore) == 97, "failed reimport preserves previous payload");
    Semaphore native(d, 5, VK_EXTERNAL_SEMAPHORE_HANDLE_TYPE_OPAQUE_WIN32_BIT);
    Handle opaque;
    opaque.h = native.share(VK_EXTERNAL_SEMAPHORE_HANDLE_TYPE_OPAQUE_WIN32_BIT);
    need(d.import(imported.semaphore, opaque.h) == VK_ERROR_INVALID_EXTERNAL_HANDLE,
        "native opaque metadata cannot become virtual D3D12 fence");
    need(d.counter(imported.semaphore) == 97, "metadata rejection preserves previous payload");
    check(d.import(imported.semaphore, opaque.h, VK_EXTERNAL_SEMAPHORE_HANDLE_TYPE_OPAQUE_WIN32_BIT), "ordinary opaque import");
    need(d.counter(imported.semaphore) == 5, "native opaque counter remains supported");
    std::printf("values initial-high rewind timeout0 timeout-finite import/reimport temporary/type0-negative PASS\n");
    return 0;
}
static int batch_peak() {
    Device d;
    for (unsigned iteration = 0; iteration != 64; ++iteration) {
        Semaphore fence(d);
        Waiter waiter(d, fence.semaphore, 20);
        uint64_t values[] = {20, 10, 15};
        VkTimelineSemaphoreSubmitInfo timelines[3] = {};
        VkSubmitInfo submits[3] = {};
        for (unsigned i = 0; i != 3; ++i) {
            timelines[i].sType = VK_STRUCTURE_TYPE_TIMELINE_SEMAPHORE_SUBMIT_INFO;
            timelines[i].signalSemaphoreValueCount = 1;
            timelines[i].pSignalSemaphoreValues = &values[i];
            submits[i].sType = VK_STRUCTURE_TYPE_SUBMIT_INFO;
            submits[i].pNext = &timelines[i];
            submits[i].signalSemaphoreCount = 1;
            submits[i].pSignalSemaphores = &fence.semaphore;
        }
        check(d.queueSubmit(d.queue, 3, submits, VK_NULL_HANDLE), "submit peak/rewind batch");
        check(waiter.finish(), "wait observes batch peak 20");
        check(d.queueWaitIdle(d.queue), "batch queue idle");
        const ULONGLONG deadline = GetTickCount64() + 1000;
        while (d.counter(fence.semaphore) != 15 && GetTickCount64() < deadline) Sleep(1);
        need(d.counter(fence.semaphore) == 15, "batch ends at virtual value15");
    }
    std::printf("batch-peak 64x GPU20->10->15 priorWait20 PASS\n");
    return 0;
}
static int reset_history() {
    Device d;
    Semaphore fence(d);
    Waiter waiter(d, fence.semaphore, 1000);
    VkResult signal_result = VK_SUCCESS;
    for (unsigned i = 0; i != 24 && signal_result == VK_SUCCESS; ++i) {
        signal_result = d.signal(fence.semaphore, 1);
        if (signal_result == VK_SUCCESS) signal_result = d.signal(fence.semaphore, 0);
    }
    need(waiter.finish() == VK_ERROR_DEVICE_LOST, "lost reset history must fail, never satisfy unreached1000");
    uint64_t value = 0;
    need(d.getCounter(d.device, fence.semaphore, &value) == VK_ERROR_DEVICE_LOST, "broken context counter must fail");
    Semaphore control(d);
    check(d.signal(control.semaphore, 7), "unrelated fence still signals");
    need(d.counter(control.semaphore) == 7, "unrelated fence still usable");
    std::printf("reset-history >16rewinds unreachedWait1000=DEVICE_LOST control7 PASS\n");
    return 0;
}
static int old_server_reject() {
    Device d;
    VkExportSemaphoreCreateInfo external = {};
    external.sType = VK_STRUCTURE_TYPE_EXPORT_SEMAPHORE_CREATE_INFO;
    external.handleTypes = VK_EXTERNAL_SEMAPHORE_HANDLE_TYPE_D3D12_FENCE_BIT;
    VkSemaphoreTypeCreateInfo timeline = {};
    timeline.sType = VK_STRUCTURE_TYPE_SEMAPHORE_TYPE_CREATE_INFO;
    timeline.pNext = &external;
    timeline.semaphoreType = VK_SEMAPHORE_TYPE_TIMELINE;
    VkSemaphoreCreateInfo info = {};
    info.sType = VK_STRUCTURE_TYPE_SEMAPHORE_CREATE_INFO;
    info.pNext = &timeline;
    VkSemaphore semaphore = VK_NULL_HANDLE;
    const VkResult result = d.createSemaphore(d.device, &info, nullptr, &semaphore);
    if (result == VK_SUCCESS) d.destroySemaphore(d.device, semaphore, nullptr);
    need(result < 0, "new virtual context must not be exported through unaware old server");
    Semaphore ordinary(d, 5, VK_EXTERNAL_SEMAPHORE_HANDLE_TYPE_OPAQUE_WIN32_BIT);
    need(d.counter(ordinary.semaphore) == 5, "ordinary opaque semaphore remains supported by old server");
    std::printf("old-server-reject virtualCreate=%d ordinaryOpaque5 PASS\n", result);
    return 0;
}
static HANDLE number(const char *text) {
    return reinterpret_cast<HANDLE>(static_cast<uintptr_t>(std::strtoull(text, nullptr, 10)));
}
static int crash_creator(HANDLE pipe, HANDLE release) {
    Device d;
    Semaphore fence(d, 5);
    Handle shared;
    shared.h = fence.share();
    uintptr_t data = reinterpret_cast<uintptr_t>(shared.h);
    DWORD written = 0;
    need(WriteFile(pipe, &data, sizeof(data), &written, nullptr) && written == sizeof(data), "write crash export");
    need(WaitForSingleObject(release, 5000) == WAIT_OBJECT_0, "crash creator release timeout");
    // The test-only hook exits this process immediately after acquiring its robust context mutex.
    check(d.signal(fence.semaphore, 9), "crash creator signal");
    Sleep(2000);
    throw std::runtime_error("owner-death test hook did not trigger");
}
static int owner_death(const char *arm_path) {
    DeleteFileA(arm_path);
    Device d;
    SECURITY_ATTRIBUTES attributes = {sizeof(attributes), nullptr, TRUE};
    Handle read, write, release;
    need(CreatePipe(&read.h, &write.h, &attributes, 0), "owner death pipe");
    release.h = CreateEventW(&attributes, FALSE, FALSE, nullptr);
    need(release.h != nullptr, "owner death release event");
    char executable[32768];
    need(GetModuleFileNameA(nullptr, executable, sizeof(executable)), "executable path");
    std::string command = std::string("\"") + executable + "\" --fence-crash-creator "
        + std::to_string(reinterpret_cast<uintptr_t>(write.h)) + " "
        + std::to_string(reinterpret_cast<uintptr_t>(release.h));
    STARTUPINFOA startup = {};
    startup.cb = sizeof(startup);
    PROCESS_INFORMATION process = {};
    need(CreateProcessA(executable, command.data(), nullptr, nullptr, TRUE, 0,
        nullptr, nullptr, &startup, &process), "create crash owner");
    Handle child, child_thread;
    child.h = process.hProcess;
    child_thread.h = process.hThread;
    uintptr_t data = 0;
    DWORD available = 0, received = 0;
    const ULONGLONG deadline = GetTickCount64() + 5000;
    while (available < sizeof(data) && GetTickCount64() < deadline) {
        need(PeekNamedPipe(read.h, nullptr, 0, nullptr, &available, nullptr), "peek crash export");
        if (available < sizeof(data)) Sleep(1);
    }
    need(available >= sizeof(data), "crash creator export timeout");
    need(ReadFile(read.h, &data, sizeof(data), &received, nullptr) && received == sizeof(data), "read crash export");
    Handle shared;
    need(DuplicateHandle(child.h, reinterpret_cast<HANDLE>(data), GetCurrentProcess(), &shared.h,
        0, FALSE, DUPLICATE_SAME_ACCESS), "duplicate crash export");
    Semaphore imported(d, 0, static_cast<VkExternalSemaphoreHandleTypeFlagBits>(0));
    check(d.import(imported.semaphore, shared.h), "import crash owner context");
    need(d.counter(imported.semaphore) == 5, "value before owner death");
    Waiter waiter(d, imported.semaphore, 1000);
    Handle armed;
    armed.h = CreateFileA(arm_path, GENERIC_WRITE, FILE_SHARE_READ | FILE_SHARE_WRITE,
        nullptr, CREATE_ALWAYS, FILE_ATTRIBUTE_NORMAL, nullptr);
    need(armed.h != INVALID_HANDLE_VALUE, "arm owner death hook");
    const ULONGLONG started = GetTickCount64();
    SetEvent(release.h);
    need(WaitForSingleObject(child.h, 3000) == WAIT_OBJECT_0, "crash owner did not exit");
    DWORD exit_code = 0;
    need(GetExitCodeProcess(child.h, &exit_code), "read crash creator exit code");
    std::printf("owner-death creator Windows exit=%lu (Linux hook exits86)\n", static_cast<unsigned long>(exit_code));
    need(waiter.finish() == VK_ERROR_DEVICE_LOST, "infinite host wait must fail after mutex owner death");
    uint64_t value = 0;
    need(d.getCounter(d.device, imported.semaphore, &value) == VK_ERROR_DEVICE_LOST, "dead owner counter must fail");
    Semaphore control(d);
    check(d.signal(control.semaphore, 7), "unrelated fence after owner death");
    need(d.counter(control.semaphore) == 7, "unrelated fence value after owner death");
    CloseHandle(armed.h);
    armed.h = nullptr;
    DeleteFileA(arm_path);
    std::printf("owner-death robustMutexExit infiniteWait=DEVICE_LOST elapsed=%llums control7 PASS\n",
        static_cast<unsigned long long>(GetTickCount64() - started));
    return 0;
}
int main(int argc, char **argv) {
    std::setvbuf(stdout, nullptr, _IONBF, 0);
    try {
        if (argc == 4 && std::string(argv[1]) == "--fence-crash-creator") return crash_creator(number(argv[2]), number(argv[3]));
        if (argc == 3 && std::string(argv[1]) == "owner-death") return owner_death(argv[2]);
        need(argc == 2, "usage: vulkan-shared-fence-context.exe values|batch-peak|reset-history|owner-death <arm-file>");
        const std::string mode = argv[1];
        if (mode == "values") return values();
        if (mode == "batch-peak") return batch_peak();
        if (mode == "reset-history") return reset_history();
        if (mode == "old-server-reject") return old_server_reject();
        throw std::runtime_error("unknown test mode");
    } catch (const std::exception &error) {
        std::fprintf(stderr, "FAIL %s\n", error.what());
        return 2;
    }
}
