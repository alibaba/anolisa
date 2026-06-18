// SPDX-License-Identifier: (LGPL-2.1 OR BSD-2-Clause)
// Copyright (c) 2025 AgentSight Project
//
// TCP plain-text traffic capture BPF program.
// Hooks tcp_sendmsg (fentry) and tcp_recvmsg (fentry+fexit) to capture
// HTTP traffic on configurable IP/port targets. Emits probe_SSL_data_t events
// (same format as sslsniff) so the entire downstream pipeline works unchanged.
// Filters by destination IP/port only; no process-level filtering.

#define NO_TRACED_PROCESSES_MAP
#include "vmlinux.h"
#include <bpf/bpf_core_read.h>
#include <bpf/bpf_helpers.h>
#include <bpf/bpf_tracing.h>
#include <bpf/bpf_endian.h>
#include "sslsniff.h"
#include "common.h"

// MSG_PEEK is a socket flag not exported by vmlinux.h
#ifndef MSG_PEEK
#define MSG_PEEK 2
#endif

// --- CO-RE compatibility for iov_iter fields ---
// Kernel 6.4+ renamed iov_iter.iov to iov_iter.__iov.
struct iov_iter___new {
    const struct iovec *__iov;
};

// Kernel 6.0+ added ITER_UBUF: read()/write() on sockets use ubuf instead of iov.
struct iov_iter___ubuf {
    void *ubuf;
    u8 iter_type;
};

// ITER_UBUF = 5 in kernel 6.0+ (ITER_IOVEC=0, ITER_KVEC=1, ITER_BVEC=2, ...)
#define ITER_UBUF_TYPE 5

// Result of extracting user buffer from msghdr
struct msg_buf_info {
    void *buf;   // user-space buffer pointer
    u64 len;     // length of THIS buffer (not total msg size)
};

// Extract user-space buffer pointer and length from msghdr's iov_iter.
// For ITER_UBUF: returns ubuf pointer and iter->count (contiguous).
// For ITER_IOVEC: returns iov[0].iov_base and iov[0].iov_len (first segment only).
//   writev() scatters data across iovecs; we capture only the first segment
//   to avoid reading beyond its boundary into unrelated memory.
static __always_inline struct msg_buf_info get_msg_buf_info(struct msghdr *msg)
{
    struct msg_buf_info info = { .buf = NULL, .len = 0 };
    struct iov_iter *iter = &msg->msg_iter;

    // Try ITER_UBUF first (kernel 6.0+, used by read()/write() on sockets)
    struct iov_iter___ubuf *ubuf_iter = (void *)iter;
    if (bpf_core_field_exists(ubuf_iter->ubuf)) {
        u8 type = BPF_CORE_READ(ubuf_iter, iter_type);
        if (type == ITER_UBUF_TYPE) {
            info.buf = BPF_CORE_READ(ubuf_iter, ubuf);
            info.len = BPF_CORE_READ(iter, count);
            return info;
        }
    }

    // Fall back to ITER_IOVEC (sendmsg/recvmsg/writev syscalls)
    struct iov_iter___new *new_iter = (void *)iter;
    const struct iovec *iov;
    if (bpf_core_field_exists(new_iter->__iov)) {
        iov = BPF_CORE_READ(new_iter, __iov);
    } else {
        iov = BPF_CORE_READ(iter, iov);
    }
    if (!iov)
        return info;
    info.buf = BPF_CORE_READ(iov, iov_base);
    info.len = BPF_CORE_READ(iov, iov_len);
    return info;
}

// --- IP/port filter map (populated from userspace) ---
// Key: destination IP + port, 0 = wildcard (any IP or any port).
struct tcp_target_key {
    __be32 ip;    // destination IPv4, network byte order; 0 = any IP
    __be16 port;  // destination port, network byte order; 0 = any port
    __u16  pad;   // alignment padding
};

struct {
    __uint(type, BPF_MAP_TYPE_HASH);
    __uint(max_entries, 64);
    __type(key, struct tcp_target_key);
    __type(value, __u8);
} tcp_targets SEC(".maps");

