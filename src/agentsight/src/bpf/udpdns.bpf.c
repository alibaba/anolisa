// SPDX-License-Identifier: (LGPL-2.1 OR BSD-2-Clause)
// Copyright (c) 2025 AgentSight Project
//
// UDP DNS BPF program
// Captures domain names from DNS query packets by hooking udp_sendmsg
// and filtering for destination port 53. Much lighter than hooking tcp_sendmsg
// for TLS SNI parsing since DNS queries are infrequent compared to TCP sends.

#include "vmlinux.h"
#include <bpf/bpf_core_read.h>
#include <bpf/bpf_helpers.h>
#include <bpf/bpf_tracing.h>
#include <bpf/bpf_endian.h>
#include "udpdns.h"

// Include common.h with traced_processes map - skip already-traced processes
#include "common.h"

// DNS query buffer size (RFC 1035: UDP DNS messages <= 512 bytes)
#define DNS_BUF_MAX 512
#define DNS_BUF_MASK (DNS_BUF_MAX - 1)  // 0x1FF

// Force compiler to keep the bitmask for BPF verifier safety on kernel 5.10+
#define BOUNDED(x) ({ \
    __u32 __val = (x); \
    asm volatile("" : "+r"(__val)); \
    __val &= DNS_BUF_MASK; \
    __val; \
})

// Domain output buffer bitmask (MAX_DOMAIN_LEN = 256, power of 2)
#define DOMAIN_MASK (MAX_DOMAIN_LEN - 1)  // 0xFF

#define BOUNDED_DOMAIN(x) ({ \
    __u32 __val = (x); \
    asm volatile("" : "+r"(__val)); \
    __val &= DOMAIN_MASK; \
    __val; \
})

// DNS header constants
#define DNS_HEADER_LEN 12
#define DNS_QR_MASK    0x80  // QR bit in flags byte 0 (1=response, 0=query)
#define DNS_PORT       53

// Max labels to parse (real domains rarely exceed 10 labels)
// Reduced from 32 to 10 to avoid BPF verifier -E2BIG with nested loops
#define MAX_LABELS 10
// Max label length per RFC 1035 — capped at 32 for verifier budget
// (real labels > 32 chars are extremely rare in practice)
#define MAX_LABEL_LEN 32

// Deduplication key: {pid, domain_hash}
struct dns_dedup_key {
    __u32 pid;
    __u32 domain_hash;
};

// LRU hash for deduplication - avoids re-reporting same domain for same process
struct {
    __uint(type, BPF_MAP_TYPE_LRU_HASH);
    __uint(max_entries, 4096);
    __type(key, struct dns_dedup_key);
    __type(value, __u8);
} seen_dns SEC(".maps");

// Per-CPU scratch buffer for reading DNS payload (avoids stack overflow)
struct {
    __uint(type, BPF_MAP_TYPE_PERCPU_ARRAY);
    __uint(max_entries, 1);
    __type(key, __u32);
    __type(value, __u8[DNS_BUF_MAX]);
} dns_scratch SEC(".maps");

// Simple djb2 hash for domain deduplication
// Uses bounded loop (no #pragma unroll) to avoid instruction explosion.
// Kernel 5.3+ verifier handles bounded loops natively.
static __always_inline __u32 djb2_hash(const char *str, __u32 len)
{
    __u32 hash = 5381;
    __u32 cap = len;
    if (cap > MAX_DOMAIN_LEN)
        cap = MAX_DOMAIN_LEN;
    for (__u32 i = 0; i < cap && i < MAX_DOMAIN_LEN; i++) {
        hash = ((hash << 5) + hash) + (unsigned char)str[i];
    }
    return hash;
}

