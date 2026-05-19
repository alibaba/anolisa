#ifndef COMMON_H
#define COMMON_H

#include "vmlinux.h"
#include <bpf/bpf_helpers.h>
#include <bpf/bpf_core_read.h>

#ifndef RING_BUFFER_SIZE
#define RING_BUFFER_SIZE (32 * 1024 * 1024)
#endif

#ifndef MAX_TRACED_PROCESSES
#define MAX_TRACED_PROCESSES 1024
#endif


// Event source identifiers - first field of every ringbuffer event
// Allows unified dispatch from a shared ring buffer
typedef enum {
    EVENT_SOURCE_PROC = 1,   // Process events (proctrace)
    EVENT_SOURCE_SSL  = 2,   // SSL/TLS traffic events (sslsniff)
    EVENT_SOURCE_PROCMON = 3, // Process monitor events (procmon)
    EVENT_SOURCE_FILEWATCH = 4, // File watch events (filewatch)
    EVENT_SOURCE_FILEWRITE = 5, // File write events (filewrite)
    EVENT_SOURCE_UDPDNS = 6,   // UDP DNS query events (udpdns)
} event_source_t;

// Common event header - every ringbuffer event MUST start with this
// Allows user-space to read source and dispatch to the right handler
struct common_event_hdr {
    u32 source;  // event_source_t - identifies the event producer
};

// Shared ring buffer - used by all BPF programs to avoid wasting memory
struct
{
    __uint(type, BPF_MAP_TYPE_RINGBUF);
    __uint(max_entries, RING_BUFFER_SIZE);
} rb SEC(".maps");

#ifndef NO_TRACED_PROCESSES_MAP
// Shared traced_processes map - used by all BPF programs for process filtering
struct
{
    __uint(type, BPF_MAP_TYPE_HASH);
    __uint(max_entries, MAX_TRACED_PROCESSES);
    __type(key, u32);
    __type(value, u32);
} traced_processes SEC(".maps");
#endif

struct pid_link
{
  struct hlist_node node;
  struct pid *pid;
};

struct task_struct___older_v50
{
  struct pid_link pids[PIDTYPE_MAX];
};


static inline u32 get_task_ns_pid(struct task_struct *task)
{
  unsigned int level = 0;
  struct pid *pid = NULL;

  if (bpf_core_type_exists(struct pid_link))
  {
    struct task_struct___older_v50 *t = (void *)task;
    pid = BPF_CORE_READ(t, pids[PIDTYPE_PID].pid);
  }
  else
  {
    pid = BPF_CORE_READ(task, thread_pid);
  }

  level = BPF_CORE_READ(pid, level);

  return BPF_CORE_READ(pid, numbers[level].nr);
}

/* Convenience wrapper: get the namespace PID of the current task.
 * In non-container scenarios this equals bpf_get_current_pid_tgid() >> 32. */
static __always_inline u32 current_ns_pid(void)
{
    struct task_struct *task = (struct task_struct *)bpf_get_current_task();
    return get_task_ns_pid(task);
}

/*
 * is_pid_traced - check whether the current process should be traced.
 *
 * Returns the namespace PID (to use for event->pid) if the process is traced,
 * or 0 if it should be skipped. Checks both host PID and container ns_pid so
 * that user-space can register either PID and get correct matching.
 */
#ifndef NO_TRACED_PROCESSES_MAP
static __always_inline u32 is_pid_traced(u32 host_pid)
{
    u32 *traced = bpf_map_lookup_elem(&traced_processes, &host_pid);
    if (traced)
        return host_pid;

    /* Container scenario: host PID != namespace PID.
     * Resolve the current task's ns_pid and retry the lookup. */
    struct task_struct *task = (struct task_struct *)bpf_get_current_task();
    u32 ns_pid = get_task_ns_pid(task);

    if (ns_pid != host_pid) {
        traced = bpf_map_lookup_elem(&traced_processes, &ns_pid);
        if (traced)
            return ns_pid;
    }

    return 0;
}
#endif


#endif