// --- Per-connection HTTP protocol cache ---
// Once a connection is identified as HTTP (first request/response matches),
// all subsequent data on that connection is passed through without re-checking.
// This is critical for SSE/chunked responses where later chunks don't start
// with HTTP keywords.  LRU eviction handles cleanup without explicit close hooks.
struct {
    __uint(type, BPF_MAP_TYPE_LRU_HASH);
    __uint(max_entries, 4096);
    __type(key, u64);   // sock pointer as connection identifier
    __type(value, u8);  // 1 = confirmed HTTP
} tcp_http_conns SEC(".maps");

// --- Stash map for tcp_recvmsg entry → exit ---
struct tcp_recv_args {
    u64 sk;           // struct sock * as u64
    u64 user_buf;     // user-space buffer pointer captured at fentry
    u64 buf_len;      // buffer capacity captured at fentry
    u64 start_ns;
};

struct {
    __uint(type, BPF_MAP_TYPE_HASH);
    __uint(max_entries, 1024);
    __type(key, u32);   // tid
    __type(value, struct tcp_recv_args);
} tcp_recv_stash SEC(".maps");

// Check if the connection's destination matches any configured target.
// Lookup priority (most-specific first):
//   1. exact ip+port match
//   2. ip-only match (port=0 means any port)
//   3. port-only match (ip=0 means any ip)
//   4. full wildcard (ip=0, port=0) — capture every TCP connection
static __always_inline bool is_target_conn(struct sock *sk)
{
    struct tcp_target_key key = {};
    __be32 daddr = BPF_CORE_READ(sk, __sk_common.skc_daddr);
    __be16 dport = BPF_CORE_READ(sk, __sk_common.skc_dport);

    // 1. exact ip+port
    key.ip = daddr;
    key.port = dport;
    if (bpf_map_lookup_elem(&tcp_targets, &key))
        return true;

    // 2. ip-only (port wildcard)
    key.port = 0;
    if (bpf_map_lookup_elem(&tcp_targets, &key))
        return true;

    // 3. port-only (ip wildcard)
    key.ip = 0;
    key.port = dport;
    if (bpf_map_lookup_elem(&tcp_targets, &key))
        return true;

    // 4. full wildcard — match-all (ip=0, port=0)
    key.port = 0;
    return bpf_map_lookup_elem(&tcp_targets, &key) != NULL;
}

// Lightweight HTTP protocol detection on already-copied buffer.
// Returns true if the payload starts with a known HTTP method or "HTTP" response.
// Used to filter non-HTTP traffic (TLS, Redis, MySQL, etc.) in the BPF layer
// before submitting to the ring buffer, avoiding costly kernel→userspace copies.
static __always_inline bool is_http_payload(const char *buf, u32 len)
{
    if (len < 4)
        return false;
    if (buf[0] == 'H' && buf[1] == 'T' && buf[2] == 'T' && buf[3] == 'P')
        return true;
    if (buf[0] == 'G' && buf[1] == 'E' && buf[2] == 'T' && buf[3] == ' ')
        return true;
    if (buf[0] == 'P' && buf[1] == 'O' && buf[2] == 'S' && buf[3] == 'T')
        return true;
    if (buf[0] == 'P' && buf[1] == 'U' && buf[2] == 'T' && buf[3] == ' ')
        return true;
    if (buf[0] == 'H' && buf[1] == 'E' && buf[2] == 'A' && buf[3] == 'D')
        return true;
    if (len >= 6 && buf[0] == 'D' && buf[1] == 'E' && buf[2] == 'L' &&
        buf[3] == 'E' && buf[4] == 'T' && buf[5] == 'E')
        return true;
    if (len >= 5 && buf[0] == 'P' && buf[1] == 'A' && buf[2] == 'T' &&
        buf[3] == 'C' && buf[4] == 'H')
        return true;
    if (buf[0] == 'O' && buf[1] == 'P' && buf[2] == 'T' && buf[3] == 'I')
        return true;
    if (buf[0] == 'C' && buf[1] == 'O' && buf[2] == 'N' && buf[3] == 'N')
        return true;
    return false;
}

