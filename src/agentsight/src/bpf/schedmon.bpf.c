// SPDX-License-Identifier: (LGPL-2.1 OR BSD-2-Clause)
// Copyright (c) 2025 AgentSight Project
//
// Scheduler monitor BPF program
// Detects idle/active state transitions for traced Agent processes via the
// BTF-typed sched_switch / sched_wakeup tracepoints.
//
// Using tp_btf (rather than the format-struct tracepoints) gives direct typed
// access to the task_struct, which lets us:
//   1. read the *raw* prev->__state (TASK_RUNNING == 0) and the `preempt` flag,
//      so a task that is merely preempted while still runnable is NOT treated as
//      going to sleep — only a genuinely blocking task emits SLEEP;
//   2. filter by tgid (thread-group id) so we only watch traced Agent families,
//      but emit per-tid (thread id), so userspace can tell a family is ACTIVE
//      while ANY of its threads is runnable (a multithreaded process commonly has
//      some threads sleeping while others run).
//
// Events are rate-limited per-tid to avoid flooding the shared ring buffer.
#include "vmlinux.h"
#include <bpf/bpf_core_read.h>
#include <bpf/bpf_helpers.h>
#include <bpf/bpf_tracing.h>
#include "schedmon.h"
#include "common.h"

struct pid_sched_state {
    u8 last_state;   // sched_event_type
};

// Per-tid last emitted state, used to deduplicate consecutive same-state events
// (a running thread does not re-emit WAKEUP). We deliberately do NOT apply a
// time-based cooldown: a thread can legitimately wake and immediately block again
// (WAKEUP then SLEEP within microseconds), and dropping that SLEEP would strand
// the thread in the userspace runnable set, pinning its family ACTIVE forever.
// LRU so it self-evicts (schedmon has no thread-exit hook) and update never fails.
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

// sched_switch: prev (one thread) is going off-CPU.
// Emit SLEEP only when prev is actually blocking (not merely preempted while
// still runnable). Filtered by tgid, emitted per-tid.
SEC("tp_btf/sched_switch")
int BPF_PROG(handle_sched_switch, bool preempt, struct task_struct *prev,
             struct task_struct *next)
{
    // Preempted tasks stay runnable — they are not going to sleep.
    if (preempt)
        return 0;

    // prev->__state has already been set to the target state by the scheduler.
    // TASK_RUNNING (0) means it is still runnable (e.g. yielded); skip it.
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

// sched_wakeup: task p (one thread) is being woken up. Any thread waking makes
// its family ACTIVE. Filtered by tgid, emitted per-tid.
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
