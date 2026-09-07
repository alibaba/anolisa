// SPDX-License-Identifier: (LGPL-2.1 OR BSD-2-Clause)
// Copyright (c) 2023 Yusheng Zheng
//
// Based on sslsniff from BCC by Adrian Lopez & Mark Drayton.
// 15-Aug-2023   Yusheng Zheng   Created this.
#include "vmlinux.h"
#include <bpf/bpf_core_read.h>
#include <bpf/bpf_endian.h>
#include <bpf/bpf_helpers.h>
#include <bpf/bpf_tracing.h>
#include "sslsniff.h"
#include "common.h"  

struct {
    __uint(type, BPF_MAP_TYPE_HASH);
    __uint(max_entries, 1024);
    __type(key, u32);
    __type(value, size_t*);
} readbytes_ptrs SEC(".maps");

#define MAX_ENTRIES 1024

struct {
    __uint(type, BPF_MAP_TYPE_HASH);
    __uint(max_entries, MAX_ENTRIES);
    __type(key, u32);
    __type(value, u64);
} ssl_ptrs SEC(".maps");

#define min(x, y)                      \
    ({                                 \
        typeof(x) _min1 = (x);         \
        typeof(y) _min2 = (y);         \
        (void)(&_min1 == &_min2);      \
        _min1 < _min2 ? _min1 : _min2; \
    })

/* ssl_data per-CPU array removed - ring buffer allocates memory directly */

struct {
    __uint(type, BPF_MAP_TYPE_HASH);
    __uint(max_entries, MAX_ENTRIES);
    __type(key, u32);
    __type(value, u64);
} start_ns SEC(".maps");

struct {
    __uint(type, BPF_MAP_TYPE_HASH);
    __uint(max_entries, MAX_ENTRIES);
    __type(key, u32);
    __type(value, u64);
} bufs SEC(".maps");


static __always_inline u32 trace_allowed(u32 uid, u32 pid)
{
    return is_pid_traced(pid);
}

SEC("uprobe/do_handshake")
int BPF_UPROBE(probe_SSL_rw_enter, void *ssl, void *buf, int num) {
    u64 pid_tgid = bpf_get_current_pid_tgid();
    u32 pid = pid_tgid >> 32;
    u32 tid = pid_tgid;
    u32 uid = bpf_get_current_uid_gid();
    u64 ts = bpf_ktime_get_ns();

    u32 ns_pid = trace_allowed(uid, pid);
    if (!ns_pid) {
        return 0;
    }

    /* store arg info for later lookup */
    u64 ssl_ptr_val = (u64)ssl;
    bpf_map_update_elem(&ssl_ptrs, &tid, &ssl_ptr_val, BPF_ANY);
    bpf_map_update_elem(&bufs, &tid, &buf, BPF_ANY);
    bpf_map_update_elem(&start_ns, &tid, &ts, BPF_ANY);
    return 0;
}

/* Emit ONE SSL record using the per-tier record TYPE (e.g. probe_SSL_data_small)
 * whose buf[] is exactly TIER bytes. The reservation is sizeof(struct TYPE) — a
 * true compile-time constant — and the payload is clamped to TIER and copied
 * WITHIN that type's buf: the only shape the verifier accepts. (Reserving fewer
 * bytes than sizeof(struct) and then writing into a LARGER typed buf[] is
 * REJECTED with EACCES at load — confirmed on 6.6.) A small SSL call reserves a
 * small type, so it no longer pads the shared ring to the 4 MiB worst case
 * (#759). `truncated` is set only when the payload exceeds the chosen tier
 * (possible only at the top 4 MiB tier). */
