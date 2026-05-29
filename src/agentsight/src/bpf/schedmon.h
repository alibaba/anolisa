// SPDX-License-Identifier: (LGPL-2.1 OR BSD-2-Clause)
// Copyright (c) 2025 AgentSight Project
//
// Scheduler monitor BPF program header
// Detects idle/active state transitions for traced Agent processes
#ifndef __SCHEDMON_H
#define __SCHEDMON_H

#define TASK_COMM_LEN    16

typedef signed char         s8;
typedef unsigned char       u8;
typedef signed short        s16;
typedef unsigned short      u16;
typedef signed int          s32;
typedef unsigned int        u32;
typedef signed long long    s64;
typedef unsigned long long  u64;

enum sched_event_type {
    SCHED_EVENT_SLEEP  = 1,
    SCHED_EVENT_WAKEUP = 2,
};

struct sched_event {
    u32 source;          // EVENT_SOURCE_SCHED
    u32 tgid;            // thread-group id — identifies the Agent family
    u32 tid;             // thread id — tracked individually so a family is ACTIVE
                         // while ANY of its threads is runnable
    u64 timestamp_ns;
    u8  event_type;      // enum sched_event_type
    u8  pad[3];
};

#endif /* __SCHEDMON_H */