SEC("fentry/udp_sendmsg")
int BPF_PROG(trace_udp_sendmsg, struct sock *sk, struct msghdr *msg, size_t size)
{
    // Fast path: check destination port == 53 (DNS)
    __u16 dport = BPF_CORE_READ(sk, __sk_common.skc_dport);
    if (dport != bpf_htons(DNS_PORT))
        return 0;

    // Minimum DNS query: 12 (header) + 1 (min QNAME) + 4 (QTYPE+QCLASS) = 17 bytes
    if (size < 17)
        return 0;

    // Get process info
    __u64 pid_tgid = bpf_get_current_pid_tgid();
    __u32 pid = pid_tgid >> 32;
    __u32 tid = (__u32)pid_tgid;

    // Skip processes already being traced - no need to discover them again
    if (bpf_map_lookup_elem(&traced_processes, &pid))
        return 0;

    // Read the first iovec from msg_iter to get user-space buffer pointer
    const struct iovec *iov = BPF_CORE_READ(msg, msg_iter.iov);
    if (!iov)
        return 0;

    void *iov_base = BPF_CORE_READ(iov, iov_base);
    size_t iov_len = BPF_CORE_READ(iov, iov_len);
    if (!iov_base || iov_len < 17)
        return 0;

    // Get scratch buffer
    __u32 zero = 0;
    __u8 *buf = bpf_map_lookup_elem(&dns_scratch, &zero);
    if (!buf)
        return 0;

    // Clamp read size to buffer capacity
    __u32 read_len = iov_len;
    if (read_len > DNS_BUF_MAX)
        read_len = DNS_BUF_MAX;

    // Read user-space buffer into scratch
    int ret = bpf_probe_read_user(buf, read_len & DNS_BUF_MASK, iov_base);
    if (ret != 0)
        return 0;

    // --- Validate DNS header ---
    // Byte 2: flags (high byte) - QR bit must be 0 (query)
    if (buf[2] & DNS_QR_MASK)
        return 0;  // This is a response, not a query

    // Bytes 4-5: QDCOUNT (question count) - must be >= 1
    __u16 qdcount = ((__u16)buf[4] << 8) | (__u16)buf[5];
    if (qdcount == 0)
        return 0;

    // --- Parse QNAME starting at offset 12 (after DNS header) ---
    // DNS wire format: sequence of (length, label_bytes...) terminated by 0x00
    // Convert to dotted notation: "api.openai.com"
    __u32 off = DNS_HEADER_LEN;  // offset into scratch buf
    __u32 doff = 0;              // offset into domain output

    // Temporary domain storage on stack (will be copied to ringbuf event)
    // We'll write directly into the event after reserving ringbuf space
    // But first, let's parse into a temporary area to compute hash for dedup

    // Reserve ring buffer event early so we can write domain directly into it
    struct udpdns_event *event = bpf_ringbuf_reserve(&rb, sizeof(*event), 0);
    if (!event)
        return 0;

    __builtin_memset(event->domain, 0, MAX_DOMAIN_LEN);

    // Parse DNS labels into dotted domain notation.
    // Use bounded loops (no #pragma unroll) to stay within verifier budget.
    for (int i = 0; i < MAX_LABELS; i++) {
        if (off >= read_len)
            break;

        __u8 label_len = buf[BOUNDED(off)];

        // End of name (root label)
        if (label_len == 0)
            break;

        // Sanity: label length must be <= MAX_LABEL_LEN and not a pointer (0xC0 prefix)
        if (label_len > MAX_LABEL_LEN || (label_len & 0xC0) != 0)
            break;

        off += 1;

        // Add dot separator between labels (not before first)
        if (doff > 0 && doff < MAX_DOMAIN_LEN - 1) {
            event->domain[BOUNDED_DOMAIN(doff)] = '.';
            doff++;
        }

        // Copy label bytes
        for (int j = 0; j < MAX_LABEL_LEN; j++) {
            if ((__u32)j >= label_len)
                break;
            if (doff >= MAX_DOMAIN_LEN - 1)
                break;
            if (off >= read_len)
                break;

            event->domain[BOUNDED_DOMAIN(doff)] = buf[BOUNDED(off)];
            doff++;
            off++;
        }
    }

    // Empty domain - discard
    if (doff == 0) {
        bpf_ringbuf_discard(event, 0);
        return 0;
    }

    // Null-terminate
    event->domain[BOUNDED_DOMAIN(doff)] = '\0';
    event->domain_len = doff;

    // Deduplication: check if we've already seen this (pid, domain) pair
    __u32 hash = djb2_hash(event->domain, doff);
    struct dns_dedup_key dedup_key = {
        .pid = pid,
        .domain_hash = hash,
    };
    if (bpf_map_lookup_elem(&seen_dns, &dedup_key)) {
        bpf_ringbuf_discard(event, 0);
        return 0;
    }

    // Fill remaining event fields
    event->source = EVENT_SOURCE_UDPDNS;
    event->timestamp_ns = bpf_ktime_get_ns();
    event->pid = pid;
    event->tid = tid;
    event->uid = bpf_get_current_uid_gid();
    bpf_get_current_comm(&event->comm, sizeof(event->comm));

    bpf_ringbuf_submit(event, 0);

    // Mark as seen
    __u8 val = 1;
    bpf_map_update_elem(&seen_dns, &dedup_key, &val, BPF_ANY);

    return 0;
}

char LICENSE[] SEC("license") = "GPL";
