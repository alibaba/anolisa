// SPDX-License-Identifier: (LGPL-2.1 OR BSD-2-Clause)
// Copyright (c) 2025 AgentSight Project
//
// TLS SNI BPF program
// Captures Server Name Indication from TLS ClientHello messages
// by hooking tcp_sendmsg and parsing the first bytes of the TCP payload.
// This is SSL-library-agnostic and works for all TLS clients.

#include "vmlinux.h"
#include <bpf/bpf_core_read.h>
#include <bpf/bpf_helpers.h>
#include <bpf/bpf_tracing.h>
#include <bpf/bpf_endian.h>
#include "tlssni.h"

// Do not use traced_processes map - capture all TLS SNI globally
#define NO_TRACED_PROCESSES_MAP
#include "common.h"

// Use power-of-2 buffer size so that bitmask guarantees verifier safety.
#define TLS_HELLO_MAX 512
#define BUF_MASK      (TLS_HELLO_MAX - 1)   // 0x1FF

// Force compiler to keep the bitmask by inserting an asm barrier.
// Without this, clang optimizes away the & BUF_MASK when it can prove
// the value is already < TLS_HELLO_MAX, but the BPF verifier on kernel
// 5.10 cannot follow the same reasoning through complex control flow.
#define BOUNDED(x) ({ \
    __u32 __val = (x); \
    asm volatile("" : "+r"(__val)); \
    __val &= BUF_MASK; \
    __val; \
})

// TLS constants
#define TLS_CONTENT_TYPE_HANDSHAKE 0x16
#define TLS_HANDSHAKE_CLIENT_HELLO 0x01
#define TLS_EXT_SERVER_NAME 0x0000

// Connection deduplication key
struct conn_key {
    __u32 pid;
    __u32 daddr;
    __u16 dport;
    __u16 pad;
};

// LRU hash for connection deduplication - avoids re-parsing same connection
struct {
    __uint(type, BPF_MAP_TYPE_LRU_HASH);
    __uint(max_entries, 4096);
    __type(key, struct conn_key);
    __type(value, __u8);
} seen_connections SEC(".maps");

// Per-CPU scratch buffer for reading TCP payload (avoids stack overflow)
struct {
    __uint(type, BPF_MAP_TYPE_PERCPU_ARRAY);
    __uint(max_entries, 1);
    __type(key, __u32);
    __type(value, __u8[TLS_HELLO_MAX]);
} scratch_buf SEC(".maps");

