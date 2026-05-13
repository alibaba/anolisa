// SPDX-License-Identifier: (LGPL-2.1 OR BSD-2-Clause)
// Copyright (c) 2025 AgentSight Project
//
// UDP DNS event structure definition
// Used by udpdns BPF program to report extracted domain names from DNS queries

#ifndef UDPDNS_H
#define UDPDNS_H

#define TASK_COMM_LEN 16
#define MAX_DOMAIN_LEN 256

typedef unsigned char       u8;
typedef unsigned short      u16;
typedef unsigned int        u32;
typedef unsigned long long  u64;

struct udpdns_event {
    u32 source;             // EVENT_SOURCE_UDPDNS (6)
    u64 timestamp_ns;
    u32 pid;
    u32 tid;
    u32 uid;
    u32 domain_len;         // actual domain string length (dotted notation)
    char comm[TASK_COMM_LEN];
    char domain[MAX_DOMAIN_LEN];
};

#endif