#define SSL_EMIT_ONE(TYPE, TIER, src_, len_, rw_, ts_, delta_, pid_, tid_, uid_, ssl_, ishs_) \
    do {                                                                        \
        struct TYPE *_d = bpf_ringbuf_reserve(&rb, sizeof(struct TYPE), 0);     \
        if (!_d)                                                                \
            break;                                                              \
        _d->source = EVENT_SOURCE_SSL;                                          \
        _d->timestamp_ns = (ts_);                                              \
        _d->delta_ns = (delta_);                                               \
        _d->pid = (pid_);                                                       \
        _d->tid = (tid_);                                                       \
        _d->uid = (uid_);                                                       \
        _d->len = (u32)(len_);                                                  \
        _d->rw = (rw_);                                                         \
        _d->is_handshake = (ishs_);                                             \
        _d->ssl_ptr = (ssl_);                                                   \
        _d->truncated = ((u32)(len_) > (u32)(TIER)) ? 1 : 0;                    \
        bpf_get_current_comm(&_d->comm, sizeof(_d->comm));                      \
        /* Re-clamp the copy length to the tier right before the read. Three    \
         * verifier traps must be dodged: (1) a length computed earlier is      \
         * spilled across bpf_get_current_comm() and reloaded as an UNBOUNDED   \
         * scalar; (2) the outer tier-selection already proved len_ <= TIER, so \
         * clang folds a plain `if (_n > TIER)` away as dead code -- yet the     \
         * verifier does NOT carry that bound across the spill, so the clamp     \
         * still has to execute; (3) on kernel 5.15, clang may substitute the   \
         * original signed `len` register for _n at the call site, even after   \
         * the clamp, because it proves they hold the same value — the second   \
         * barrier forces clang to use _n's own (clamped, unsigned) register.   \
         * Without the barriers the load is rejected on 5.15:                   \
         * "R2 min value is negative, either use unsigned or 'var &= const'". */ \
        u32 _n = (u32)(len_);                                                   \
        asm volatile("" : "+r"(_n));                                            \
        if (_n > (u32)(TIER))                                                   \
            _n = (u32)(TIER);                                                   \
        asm volatile("" : "+r"(_n));                                            \
        int _rc = (src_) ? bpf_probe_read_user(&_d->buf, _n, (const char *)(src_)) : -1; \
        if (_rc) { _d->buf_filled = 0; _d->buf_size = 0; }                      \
        else     { _d->buf_filled = 1; _d->buf_size = _n; }                     \
        bpf_ringbuf_submit(_d, 0);                                              \
    } while (0)

/* Pick the smallest tier (and its record type) that holds `len_` and emit one. */
#define SSL_EMIT_TIERED(src_, len_, rw_, ts_, delta_, pid_, tid_, uid_, ssl_, ishs_)       \
    do {                                                                                   \
        u32 _l = (u32)(len_);                                                              \
        if (_l <= SSL_TIER_SMALL)                                                          \
            SSL_EMIT_ONE(probe_SSL_data_small, SSL_TIER_SMALL, src_, len_, rw_, ts_, delta_, pid_, tid_, uid_, ssl_, ishs_);   \
        else if (_l <= SSL_TIER_MEDIUM)                                                    \
            SSL_EMIT_ONE(probe_SSL_data_medium, SSL_TIER_MEDIUM, src_, len_, rw_, ts_, delta_, pid_, tid_, uid_, ssl_, ishs_); \
        else if (_l <= SSL_TIER_LARGE)                                                     \
            SSL_EMIT_ONE(probe_SSL_data_large, SSL_TIER_LARGE, src_, len_, rw_, ts_, delta_, pid_, tid_, uid_, ssl_, ishs_);   \
        else                                                                              \
            SSL_EMIT_ONE(probe_SSL_data_t, MAX_BUF_SIZE, src_, len_, rw_, ts_, delta_, pid_, tid_, uid_, ssl_, ishs_);         \
    } while (0)

static int SSL_exit(struct pt_regs *ctx, int rw) {
    u64 pid_tgid = bpf_get_current_pid_tgid();
    u32 pid = pid_tgid >> 32;
    u32 tid = (u32)pid_tgid;
    u32 uid = bpf_get_current_uid_gid();
    u64 ts = bpf_ktime_get_ns();

    u32 ns_pid = trace_allowed(uid, pid);
    if (!ns_pid) {
        return 0;
    }

    /* fetch the buffer pointer + timing stored at enter */
    u64 *bufp = bpf_map_lookup_elem(&bufs, &tid);
    if (bufp == 0)
        return 0;

    u64 *tsp = bpf_map_lookup_elem(&start_ns, &tid);
    if (!tsp)
        return 0;
    u64 delta_ns = ts - *tsp;

    u64 *ssl_ptrp = bpf_map_lookup_elem(&ssl_ptrs, &tid);
    u64 ssl_ptr = ssl_ptrp ? *ssl_ptrp : 0;

    int len = PT_REGS_RC(ctx);
    const char *src = (const char *)*bufp;

    bpf_map_delete_elem(&bufs, &tid);
    bpf_map_delete_elem(&start_ns, &tid);
    bpf_map_delete_elem(&ssl_ptrs, &tid);

    if (len <= 0)  // no data
        return 0;

    /* Tiered emit: reserve the smallest tier that fits `len` (#759), capture up
     * to 4 MiB whole (#763, no regression vs the prior single 4 MiB reserve). */
    SSL_EMIT_TIERED(src, len, rw, ts, delta_ns, ns_pid, tid, uid, ssl_ptr, 0);
    return 0;
}

