// SPDX-License-Identifier: (LGPL-2.1 OR BSD-2-Clause)
// Copyright (c) 2025 AgentSight Project
//
// LSM audit BPF program header
// Observe-only security auditing of traced Agent families via BPF LSM hooks:
//   - socket_connect: every outbound connection attempt (dst IP:port)
//   - file_open:      every file opened (basename + open flags)
#ifndef __LSMAUDIT_H
#define __LSMAUDIT_H

#define LSM_COMM_LEN 16
#define LSM_PATH_LEN 256

typedef signed char         s8;
typedef unsigned char       u8;
typedef signed short        s16;
typedef unsigned short      u16;
typedef signed int          s32;
typedef unsigned int        u32;
typedef signed long long    s64;
typedef unsigned long long  u64;

enum lsm_event_kind {
    LSM_EVENT_CONNECT   = 1,
    LSM_EVENT_FILE_OPEN = 2,
};

// A single fixed-size record covers both hooks; `kind` selects which fields are
// meaningful. Unused fields are zeroed by the producer.
// NB: named lsm_audit_event (not lsm_event) — the kernel's vmlinux.h already
// declares `enum lsm_event`, which would collide.
struct lsm_audit_event {
    u32  source;            // EVENT_SOURCE_LSM
    u32  pid;               // namespace PID of the traced Agent (is_pid_traced result)
    u32  tid;               // thread id
    u32  uid;
    u64  timestamp_ns;
    u8   kind;              // enum lsm_event_kind
    u8   family;            // connect: AF_INET(2) / AF_INET6(10); file_open: 0
    u16  dport;             // connect: destination port, network byte order
    s32  open_flags;        // file_open: file->f_flags; connect: 0
    u8   daddr[16];         // connect: IPv4 in [0..4] / IPv6 in [0..16]; file_open: 0
    char comm[LSM_COMM_LEN];
    char path[LSM_PATH_LEN]; // file_open: file basename; connect: empty
};

#endif /* __LSMAUDIT_H */
