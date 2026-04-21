// SPDX-License-Identifier: (LGPL-2.1 OR BSD-2-Clause)
// Copyright (c) 2025 AgentSight Project
//
// File watch BPF program
// Monitors openat syscalls for .jsonl files from traced processes
#include "vmlinux.h"
#include <bpf/bpf_core_read.h>
#include <bpf/bpf_helpers.h>
#include <bpf/bpf_tracing.h>
#include "filewatch.h"
#include "common.h"

// Tracepoint for openat - captures file open events
// Filters for .jsonl suffix at BPF layer to minimize user-space overhead
SEC("tp/syscalls/sys_enter_openat")
int trace_openat_enter(struct trace_event_raw_sys_enter *ctx)
{
    u64 pid_tgid = bpf_get_current_pid_tgid();
    u32 pid = pid_tgid >> 32;

    // Only monitor traced processes
    u32 *val = bpf_map_lookup_elem(&traced_processes, &pid);
    if (!val)
        return 0;

    // Reserve space in ring buffer
    struct filewatch_event *event = bpf_ringbuf_reserve(&rb, sizeof(*event), 0);
    if (!event)
        return 0;

    // Read filename from user-space
    const char *filename_ptr = (const char *)ctx->args[1];
    int len = bpf_probe_read_user_str(event->filename, MAX_FILENAME_LEN, filename_ptr);

    // len includes null terminator; need at least 7 chars: x.jsonl\0
    if (len < 8) {
        bpf_ringbuf_discard(event, 0);
        return 0;
    }

    // Check .jsonl suffix (len includes null terminator, so last char is at len-2)
    // suffix starts at offset len-7: '.', 'j', 's', 'o', 'n', 'l', '\0'
    int off = len - 7;
    if (event->filename[off]     != '.' ||
        event->filename[off + 1] != 'j' ||
        event->filename[off + 2] != 's' ||
        event->filename[off + 3] != 'o' ||
        event->filename[off + 4] != 'n' ||
        event->filename[off + 5] != 'l') {
        bpf_ringbuf_discard(event, 0);
        return 0;
    }

    // Fill remaining event fields
    event->source = EVENT_SOURCE_FILEWATCH;
    event->timestamp_ns = bpf_ktime_get_ns();
    event->pid = pid;
    event->tid = (u32)pid_tgid;
    event->uid = bpf_get_current_uid_gid();
    event->flags = (s32)ctx->args[2];
    bpf_get_current_comm(&event->comm, sizeof(event->comm));

    bpf_ringbuf_submit(event, 0);
    return 0;
}

char LICENSE[] SEC("license") = "GPL";