SEC("uretprobe/SSL_read")
int BPF_URETPROBE(probe_SSL_read_exit) {
    return (SSL_exit(ctx, 0));
}

SEC("uretprobe/SSL_write")
int BPF_URETPROBE(probe_SSL_write_exit) {
    return (SSL_exit(ctx, 1));
}

SEC("uprobe/SSL_write_ex")
int BPF_UPROBE(probe_SSL_write_ex_enter, void *ssl, void *buf, size_t num, size_t *readbytes) {
    u64 pid_tgid = bpf_get_current_pid_tgid();
    u32 pid = pid_tgid >> 32;
    u32 tid = (u32)pid_tgid;
    u32 uid = bpf_get_current_uid_gid();
    u64 ts = bpf_ktime_get_ns();

    u32 ns_pid = trace_allowed(uid, pid);
    if (!ns_pid) {
        return 0;
    }

    u64 ssl_ptr_val = (u64)ssl;
    bpf_map_update_elem(&ssl_ptrs, &tid, &ssl_ptr_val, BPF_ANY);
    bpf_map_update_elem(&bufs, &tid, &buf, BPF_ANY);
    bpf_map_update_elem(&start_ns, &tid, &ts, BPF_ANY); 
    
    bpf_map_update_elem(&readbytes_ptrs, &tid, &readbytes, BPF_ANY);

    return 0;
}

SEC("uprobe/SSL_read_ex")
int BPF_UPROBE(probe_SSL_read_ex_enter, void *ssl, void *buf, size_t num, size_t *readbytes) {
    u64 pid_tgid = bpf_get_current_pid_tgid();
    u32 pid = pid_tgid >> 32;
    u32 tid = (u32)pid_tgid;
    u32 uid = bpf_get_current_uid_gid();
    u64 ts = bpf_ktime_get_ns();

    u32 ns_pid = trace_allowed(uid, pid);
    if (!ns_pid) {
        return 0;
    }

    u64 ssl_ptr_val = (u64)ssl;
    bpf_map_update_elem(&ssl_ptrs, &tid, &ssl_ptr_val, BPF_ANY);
    bpf_map_update_elem(&bufs, &tid, &buf, BPF_ANY);
    bpf_map_update_elem(&start_ns, &tid, &ts, BPF_ANY); 

    bpf_map_update_elem(&readbytes_ptrs, &tid, &readbytes, BPF_ANY);

    return 0;
}

static int ex_SSL_exit(struct pt_regs *ctx, int rw, int len) {
    u64 pid_tgid = bpf_get_current_pid_tgid();
    u32 pid = pid_tgid >> 32;
    u32 tid = (u32)pid_tgid;
    u32 uid = bpf_get_current_uid_gid();
    u64 ts = bpf_ktime_get_ns();

    u32 ns_pid = trace_allowed(uid, pid);
    if (!ns_pid) {
        return 0;
    }

    /* store arg info for later lookup */
    u64 *bufp = bpf_map_lookup_elem(&bufs, &tid);
    if (bufp == 0)
        return 0;

    u64 *tsp = bpf_map_lookup_elem(&start_ns, &tid);
    if (!tsp)
        return 0;
    u64 delta_ns = ts - *tsp;

    /* lookup ssl pointer for connection tracking */
    u64 *ssl_ptrp = bpf_map_lookup_elem(&ssl_ptrs, &tid);
    u64 ssl_ptr = ssl_ptrp ? *ssl_ptrp : 0;

    const char *src = (const char *)*bufp;

    bpf_map_delete_elem(&bufs, &tid);
    bpf_map_delete_elem(&start_ns, &tid);
    bpf_map_delete_elem(&ssl_ptrs, &tid);

    if (len <= 0)  // no data
        return 0;

    /* Tiered emit (same as SSL_exit). The old `& 0xFFFFF` mask silently capped
     * the _ex path at 1 MiB; the tier clamp restores the full 4 MiB cap,
     * consistent with the non-_ex path. The per-tier branch also narrows the
     * (user-read-derived) len for the verifier. */
    SSL_EMIT_TIERED(src, len, rw, ts, delta_ns, ns_pid, tid, uid, ssl_ptr, 0);
    return 0;
}

