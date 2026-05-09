// SPDX-License-Identifier: (LGPL-2.1 OR BSD-2-Clause)
// Copyright (c) 2025 AgentSight Project
//
// TLS SNI event structure definition
// Used by tlssni BPF program to report extracted Server Name Indication

#ifndef TLSSNI_H
#define TLSSNI_H

#define TASK_COMM_LEN 16
#define MAX_SNI_LEN 256

typedef unsigned char       u8;
typedef unsigned short      u16;
typedef unsigned int        u32;
typedef unsigned long long  u64;

struct tlssni_event {
    u32 source;             // EVENT_SOURCE_TLSSNI (6)
    u64 timestamp_ns;
    u32 pid;
    u32 tid;
    u32 uid;
    u32 sni_len;            // actual SNI string length
    char comm[TASK_COMM_LEN];
    char sni_name[MAX_SNI_LEN];
};

#endif