SEC("fentry/tcp_sendmsg")
int BPF_PROG(trace_tcp_sendmsg, struct sock *sk, struct msghdr *msg, size_t size)
{
    // Quick size check: TLS record header (5) + handshake header (4) + minimum ClientHello
    if (size < 43)
        return 0;

    // Get process info
    __u64 pid_tgid = bpf_get_current_pid_tgid();
    __u32 pid = pid_tgid >> 32;
    __u32 tid = (__u32)pid_tgid;

    // Get destination address and port from sock for deduplication
    __u32 daddr = BPF_CORE_READ(sk, __sk_common.skc_daddr);
    __u16 dport = BPF_CORE_READ(sk, __sk_common.skc_dport);

    // Check deduplication map
    struct conn_key key = {
        .pid = pid,
        .daddr = daddr,
        .dport = dport,
        .pad = 0,
    };
    if (bpf_map_lookup_elem(&seen_connections, &key))
        return 0;

    // Read the first iovec from msg_iter to get user-space buffer pointer
    const struct iovec *iov = BPF_CORE_READ(msg, msg_iter.iov);
    if (!iov)
        return 0;

    void *iov_base = BPF_CORE_READ(iov, iov_base);
    size_t iov_len = BPF_CORE_READ(iov, iov_len);
    if (!iov_base || iov_len < 43)
        return 0;

    // Get scratch buffer
    __u32 zero = 0;
    __u8 *buf = bpf_map_lookup_elem(&scratch_buf, &zero);
    if (!buf)
        return 0;

    // Clamp read size to buffer capacity
    __u32 read_len = iov_len;
    if (read_len > TLS_HELLO_MAX)
        read_len = TLS_HELLO_MAX;

    // Read user-space buffer into scratch
    int ret = bpf_probe_read_user(buf, read_len & BUF_MASK, iov_base);
    if (ret != 0)
        return 0;

    // --- Fast filter: check TLS Record header ---
    if (buf[0] != TLS_CONTENT_TYPE_HANDSHAKE)
        return 0;
    if (buf[1] != 0x03)
        return 0;
    if (buf[5] != TLS_HANDSHAKE_CLIENT_HELLO)
        return 0;

    // --- Parse ClientHello to find SNI extension ---
    // off = 5 (record hdr) + 4 (hs hdr) + 2 (version) + 32 (random) = 43
    __u32 off = 43;

    // Session ID (variable length)
    if (off >= TLS_HELLO_MAX)
        return 0;
    __u8 session_id_len = buf[BOUNDED(off)];
    off += 1 + session_id_len;

    // Cipher Suites (variable length, 2-byte length prefix)
    if (off + 2 >= TLS_HELLO_MAX)
        return 0;
    __u16 cipher_suites_len = ((__u16)buf[BOUNDED(off)] << 8) | (__u16)buf[BOUNDED(off + 1)];
    off += 2 + cipher_suites_len;

    // Compression Methods (variable length, 1-byte length prefix)
    if (off >= TLS_HELLO_MAX)
        return 0;
    __u8 compression_len = buf[BOUNDED(off)];
    off += 1 + compression_len;

    // Extensions length (2 bytes)
    if (off + 2 >= TLS_HELLO_MAX)
        return 0;
    __u16 extensions_total_len = ((__u16)buf[BOUNDED(off)] << 8) | (__u16)buf[BOUNDED(off + 1)];
    off += 2;

    __u32 extensions_end = off + extensions_total_len;
    if (extensions_end > TLS_HELLO_MAX)
        extensions_end = TLS_HELLO_MAX;

    // Iterate extensions (bounded loop for BPF verifier)
    #pragma unroll
    for (int i = 0; i < 24; i++) {
        // Need at least 4 bytes for extension header (type:2 + length:2)
        if (off + 4 > extensions_end)
            break;
        if (off + 4 >= TLS_HELLO_MAX)
            break;

        __u16 ext_type = ((__u16)buf[BOUNDED(off)] << 8) | (__u16)buf[BOUNDED(off + 1)];
        __u16 ext_len = ((__u16)buf[BOUNDED(off + 2)] << 8) | (__u16)buf[BOUNDED(off + 3)];

        if (ext_type == TLS_EXT_SERVER_NAME) {
            // SNI extension: list_len(2) + name_type(1) + name_len(2) + name
            __u32 sni_off = off + 4;

            // Need at least 5 more bytes
            if (sni_off + 5 >= TLS_HELLO_MAX)
                break;

            // Skip list_length(2) + name_type(1) = 3 bytes
            sni_off += 3;

            // Read server_name_length (2 bytes)
            __u16 name_len = ((__u16)buf[BOUNDED(sni_off)] << 8) | (__u16)buf[BOUNDED(sni_off + 1)];
            sni_off += 2;

            if (name_len == 0 || name_len > MAX_SNI_LEN - 1)
                break;
            if (sni_off + name_len > extensions_end)
                break;
            if (sni_off + name_len >= TLS_HELLO_MAX)
                break;

            // Reserve ring buffer event
            struct tlssni_event *event = bpf_ringbuf_reserve(&rb, sizeof(*event), 0);
            if (!event)
                return 0;

            event->source = EVENT_SOURCE_TLSSNI;
            event->timestamp_ns = bpf_ktime_get_ns();
            event->pid = pid;
            event->tid = tid;
            event->uid = bpf_get_current_uid_gid();
            event->sni_len = name_len;
            bpf_get_current_comm(&event->comm, sizeof(event->comm));

            // Copy SNI name from scratch buffer
            __builtin_memset(event->sni_name, 0, MAX_SNI_LEN);

            __u32 copy_len = name_len;
            if (copy_len > MAX_SNI_LEN - 1)
                copy_len = MAX_SNI_LEN - 1;

            // BOUNDED ensures src stays within buf
            __u32 src = BOUNDED(sni_off);
            if (src + copy_len <= TLS_HELLO_MAX) {
                bpf_probe_read_kernel(event->sni_name, copy_len & 0xFF, buf + src);
            }

            bpf_ringbuf_submit(event, 0);

            // Mark connection as seen
            __u8 val = 1;
            bpf_map_update_elem(&seen_connections, &key, &val, BPF_ANY);
            return 0;
        }

        off += 4 + ext_len;
    }

    return 0;
}

char LICENSE[] SEC("license") = "GPL";