// Emit a probe_SSL_data_t event given a pre-resolved user buffer pointer.
static __always_inline int emit_tcp_event_buf(
    struct sock *sk,
    void *user_buf,
    u32 data_len,
    int rw,           // 1=send(request), 0=recv(response)
    u64 start_ns)
{
    if (data_len == 0 || !user_buf)
        return 0;

    // Reserve ring buffer event (same struct as sslsniff)
    struct probe_SSL_data_t *data = bpf_ringbuf_reserve(&rb, sizeof(*data), 0);
    if (!data)
        return 0;

    u64 now = bpf_ktime_get_ns();
    u64 pid_tgid = bpf_get_current_pid_tgid();

    data->source = EVENT_SOURCE_SSL;  // reuse SSL source for seamless pipeline
    data->timestamp_ns = now;
    data->delta_ns = (start_ns > 0) ? (now - start_ns) : 0;
    data->pid = (u32)(pid_tgid >> 32);
    data->tid = (u32)pid_tgid;
    data->uid = bpf_get_current_uid_gid();
    data->len = data_len;
    data->rw = rw;
    data->is_handshake = false;
    /* Initialize the shared `truncated` header field. tcpsniff masks the payload
     * to <=1 MiB (below) and never truncates against the 4 MiB cap, so it is 0.
     * This MUST be set: every EVENT_SOURCE_SSL record shares this header and the
     * consumer (sslsniff::from_bytes) reads `truncated`; bpf_ringbuf_reserve does
     * not zero the reservation, so an omitted field reads stale ring bytes. */
    data->truncated = 0;
    data->ssl_ptr = (u64)sk;  // use sock pointer as connection identifier

    // Clamp buffer size for verifier
    u32 buf_copy_size = data_len & 0xFFFFF;
    if (buf_copy_size > MAX_BUF_SIZE)
        buf_copy_size = MAX_BUF_SIZE;

    bpf_get_current_comm(&data->comm, sizeof(data->comm));

    int ret = bpf_probe_read_user(&data->buf, buf_copy_size, user_buf);
    if (ret == 0) {
        u64 sk_key = (u64)sk;
        if (!bpf_map_lookup_elem(&tcp_http_conns, &sk_key)) {
            if (!is_http_payload((const char *)data->buf, buf_copy_size)) {
                bpf_ringbuf_discard(data, 0);
                return 0;
            }
            u8 val = 1;
            bpf_map_update_elem(&tcp_http_conns, &sk_key, &val, BPF_ANY);
        }
        data->buf_filled = 1;
        data->buf_size = buf_copy_size;
    } else {
        data->buf_filled = 0;
        data->buf_size = 0;
    }

    bpf_ringbuf_submit(data, 0);
    return 0;
}

// Get the iov pointer from iov_iter (CO-RE safe)
static __always_inline const struct iovec *get_iov_ptr(struct iov_iter *iter)
{
    struct iov_iter___new *new_iter = (void *)iter;
    if (bpf_core_field_exists(new_iter->__iov))
        return BPF_CORE_READ(new_iter, __iov);
    return BPF_CORE_READ(iter, iov);
}