SEC("uretprobe/SSL_write_ex")
int BPF_URETPROBE(probe_SSL_write_ex_exit)
{
    u32 tid = (u32)bpf_get_current_pid_tgid();
    size_t **readbytes_ptr = bpf_map_lookup_elem(&readbytes_ptrs, &tid);
    if (!readbytes_ptr)
        return 0;

    size_t written = 0;
    bpf_probe_read_user(&written, sizeof(written), *readbytes_ptr);
    bpf_map_delete_elem(&readbytes_ptrs, &tid);

    int ret = PT_REGS_RC(ctx);
    int len = (ret == 1) ? written : 0;

    return ex_SSL_exit(ctx, 1, len);
}

SEC("uretprobe/SSL_read_ex")
int BPF_URETPROBE(probe_SSL_read_ex_exit)
{
    u32 tid = (u32)bpf_get_current_pid_tgid();
    size_t **readbytes_ptr = bpf_map_lookup_elem(&readbytes_ptrs, &tid);
    if (!readbytes_ptr)
        return 0;

    size_t written = 0;
    bpf_probe_read_user(&written, sizeof(written), *readbytes_ptr);
    bpf_map_delete_elem(&readbytes_ptrs, &tid);

    int ret = PT_REGS_RC(ctx);
    int len = (ret == 1) ? written : 0;

    return ex_SSL_exit(ctx, 0, len);
}

SEC("uprobe/do_handshake")
int BPF_UPROBE(probe_SSL_do_handshake_enter, void *ssl) {
    u64 pid_tgid = bpf_get_current_pid_tgid();
    u32 pid = pid_tgid >> 32;
    u32 tid = (u32)pid_tgid;
    u64 ts = bpf_ktime_get_ns();
    u32 uid = bpf_get_current_uid_gid();

    u32 ns_pid = trace_allowed(uid, pid);
    if (!ns_pid) {
        return 0;
    }

    /* store arg info for later lookup */
    u64 ssl_ptr_val = (u64)ssl;
    bpf_map_update_elem(&ssl_ptrs, &tid, &ssl_ptr_val, BPF_ANY);
    bpf_map_update_elem(&start_ns, &tid, &ts, BPF_ANY);
    return 0;
}

SEC("uretprobe/do_handshake")
int BPF_URETPROBE(probe_SSL_do_handshake_exit) {
    u64 pid_tgid = bpf_get_current_pid_tgid();
    u32 pid = pid_tgid >> 32;
    u32 tid = (u32)pid_tgid;
    u32 uid = bpf_get_current_uid_gid();
    u64 ts = bpf_ktime_get_ns();
    int ret = 0;

    /* use kernel terminology here for tgid/pid: */
    u32 tgid = pid_tgid >> 32;

    u32 ns_pid = trace_allowed(tgid, pid);
    if (!ns_pid) {
        return 0;
    }

    u64 *tsp = bpf_map_lookup_elem(&start_ns, &tid);
    if (tsp == 0)
        return 0;

    ret = PT_REGS_RC(ctx);
    if (ret <= 0)  // handshake failed
        return 0;

    /* Handshake records carry no payload: reserve ONLY the header (no buf), so a
     * handshake costs ~one header in the ring instead of the prior 4 MiB reserve. */
    struct probe_SSL_data_t *data =
        bpf_ringbuf_reserve(&rb, __builtin_offsetof(struct probe_SSL_data_t, buf), 0);
    if (!data)
        return 0;

    data->source = EVENT_SOURCE_SSL;
    data->timestamp_ns = ts;
    data->delta_ns = ts - *tsp;
    data->pid = ns_pid;
    data->tid = tid;
    data->uid = uid;
    data->len = ret;
    data->buf_filled = 0;
    data->buf_size = 0;
    data->rw = 2;
    data->is_handshake = 1;
    data->truncated = 0;
    data->ssl_ptr = 0;
    bpf_get_current_comm(&data->comm, sizeof(data->comm));
    bpf_map_delete_elem(&start_ns, &tid);

    /* submit to ring buffer */
    bpf_ringbuf_submit(data, 0);
    return 0;
}

