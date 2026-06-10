// SPDX-License-Identifier: (LGPL-2.1 OR BSD-2-Clause)
// Copyright (c) 2025 AgentSight Project
//
// Scheduler monitor BPF program — detects idle/active state transitions for
// traced Agent processes via BTF-typed sched_switch / sched_wakeup tracepoints.
#include "vmlinux.h"
#include <bpf/bpf_core_read.h>
#include <bpf/bpf_helpers.h>
#include <bpf/bpf_tracing.h>
#include "schedmon.h"
#include "common.h"

struct pid_sched_state {
    u8 last_state;
};

struct {
    __uint(type, BPF_MAP_TYPE_LRU_HASH);
    __uint(max_entries, MAX_TRACED_PROCESSES);
    __type(key, u32);
    __type(value, struct pid_sched_state);
} pid_sched_state SEC(".maps");

static __always_inline int
emit_sched_event(u32 tgid, u32 tid, u64 now, u8 event_type)
{
    struct pid_sched_state *st = bpf_map_lookup_elem(&pid_sched_state, &tid);
    if (st) {
        if (st->last_state == event_type)
            return 0;
        st->last_state = event_type;
    } else {
        struct pid_sched_state new_st = {
            .last_state = event_type,
        };
        bpf_map_update_elem(&pid_sched_state, &tid, &new_st, BPF_ANY);
    }

    struct sched_event *ev = bpf_ringbuf_reserve(&rb, sizeof(*ev), 0);
    if (!ev)
        return 0;

    ev->source = EVENT_SOURCE_SCHED;
    ev->tgid = tgid;
    ev->tid = tid;
    ev->timestamp_ns = now;
    ev->event_type = event_type;
    ev->pad[0] = 0;
    ev->pad[1] = 0;
    ev->pad[2] = 0;

    bpf_ringbuf_submit(ev, 0);
    return 0;
}

SEC("tp_btf/sched_switch")
int BPF_PROG(handle_sched_switch, bool preempt, struct task_struct *prev,
             struct task_struct *next)
{
    if (preempt)
        return 0;

    unsigned int state = BPF_CORE_READ(prev, __state);
    if (state == 0)
        return 0;

    u32 tgid = BPF_CORE_READ(prev, tgid);
    u32 *traced = bpf_map_lookup_elem(&traced_processes, &tgid);
    if (!traced)
        return 0;

    u32 tid = BPF_CORE_READ(prev, pid);
    return emit_sched_event(tgid, tid, bpf_ktime_get_ns(), SCHED_EVENT_SLEEP);
}

SEC("tp_btf/sched_wakeup")
int BPF_PROG(handle_sched_wakeup, struct task_struct *p)
{
    u32 tgid = BPF_CORE_READ(p, tgid);
    u32 *traced = bpf_map_lookup_elem(&traced_processes, &tgid);
    if (!traced)
        return 0;

    u32 tid = BPF_CORE_READ(p, pid);
    return emit_sched_event(tgid, tid, bpf_ktime_get_ns(), SCHED_EVENT_WAKEUP);
}

char LICENSE[] SEC("license") = "GPL";
