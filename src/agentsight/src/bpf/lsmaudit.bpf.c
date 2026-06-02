// SPDX-License-Identifier: (LGPL-2.1 OR BSD-2-Clause)
// Copyright (c) 2025 AgentSight Project
//
// LSM audit BPF program — observe-only security auditing for traced Agent
// families. It attaches to BPF LSM hooks but NEVER changes the verdict: every
// program returns the incoming `ret` unchanged, so no operation is ever denied.
// (The LSM attach point keeps the door open to future enforcement.)
//
//   - lsm/socket_connect: records each outbound connection (dst IP:port).
//     This is the signal the other probes miss — sslsniff/tcpsniff/udpdns only
//     see TLS / specific ports / DNS, whereas this catches every connect()
//     regardless of protocol or port.
//   - lsm/file_open: records each file opened (basename + open flags), bounded
//     by a per-(pid,inode) LRU so a chatty Agent cannot flood the ring buffer.
//     NOTE: bpf_d_path is allowlist-rejected for lsm/file_open (the allowlist
//     keys on security_file_open, not the bpf_lsm_file_open attach point), so we
//     record the basename via the proven filewrite pattern; full path is future
//     work.
//
// Filtered to traced Agent families via the shared traced_processes map
// (is_pid_traced). Emits through the shared ring buffer (EVENT_SOURCE_LSM).
#include "vmlinux.h"
#include <bpf/bpf_core_read.h>
#include <bpf/bpf_helpers.h>
#include <bpf/bpf_tracing.h>
#include "lsmaudit.h"
#include "common.h"

#ifndef AF_INET
#define AF_INET  2
#endif
#ifndef AF_INET6
#define AF_INET6 10
#endif

// Per-(pid,inode) dedup for file_open: emit once per file per process until LRU
// eviction, so repeated opens of the same file don't flood the ring buffer.
struct file_open_key {
    u64 pid;
    u64 ino;
};

struct {
    __uint(type, BPF_MAP_TYPE_LRU_HASH);
    __uint(max_entries, 8192);
    __type(key, struct file_open_key);
    __type(value, u8);
} file_open_seen SEC(".maps");

// lsm/socket_connect — one event per outbound connection attempt.
SEC("lsm/socket_connect")
int BPF_PROG(audit_socket_connect, struct socket *sock, struct sockaddr *address,
             int addrlen, int ret)
{
    u64 pid_tgid = bpf_get_current_pid_tgid();
    u32 pid = pid_tgid >> 32;
    u32 ns_pid = is_pid_traced(pid);
    if (!ns_pid)
        return ret;   // observe-only: propagate prior verdict unchanged

    u16 family = BPF_CORE_READ(address, sa_family);
    if (family != AF_INET && family != AF_INET6)
        return ret;   // skip AF_UNIX and friends

    // LSM hook fires before the protocol-level length check, so the sockaddr
    // fields beyond what addrlen covers may be uninitialized stack residue.
    // Drop the event rather than read garbage as the destination.
    if (family == AF_INET  && addrlen < (int)sizeof(struct sockaddr_in))
        return ret;
    if (family == AF_INET6 && addrlen < (int)sizeof(struct sockaddr_in6))
        return ret;

    struct lsm_audit_event *e = bpf_ringbuf_reserve(&rb, sizeof(*e), 0);
    if (!e)
        return ret;

    // Zero the whole record so any unwritten byte (especially path[]'s tail
    // after the null terminator) cannot leak previous ringbuf contents — the
    // ringbuf is shared with sslsniff/filewrite, so leftover bytes can be
    // real TLS plaintext or file content.
    __builtin_memset(e, 0, sizeof(*e));

    e->source = EVENT_SOURCE_LSM;
    e->kind = LSM_EVENT_CONNECT;
    e->pid = ns_pid;
    e->tid = (u32)pid_tgid;
    e->uid = bpf_get_current_uid_gid();
    e->timestamp_ns = bpf_ktime_get_ns();
    e->family = (u8)family;
    bpf_get_current_comm(&e->comm, sizeof(e->comm));

    if (family == AF_INET) {
        struct sockaddr_in *in4 = (struct sockaddr_in *)address;
        e->dport = BPF_CORE_READ(in4, sin_port);
        u32 a = BPF_CORE_READ(in4, sin_addr.s_addr);
        __builtin_memcpy(e->daddr, &a, sizeof(a));
    } else {
        struct sockaddr_in6 *in6 = (struct sockaddr_in6 *)address;
        e->dport = BPF_CORE_READ(in6, sin6_port);
        BPF_CORE_READ_INTO(&e->daddr, in6, sin6_addr);
    }

    bpf_ringbuf_submit(e, 0);
    return ret;
}

// lsm/file_open — one event per (pid,inode), deduped via LRU.
SEC("lsm/file_open")
int BPF_PROG(audit_file_open, struct file *file, int ret)
{
    u64 pid_tgid = bpf_get_current_pid_tgid();
    u32 pid = pid_tgid >> 32;
    u32 ns_pid = is_pid_traced(pid);
    if (!ns_pid)
        return ret;   // observe-only: propagate prior verdict unchanged

    struct file_open_key key = {
        .pid = ns_pid,
        .ino = BPF_CORE_READ(file, f_inode, i_ino),
    };
    if (bpf_map_lookup_elem(&file_open_seen, &key))
        return ret;   // already recorded this file for this process

    // basename from file->f_path.dentry->d_name.name (proven filewrite pattern)
    const unsigned char *name = BPF_CORE_READ(file, f_path.dentry, d_name.name);
    if (!name)
        return ret;

    struct lsm_audit_event *e = bpf_ringbuf_reserve(&rb, sizeof(*e), 0);
    if (!e)
        return ret;

    // Zero the whole record — see audit_socket_connect for rationale (path[]'s
    // tail after the null terminator must not leak ringbuf residue).
    __builtin_memset(e, 0, sizeof(*e));

    e->source = EVENT_SOURCE_LSM;
    e->kind = LSM_EVENT_FILE_OPEN;
    e->pid = ns_pid;
    e->tid = (u32)pid_tgid;
    e->uid = bpf_get_current_uid_gid();
    e->timestamp_ns = bpf_ktime_get_ns();
    e->open_flags = BPF_CORE_READ(file, f_flags);
    bpf_get_current_comm(&e->comm, sizeof(e->comm));

    int n = bpf_probe_read_kernel_str(e->path, sizeof(e->path), name);
    if (n <= 0) {
        // Path read failed — discard the slot and DO NOT mark the dedup so a
        // future open of the same file still gets a chance to be recorded.
        bpf_ringbuf_discard(e, 0);
        return ret;
    }

    bpf_ringbuf_submit(e, 0);

    // Mark dedup AFTER successful submit so reserve / read failures above do
    // not permanently suppress the file for this pid until LRU eviction.
    u8 one = 1;
    bpf_map_update_elem(&file_open_seen, &key, &one, BPF_ANY);
    return ret;
}

char LICENSE[] SEC("license") = "GPL";
