// memhist: who copies memory and who allocates it, and how much, by the
// function that asked. For when a profile says "a quarter of this frame
// is in libc" and nothing says whose: perf's call graphs are refused to
// an unprivileged user here, and a flat profile ends at memmove and
// malloc. tools/dev/memhist builds this, runs a program with it
// (LD_PRELOAD) and names what it finds.
//
// It stands in for memcpy, memmove, malloc and realloc, and counts each
// call's bytes against the address it returns to: for a copy, the
// function that made it; for an allocation, the function that asked for
// it, since Rust's allocator reaches malloc by tail calls. SIGUSR1 starts
// and stops the count; SIGUSR2 writes it to $MEMHIST_OUT, a line a site:
// "bytes calls module offset" for copies, the same after an "A" for
// allocations. The copies themselves are plain `rep movsb`: slower than
// glibc's, so time nothing under it.
#define _GNU_SOURCE
#include <dlfcn.h>
#include <signal.h>
#include <stdatomic.h>
#include <stddef.h>
#include <stdint.h>
#include <stdio.h>
#include <stdlib.h>
#include <string.h>

#define N 65536
static struct { _Atomic uintptr_t addr; _Atomic uint64_t bytes, calls; } tab[N];
static _Atomic int on = 0;

static inline void note(void *ra, size_t n) {
    if (!atomic_load_explicit(&on, memory_order_relaxed)) return;
    uintptr_t a = (uintptr_t)ra;
    size_t i = (size_t)((a * 11400714819323198485ull) >> 48);
    for (int k = 0; k < 32; k++, i = (i + 1) & (N - 1)) {
        uintptr_t cur = atomic_load_explicit(&tab[i].addr, memory_order_relaxed);
        if (cur == 0) {
            uintptr_t expect = 0;
            if (atomic_compare_exchange_strong(&tab[i].addr, &expect, a)) cur = a; else cur = expect;
        }
        if (cur == a) {
            atomic_fetch_add_explicit(&tab[i].bytes, n, memory_order_relaxed);
            atomic_fetch_add_explicit(&tab[i].calls, 1, memory_order_relaxed);
            return;
        }
    }
}

static inline void *fwd(void *d, const void *s, size_t n) {
    void *r = d;
    __asm__ volatile("rep movsb" : "+D"(d), "+S"(s), "+c"(n) : : "memory");
    return r;
}

void *memcpy(void *d, const void *s, size_t n) {
    note(__builtin_return_address(0), n);
    return fwd(d, s, n);
}

void *memmove(void *d, const void *s, size_t n) {
    note(__builtin_return_address(0), n);
    if ((uintptr_t)d - (uintptr_t)s >= n) return fwd(d, s, n);
    void *r = d;
    unsigned char *dd = (unsigned char *)d + n - 1;
    const unsigned char *ss = (const unsigned char *)s + n - 1;
    __asm__ volatile("std; rep movsb; cld" : "+D"(dd), "+S"(ss), "+c"(n) : : "memory");
    return r;
}

// Allocations too, by who asked (Rust's allocator reaches malloc by
// tail calls, so the address is the asking function's): in a table of
// their own, written after the copies with an "A" before each.
static struct { _Atomic uintptr_t addr; _Atomic uint64_t bytes, calls; } atab[N];
static inline void anote(void *ra, size_t n) {
    if (!atomic_load_explicit(&on, memory_order_relaxed)) return;
    uintptr_t a = (uintptr_t)ra;
    size_t i = (size_t)((a * 11400714819323198485ull) >> 48);
    for (int k = 0; k < 32; k++, i = (i + 1) & (N - 1)) {
        uintptr_t cur = atomic_load_explicit(&atab[i].addr, memory_order_relaxed);
        if (cur == 0) {
            uintptr_t expect = 0;
            if (atomic_compare_exchange_strong(&atab[i].addr, &expect, a)) cur = a; else cur = expect;
        }
        if (cur == a) {
            atomic_fetch_add_explicit(&atab[i].bytes, n, memory_order_relaxed);
            atomic_fetch_add_explicit(&atab[i].calls, 1, memory_order_relaxed);
            return;
        }
    }
}
extern void *__libc_malloc(size_t);
extern void *__libc_realloc(void *, size_t);
void *malloc(size_t n) { anote(__builtin_return_address(0), n); return __libc_malloc(n); }
void *realloc(void *p, size_t n) { anote(__builtin_return_address(0), n); return __libc_realloc(p, n); }

static void toggle(int sig) { (void)sig; atomic_fetch_xor(&on, 1); }

static void dump(int sig) {
    (void)sig;
    const char *path = getenv("MEMHIST_OUT");
    FILE *f = fopen(path ? path : "/var/tmp/memhist.txt", "w");
    if (!f) return;
    for (size_t i = 0; i < N; i++) {
        uintptr_t a = tab[i].addr;
        if (!a) continue;
        Dl_info info;
        if (dladdr((void *)a, &info) && info.dli_fname)
            fprintf(f, "%llu %llu %s 0x%lx\n", (unsigned long long)tab[i].bytes, (unsigned long long)tab[i].calls, info.dli_fname, (unsigned long)(a - (uintptr_t)info.dli_fbase));
        else
            fprintf(f, "%llu %llu ? 0x%lx\n", (unsigned long long)tab[i].bytes, (unsigned long long)tab[i].calls, (unsigned long)a);
    }
    for (size_t i = 0; i < N; i++) {
        uintptr_t a = atab[i].addr;
        if (!a) continue;
        Dl_info info;
        if (dladdr((void *)a, &info) && info.dli_fname)
            fprintf(f, "A %llu %llu %s 0x%lx\n", (unsigned long long)atab[i].bytes, (unsigned long long)atab[i].calls, info.dli_fname, (unsigned long)(a - (uintptr_t)info.dli_fbase));
    }
    fclose(f);
}

__attribute__((constructor)) static void init(void) {
    signal(SIGUSR1, toggle);
    signal(SIGUSR2, dump);
}
