// SPDX-License-Identifier: (LGPL-2.1 OR BSD-2-Clause)
// Copyright (c) 2025 AgentSight Project
//
// File write BPF program
// Monitors vfs_write calls from traced processes writing to .jsonl files
// Uses fentry (BPF trampoline) for minimal overhead on the hot vfs_write path
#include "vmlinux.h"
#include <bpf/bpf_core_read.h>
#include <bpf/bpf_helpers.h>
#include <bpf/bpf_tracing.h>
#include "filewrite.h"
#include "common.h"

// UUID format: 8-4-4-4-12 hex digits with hyphens (36 chars total)
#define UUID_LEN 36
// Expected filename: <uuid>.jsonl = 36 + 6 = 42 chars
#define UUID_JSONL_LEN (UUID_LEN + 6)

static __always_inline int is_hex(char c)
{
    return (c >= '0' && c <= '9') ||
           (c >= 'a' && c <= 'f') ||
           (c >= 'A' && c <= 'F');
}

// Validate UUID format: xxxxxxxx-xxxx-xxxx-xxxx-xxxxxxxxxxxx
// Hyphens at positions 8, 13, 18, 23; hex digits elsewhere
static __always_inline int is_uuid(const char *s)
{
    #pragma unroll
    for (int i = 0; i < UUID_LEN; i++) {
        if (i == 8 || i == 13 || i == 18 || i == 23) {
            if (s[i] != '-')
                return 0;
        } else {
            if (!is_hex(s[i]))
                return 0;
        }
    }
    return 1;
}

// fentry hook on vfs_write - triggers before write executes
// Signature: ssize_t vfs_write(struct file *file, const char __user *buf, size_t count, loff_t *pos)
SEC("fentry/vfs_write")
int BPF_PROG(trace_vfs_write, struct file *file, const char *buf, size_t count, loff_t *pos)
{
    u64 pid_tgid = bpf_get_current_pid_tgid();
    u32 pid = pid_tgid >> 32;

    // Only monitor traced processes
    u32 *val = bpf_map_lookup_elem(&traced_processes, &pid);
    if (!val)
        return 0;

    // Extract filename from file->f_path.dentry->d_name.name (basename)
    // Check early so we can skip non-.jsonl files before reserving ringbuf
    const unsigned char *name_ptr = BPF_CORE_READ(file, f_path.dentry, d_name.name);
    if (!name_ptr)
        return 0;

    char fname[MAX_FILENAME_LEN];
    int ret = bpf_probe_read_kernel_str(fname, sizeof(fname), name_ptr);
    if (ret <= 0)
        return 0;

    // ret includes the null terminator, so string length = ret - 1
    // Filename must be exactly "<uuid>.jsonl" = 42 chars
    int slen = ret - 1;
    if (slen != UUID_JSONL_LEN)
        return 0;

    // Compare last 6 characters against ".jsonl"
    if (fname[UUID_LEN]     != '.' ||
        fname[UUID_LEN + 1] != 'j' ||
        fname[UUID_LEN + 2] != 's' ||
        fname[UUID_LEN + 3] != 'o' ||
        fname[UUID_LEN + 4] != 'n' ||
        fname[UUID_LEN + 5] != 'l')
        return 0;

    // Validate UUID portion of filename
    if (!is_uuid(fname))
        return 0;

    // Reserve space in ring buffer
    struct filewrite_event *event = bpf_ringbuf_reserve(&rb, sizeof(*event), 0);
    if (!event)
        return 0;

    // Fill metadata
    event->source = EVENT_SOURCE_FILEWRITE;
    event->timestamp_ns = bpf_ktime_get_ns();
    event->pid = pid;
    event->tid = (u32)pid_tgid;
    event->uid = bpf_get_current_uid_gid();
    event->write_size = (u32)count;
    bpf_get_current_comm(&event->comm, sizeof(event->comm));

    // Copy filename we already read into the event
    __builtin_memcpy(event->filename, fname, MAX_FILENAME_LEN);

    // Copy write content (up to 16KB)
    // Explicit bounds clamping to satisfy eBPF verifier:
    // bitmask first to ensure value range, then clamp to actual max
    u32 copy_size = (u32)count & 0x3FFF;  // Mask to 14 bits (16383)
    if (copy_size > MAX_FILEWRITE_BUF)
        copy_size = MAX_FILEWRITE_BUF;

    ret = bpf_probe_read_user(event->buf, copy_size, buf);
    if (ret != 0) {
        // Failed to read user buffer
        bpf_ringbuf_discard(event, 0);
        return 0;
    }
    event->buf_size = copy_size;

    bpf_ringbuf_submit(event, 0);
    return 0;
}

char LICENSE[] SEC("license") = "GPL";