/* ─── rustls plaintext taps (#3042) ──────────────────────────────────────────
 *
 * rustls is pure Rust and exports no SSL_read/SSL_write, so every probe above
 * is unattachable on a rustls process. The hooks here are its two plaintext
 * chokepoints on `CommonState`, taken at function entry:
 *
 *   buffer_plaintext(&mut self, payload: OutboundChunks, sendable: &mut _) -> usize
 *     rdi = &mut CommonState, rsi = &OutboundChunks
 *   take_received_plaintext(&mut self, bytes: Payload)
 *     rdi = &mut CommonState, rsi = &Payload
 *
 * Why this layer and not the AEAD one (`MessageEncrypter/Decrypter::encrypt`,
 * which is also reachable): the encrypter and decrypter are two separate heap
 * objects, so hooking them yields a different `self` per direction. Userspace
 * keys connections on (pid, ssl_ptr), and the HTTP/2 aggregator correlates a
 * request with its response inside one connection -- with split identities the
 * request never completes and no token usage is ever extracted. Both hooks here
 * take the *same* `&mut CommonState`, so one connection stays one connection.
 *
 * Hooking above the record layer also means: application data only, so no
 * content-type filtering; independent of the negotiated cipher suite, so one
 * probe per direction instead of one per suite; and the plaintext is an
 * argument, so neither direction needs a uretprobe.
 *
 * The offsets below are NOT part of rustls' stable API. They were measured on a
 * real build with `offset_of!` and cross-checked against the disassembly rather
 * than inferred from the type declarations, because both types are niche-encoded
 * in ways the source does not show. Nothing is trusted blindly: a NULL pointer
 * or an implausible length is dropped, so a layout change degrades to capturing
 * nothing rather than to emitting garbage.
 */

/* OutboundChunks (write direction) is niche-encoded: eightbyte 0 is Multiple's
 * `chunks` pointer, and NULL there means the Single variant. The two variants
 * overlap, so each eightbyte has a different meaning per variant and the names
 * below are kept distinct on purpose -- reading Multiple's chunk count from
 * Single's `len` slot silently yields `start`, which is 0 in practice and makes
 * every gather write look empty.
 *
 *   Single   { tag = 0  @0, ptr @8, len   @16 }
 *   Multiple { chunks   @0, n   @8, start @16, end @24 }
 */
#define RUSTLS_CHUNKS_TAG_OFF 0
#define RUSTLS_SINGLE_PTR_OFF 8
#define RUSTLS_SINGLE_LEN_OFF 16
#define RUSTLS_MULTI_N_OFF 8
#define RUSTLS_MULTI_START_OFF 16
#define RUSTLS_MULTI_END_OFF 24
/* Fat-pointer stride inside the `&[&[u8]]` array. */
#define RUSTLS_SLICE_STRIDE 16
/* Chunks emitted per gather write. HTTP/2 writes a frame header and its payload
 * as separate slices, so a handful covers real traffic, and the bound keeps the
 * unrolled loop inside the verifier's complexity budget. Overflow is counted
 * rather than silently dropped. */
#define RUSTLS_MAX_GATHER_CHUNKS 4
/* Payload (read direction) needs no variant branch: Borrowed and Owned keep the
 * slice pointer and length at the same offsets, because the discriminant is
 * niche-encoded into the eightbyte that Owned uses as Vec's capacity (Borrowed
 * stores 0x8000000000000000 there, which no real capacity can be). */
#define RUSTLS_PAYLOAD_PTR_OFF 8
#define RUSTLS_PAYLOAD_LEN_OFF 16

/* Gather writes (OutboundChunks::Multiple) whose chunk count exceeded
 * RUSTLS_MAX_GATHER_CHUNKS, so part of the payload was not emitted. Exposed
 * through the skeleton's .bss so the gap is measurable instead of silent. */
__u64 rustls_gather_skips = 0;

/* Emit one plaintext buffer belonging to connection `conn`.
 *
 * `len` is the caller's full plaintext, which at this layer is a whole
 * application write rather than a single TLS record, so it is bounded by the
 * capture cap and not by the record size. SSL_EMIT_TIERED clamps the copy and
 * flags truncation, exactly as it does for a large SSL_write.
 */
