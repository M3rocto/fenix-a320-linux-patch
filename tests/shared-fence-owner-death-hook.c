/* SPDX-License-Identifier: MIT
 * Test-only fault injection. Never install this in a runner or user prefix.
 * LD_PRELOAD this library only for the dedicated vulkan-shared-fence-context
 * owner-death regression. It acts solely in --fence-crash-creator processes,
 * after their own robust process-shared mutex is acquired and an explicit
 * FENIX_TEST_FENCE_CRASH_ARM file has been created by the test controller.
 * Build: cc -shared -fPIC -O2 -Wall -Wextra -Werror -o hook.so this.c -ldl -pthread
 */
#define _GNU_SOURCE
#include <dlfcn.h>
#include <fcntl.h>
#include <pthread.h>
#include <stdatomic.h>
#include <stdlib.h>
#include <string.h>
#include <unistd.h>

static int (*real_init)(pthread_mutex_t *, const pthread_mutexattr_t *);
static int (*real_lock)(pthread_mutex_t *);
static _Atomic(pthread_mutex_t *) tracked[64];
static _Atomic unsigned tracked_count;
static int creator;
static char arm_file[4096];

__attribute__((constructor)) static void setup(void)
{
    char command[16384];
    const char *path;
    ssize_t size;
    int fd;
    void *symbol = dlsym(RTLD_NEXT, "pthread_mutex_init");
    memcpy(&real_init, &symbol, sizeof(real_init));
    symbol = dlsym(RTLD_NEXT, "pthread_mutex_lock");
    memcpy(&real_lock, &symbol, sizeof(real_lock));
    if ((fd = open("/proc/self/cmdline", O_RDONLY | O_CLOEXEC)) < 0) return;
    size = read(fd, command, sizeof(command));
    close(fd);
    creator = size > 0 && memmem(command, (size_t)size,
        "--fence-crash-creator", sizeof("--fence-crash-creator") - 1) != NULL;
    path = getenv("FENIX_TEST_FENCE_CRASH_ARM");
    if (!path || path[0] != '/' || strlen(path) >= sizeof(arm_file)) creator = 0;
    if (creator) strcpy(arm_file, path);
}

int pthread_mutex_init(pthread_mutex_t *mutex, const pthread_mutexattr_t *attr)
{
    int shared = 0, robust = 0, result;
    if (!real_init) _exit(87);
    result = real_init(mutex, attr);
    if (!result && creator && attr && !pthread_mutexattr_getpshared(attr, &shared) &&
        !pthread_mutexattr_getrobust(attr, &robust) && shared == PTHREAD_PROCESS_SHARED && robust == PTHREAD_MUTEX_ROBUST)
    {
        unsigned index = atomic_fetch_add_explicit(&tracked_count, 1, memory_order_relaxed);
        if (index < sizeof(tracked) / sizeof(*tracked))
            atomic_store_explicit(&tracked[index], mutex, memory_order_release);
    }
    return result;
}

int pthread_mutex_lock(pthread_mutex_t *mutex)
{
    static const char message[] = "FENIX_TEST_OWNER_DEATH acquired robust process-shared mutex; exact exit86\n";
    unsigned count, i;
    int result;
    if (!real_lock) _exit(87);
    result = real_lock(mutex);
    if (!result && creator && !access(arm_file, F_OK))
    {
        count = atomic_load_explicit(&tracked_count, memory_order_acquire);
        if (count > sizeof(tracked) / sizeof(*tracked)) count = sizeof(tracked) / sizeof(*tracked);
        for (i = 0; i < count; ++i)
            if (atomic_load_explicit(&tracked[i], memory_order_acquire) == mutex)
            {
                (void)write(STDERR_FILENO, message, sizeof(message) - 1);
                _exit(86); /* Deliberately leave this mutex owned, exercising EOWNERDEAD. */
            }
    }
    return result;
}
