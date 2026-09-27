// libSystem functions Rust std and LLVM reach for that Mac OS X 10.6 lacks or treats
// differently. Linked into the executable, so its own references bind here first under
// the two-level namespace; libraries the system loads keep the real ones.
#include <dirent.h>
#include <dlfcn.h>
#include <errno.h>
#include <fcntl.h>
#include <limits.h>
#include <math.h>
#include <mach/mach_time.h>
#include <stdarg.h>
#include <stdint.h>
#include <string.h>
#include <sys/stat.h>
#include <sys/time.h>
#include <time.h>
#include <unistd.h>

#define SNOW_O_CLOEXEC 0x01000000   // 10.7
#define SNOW_F_DUPFD_CLOEXEC 67     // 10.7
#define SNOW_AT_FDCWD -2
#define SNOW_AT_SYMLINK_NOFOLLOW 0x0020
#define SNOW_AT_REMOVEDIR 0x0080

// The real function this file overrides, from the next image in load order.
#define REAL(name) \
    static __typeof__(name) *real_##name; \
    if (!real_##name) real_##name = (__typeof__(name) *)dlsym(RTLD_NEXT, #name)

// 10.6's kernel predates O_CLOEXEC; set the flag after the fact.
int open(const char *path, int flags, ...) {
    REAL(open);
    va_list ap;
    va_start(ap, flags);
    int mode = (flags & O_CREAT) ? va_arg(ap, int) : 0;
    va_end(ap);
    int fd = real_open(path, flags & ~SNOW_O_CLOEXEC, mode);
    if (fd >= 0 && (flags & SNOW_O_CLOEXEC)) fcntl(fd, F_SETFD, FD_CLOEXEC);
    return fd;
}

int fcntl(int fd, int cmd, ...) {
    REAL(fcntl);
    va_list ap;
    va_start(ap, cmd);
    intptr_t arg = va_arg(ap, intptr_t);
    va_end(ap);
    if (cmd == SNOW_F_DUPFD_CLOEXEC) {
        int copy = real_fcntl(fd, F_DUPFD, arg);
        if (copy >= 0) real_fcntl(copy, F_SETFD, FD_CLOEXEC);
        return copy;
    }
    return real_fcntl(fd, cmd, arg);
}

// The *at family arrived in 10.10. Emulated through the directory's path, so a
// directory renamed mid-walk is not followed as the real calls would.
static int resolve_at(int dirfd, const char *path, char out[PATH_MAX]) {
    if (dirfd == SNOW_AT_FDCWD || path[0] == '/') {
        if (strlcpy(out, path, PATH_MAX) >= PATH_MAX) return errno = ENAMETOOLONG, -1;
        return 0;
    }
    if (fcntl(dirfd, F_GETPATH, out) == -1) return -1;
    if (strlcat(out, "/", PATH_MAX) >= PATH_MAX || strlcat(out, path, PATH_MAX) >= PATH_MAX)
        return errno = ENAMETOOLONG, -1;
    return 0;
}

int openat(int dirfd, const char *path, int flags, ...) {
    va_list ap;
    va_start(ap, flags);
    int mode = (flags & O_CREAT) ? va_arg(ap, int) : 0;
    va_end(ap);
    char full[PATH_MAX];
    if (resolve_at(dirfd, path, full) == -1) return -1;
    return open(full, flags, mode);
}

int unlinkat(int dirfd, const char *path, int flags) {
    char full[PATH_MAX];
    if (resolve_at(dirfd, path, full) == -1) return -1;
    return (flags & SNOW_AT_REMOVEDIR) ? rmdir(full) : unlink(full);
}

int fstatat(int dirfd, const char *path, struct stat *st, int flags)
    __asm("_fstatat" __DARWIN_SUF_64_BIT_INO_T);
int fstatat(int dirfd, const char *path, struct stat *st, int flags) {
    char full[PATH_MAX];
    if (resolve_at(dirfd, path, full) == -1) return -1;
    return (flags & SNOW_AT_SYMLINK_NOFOLLOW) ? lstat(full, st) : stat(full, st);
}

int linkat(int fromfd, const char *from, int tofd, const char *to, int flags) {
    char source[PATH_MAX], target[PATH_MAX];
    (void)flags;  // std passes 0
    if (resolve_at(fromfd, from, source) == -1 || resolve_at(tofd, to, target) == -1) return -1;
    return link(source, target);
}

// Clones are APFS-only (10.12); std's fs::copy falls back to fcopyfile on ENOTSUP.
int fclonefileat(int fd, int dirfd, const char *to, int flags) {
    (void)fd, (void)dirfd, (void)to, (void)flags;
    return errno = ENOTSUP, -1;
}

// Callers keep using `fd` (std's remove_dir_all passes it to openat/unlinkat), so the
// stream takes over that number rather than closing it.
DIR *fdopendir(int fd) __asm("_fdopendir" __DARWIN_SUF_64_BIT_INO_T __DARWIN_SUF_UNIX03);
DIR *fdopendir(int fd) {
    char path[PATH_MAX];
    if (fcntl(fd, F_GETPATH, path) == -1) return NULL;
    DIR *dir = opendir(path);
    if (!dir) return NULL;
    if (dup2(dir->__dd_fd, fd) == -1) {
        closedir(dir);
        return NULL;
    }
    close(dir->__dd_fd);
    dir->__dd_fd = fd;
    fcntl(fd, F_SETFD, FD_CLOEXEC);
    return dir;
}

// A function only from 10.8; earlier headers define it as this macro.
#undef dirfd
int dirfd(DIR *dir) {
    return dir->__dd_fd;
}

// clock_gettime arrived in 10.12. std asks for CLOCK_UPTIME_RAW (Instant) and
// CLOCK_REALTIME (SystemTime).
int clock_gettime(int clock, struct timespec *ts) {
    switch (clock) {
    case 0: {  // CLOCK_REALTIME
        struct timeval tv;
        gettimeofday(&tv, NULL);
        ts->tv_sec = tv.tv_sec;
        ts->tv_nsec = tv.tv_usec * 1000;
        return 0;
    }
    case 4: case 5: case 6: case 8: case 9: {  // monotonic and uptime clocks
        static mach_timebase_info_data_t base;
        if (!base.denom) mach_timebase_info(&base);
        uint64_t ns = mach_absolute_time() * base.numer / base.denom;
        ts->tv_sec = ns / 1000000000;
        ts->tv_nsec = ns % 1000000000;
        return 0;
    }
    default:
        errno = EINVAL;
        return -1;
    }
}

static int read_urandom(void *bytes, size_t count) {
    static int urandom = -1;
    if (urandom < 0) {
        int fd = open("/dev/urandom", O_RDONLY | SNOW_O_CLOEXEC);
        if (fd < 0) return -1;
        if (!__sync_bool_compare_and_swap(&urandom, -1, fd)) close(fd);
    }
    for (uint8_t *p = bytes; count;) {
        ssize_t n = read(urandom, p, count);
        if (n < 0 && errno == EINTR) continue;
        if (n <= 0) return -1;
        p += n;
        count -= (size_t)n;
    }
    return 0;
}

// CCRandomGenerateBytes (std) arrived in 10.10, getentropy (getrandom) in 10.12.
int32_t CCRandomGenerateBytes(void *bytes, size_t count) {
    return read_urandom(bytes, count);
}

int getentropy(void *bytes, size_t count) {
    if (count > 256) return errno = EIO, -1;
    return read_urandom(bytes, count);
}

// LLVM emits these for macOS 10.9+, and rustc hands LLVM at least 10.12.
struct snow_double2 { double sinval, cosval; };
struct snow_float2 { float sinval, cosval; };
struct snow_double2 __sincos_stret(double x) { return (struct snow_double2){sin(x), cos(x)}; }
struct snow_float2 __sincosf_stret(float x) { return (struct snow_float2){sinf(x), cosf(x)}; }
double __exp10(double x) { return pow(10.0, x); }
float __exp10f(float x) { return powf(10.0f, x); }