static __always_inline int rustls_emit(void *conn, u64 ptr, u64 len, int rw)
{
    u64 pid_tgid = bpf_get_current_pid_tgid();
    u32 pid = pid_tgid >> 32;
    u32 tid = (u32)pid_tgid;
    u32 uid = bpf_get_current_uid_gid();
    u64 ts = bpf_ktime_get_ns();

    u32 ns_pid = trace_allowed(uid, pid);
    if (!ns_pid)
        return 0;
    if (!ptr || len == 0 || len > MAX_BUF_SIZE)
        return 0;

    /* Captured at function entry, so there is no paired uretprobe and hence no
     * measurable in-function duration. */
    SSL_EMIT_TIERED((const char *)ptr, len, rw, ts, 0, ns_pid, tid, uid, (u64)conn, 0);
    return 0;
}

/* Largest single chunk the gather path copies. Both bounds must be compile-time
 * constants for the verifier to prove `off + take` stays inside the reservation,
 * and HTTP/2's default max frame size is 16 KiB, so a per-frame slice fits. */
#define RUSTLS_GATHER_CHUNK_MAX SSL_TIER_SMALL
#define RUSTLS_GATHER_TIER SSL_TIER_MEDIUM

/* Concatenate a gather write's chunks into a single ring-buffer record.
 *
 * `chunks` is rustls' `&[&[u8]]` data pointer, `n` its (already bounded) length,
 * and [start,end) the logical window over the concatenation.
 *
 * Anything that would not fit is dropped whole and counted, never truncated:
 * these bytes feed a framed protocol parser, so a short copy desynchronises the
 * frame stream and produces garbage rather than partial data.
 *
 * Only one tier is used. Unrolling the copy loop once per tier multiplies
 * program size, and a gather write is one request body per LLM call rather than
 * a hot path. The medium tier is the smallest that holds a realistic chat
 * request (an 18 KiB body was observed) without reserving the 4 MiB worst case
 * that #759 removed.
 */
static __always_inline int rustls_gather_emit(void *conn, const char *chunks, u64 n, u64 start,
                                             u64 end)
{
    u64 pid_tgid = bpf_get_current_pid_tgid();
    u32 pid = pid_tgid >> 32;
    u32 tid = (u32)pid_tgid;
    u32 uid = bpf_get_current_uid_gid();
    u64 ts = bpf_ktime_get_ns();

    u32 ns_pid = trace_allowed(uid, pid);
    if (!ns_pid)
        return 0;

    u64 total = end - start;
    if (total == 0 || total > RUSTLS_GATHER_TIER) {
        /* Too large for one reservation; a partial copy would corrupt framing. */
        if (total)
            __sync_fetch_and_add(&rustls_gather_skips, 1);
        return 0;
    }

    struct probe_SSL_data_medium *d = bpf_ringbuf_reserve(&rb, sizeof(*d), 0);
    if (!d)
        return 0;

    d->source = EVENT_SOURCE_SSL;
    d->timestamp_ns = ts;
    d->delta_ns = 0;
    d->pid = ns_pid;
    d->tid = tid;
    d->uid = uid;
    d->len = (u32)total;
    d->rw = 1;
    d->is_handshake = 0;
    d->ssl_ptr = (u64)conn;
    d->truncated = 0;
    bpf_get_current_comm(&d->comm, sizeof(d->comm));

    /* `pos` is the logical offset of the current chunk's first byte. */
    u64 pos = 0;
    u32 written = 0;
    bool complete = true;
#pragma unroll
    for (int i = 0; i < RUSTLS_MAX_GATHER_CHUNKS; i++) {
        if ((u64)i >= n || pos >= end)
            break;

        u64 cptr = 0;
        u64 clen = 0;
        const char *slot = chunks + (u64)i * RUSTLS_SLICE_STRIDE;
        if (bpf_probe_read_user(&cptr, sizeof(cptr), slot) ||
            bpf_probe_read_user(&clen, sizeof(clen), slot + 8)) {
            complete = false;
            break;
        }

        /* Intersect [pos, pos+clen) with the [start, end) window. */
        u64 chunk_end = pos + clen;
        u64 lo = pos > start ? pos : start;
        u64 hi = chunk_end < end ? chunk_end : end;
        u64 skip = lo - pos; /* bytes of this chunk before the window */
        pos = chunk_end;
        if (hi <= lo || !cptr)
            continue;

        u64 want = hi - lo;
        /* Both operands of the bounds check are constants, which is what lets the
         * verifier carry `off + take <= sizeof(buf)` across the spill that the
         * helper call forces (same trap SSL_EMIT_ONE documents). */
        if (want > RUSTLS_GATHER_CHUNK_MAX || written > RUSTLS_GATHER_TIER - RUSTLS_GATHER_CHUNK_MAX) {
            complete = false;
            break;
        }
        u32 off = written;
        u32 take = (u32)want;
        /* Re-clamp behind a barrier: the value is spilled across the call above,
         * and the verifier reloads it as an unbounded scalar otherwise. */
        asm volatile("" : "+r"(take));
        if (take > RUSTLS_GATHER_CHUNK_MAX)
            take = RUSTLS_GATHER_CHUNK_MAX;
        asm volatile("" : "+r"(take));
        if (take == 0)
            continue;
        if (bpf_probe_read_user(&d->buf[off], take, (const char *)(cptr + skip))) {
            complete = false;
            break;
        }
        written += take;
    }

    /* A hole anywhere makes the remaining bytes unparseable, so drop the record
     * rather than hand the frame parser a corrupt stream. */
    if (!complete || written != (u32)total) {
        __sync_fetch_and_add(&rustls_gather_skips, 1);
        bpf_ringbuf_discard(d, 0);
        return 0;
    }
    d->buf_filled = 1;
    d->buf_size = written;
    bpf_ringbuf_submit(d, 0);
    return 0;
}