// --- tcp_sendmsg: capture outgoing data (HTTP requests) ---
// For writev() (ITER_IOVEC), data is scattered across multiple iovecs.
// Node.js typically uses writev with iov[0]=HTTP headers, iov[1]=JSON body.
// We read both segments into the event buffer for correct parsing.
// For write() (ITER_UBUF), data is contiguous — single copy.
SEC("fentry/tcp_sendmsg")
int BPF_PROG(trace_tcp_sendmsg, struct sock *sk, struct msghdr *msg, size_t size)
{
    if (!is_target_conn(sk))
        return 0;

    struct iov_iter *iter = &msg->msg_iter;

    // Check if ITER_UBUF (contiguous buffer from write() syscall)
    struct iov_iter___ubuf *ubuf_iter = (void *)iter;
    if (bpf_core_field_exists(ubuf_iter->ubuf)) {
        u8 type = BPF_CORE_READ(ubuf_iter, iter_type);
        if (type == ITER_UBUF_TYPE) {
            void *ubuf = BPF_CORE_READ(ubuf_iter, ubuf);
            u32 count = (u32)BPF_CORE_READ(iter, count);
            if (count > (u32)size)
                count = (u32)size;
            return emit_tcp_event_buf(sk, ubuf, count, 1, 0);
        }
    }

    // ITER_IOVEC: scatter-gather from writev()/sendmsg()
    // Read iov[0] and iov[1] into the event buffer concatenated.
    const struct iovec *iov = get_iov_ptr(iter);
    if (!iov)
        return 0;

    void *iov0_base = BPF_CORE_READ(iov, iov_base);
    u64 iov0_len = BPF_CORE_READ(iov, iov_len);
    if (!iov0_base || iov0_len == 0)
        return 0;

    // Reserve ring buffer event
    struct probe_SSL_data_t *data = bpf_ringbuf_reserve(&rb, sizeof(*data), 0);
    if (!data)
        return 0;

    u64 now = bpf_ktime_get_ns();
    u64 pid_tgid = bpf_get_current_pid_tgid();
    data->source = EVENT_SOURCE_SSL;
    data->timestamp_ns = now;
    data->delta_ns = 0;
    data->pid = (u32)(pid_tgid >> 32);
    data->tid = (u32)pid_tgid;
    data->uid = bpf_get_current_uid_gid();
    data->len = (u32)size;
    data->rw = 1;
    data->is_handshake = false;
    data->truncated = 0;  /* see emit_tcp_event_buf: shared header field, never truncates against the 4 MiB cap */
    data->ssl_ptr = (u64)sk;
    bpf_get_current_comm(&data->comm, sizeof(data->comm));

    // Copy iov[0] (HTTP headers)
    u32 iov0_copy = (u32)iov0_len & 0xFFFFF;
    if (iov0_copy > MAX_BUF_SIZE)
        iov0_copy = MAX_BUF_SIZE;

    int ret = bpf_probe_read_user(&data->buf[0], iov0_copy, iov0_base);
    if (ret != 0) {
        bpf_ringbuf_discard(data, 0);
        return 0;
    }

    u64 sk_key = (u64)sk;
    if (!bpf_map_lookup_elem(&tcp_http_conns, &sk_key)) {
        if (!is_http_payload((const char *)data->buf, iov0_copy)) {
            bpf_ringbuf_discard(data, 0);
            return 0;
        }
        u8 val = 1;
        bpf_map_update_elem(&tcp_http_conns, &sk_key, &val, BPF_ANY);
    }

    u32 total_copied = iov0_copy;

    // Try to also copy iov[1] (JSON body) if there's space
    u32 nr_segs = (u32)BPF_CORE_READ(iter, nr_segs);
    if (nr_segs >= 2 && total_copied < MAX_BUF_SIZE) {
        const struct iovec *iov1 = &iov[1];
        void *iov1_base = BPF_CORE_READ(iov1, iov_base);
        u64 iov1_len = BPF_CORE_READ(iov1, iov_len);

        if (iov1_base && iov1_len > 0) {
            u32 remaining = MAX_BUF_SIZE - total_copied;
            u32 iov1_copy = (u32)iov1_len & 0xFFFFF;
            if (iov1_copy > remaining)
                iov1_copy = remaining;

            // Verifier needs bounded offset
            u32 offset = total_copied & 0xFFFFF;
            if (offset + iov1_copy <= MAX_BUF_SIZE) {
                ret = bpf_probe_read_user(&data->buf[offset], iov1_copy, iov1_base);
                if (ret == 0)
                    total_copied += iov1_copy;
            }
        }
    }

    data->buf_filled = 1;
    data->buf_size = total_copied;
    bpf_ringbuf_submit(data, 0);
    return 0;
}