SEC("uprobe/rustls_buffer_plaintext")
int BPF_UPROBE(probe_rustls_write_plaintext, void *conn, void *chunks) {
    u64 tag = 0;
    if (bpf_probe_read_user(&tag, sizeof(tag), (const char *)chunks + RUSTLS_CHUNKS_TAG_OFF))
        return 0;

    if (tag == 0) {
        u64 ptr = 0;
        u64 len = 0;
        if (bpf_probe_read_user(&ptr, sizeof(ptr), (const char *)chunks + RUSTLS_SINGLE_PTR_OFF))
            return 0;
        if (bpf_probe_read_user(&len, sizeof(len), (const char *)chunks + RUSTLS_SINGLE_LEN_OFF))
            return 0;
        return rustls_emit(conn, ptr, len, 1);
    }

    /* Multiple: a gather write. `tag` is the &[&[u8]] data pointer and the
     * logical payload is the concatenation of the chunks sliced by [start,end).
     *
     * The chunks MUST be concatenated into one event rather than emitted one per
     * chunk. HTTP/2 is framed, and `parse_ssl_event` parses each event on its
     * own: a chunk that starts mid-frame is unparseable and degrades to RawData,
     * which only the HTTP/1.1 path knows how to continue. Verified the hard way —
     * per-chunk emission delivered every byte in order and still produced zero
     * parsed frames.
     */
    u64 n = 0;
    u64 start = 0;
    u64 end = 0;
    if (bpf_probe_read_user(&n, sizeof(n), (const char *)chunks + RUSTLS_MULTI_N_OFF))
        return 0;
    if (bpf_probe_read_user(&start, sizeof(start), (const char *)chunks + RUSTLS_MULTI_START_OFF))
        return 0;
    if (bpf_probe_read_user(&end, sizeof(end), (const char *)chunks + RUSTLS_MULTI_END_OFF))
        return 0;
    if (end <= start)
        return 0;
    if (n > RUSTLS_MAX_GATHER_CHUNKS) {
        __sync_fetch_and_add(&rustls_gather_skips, 1);
        n = RUSTLS_MAX_GATHER_CHUNKS;
    }
    return rustls_gather_emit(conn, (const char *)tag, n, start, end);
}

SEC("uprobe/rustls_take_received_plaintext")
int BPF_UPROBE(probe_rustls_read_plaintext, void *conn, void *payload) {
    u64 ptr = 0;
    u64 len = 0;
    if (bpf_probe_read_user(&ptr, sizeof(ptr), (const char *)payload + RUSTLS_PAYLOAD_PTR_OFF))
        return 0;
    if (bpf_probe_read_user(&len, sizeof(len), (const char *)payload + RUSTLS_PAYLOAD_LEN_OFF))
        return 0;
    return rustls_emit(conn, ptr, len, 0);
}

char LICENSE[] SEC("license") = "GPL";