// --- tcp_recvmsg entry: stash user_buf pointer for fexit ---
SEC("fentry/tcp_recvmsg")
int BPF_PROG(trace_tcp_recvmsg_entry, struct sock *sk, struct msghdr *msg,
             size_t size, int flags)
{
    if (!is_target_conn(sk))
        return 0;

    // Peek flag means data won't be consumed — skip
    if (flags & MSG_PEEK)
        return 0;

    // Capture user buffer pointer NOW, before kernel advances iov_iter
    struct msg_buf_info bi = get_msg_buf_info(msg);
    if (!bi.buf)
        return 0;

    u32 tid = (u32)bpf_get_current_pid_tgid();
    struct tcp_recv_args args = {
        .sk = (u64)sk,
        .user_buf = (u64)bi.buf,
        .buf_len = bi.len,
        .start_ns = bpf_ktime_get_ns(),
    };
    bpf_map_update_elem(&tcp_recv_stash, &tid, &args, BPF_ANY);
    return 0;
}

// --- tcp_recvmsg exit: read received data using stashed buffer pointer ---
SEC("fexit/tcp_recvmsg")
int BPF_PROG(trace_tcp_recvmsg_exit, struct sock *sk, struct msghdr *msg,
             size_t size, int flags, int *addr_len, int ret)
{
    u32 tid = (u32)bpf_get_current_pid_tgid();

    struct tcp_recv_args *args = bpf_map_lookup_elem(&tcp_recv_stash, &tid);
    if (!args)
        return 0;

    u64 stashed_sk = args->sk;
    u64 stashed_buf = args->user_buf;
    u64 stashed_len = args->buf_len;
    u64 start_ns = args->start_ns;
    bpf_map_delete_elem(&tcp_recv_stash, &tid);

    if (ret <= 0)
        return 0;

    // Clamp ret to buffer capacity
    u32 copy_len = (u32)ret;
    if ((u64)ret > stashed_len)
        copy_len = (u32)stashed_len;

    return emit_tcp_event_buf(
        (struct sock *)stashed_sk,
        (void *)stashed_buf,
        copy_len, 0, start_ns);
}

// --- Kernel 5.8–5.17 variants ---
// tcp_recvmsg had an extra `int nonblock` parameter before 5.18 (commit ec095263a965).
// Signature: int tcp_recvmsg(struct sock *sk, struct msghdr *msg, size_t len,
//                            int nonblock, int flags, int *addr_len)
// Userspace tries the new (5.18+) programs first; falls back to these on older kernels.

SEC("fentry/tcp_recvmsg")
int BPF_PROG(trace_tcp_recvmsg_entry_old, struct sock *sk, struct msghdr *msg,
             size_t size, int nonblock, int flags)
{
    if (!is_target_conn(sk))
        return 0;

    if (flags & MSG_PEEK)
        return 0;

    struct msg_buf_info bi = get_msg_buf_info(msg);
    if (!bi.buf)
        return 0;

    u32 tid = (u32)bpf_get_current_pid_tgid();
    struct tcp_recv_args args = {
        .sk = (u64)sk,
        .user_buf = (u64)bi.buf,
        .buf_len = bi.len,
        .start_ns = bpf_ktime_get_ns(),
    };
    bpf_map_update_elem(&tcp_recv_stash, &tid, &args, BPF_ANY);
    return 0;
}

SEC("fexit/tcp_recvmsg")
int BPF_PROG(trace_tcp_recvmsg_exit_old, struct sock *sk, struct msghdr *msg,
             size_t size, int nonblock, int flags, int *addr_len, int ret)
{
    u32 tid = (u32)bpf_get_current_pid_tgid();

    struct tcp_recv_args *args = bpf_map_lookup_elem(&tcp_recv_stash, &tid);
    if (!args)
        return 0;

    u64 stashed_sk = args->sk;
    u64 stashed_buf = args->user_buf;
    u64 stashed_len = args->buf_len;
    u64 start_ns = args->start_ns;
    bpf_map_delete_elem(&tcp_recv_stash, &tid);

    if (ret <= 0)
        return 0;

    u32 copy_len = (u32)ret;
    if ((u64)ret > stashed_len)
        copy_len = (u32)stashed_len;

    return emit_tcp_event_buf(
        (struct sock *)stashed_sk,
        (void *)stashed_buf,
        copy_len, 0, start_ns);
}

char LICENSE[] SEC("license") = "GPL";
