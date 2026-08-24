// SPDX-License-Identifier: MIT
// Copyright (c) 2026 eunomia-bpf org.
//
//! eBPF IFC engine loader (aya).
//!
//! Loads the prebuilt CO-RE object `process.bpf.o` (compiled from the untouched
//! kernel C in this directory), installs the compiled policy into writable BPF
//! array maps, attaches the enforcer, and surfaces `TAINT_VIOLATION` events.
//! Supports hot-reload of policy rules via `ReloadHandle` (user ring buffer).
//!
//! The config blob is exactly the `struct taint_config` the collector's DSL
//! compiler already produces (the same bytes the C loader read from `--config`).

use std::collections::{HashMap as StdHashMap, HashSet, VecDeque};
use std::io::{self, Read};
use std::os::fd::{AsFd, AsRawFd, FromRawFd, OwnedFd};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Mutex;

use aya::maps::{Array, HashMap, Map, MapData, MapError, ProgramArray, RingBuf};
use aya::programs::{Lsm, ProgramFd, TracePoint};
use aya::{Btf, Ebpf, EbpfLoader};

pub mod capability;
use capability::{
    CapState, DeltaRequest, AUTH_ADD_LABEL, AUTH_BIND_RULE, AUTH_DECLASSIFY, AUTH_DELEGATE,
    AUTH_NARROW_SCOPE, AUTH_REQUIRE_GATE, TARGET_CHILD, TARGET_SELF,
};

const BPF_ANY: u64 = 0;
const BPF_NOEXIST: u64 = 1;
pub const GLOBAL_ACTIVE_DOMAIN_ID: u32 = u32::MAX;

// ---- prebuilt eBPF object, 8-byte aligned for aya's ELF parser ----
#[repr(align(8))]
struct Aligned<T: ?Sized>(T);
static OBJECT: &Aligned<[u8]> =
    &Aligned(*include_bytes!(concat!(env!("OUT_DIR"), "/process.bpf.o")));
fn object_bytes() -> &'static [u8] {
    &OBJECT.0
}

// ===================== ABI mirrors (must match bpf/taint.h) =====================
// Identical to collector/src/dsl/lower.rs; guarded by abi_size_matches() below.
const PAT: usize = 64;
const ARG: usize = 24;
const MAX_UPDATES: usize = 320;
const MAX_RULES: usize = 128;
const MAX_TAINT_LABELS: usize = 64;
const M_SUFFIX: u8 = 2;
const M_CONTAINS: u8 = 4;
const OP_EXEC: u8 = 0;
const OP_OPEN: u8 = 1;
const OP_WRITE: u8 = 2;
const OP_CONNECT: u8 = 3;
const OP_RECV: u8 = 4;
const EFFECT_BLOCK: u8 = 1;
const C_TARGET: u8 = 3;
const FEAT_PATH_CONTAINS: u32 = 1 << 0;
const FEAT_PATH_SUFFIX: u32 = 1 << 1;
const FEAT_OPEN_RULES: u32 = 1 << 2;
const FEAT_WRITE_RULES: u32 = 1 << 3;
const FEAT_CONNECT: u32 = 1 << 4;
const FEAT_RECV: u32 = 1 << 5;
const FEAT_FILE_FLOW: u32 = 1 << 6;
const FEAT_BLOCK_EXEC: u32 = 1 << 7;
const FEAT_BLOCK_FILE: u32 = 1 << 8;
const FEAT_BLOCK_CONNECT: u32 = 1 << 9;

#[repr(C)]
#[derive(Clone, Copy)]
struct CUpdate {
    op: u8,
    m: u8,
    target: [u8; PAT],
    arg: [u8; ARG],
    add: u64,
    del: u64,
    gates: u64,
    invals: u64,
    ipv4: u32,
    ipv4_mask: u32,
    gate_exit_code: i32,
    domain_id: u32,
}
#[repr(C)]
#[derive(Clone, Copy)]
struct CRule {
    op: u8,
    m: u8,
    cond_kind: u8,
    cond_neg: u8,
    cond_match: u8,
    effect: u8,
    target: [u8; PAT],
    arg: [u8; ARG],
    cond_pat: [u8; PAT],
    req: u64,
    forbid: u64,
    gate: u64,
    rule_id: u32,
    ipv4: u32,
    ipv4_mask: u32,
    cond_ipv4: u32,
    cond_ipv4_mask: u32,
    gate_idx: u32,
    domain_id: u32,
    since_mask: u64,
    ttl_ns: u64,
}
#[repr(C)]
#[derive(Clone, Copy)]
struct CConfig {
    n_updates: u32,
    n_rules: u32,
    updates: [CUpdate; MAX_UPDATES],
    rules: [CRule; MAX_RULES],
}

/// Expected policy config blob size — ABI contract with actplane-ifc-compiler.
pub const EXPECTED_CONFIG_BLOB_SIZE: usize = std::mem::size_of::<CConfig>();

// proc_state seed (bpf/taint_engine.bpf.h: { u64 labels; u64 lin_gates; }).
#[repr(C)]
#[derive(Clone, Copy)]
struct ProcState {
    labels: u64,
    lin_gates: u64,
}

#[repr(C)]
#[derive(Clone, Copy)]
struct PidDomainKey {
    pid: i32,
    domain_id: u32,
}

#[repr(C)]
#[derive(Clone, Copy, Default)]
struct CapPolicyMask {
    lo: u64,
    hi: u64,
}

unsafe impl aya::Pod for CUpdate {}
unsafe impl aya::Pod for CRule {}
unsafe impl aya::Pod for ProcState {}
unsafe impl aya::Pod for PidDomainKey {}
unsafe impl aya::Pod for CapPolicyMask {}

// ringbuf event (bpf/process.h: struct event).
const EVENT_TYPE_TAINT_VIOLATION: i32 = 3;
const COMM_LEN: usize = 16;
const FILENAME_LEN: usize = 127;

#[repr(C)]
#[derive(Clone, Copy)]
struct Event {
    etype: i32,
    pid: i32,
    ppid: i32,
    blocked: u32,
    killed: u32,
    effect: u32,
    op: u32,
    domain_id: u32,
    session_root: i32,
    timestamp_ns: u64,
    comm: [u8; COMM_LEN],
    filename: [u8; FILENAME_LEN],
    taint_rule_id: u32,
    conn_ip: u32,
    taint_label: u64,
    matched_label: u64,
    matched_labels: u64,
    prov_label: u64,
    prov_timestamp_ns: u64,
    prov_pid: i32,
    prov_op: u32,
    prov_ip: u32,
    prov_target: [u8; FILENAME_LEN],
}

/// Provenance for the label that caused a policy violation.
#[derive(Debug, Clone)]
pub struct Provenance {
    pub label: u64,
    pub timestamp_ns: u64,
    pub pid: i32,
    pub op: u32,
    pub target: String,
}

/// A policy violation reported by the kernel.
#[derive(Debug, Clone)]
pub struct Violation {
    pub effect: u32, // 0 notify, 1 block, 2 kill
    pub blocked: bool,
    pub killed: bool,
    pub comm: String,
    pub pid: i32,
    pub ppid: i32,
    pub target: String, // exe/path, or "a.b.c.d" for connect
    pub rule_id: u32,
    pub op: u32,
    pub domain_id: u32,
    pub session_root: i32,
    pub label: u64,
    pub matched_label: u64,
    pub matched_labels: u64,
    pub provenance: Option<Provenance>,
    pub timestamp_ns: u64,
}

/// Parameters for creating a child runtime policy domain and binding a pid to it.
#[derive(Debug, Clone, Copy)]
pub struct ChildDomainSpec {
    pub parent_pid: i32,
    pub parent_id: u32,
    pub child_id: u32,
    pub pid: i32,
    pub scope_id: u32,
    pub labels: u64,
    pub authority_mask: u64,
    pub target_mask: u64,
    pub restrict_mask: u64,
    pub gate_mask: u64,
    pub label_mask: u64,
}

impl Default for ChildDomainSpec {
    fn default() -> Self {
        Self {
            parent_pid: 0,
            parent_id: 0,
            child_id: 0,
            pid: 0,
            scope_id: 0,
            labels: 0,
            authority_mask: 0,
            target_mask: TARGET_SELF,
            restrict_mask: 0,
            gate_mask: 0,
            label_mask: 0,
        }
    }
}

fn scope_subset(new_scope: u32, old_scope: u32) -> bool {
    new_scope == 0 || old_scope == 0 || new_scope >= old_scope
}

fn child_domain_state(parent: &CapState, spec: ChildDomainSpec) -> io::Result<CapState> {
    if spec.parent_pid <= 0 || spec.pid <= 0 {
        return Err(err("parent_pid and pid must both be positive"));
    }
    if spec.parent_id == 0 || spec.child_id == 0 {
        return Err(err("parent_id and child_id must both be set"));
    }
    if spec.parent_id == spec.child_id {
        return Err(err("child domain id must differ from parent domain id"));
    }
    if parent.target_mask & TARGET_CHILD == 0 {
        return Err(err("parent domain cannot target child domains"));
    }
    if parent.authority_mask & AUTH_BIND_RULE == 0 {
        return Err(err("parent domain lacks bind-rule authority"));
    }
    let scope_id = if spec.scope_id == 0 {
        parent.scope_id
    } else {
        spec.scope_id
    };
    if !scope_subset(scope_id, parent.scope_id) {
        return Err(err("child domain scope would widen the parent scope"));
    }
    if spec.target_mask & !(TARGET_SELF | TARGET_CHILD) != 0 {
        return Err(err("child domain target mask contains unknown bits"));
    }
    if spec.authority_mask & !parent.authority_mask != 0 {
        return Err(err("child domain authority exceeds parent authority"));
    }
    if spec.label_mask & !parent.label_mask != 0 {
        return Err(err(
            "child domain label authority exceeds parent label authority",
        ));
    }
    if (spec.labels | spec.restrict_mask) & !parent.label_mask != 0 {
        return Err(err(
            "child domain initial labels exceed parent label authority",
        ));
    }
    if spec.gate_mask & !parent.gate_mask != 0 {
        return Err(err("child domain gate mask exceeds parent gate mask"));
    }
    Ok(CapState {
        parent: spec.parent_id,
        scope_id,
        labels: spec.labels,
        authority_mask: spec.authority_mask,
        target_mask: spec.target_mask,
        restrict_mask: spec.restrict_mask,
        gate_mask: spec.gate_mask,
        label_mask: spec.label_mask,
    })
}

/// Map-fd backed control handle for binding child domains while the loader polls events.
pub struct DomainHandle {
    cap_task_fd: OwnedFd,
    cap_state_fd: OwnedFd,
    ts_proc_domains_fd: OwnedFd,
    ts_root_fd: OwnedFd,
}

fn dup_cloexec_fd(fd: std::os::fd::RawFd) -> io::Result<OwnedFd> {
    let dup = unsafe { libc::fcntl(fd, libc::F_DUPFD_CLOEXEC, 0) };
    if dup < 0 {
        return Err(io::Error::last_os_error());
    }
    Ok(unsafe { OwnedFd::from_raw_fd(dup) })
}

fn dup_owned_fd(fd: &OwnedFd) -> io::Result<OwnedFd> {
    dup_cloexec_fd(fd.as_raw_fd())
}

fn dup_hash_map_fd(bpf: &Ebpf, name: &str) -> io::Result<OwnedFd> {
    let map = bpf
        .map(name)
        .ok_or_else(|| err(format!("{name} missing")))?;
    let data = match map {
        Map::HashMap(data) | Map::LruHashMap(data) => data,
        _ => return Err(err(format!("{name} is not a hash map"))),
    };
    dup_cloexec_fd(data.fd().as_fd().as_raw_fd())
}

fn dup_array_map_fd(bpf: &Ebpf, name: &str) -> io::Result<OwnedFd> {
    let map = bpf
        .map(name)
        .ok_or_else(|| err(format!("{name} missing")))?;
    let data = match map {
        Map::Array(data) => data,
        _ => return Err(err(format!("{name} is not an array map"))),
    };
    dup_cloexec_fd(data.fd().as_fd().as_raw_fd())
}

fn hash_map_from_fd<K: aya::Pod, V: aya::Pod>(fd: &OwnedFd) -> io::Result<HashMap<MapData, K, V>> {
    let data = MapData::from_fd(dup_owned_fd(fd)?).map_err(|e| err(format!("map from fd: {e}")))?;
    HashMap::try_from(Map::HashMap(data)).map_err(|e| err(format!("typed hash map: {e}")))
}

fn map_get<K: aya::Pod, V: aya::Pod>(
    map: &HashMap<MapData, K, V>,
    key: &K,
    what: &str,
) -> io::Result<V> {
    map.get(key, BPF_ANY)
        .map_err(|e| err(format!("{what}: {e}")))
}

fn map_get_optional<K: aya::Pod, V: aya::Pod>(
    map: &HashMap<MapData, K, V>,
    key: &K,
    what: &str,
) -> io::Result<Option<V>> {
    match map.get(key, BPF_ANY) {
        Ok(v) => Ok(Some(v)),
        Err(MapError::KeyNotFound) => Ok(None),
        Err(e) => Err(err(format!("{what}: {e}"))),
    }
}

impl DomainHandle {
    /// Create a child domain below an existing parent domain and bind `spec.pid`.
    ///
    /// The child domain is installed before the pid is made active in `cap_task`,
    /// so a partially written domain cannot affect kernel matching.
    pub fn bind_child_domain(&self, spec: ChildDomainSpec) -> io::Result<()> {
        let mut states: HashMap<_, u32, CapState> = hash_map_from_fd(&self.cap_state_fd)?;
        let parent = map_get(&states, &spec.parent_id, "lookup parent domain")?;
        if map_get_optional(&states, &spec.child_id, "lookup child domain")?.is_some() {
            return Err(err(format!(
                "child domain {} already exists",
                spec.child_id
            )));
        }
        let child = child_domain_state(&parent, spec)?;

        let mut proc: HashMap<_, PidDomainKey, ProcState> =
            hash_map_from_fd(&self.ts_proc_domains_fd)?;
        let mut roots: HashMap<_, i32, i32> = hash_map_from_fd(&self.ts_root_fd)?;
        let mut tasks: HashMap<_, i32, u32> = hash_map_from_fd(&self.cap_task_fd)?;

        let root = map_get_optional(&roots, &spec.parent_pid, "lookup parent root")?
            .unwrap_or(spec.parent_pid);
        states
            .insert(spec.child_id, child, BPF_NOEXIST)
            .map_err(|e| err(format!("seed child cap_state: {e}")))?;
        proc.insert(
            PidDomainKey {
                pid: spec.pid,
                domain_id: spec.child_id,
            },
            ProcState {
                labels: 0,
                lin_gates: 0,
            },
            BPF_ANY,
        )
        .map_err(|e| err(format!("seed child ts_proc_domains: {e}")))?;
        roots
            .insert(spec.pid, root, BPF_ANY)
            .map_err(|e| err(format!("seed child ts_root: {e}")))?;
        tasks
            .insert(spec.pid, spec.child_id, BPF_ANY)
            .map_err(|e| err(format!("bind child cap_task: {e}")))?;
        Ok(())
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum TracepointNeed {
    Core,
    CoreExec,
    ExecArgs,
    FileOpen,
    FileWritePath,
    FdFlow,
    ConnectOrRecv,
    SendAddr,
    RecvAddr,
    FileIpcAdvanced,
    MmapAdvanced,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
struct TracepointSpec {
    name: &'static str,
    category: &'static str,
    event: &'static str,
    need: TracepointNeed,
}

/// Tracepoint programs. The loader attaches only the subset required by the
/// loaded hook set instead of attaching the entire object by default.
const TRACEPOINTS: &[TracepointSpec] = &[
    TracepointSpec {
        name: "handle_fork",
        category: "sched",
        event: "sched_process_fork",
        need: TracepointNeed::Core,
    },
    TracepointSpec {
        name: "handle_exec",
        category: "sched",
        event: "sched_process_exec",
        need: TracepointNeed::CoreExec,
    },
    TracepointSpec {
        name: "handle_exec_args",
        category: "sched",
        event: "sched_process_exec",
        need: TracepointNeed::ExecArgs,
    },
    TracepointSpec {
        name: "handle_exit",
        category: "sched",
        event: "sched_process_exit",
        need: TracepointNeed::Core,
    },
    TracepointSpec {
        name: "trace_openat",
        category: "syscalls",
        event: "sys_enter_openat",
        need: TracepointNeed::FileOpen,
    },
    TracepointSpec {
        name: "trace_openat_exit",
        category: "syscalls",
        event: "sys_exit_openat",
        need: TracepointNeed::FileOpen,
    },
    TracepointSpec {
        name: "trace_open",
        category: "syscalls",
        event: "sys_enter_open",
        need: TracepointNeed::FileOpen,
    },
    TracepointSpec {
        name: "trace_open_exit",
        category: "syscalls",
        event: "sys_exit_open",
        need: TracepointNeed::FileOpen,
    },
    TracepointSpec {
        name: "trace_openat2",
        category: "syscalls",
        event: "sys_enter_openat2",
        need: TracepointNeed::FileOpen,
    },
    TracepointSpec {
        name: "trace_openat2_exit",
        category: "syscalls",
        event: "sys_exit_openat2",
        need: TracepointNeed::FileOpen,
    },
    TracepointSpec {
        name: "trace_creat",
        category: "syscalls",
        event: "sys_enter_creat",
        need: TracepointNeed::FileWritePath,
    },
    TracepointSpec {
        name: "trace_creat_exit",
        category: "syscalls",
        event: "sys_exit_creat",
        need: TracepointNeed::FileWritePath,
    },
    TracepointSpec {
        name: "trace_truncate",
        category: "syscalls",
        event: "sys_enter_truncate",
        need: TracepointNeed::FileWritePath,
    },
    TracepointSpec {
        name: "trace_truncate_exit",
        category: "syscalls",
        event: "sys_exit_truncate",
        need: TracepointNeed::FileWritePath,
    },
    TracepointSpec {
        name: "trace_pipe",
        category: "syscalls",
        event: "sys_enter_pipe",
        need: TracepointNeed::FileIpcAdvanced,
    },
    TracepointSpec {
        name: "trace_pipe_exit",
        category: "syscalls",
        event: "sys_exit_pipe",
        need: TracepointNeed::FileIpcAdvanced,
    },
    TracepointSpec {
        name: "trace_pipe2",
        category: "syscalls",
        event: "sys_enter_pipe2",
        need: TracepointNeed::FileIpcAdvanced,
    },
    TracepointSpec {
        name: "trace_pipe2_exit",
        category: "syscalls",
        event: "sys_exit_pipe2",
        need: TracepointNeed::FileIpcAdvanced,
    },
    TracepointSpec {
        name: "trace_socketpair",
        category: "syscalls",
        event: "sys_enter_socketpair",
        need: TracepointNeed::FileIpcAdvanced,
    },
    TracepointSpec {
        name: "trace_socketpair_exit",
        category: "syscalls",
        event: "sys_exit_socketpair",
        need: TracepointNeed::FileIpcAdvanced,
    },
    TracepointSpec {
        name: "trace_bind",
        category: "syscalls",
        event: "sys_enter_bind",
        need: TracepointNeed::FileIpcAdvanced,
    },
    TracepointSpec {
        name: "trace_bind_exit",
        category: "syscalls",
        event: "sys_exit_bind",
        need: TracepointNeed::FileIpcAdvanced,
    },
    TracepointSpec {
        name: "trace_accept",
        category: "syscalls",
        event: "sys_enter_accept",
        need: TracepointNeed::FileIpcAdvanced,
    },
    TracepointSpec {
        name: "trace_accept_exit",
        category: "syscalls",
        event: "sys_exit_accept",
        need: TracepointNeed::FileIpcAdvanced,
    },
    TracepointSpec {
        name: "trace_accept4",
        category: "syscalls",
        event: "sys_enter_accept4",
        need: TracepointNeed::FileIpcAdvanced,
    },
    TracepointSpec {
        name: "trace_accept4_exit",
        category: "syscalls",
        event: "sys_exit_accept4",
        need: TracepointNeed::FileIpcAdvanced,
    },
    TracepointSpec {
        name: "trace_unlink",
        category: "syscalls",
        event: "sys_enter_unlink",
        need: TracepointNeed::FileWritePath,
    },
    TracepointSpec {
        name: "trace_unlinkat",
        category: "syscalls",
        event: "sys_enter_unlinkat",
        need: TracepointNeed::FileWritePath,
    },
    TracepointSpec {
        name: "trace_rename",
        category: "syscalls",
        event: "sys_enter_rename",
        need: TracepointNeed::FileWritePath,
    },
    TracepointSpec {
        name: "trace_renameat",
        category: "syscalls",
        event: "sys_enter_renameat",
        need: TracepointNeed::FileWritePath,
    },
    TracepointSpec {
        name: "trace_renameat2",
        category: "syscalls",
        event: "sys_enter_renameat2",
        need: TracepointNeed::FileWritePath,
    },
    TracepointSpec {
        name: "trace_connect",
        category: "syscalls",
        event: "sys_enter_connect",
        need: TracepointNeed::ConnectOrRecv,
    },
    TracepointSpec {
        name: "trace_connect_exit",
        category: "syscalls",
        event: "sys_exit_connect",
        need: TracepointNeed::ConnectOrRecv,
    },
    TracepointSpec {
        name: "trace_read",
        category: "syscalls",
        event: "sys_enter_read",
        need: TracepointNeed::FdFlow,
    },
    TracepointSpec {
        name: "trace_read_exit",
        category: "syscalls",
        event: "sys_exit_read",
        need: TracepointNeed::FdFlow,
    },
    TracepointSpec {
        name: "trace_write",
        category: "syscalls",
        event: "sys_enter_write",
        need: TracepointNeed::FdFlow,
    },
    TracepointSpec {
        name: "trace_write_exit",
        category: "syscalls",
        event: "sys_exit_write",
        need: TracepointNeed::FdFlow,
    },
    TracepointSpec {
        name: "trace_mmap",
        category: "syscalls",
        event: "sys_enter_mmap",
        need: TracepointNeed::MmapAdvanced,
    },
    TracepointSpec {
        name: "trace_mmap_exit",
        category: "syscalls",
        event: "sys_exit_mmap",
        need: TracepointNeed::MmapAdvanced,
    },
    TracepointSpec {
        name: "trace_mprotect",
        category: "syscalls",
        event: "sys_enter_mprotect",
        need: TracepointNeed::MmapAdvanced,
    },
    TracepointSpec {
        name: "trace_mprotect_exit",
        category: "syscalls",
        event: "sys_exit_mprotect",
        need: TracepointNeed::MmapAdvanced,
    },
    TracepointSpec {
        name: "trace_mremap",
        category: "syscalls",
        event: "sys_enter_mremap",
        need: TracepointNeed::MmapAdvanced,
    },
    TracepointSpec {
        name: "trace_mremap_exit",
        category: "syscalls",
        event: "sys_exit_mremap",
        need: TracepointNeed::MmapAdvanced,
    },
    TracepointSpec {
        name: "trace_munmap",
        category: "syscalls",
        event: "sys_enter_munmap",
        need: TracepointNeed::MmapAdvanced,
    },
    TracepointSpec {
        name: "trace_munmap_exit",
        category: "syscalls",
        event: "sys_exit_munmap",
        need: TracepointNeed::MmapAdvanced,
    },
    TracepointSpec {
        name: "trace_sendto",
        category: "syscalls",
        event: "sys_enter_sendto",
        need: TracepointNeed::SendAddr,
    },
    TracepointSpec {
        name: "trace_sendto_exit",
        category: "syscalls",
        event: "sys_exit_sendto",
        need: TracepointNeed::SendAddr,
    },
    TracepointSpec {
        name: "trace_recvfrom",
        category: "syscalls",
        event: "sys_enter_recvfrom",
        need: TracepointNeed::RecvAddr,
    },
    TracepointSpec {
        name: "trace_recvfrom_exit",
        category: "syscalls",
        event: "sys_exit_recvfrom",
        need: TracepointNeed::RecvAddr,
    },
    TracepointSpec {
        name: "trace_sendmsg",
        category: "syscalls",
        event: "sys_enter_sendmsg",
        need: TracepointNeed::SendAddr,
    },
    TracepointSpec {
        name: "trace_sendmsg_exit",
        category: "syscalls",
        event: "sys_exit_sendmsg",
        need: TracepointNeed::SendAddr,
    },
    TracepointSpec {
        name: "trace_recvmsg",
        category: "syscalls",
        event: "sys_enter_recvmsg",
        need: TracepointNeed::RecvAddr,
    },
    TracepointSpec {
        name: "trace_recvmsg_exit",
        category: "syscalls",
        event: "sys_exit_recvmsg",
        need: TracepointNeed::RecvAddr,
    },
    TracepointSpec {
        name: "trace_close",
        category: "syscalls",
        event: "sys_enter_close",
        need: TracepointNeed::FdFlow,
    },
    TracepointSpec {
        name: "trace_dup",
        category: "syscalls",
        event: "sys_enter_dup",
        need: TracepointNeed::FdFlow,
    },
    TracepointSpec {
        name: "trace_dup_exit",
        category: "syscalls",
        event: "sys_exit_dup",
        need: TracepointNeed::FdFlow,
    },
    TracepointSpec {
        name: "trace_dup2",
        category: "syscalls",
        event: "sys_enter_dup2",
        need: TracepointNeed::FdFlow,
    },
    TracepointSpec {
        name: "trace_dup2_exit",
        category: "syscalls",
        event: "sys_exit_dup2",
        need: TracepointNeed::FdFlow,
    },
    TracepointSpec {
        name: "trace_dup3",
        category: "syscalls",
        event: "sys_enter_dup3",
        need: TracepointNeed::FdFlow,
    },
    TracepointSpec {
        name: "trace_dup3_exit",
        category: "syscalls",
        event: "sys_exit_dup3",
        need: TracepointNeed::FdFlow,
    },
    TracepointSpec {
        name: "trace_fcntl",
        category: "syscalls",
        event: "sys_enter_fcntl",
        need: TracepointNeed::FdFlow,
    },
    TracepointSpec {
        name: "trace_fcntl_exit",
        category: "syscalls",
        event: "sys_exit_fcntl",
        need: TracepointNeed::FdFlow,
    },
    TracepointSpec {
        name: "trace_sendfile64",
        category: "syscalls",
        event: "sys_enter_sendfile64",
        need: TracepointNeed::FileIpcAdvanced,
    },
    TracepointSpec {
        name: "trace_sendfile64_exit",
        category: "syscalls",
        event: "sys_exit_sendfile64",
        need: TracepointNeed::FileIpcAdvanced,
    },
    TracepointSpec {
        name: "trace_copy_file_range",
        category: "syscalls",
        event: "sys_enter_copy_file_range",
        need: TracepointNeed::FileIpcAdvanced,
    },
    TracepointSpec {
        name: "trace_copy_file_range_exit",
        category: "syscalls",
        event: "sys_exit_copy_file_range",
        need: TracepointNeed::FileIpcAdvanced,
    },
    TracepointSpec {
        name: "trace_splice",
        category: "syscalls",
        event: "sys_enter_splice",
        need: TracepointNeed::FileIpcAdvanced,
    },
    TracepointSpec {
        name: "trace_splice_exit",
        category: "syscalls",
        event: "sys_exit_splice",
        need: TracepointNeed::FileIpcAdvanced,
    },
    TracepointSpec {
        name: "cap_drain_tick",
        category: "syscalls",
        event: "sys_enter_getpid",
        need: TracepointNeed::Core,
    },
    TracepointSpec {
        name: "argv_cache_execve",
        category: "syscalls",
        event: "sys_enter_execve",
        need: TracepointNeed::ExecArgs,
    },
    TracepointSpec {
        name: "argv_cache_execveat",
        category: "syscalls",
        event: "sys_enter_execveat",
        need: TracepointNeed::ExecArgs,
    },
    TracepointSpec {
        name: "argv_cache_cleanup_execve",
        category: "syscalls",
        event: "sys_exit_execve",
        need: TracepointNeed::ExecArgs,
    },
    TracepointSpec {
        name: "argv_cache_cleanup_execveat",
        category: "syscalls",
        event: "sys_exit_execveat",
        need: TracepointNeed::ExecArgs,
    },
];

const EXEC_TAIL_PROGS: &[(u32, &str)] = &[
    (0, "exec_tp_update_simple"),
    (1, "exec_tp_update_prefix"),
    (2, "exec_tp_rule_simple"),
    (3, "exec_tp_rule_complex"),
];

/// LSM programs: (fn name, hook). Attached only when BPF LSM is active.
const LSM_PROGS: &[(&str, &str)] = &[
    ("enforce_bprm_check_security", "bprm_check_security"),
    ("enforce_file_open", "file_open"),
    ("enforce_file_permission", "file_permission"),
    ("enforce_file_truncate", "file_truncate"),
    ("enforce_mmap_file", "mmap_file"),
    ("enforce_file_mprotect", "file_mprotect"),
    ("enforce_path_truncate", "path_truncate"),
    ("enforce_path_unlink", "path_unlink"),
    ("enforce_path_rename", "path_rename"),
    ("enforce_socket_connect", "socket_connect"),
    ("enforce_socket_recvmsg", "socket_recvmsg"),
    ("enforce_task_kill", "task_kill"),
    ("enforce_ptrace_access_check", "ptrace_access_check"),
    ("enforce_bpf_syscall", "bpf"),
];

const ALL_HOOK_FEATURES: u32 = FEAT_CONNECT
    | FEAT_RECV
    | FEAT_FILE_FLOW
    | FEAT_BLOCK_EXEC
    | FEAT_BLOCK_FILE
    | FEAT_BLOCK_CONNECT;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum HookProfile {
    Minimal,
    Full,
}

impl HookProfile {
    fn from_env() -> Self {
        match std::env::var("ACTPLANE_HOOK_PROFILE") {
            Ok(v)
                if v.eq_ignore_ascii_case("full")
                    || v.eq_ignore_ascii_case("all")
                    || v.eq_ignore_ascii_case("wide") =>
            {
                HookProfile::Full
            }
            _ => HookProfile::Minimal,
        }
    }

    fn advanced_tracepoints(self) -> bool {
        self == HookProfile::Full
            || std::env::var_os("ACTPLANE_ENABLE_ADVANCED_HOOKS").is_some()
            || std::env::var_os("ACTPLANE_ADVANCED_TRACEPOINTS").is_some()
    }
}

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
struct HookBudget {
    features: u32,
    file_write: bool,
    advanced_tracepoints: bool,
}

impl HookBudget {
    fn from_config(cfg: &CConfig, reserve: HookReserve) -> Self {
        let profile = HookProfile::from_env();
        let mut budget = match profile {
            HookProfile::Minimal => HookBudget {
                features: config_features(cfg),
                file_write: config_has_file_write(cfg),
                advanced_tracepoints: profile.advanced_tracepoints()
                    || reserve.advanced_tracepoints,
            },
            HookProfile::Full => HookBudget {
                features: config_features(cfg) | ALL_HOOK_FEATURES,
                file_write: true,
                advanced_tracepoints: profile.advanced_tracepoints()
                    || reserve.advanced_tracepoints,
            },
        };
        if reserve.file_flow {
            budget.features |= FEAT_FILE_FLOW;
        }
        if std::env::var_os("ACTPLANE_RESERVE_FILE_FLOW").is_some() {
            budget.features |= FEAT_FILE_FLOW;
        }
        if reserve.file_write_paths {
            budget.file_write = true;
        }
        if reserve.network {
            budget.features |= FEAT_CONNECT | FEAT_RECV;
        }
        if reserve.block_exec {
            budget.features |= FEAT_BLOCK_EXEC;
        }
        if reserve.block_file {
            budget.features |= FEAT_BLOCK_FILE;
        }
        if reserve.block_connect {
            budget.features |= FEAT_BLOCK_CONNECT;
        }
        budget
    }

    fn has_file_flow(self) -> bool {
        self.features & FEAT_FILE_FLOW != 0
    }

    fn has_open_rules(self) -> bool {
        self.features & FEAT_OPEN_RULES != 0
    }

    fn has_file_write(self) -> bool {
        self.file_write
    }

    fn has_connect(self) -> bool {
        self.features & FEAT_CONNECT != 0
    }

    fn has_recv(self) -> bool {
        self.features & FEAT_RECV != 0
    }
}

fn tracepoint_needed(spec: &TracepointSpec, budget: HookBudget) -> bool {
    match spec.need {
        TracepointNeed::Core => true,
        TracepointNeed::CoreExec => false,
        TracepointNeed::ExecArgs => true,
        TracepointNeed::FileOpen => budget.has_file_flow() || budget.has_open_rules(),
        TracepointNeed::FileWritePath => budget.has_file_write(),
        TracepointNeed::FdFlow => {
            budget.has_file_flow() || budget.has_connect() || budget.has_recv()
        }
        TracepointNeed::ConnectOrRecv => {
            budget.has_connect()
                || budget.has_recv()
                || (budget.has_file_flow() && budget.advanced_tracepoints)
        }
        TracepointNeed::SendAddr => {
            budget.has_connect() || (budget.has_file_flow() && budget.advanced_tracepoints)
        }
        TracepointNeed::RecvAddr => {
            budget.has_recv() || (budget.has_file_flow() && budget.advanced_tracepoints)
        }
        TracepointNeed::FileIpcAdvanced | TracepointNeed::MmapAdvanced => {
            budget.has_file_flow() && budget.advanced_tracepoints
        }
    }
}

fn lsm_needed(
    name: &str,
    block_exec: bool,
    block_file: bool,
    block_connect: bool,
    recv_flow: bool,
    advanced_hooks: bool,
) -> bool {
    match name {
        "enforce_task_kill" | "enforce_ptrace_access_check" | "enforce_bpf_syscall" => true,
        "enforce_bprm_check_security" => block_exec,
        "enforce_socket_connect" => block_connect,
        "enforce_socket_recvmsg" => recv_flow,
        "enforce_file_permission" => block_file,
        "enforce_mmap_file" | "enforce_file_mprotect" => advanced_hooks && block_file,
        "enforce_file_open"
        | "enforce_file_truncate"
        | "enforce_path_truncate"
        | "enforce_path_unlink"
        | "enforce_path_rename" => block_file,
        _ => false,
    }
}

fn load_exec_tail_programs(bpf: &mut Ebpf) -> io::Result<()> {
    let mut fds: Vec<(u32, ProgramFd)> = Vec::new();

    for (idx, name) in EXEC_TAIL_PROGS {
        let p: &mut TracePoint = bpf
            .program_mut(name)
            .ok_or_else(|| err(format!("program {name} missing")))?
            .try_into()
            .map_err(|e| err(format!("{name} not a tracepoint: {e}")))?;
        p.load().map_err(|e| err(format!("{name}.load: {e}")))?;
        let fd = p
            .fd()
            .map_err(|e| err(format!("{name}.fd: {e}")))?
            .try_clone()
            .map_err(|e| err(format!("{name}.fd clone: {e}")))?;
        fds.push((*idx, fd));
    }

    let mut exec_tail: ProgramArray<_> = ProgramArray::try_from(
        bpf.map_mut("exec_tail")
            .ok_or_else(|| err("map exec_tail missing"))?,
    )
    .map_err(|e| err(format!("exec_tail: {e}")))?;
    for (idx, fd) in fds {
        exec_tail
            .set(idx, &fd, 0)
            .map_err(|e| err(format!("exec_tail[{idx}]: {e}")))?;
    }
    Ok(())
}

/// True if `bpf` appears in the active LSM list (enables pre-op `block`).
pub fn bpf_lsm_active() -> bool {
    if std::env::var_os("ACTPLANE_FORCE_TRACEPOINT").is_some() {
        return false;
    }
    let mut s = String::new();
    if let Ok(mut f) = std::fs::File::open("/sys/kernel/security/lsm") {
        let _ = f.read_to_string(&mut s);
    }
    s.split(',').any(|x| x.trim() == "bpf")
}

fn err(msg: impl Into<String>) -> io::Error {
    io::Error::new(io::ErrorKind::Other, msg.into())
}

fn validate_config(cfg: &CConfig) -> io::Result<()> {
    if cfg.n_updates as usize > MAX_UPDATES {
        return Err(err(format!(
            "config declares {} updates, max is {MAX_UPDATES}",
            cfg.n_updates
        )));
    }
    if cfg.n_rules as usize > MAX_RULES {
        return Err(err(format!(
            "config declares {} rules, max is {MAX_RULES}",
            cfg.n_rules
        )));
    }
    for (i, u) in cfg.updates.iter().take(cfg.n_updates as usize).enumerate() {
        if u.op == OP_EXEC && u.m == M_SUFFIX {
            return Err(err(format!("config update[{i}]: suffix exec matches are unsupported; use DSL exec patterns that lower to exact/prefix")));
        }
    }
    for (i, r) in cfg.rules.iter().take(cfg.n_rules as usize).enumerate() {
        if r.op == OP_EXEC && r.m == M_SUFFIX {
            return Err(err(format!("config rule[{i}]: suffix exec matches are unsupported; use DSL exec patterns that lower to exact/prefix")));
        }
        if r.op == OP_EXEC && r.cond_kind == C_TARGET && r.cond_match == M_SUFFIX {
            return Err(err(format!("config rule[{i}]: suffix exec target conditions are unsupported; use exact/prefix exec patterns")));
        }
    }
    Ok(())
}

fn path_match_features(m: u8) -> u32 {
    match m {
        M_SUFFIX => FEAT_PATH_SUFFIX,
        M_CONTAINS => FEAT_PATH_CONTAINS,
        _ => 0,
    }
}

fn config_features(cfg: &CConfig) -> u32 {
    let mut features = 0;
    for u in cfg
        .updates
        .iter()
        .take((cfg.n_updates as usize).min(MAX_UPDATES))
    {
        if u.op == OP_OPEN || u.op == OP_WRITE {
            features |= FEAT_FILE_FLOW;
            features |= path_match_features(u.m);
        }
        if u.op == OP_CONNECT {
            features |= FEAT_CONNECT;
        }
        if u.op == OP_RECV {
            features |= FEAT_RECV;
        }
    }
    for r in cfg.rules.iter().take((cfg.n_rules as usize).min(MAX_RULES)) {
        if r.effect == EFFECT_BLOCK {
            if r.op == OP_EXEC {
                features |= FEAT_BLOCK_EXEC;
            }
            if r.op == OP_OPEN || r.op == OP_WRITE {
                features |= FEAT_BLOCK_FILE;
            }
            if r.op == OP_CONNECT {
                features |= FEAT_BLOCK_CONNECT;
            }
        }
        if r.op == OP_OPEN {
            features |= FEAT_OPEN_RULES | path_match_features(r.m);
            if r.cond_kind == C_TARGET {
                features |= path_match_features(r.cond_match);
            }
        }
        if r.op == OP_WRITE {
            features |= FEAT_FILE_FLOW | FEAT_WRITE_RULES | path_match_features(r.m);
            if r.cond_kind == C_TARGET {
                features |= path_match_features(r.cond_match);
            }
        }
        if r.op == OP_CONNECT {
            features |= FEAT_CONNECT;
        }
        if r.op == OP_RECV {
            features |= FEAT_RECV;
        }
    }
    features
}

fn config_has_file_write(cfg: &CConfig) -> bool {
    cfg.updates
        .iter()
        .take((cfg.n_updates as usize).min(MAX_UPDATES))
        .any(|u| u.op == OP_WRITE)
        || cfg
            .rules
            .iter()
            .take((cfg.n_rules as usize).min(MAX_RULES))
            .any(|r| r.op == OP_WRITE)
}

fn validate_supported_features(cfg: &CConfig, supported: u32, context: &str) -> io::Result<()> {
    let needed = config_features(cfg);
    let missing = needed & !supported;
    if missing == 0 {
        return Ok(());
    }
    Err(err(feature_gate_error(context, needed, supported, missing)))
}

fn feature_gate_error(context: &str, needed: u32, supported: u32, missing: u32) -> String {
    let mut names = Vec::new();
    if missing & FEAT_PATH_CONTAINS != 0 {
        names.push("path contains matches");
    }
    if missing & FEAT_PATH_SUFFIX != 0 {
        names.push("path suffix matches");
    }
    if missing & FEAT_OPEN_RULES != 0 {
        names.push("open sink rules");
    }
    if missing & FEAT_WRITE_RULES != 0 {
        names.push("write sink rules");
    }
    if missing & FEAT_CONNECT != 0 {
        names.push("connect rules or sources");
    }
    if missing & FEAT_RECV != 0 {
        names.push("recv rules or sources");
    }
    if missing & FEAT_FILE_FLOW != 0 {
        names.push("file source or sink hooks");
    }
    if missing & FEAT_BLOCK_EXEC != 0 {
        names.push("exec block hooks");
    }
    if missing & FEAT_BLOCK_FILE != 0 {
        names.push("file block hooks");
    }
    if missing & FEAT_BLOCK_CONNECT != 0 {
        names.push("connect block hooks");
    }
    let mut hints = Vec::new();
    hints.push(
        "runtime reload/delta cannot attach new hooks or enable new matcher classes after load; restart the engine with a profile or policy that enables them",
    );
    if missing
        & (FEAT_FILE_FLOW
            | FEAT_CONNECT
            | FEAT_RECV
            | FEAT_BLOCK_EXEC
            | FEAT_BLOCK_FILE
            | FEAT_BLOCK_CONNECT)
        != 0
    {
        hints.push(
            "for broad future deltas, start with ACTPLANE_HOOK_PROFILE=full; standalone file-flow users may set ACTPLANE_RESERVE_FILE_FLOW=1, and MCP/watch/child-run already reserve file-flow",
        );
    }
    if missing & (FEAT_OPEN_RULES | FEAT_WRITE_RULES | FEAT_PATH_CONTAINS | FEAT_PATH_SUFFIX) != 0 {
        hints.push(
            "file sink rule classes and path contains/suffix matcher classes must be enabled by the policy used to load the engine; hook-profile reservation does not enable them by itself",
        );
    }
    if missing & (FEAT_BLOCK_EXEC | FEAT_BLOCK_FILE | FEAT_BLOCK_CONNECT) != 0 {
        hints.push(
            "block deltas require matching BPF-LSM hooks in the loaded engine profile and an active bpf LSM; argv-token block exec is not a pre-exec block",
        );
    }
    if missing & (FEAT_CONNECT | FEAT_RECV) != 0 {
        hints.push("network deltas require network hooks in the loaded engine profile");
    }
    format!(
        "{context} requires features not enabled when the eBPF engine was loaded: {}. {}. needed=0x{needed:x}, supported=0x{supported:x}, missing=0x{missing:x}",
        names.join(", "),
        hints.join("; ")
    )
}

pub struct Loader {
    bpf: Ebpf,
    enforce: bool,
    policy_features: u32,
    /// Snapshot of `op=EXEC` source entries from the compiled config
    exec_sources: Vec<(String, u64)>,
    /// Persistent, owned RingBuf created once at load time
    ringbuf: Option<RingBuf<MapData>>,
    /// Complete source-to-bit mapping for all op types
    source_bit_map: StdHashMap<String, u8>,
}

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub struct HookReserve {
    pub file_flow: bool,
    pub file_write_paths: bool,
    pub network: bool,
    pub block_exec: bool,
    pub block_file: bool,
    pub block_connect: bool,
    pub advanced_tracepoints: bool,
}

impl HookReserve {
    pub fn runtime_file_delta() -> Self {
        HookReserve {
            file_flow: true,
            ..HookReserve::default()
        }
    }
}

impl Loader {
    /// `config_blob` is the raw `struct taint_config` produced by the collector.
    pub fn load(config_blob: &[u8]) -> io::Result<Self> {
        Self::load_with_hook_reserve(config_blob, HookReserve::default())
    }

    /// Load the engine with an explicit hook profile for later runtime deltas.
    /// This does not enable file sink rule matching or expensive path matchers
    /// unless the policy used to load the engine requires them.
    pub fn load_with_hook_reserve(
        config_blob: &[u8],
        hook_reserve: HookReserve,
    ) -> io::Result<Self> {
        if config_blob.len() != std::mem::size_of::<CConfig>() {
            return Err(err(format!(
                "config size mismatch: got {}, expected {}",
                config_blob.len(),
                std::mem::size_of::<CConfig>()
            )));
        }
        // Owned, aligned copy so we can borrow fields for set_global.
        let cfg: Box<CConfig> =
            Box::new(unsafe { std::ptr::read_unaligned(config_blob.as_ptr() as *const CConfig) });
        validate_config(&cfg)?;

        let enforce = bpf_lsm_active();
        let enforce_mode: u32 = if enforce { 1 } else { 0 };
        let hook_budget = HookBudget::from_config(&cfg, hook_reserve);
        let policy_features = hook_budget.features;

        let mut loader = EbpfLoader::new();
        loader
            .allow_unsupported_maps()
            .set_global("enforce_mode", &enforce_mode, true)
            .set_global("policy_features", &policy_features, true);

        let mut bpf = loader
            .load(object_bytes())
            .map_err(|e| err(format!("Ebpf::load: {e}")))?;

        // Populate writable array maps for updates and rules.
        populate_update_map(&mut bpf, &cfg)?;
        populate_rule_map(&mut bpf, &cfg)?;
        populate_policy_mask_map(&mut bpf, &cfg)?;

        // Loop counts in a (non-frozen) map so the verifier analyzes each
        // bpf_loop callback once. Slots: 0=rules 1=updates 5=labels.
        {
            let mut counts: Array<_, u32> = Array::try_from(
                bpf.map_mut("ts_counts")
                    .ok_or_else(|| err("map ts_counts missing"))?,
            )
            .map_err(|e| err(format!("ts_counts: {e}")))?;
            let vals = [cfg.n_rules, cfg.n_updates, 0, 0, 0, MAX_TAINT_LABELS as u32];
            for (i, v) in vals.iter().enumerate() {
                counts
                    .set(i as u32, *v, 0)
                    .map_err(|e| err(format!("ts_counts[{i}]: {e}")))?;
            }
        }

        let has_recv = hook_budget.has_recv();
        let has_block_exec = hook_budget.features & FEAT_BLOCK_EXEC != 0;
        let has_block_file = hook_budget.features & FEAT_BLOCK_FILE != 0;
        let has_block_connect = hook_budget.features & FEAT_BLOCK_CONNECT != 0;

        load_exec_tail_programs(&mut bpf)?;

        // Attach only the tracepoints required by this loaded hook set, then LSM
        // programs only when BPF LSM is active.
        for spec in TRACEPOINTS {
            if !tracepoint_needed(spec, hook_budget) {
                continue;
            }
            let p: &mut TracePoint = bpf
                .program_mut(spec.name)
                .ok_or_else(|| err(format!("program {} missing", spec.name)))?
                .try_into()
                .map_err(|e| err(format!("{} not a tracepoint: {e}", spec.name)))?;
            p.load()
                .map_err(|e| err(format!("{}.load: {e}", spec.name)))?;
            p.attach(spec.category, spec.event)
                .map_err(|e| err(format!("{}.attach: {e}", spec.name)))?;
        }
        if enforce {
            let btf = Btf::from_sys_fs().map_err(|e| err(format!("btf: {e}")))?;
            for (name, hook) in LSM_PROGS {
                if !lsm_needed(
                    name,
                    has_block_exec,
                    has_block_file,
                    has_block_connect,
                    has_recv,
                    hook_budget.advanced_tracepoints,
                ) {
                    continue;
                }
                let p: &mut Lsm = bpf
                    .program_mut(name)
                    .ok_or_else(|| err(format!("program {name} missing")))?
                    .try_into()
                    .map_err(|e| err(format!("{name} not an lsm: {e}")))?;
                p.load(hook, &btf)
                    .map_err(|e| err(format!("{name}.load: {e}")))?;
                p.attach().map_err(|e| err(format!("{name}.attach: {e}")))?;
            }
        }

        // Snapshot exec-sources for later scope_bit_for_command() lookups.
        let mut exec_sources: Vec<(String, u64)> = Vec::new();
        for i in 0..(cfg.n_updates as usize).min(MAX_UPDATES) {
            let u = &cfg.updates[i];
            if u.op != OP_EXEC {
                continue;
            }
            let raw: &[u8] = &u.target;
            let end = raw.iter().position(|&b| b == 0).unwrap_or(raw.len());
            if let Ok(s) = std::str::from_utf8(&raw[..end]) {
                exec_sources.push((s.to_string(), u.add));
            }
        }

        // Take the "rb" ring-buffer map out of `Ebpf` and create an owned
        // `RingBuf<MapData>`. Reused across hot-reload cycles to avoid aya 0.13
        // wrap-state bug (pos_cache=0 + stale consumer_pos).
        let ringbuf = match bpf.take_map("rb") {
            Some(rb_map) => {
                Some(RingBuf::try_from(rb_map).map_err(|e| err(format!("rb ringbuf: {e}")))?)
            }
            None => None,
        };

        let source_bit_map = build_full_bit_map(&cfg);

        Ok(Loader {
            bpf,
            enforce,
            policy_features,
            exec_sources,
            ringbuf,
            source_bit_map,
        })
    }

    pub fn enforce_mode(&self) -> bool {
        self.enforce
    }

    /// Mark a loader/control process as protected from subjects that are
    /// already bound to an ActPlane runtime domain. Host processes outside a
    /// domain are not denied, including root/admin unload paths.
    pub fn protect_pid(&mut self, pid: i32) -> io::Result<()> {
        if pid <= 0 {
            return Err(err("protected pid must be positive"));
        }
        let mut protected: HashMap<_, i32, u32> = HashMap::try_from(
            self.bpf
                .map_mut("te_protected_pids")
                .ok_or_else(|| err("te_protected_pids missing"))?,
        )
        .map_err(|e| err(format!("te_protected_pids: {e}")))?;
        protected
            .insert(pid, 1, 0)
            .map_err(|e| err(format!("protect pid {pid}: {e}")))?;
        Ok(())
    }

    /// Create a `ReloadHandle` that can hot-reload policy into this engine.
    ///
    /// On kernels that support `BPF_MAP_TYPE_USER_RINGBUF` (6.1+), the handle
    /// uses the ring buffer for zero-copy hot-reload.  On 5.10-compat builds
    /// where `cap_req` is compiled as a simple ARRAY mailbox, the handle is
    /// created in **compat mode** (`is_compat() == true`) — callers should fall
    /// back to `Loader::reload_policy_direct()` for policy updates.
    pub fn reload_handle(&self) -> io::Result<ReloadHandle> {
        let map = self
            .bpf
            .map("cap_req")
            .ok_or_else(|| err("cap_req missing"))?;
        let cap_req_fd = match map {
            Map::Unsupported(data) => {
                let raw = data.fd().as_fd().as_raw_fd();
                Some(dup_cloexec_fd(raw)?)
            }
            Map::Array(_) => {
                // 5.10 compat: cap_req was compiled as BPF_MAP_TYPE_ARRAY.
                None
            }
            _ => return Err(err("cap_req is neither a user ringbuf nor an array map")),
        };
        Ok(ReloadHandle {
            cap_req_fd,
            cap_task_fd: dup_hash_map_fd(&self.bpf, "cap_task")?,
            cap_state_fd: dup_hash_map_fd(&self.bpf, "cap_state")?,
            cap_policy_fd: dup_hash_map_fd(&self.bpf, "cap_policy")?,
            ts_counts_fd: dup_array_map_fd(&self.bpf, "ts_counts")?,
            append_lock: Mutex::new(()),
            policy_features: self.policy_features,
        })
    }

    /// Create a map-fd backed domain control handle.
    ///
    /// This handle can be shared with a control plane while `Loader::run` polls
    /// the ring buffer, which is how MCP can bind subagent pids without stopping
    /// enforcement.
    pub fn domain_handle(&self) -> io::Result<DomainHandle> {
        Ok(DomainHandle {
            cap_task_fd: dup_hash_map_fd(&self.bpf, "cap_task")?,
            cap_state_fd: dup_hash_map_fd(&self.bpf, "cap_state")?,
            ts_proc_domains_fd: dup_hash_map_fd(&self.bpf, "ts_proc_domains")?,
            ts_root_fd: dup_hash_map_fd(&self.bpf, "ts_root")?,
        })
    }

    /// Create a child runtime domain and bind a pid using the current loader.
    pub fn bind_child_domain(&self, spec: ChildDomainSpec) -> io::Result<()> {
        self.domain_handle()?.bind_child_domain(spec)
    }

    fn seed_global_proc_state(&mut self, pid: i32, label: u64) -> io::Result<()> {
        if pid <= 0 || label == 0 {
            return Err(err("pid and label must both be set"));
        }

        let mut proc: HashMap<_, i32, ProcState> = HashMap::try_from(
            self.bpf
                .map_mut("ts_proc")
                .ok_or_else(|| err("ts_proc missing"))?,
        )
        .map_err(|e| err(format!("ts_proc: {e}")))?;
        proc.insert(
            pid,
            ProcState {
                labels: label,
                lin_gates: 0,
            },
            0,
        )
        .map_err(|e| err(format!("seed ts_proc: {e}")))?;

        let mut root: HashMap<_, i32, i32> = HashMap::try_from(
            self.bpf
                .map_mut("ts_root")
                .ok_or_else(|| err("ts_root missing"))?,
        )
        .map_err(|e| err(format!("ts_root: {e}")))?;
        root.insert(pid, pid, 0)
            .map_err(|e| err(format!("seed ts_root: {e}")))?;
        Ok(())
    }

    /// Seed `pid` and its future descendants with an initial global label, but
    /// do not create a mutable runtime domain for policy deltas.
    pub fn seed_global_label(&mut self, pid: i32, label: u64) -> io::Result<()> {
        self.seed_global_proc_state(pid, label)?;
        let mut pid_map: HashMap<_, i32, u32> = HashMap::try_from(
            self.bpf
                .map_mut("cap_task")
                .ok_or_else(|| err("cap_task missing"))?,
        )
        .map_err(|e| err(format!("cap_task: {e}")))?;
        pid_map
            .insert(pid, GLOBAL_ACTIVE_DOMAIN_ID, 0)
            .map_err(|e| err(format!("seed cap_task: {e}")))?;
        Ok(())
    }

    /// Seed `pid` and its future descendants with an initial label.
    pub fn seed_label(&mut self, pid: i32, label: u64) -> io::Result<()> {
        self.seed_global_proc_state(pid, label)?;
        {
            let mut agent_roots: HashMap<_, i32, u8> = HashMap::try_from(
                self.bpf
                    .map_mut("ts_agent_roots")
                    .ok_or_else(|| err("ts_agent_roots missing"))?,
            )
            .map_err(|e| err(format!("ts_agent_roots: {e}")))?;
            agent_roots
                .insert(pid, 1u8, 0)
                .map_err(|e| err(format!("register agent root: {e}")))?;
        }
        self.bind_state(
            pid,
            pid as u32,
            CapState {
                scope_id: 1,
                labels: label,
                authority_mask: AUTH_BIND_RULE
                    | AUTH_NARROW_SCOPE
                    | AUTH_ADD_LABEL
                    | AUTH_REQUIRE_GATE
                    | AUTH_DECLASSIFY
                    | AUTH_DELEGATE,
                target_mask: TARGET_SELF | TARGET_CHILD,
                gate_mask: u64::MAX,
                label_mask: u64::MAX,
                ..CapState::default()
            },
        )?;
        Ok(())
    }

    /// After `seed_label(root_pid, ..)`, walk the existing descendant process
    /// tree under `root_pid` and write BPF maps for each live child so that
    /// processes forked *before* seed are also visible to enforcement.
    ///
    /// Writes per descendant:
    ///   - `cap_task[child]`  = domain_id (root_pid as u32)
    ///   - `ts_proc[child]`  = ProcState { labels, lin_gates: 0 }
    ///   - `ts_root[child]`  = root_pid
    ///
    /// Races (child exiting mid-scan) are tolerated: errors are logged and
    /// skipped. Recursion is capped at 32 levels.
    pub fn seed_descendants(&mut self, root_pid: i32, label: u64) -> io::Result<()> {
        let descendants = Self::get_descendants(root_pid, 0, 32);
        if descendants.is_empty() {
            return Ok(());
        }

        let domain_id = root_pid as u32;

        // Each map borrow requires exclusive access to self.bpf, so we process
        // them in separate scopes.
        {
            let mut cap_task_map: HashMap<_, i32, u32> = HashMap::try_from(
                self.bpf
                    .map_mut("cap_task")
                    .ok_or_else(|| err("cap_task missing"))?,
            )
            .map_err(|e| err(format!("cap_task: {e}")))?;
            for &cpid in &descendants {
                let _ = cap_task_map.insert(cpid, domain_id, 0);
            }
        }
        {
            let mut proc_map: HashMap<_, i32, ProcState> = HashMap::try_from(
                self.bpf
                    .map_mut("ts_proc")
                    .ok_or_else(|| err("ts_proc missing"))?,
            )
            .map_err(|e| err(format!("ts_proc: {e}")))?;
            for &cpid in &descendants {
                let _ = proc_map.insert(
                    cpid,
                    ProcState {
                        labels: label,
                        lin_gates: 0,
                    },
                    0,
                );
            }
        }
        {
            let mut root_map: HashMap<_, i32, i32> = HashMap::try_from(
                self.bpf
                    .map_mut("ts_root")
                    .ok_or_else(|| err("ts_root missing"))?,
            )
            .map_err(|e| err(format!("ts_root: {e}")))?;
            for &cpid in &descendants {
                let _ = root_map.insert(cpid, root_pid, 0);
            }
        }

        Ok(())
    }

    /// Recursively collect descendant PIDs by reading `/proc/{pid}/task/{tid}/children`.
    /// `depth` tracks current recursion depth; `max_depth` caps it.
    fn get_descendants(pid: i32, depth: u32, max_depth: u32) -> Vec<i32> {
        if depth >= max_depth {
            return Vec::new();
        }
        let mut result = Vec::new();
        let task_dir = format!("/proc/{}/task", pid);
        let entries = match std::fs::read_dir(&task_dir) {
            Ok(e) => e,
            Err(_) => return result,
        };
        for entry in entries.flatten() {
            let children_path = entry.path().join("children");
            let content = match std::fs::read_to_string(&children_path) {
                Ok(c) => c,
                Err(_) => continue,
            };
            for child_str in content.split_whitespace() {
                if let Ok(child_pid) = child_str.parse::<i32>() {
                    if child_pid > 0 {
                        result.push(child_pid);
                        result.extend(Self::get_descendants(child_pid, depth + 1, max_depth));
                    }
                }
            }
        }
        result
    }

    /// Bind a Linux pid to an engine state id.
    ///
    /// Binding is also a runtime domain boundary. Reset the pid's dynamic
    /// process labels in the target domain so labels inherited from a previous
    /// domain cannot be reinterpreted by the newly bound domain's local rules.
    /// Static initial labels live in `cap_state.labels`.
    pub fn bind_state(&mut self, pid: i32, target_id: u32, state: CapState) -> io::Result<()> {
        if pid <= 0 || target_id == 0 {
            return Err(err("pid and target id must both be set"));
        }
        {
            let mut states: HashMap<_, u32, CapState> = HashMap::try_from(
                self.bpf
                    .map_mut("cap_state")
                    .ok_or_else(|| err("cap_state missing"))?,
            )
            .map_err(|e| err(format!("cap_state: {e}")))?;
            states
                .insert(target_id, state, 0)
                .map_err(|e| err(format!("seed cap_state: {e}")))?;
        }
        {
            let mut proc: HashMap<_, PidDomainKey, ProcState> = HashMap::try_from(
                self.bpf
                    .map_mut("ts_proc_domains")
                    .ok_or_else(|| err("ts_proc_domains missing"))?,
            )
            .map_err(|e| err(format!("ts_proc_domains: {e}")))?;
            proc.insert(
                PidDomainKey {
                    pid,
                    domain_id: target_id,
                },
                ProcState {
                    labels: 0,
                    lin_gates: 0,
                },
                0,
            )
            .map_err(|e| err(format!("bind ts_proc_domains: {e}")))?;
        }
        {
            let mut pid_map: HashMap<_, i32, u32> = HashMap::try_from(
                self.bpf
                    .map_mut("cap_task")
                    .ok_or_else(|| err("cap_task missing"))?,
            )
            .map_err(|e| err(format!("cap_task: {e}")))?;
            pid_map
                .insert(pid, target_id, 0)
                .map_err(|e| err(format!("seed cap_task: {e}")))?;
        }
        Ok(())
    }

    /// Submit a runtime policy delta through the user-to-kernel ring buffer.
    ///
    /// The BPF side admits the request only if `caller_pid` maps to a state with
    /// the needed authority masks, and then applies a monotonic delta to
    /// `cap_state`. The caller normally sets `caller_pid` to its own pid; this
    /// method triggers a `getpid` syscall so the BPF drain hook runs.
    pub fn submit_delta(&self, req: DeltaRequest) -> io::Result<()> {
        let map = self
            .bpf
            .map("cap_req")
            .ok_or_else(|| err("cap_req missing"))?;
        let map_data = match map {
            Map::Unsupported(data) => data,
            _ => return Err(err("cap_req is not a user ringbuf map")),
        };
        let fd = map_data.fd().as_fd().as_raw_fd();
        unsafe {
            let rb = libbpf_sys::user_ring_buffer__new(fd, std::ptr::null());
            if rb.is_null() {
                return Err(io::Error::last_os_error());
            }
            let sample = libbpf_sys::user_ring_buffer__reserve(
                rb,
                std::mem::size_of::<DeltaRequest>() as u32,
            );
            if sample.is_null() {
                let e = io::Error::last_os_error();
                libbpf_sys::user_ring_buffer__free(rb);
                return Err(e);
            }
            std::ptr::copy_nonoverlapping(
                &req as *const DeltaRequest as *const u8,
                sample as *mut u8,
                std::mem::size_of::<DeltaRequest>(),
            );
            libbpf_sys::user_ring_buffer__submit(rb, sample);
            libbpf_sys::user_ring_buffer__free(rb);
            libc::syscall(libc::SYS_getpid);
        }
        Ok(())
    }

    /// Drain all pending events from the persistent ringbuf without blocking.
    pub fn drain_ringbuf(&mut self, mut on: impl FnMut(Violation)) {
        let ring = match self.ringbuf.as_mut() {
            Some(r) => r,
            None => return,
        };

        const DRAIN_LIMIT: usize = 65_536;
        let mut count = 0usize;
        while let Some(item) = ring.next() {
            count += 1;
            if count > DRAIN_LIMIT {
                break;
            }
            let bytes: &[u8] = &item;
            if bytes.len() < std::mem::size_of::<Event>() {
                continue;
            }
            let e: Event = unsafe { std::ptr::read_unaligned(bytes.as_ptr() as *const Event) };
            if e.etype != EVENT_TYPE_TAINT_VIOLATION {
                continue;
            }
            on(decode(&e));
        }
    }

    /// Poll the ring buffer until `stop` is set, delivering each violation.
    /// Uses the persistent `self.ringbuf` created once at `Loader::load()`.
    pub fn run(&mut self, stop: &AtomicBool, mut on: impl FnMut(Violation)) -> io::Result<()> {
        const MAX_ITEMS_PER_POLL: usize = 4096;
        const DEDUP_WINDOW: usize = 8192;

        let ring = self
            .ringbuf
            .as_mut()
            .ok_or_else(|| err("rb ringbuf not initialised (take_map failed at load?)"))?;
        let fd = ring.as_raw_fd();

        let mut dedup = EventDedup::new(DEDUP_WINDOW);

        while !stop.load(Ordering::Relaxed) {
            let mut pfd = libc::pollfd {
                fd,
                events: libc::POLLIN,
                revents: 0,
            };
            let r = unsafe { libc::poll(&mut pfd, 1, 100) };
            if r < 0 {
                let e = io::Error::last_os_error();
                if e.kind() == io::ErrorKind::Interrupted {
                    continue;
                }
                return Err(e);
            }
            let mut batch = 0usize;
            while let Some(item) = ring.next() {
                batch += 1;
                if batch > MAX_ITEMS_PER_POLL {
                    break;
                }
                let bytes: &[u8] = &item;
                if bytes.len() < std::mem::size_of::<Event>() {
                    continue;
                }
                let e: Event = unsafe { std::ptr::read_unaligned(bytes.as_ptr() as *const Event) };
                if e.etype != EVENT_TYPE_TAINT_VIOLATION {
                    continue;
                }
                if !dedup.admit((e.pid, e.timestamp_ns, e.effect)) {
                    continue;
                }
                on(decode(&e));
            }
        }
        Ok(())
    }

    /// Return the lowest label-bit assigned to an exec-source whose target
    /// equals `command`.
    pub fn scope_bit_for_command(&self, command: &str) -> Option<u8> {
        for (target, add) in &self.exec_sources {
            if target == command {
                if *add == 0 {
                    return None;
                }
                return Some(add.trailing_zeros() as u8);
            }
        }
        None
    }

    /// Snapshot of all `op=EXEC` source entries compiled into the policy.
    pub fn exec_sources_snapshot(&self) -> Vec<(String, u8)> {
        self.exec_sources
            .iter()
            .filter_map(|(target, add)| {
                if *add == 0 {
                    None
                } else {
                    Some((target.clone(), add.trailing_zeros() as u8))
                }
            })
            .collect()
    }

    /// Read the current taint label for `pid` directly from the BPF map.
    pub fn query_proc_label(&mut self, pid: i32) -> io::Result<u64> {
        let proc_map: HashMap<_, i32, ProcState> = HashMap::try_from(
            self.bpf
                .map_mut("ts_proc")
                .ok_or_else(|| err("ts_proc missing"))?,
        )
        .map_err(|e| err(format!("ts_proc: {e}")))?;
        match proc_map.get(&pid, 0) {
            Ok(state) => Ok(state.labels),
            Err(e) => {
                let msg = e.to_string();
                if msg.contains("ENOENT")
                    || msg.contains("No such file")
                    || msg.contains("not found")
                {
                    Ok(0)
                } else {
                    Err(err(format!("ts_proc lookup: {e}")))
                }
            }
        }
    }

    /// Remove a pid from the ts_agent_roots BPF map (called on deactivation).
    pub fn unseed_agent_root(&mut self, pid: i32) -> io::Result<()> {
        if pid <= 0 {
            return Ok(());
        }
        let mut agent_roots: HashMap<_, i32, u8> = HashMap::try_from(
            self.bpf
                .map_mut("ts_agent_roots")
                .ok_or_else(|| err("ts_agent_roots missing"))?,
        )
        .map_err(|e| err(format!("ts_agent_roots: {e}")))?;
        let _ = agent_roots.remove(&pid); // ignore ENOENT
        Ok(())
    }

    /// Direct-map hot-reload for 5.10-compat mode (no ring buffer).
    pub fn reload_policy_direct(&mut self, new_blob: &[u8]) -> io::Result<()> {
        if new_blob.len() != std::mem::size_of::<CConfig>() {
            return Err(err(format!(
                "reload_policy_direct config size mismatch: got {}, expected {}",
                new_blob.len(),
                std::mem::size_of::<CConfig>()
            )));
        }
        let cfg: Box<CConfig> =
            Box::new(unsafe { std::ptr::read_unaligned(new_blob.as_ptr() as *const CConfig) });
        validate_config(&cfg)?;

        // Phase 1: quiesce
        {
            let mut counts: Array<_, u32> = Array::try_from(
                self.bpf
                    .map_mut("ts_counts")
                    .ok_or_else(|| err("map ts_counts missing"))?,
            )
            .map_err(|e| err(format!("ts_counts: {e}")))?;
            counts
                .set(0, 0u32, 0)
                .map_err(|e| err(format!("quiesce rules: {e}")))?;
            counts
                .set(1, 0u32, 0)
                .map_err(|e| err(format!("quiesce updates: {e}")))?;
        }

        // Phase 2: write updates
        populate_update_map(&mut self.bpf, &cfg)?;

        // Phase 3: write rules
        populate_rule_map(&mut self.bpf, &cfg)?;

        // Phase 4: activate
        {
            let mut counts: Array<_, u32> = Array::try_from(
                self.bpf
                    .map_mut("ts_counts")
                    .ok_or_else(|| err("map ts_counts missing"))?,
            )
            .map_err(|e| err(format!("ts_counts: {e}")))?;
            counts
                .set(0, cfg.n_rules, 0)
                .map_err(|e| err(format!("activate rules: {e}")))?;
            counts
                .set(1, cfg.n_updates, 0)
                .map_err(|e| err(format!("activate updates: {e}")))?;
        }

        // Phase 4.5: remap process labels
        {
            let old_bit_map = &self.source_bit_map;
            let new_bit_map = build_full_bit_map(&cfg);

            let mut bit_remap: StdHashMap<u8, Option<u8>> = StdHashMap::new();
            for (source_key, old_bit) in old_bit_map {
                match new_bit_map.get(source_key) {
                    Some(&new_bit) if new_bit != *old_bit => {
                        bit_remap.insert(*old_bit, Some(new_bit));
                    }
                    None => {
                        bit_remap.insert(*old_bit, None);
                    }
                    _ => {}
                }
            }

            if !bit_remap.is_empty() {
                let mut proc_map: HashMap<_, i32, ProcState> = HashMap::try_from(
                    self.bpf
                        .map_mut("ts_proc")
                        .ok_or_else(|| err("ts_proc missing"))?,
                )
                .map_err(|e| err(format!("ts_proc for remapping: {e}")))?;

                let mut updates: Vec<(i32, ProcState)> = Vec::new();
                for result in proc_map.iter() {
                    if let Ok((pid, state)) = result {
                        let new_labels = remap_labels(state.labels, &bit_remap);
                        if new_labels != state.labels {
                            updates.push((
                                pid,
                                ProcState {
                                    labels: new_labels,
                                    lin_gates: state.lin_gates,
                                },
                            ));
                        }
                    }
                }

                for (pid, new_state) in updates {
                    let _ = proc_map.insert(pid, new_state, 0);
                }
            }
        }

        // Phase 5: Update cached exec_sources and source_bit_map
        let n = (cfg.n_updates as usize).min(MAX_UPDATES);
        self.exec_sources.clear();
        for i in 0..n {
            let u = &cfg.updates[i];
            if u.op != OP_EXEC {
                continue;
            }
            let raw: &[u8] = &u.target;
            let end = raw.iter().position(|&b| b == 0).unwrap_or(raw.len());
            if let Ok(s) = std::str::from_utf8(&raw[..end]) {
                self.exec_sources.push((s.to_string(), u.add));
            }
        }
        self.source_bit_map = build_full_bit_map(&cfg);

        Ok(())
    }

    /// Remap all process labels in ts_proc based on the difference between
    /// old and new policy blobs.
    pub fn remap_proc_labels(&mut self, old_blob: &[u8], new_blob: &[u8]) -> io::Result<()> {
        if old_blob.len() != std::mem::size_of::<CConfig>() {
            return Err(err(format!(
                "remap_proc_labels old config size mismatch: got {}, expected {}",
                old_blob.len(),
                std::mem::size_of::<CConfig>()
            )));
        }
        if new_blob.len() != std::mem::size_of::<CConfig>() {
            return Err(err(format!(
                "remap_proc_labels new config size mismatch: got {}, expected {}",
                new_blob.len(),
                std::mem::size_of::<CConfig>()
            )));
        }
        let old_cfg: Box<CConfig> =
            Box::new(unsafe { std::ptr::read_unaligned(old_blob.as_ptr() as *const CConfig) });
        let new_cfg: Box<CConfig> =
            Box::new(unsafe { std::ptr::read_unaligned(new_blob.as_ptr() as *const CConfig) });

        let old_bit_map = build_full_bit_map(&old_cfg);
        let new_bit_map = build_full_bit_map(&new_cfg);

        let mut bit_remap: StdHashMap<u8, Option<u8>> = StdHashMap::new();
        for (source_key, old_bit) in &old_bit_map {
            match new_bit_map.get(source_key) {
                Some(&new_bit) if new_bit != *old_bit => {
                    bit_remap.insert(*old_bit, Some(new_bit));
                }
                None => {
                    bit_remap.insert(*old_bit, None);
                }
                _ => {}
            }
        }

        if bit_remap.is_empty() {
            return Ok(());
        }

        let mut proc_map: HashMap<_, i32, ProcState> = HashMap::try_from(
            self.bpf
                .map_mut("ts_proc")
                .ok_or_else(|| err("ts_proc missing"))?,
        )
        .map_err(|e| err(format!("ts_proc for remapping: {e}")))?;

        let mut updates: Vec<(i32, ProcState)> = Vec::new();
        for result in proc_map.iter() {
            if let Ok((pid, state)) = result {
                let new_labels = remap_labels(state.labels, &bit_remap);
                if new_labels != state.labels {
                    updates.push((
                        pid,
                        ProcState {
                            labels: new_labels,
                            lin_gates: state.lin_gates,
                        },
                    ));
                }
            }
        }

        for (pid, new_state) in updates {
            let _ = proc_map.insert(pid, new_state, 0);
        }

        self.source_bit_map = new_bit_map;

        Ok(())
    }
}

// ── Map population helpers ──────────────────────────────────────────

fn populate_update_map(bpf: &mut Ebpf, cfg: &CConfig) -> io::Result<()> {
    let mut updates_map: Array<_, CUpdate> = Array::try_from(
        bpf.map_mut("ts_updates")
            .ok_or_else(|| err("map ts_updates missing"))?,
    )
    .map_err(|e| err(format!("ts_updates: {e}")))?;
    for i in 0..cfg.n_updates as usize {
        updates_map
            .set(i as u32, cfg.updates[i], 0)
            .map_err(|e| err(format!("ts_updates[{i}]: {e}")))?;
    }
    Ok(())
}

fn populate_rule_map(bpf: &mut Ebpf, cfg: &CConfig) -> io::Result<()> {
    let mut rules_map: Array<_, CRule> = Array::try_from(
        bpf.map_mut("ts_rules")
            .ok_or_else(|| err("map ts_rules missing"))?,
    )
    .map_err(|e| err(format!("ts_rules: {e}")))?;
    for i in 0..cfg.n_rules as usize {
        rules_map
            .set(i as u32, cfg.rules[i], 0)
            .map_err(|e| err(format!("ts_rules[{i}]: {e}")))?;
    }
    Ok(())
}

fn policy_mask_set(mask: &mut CapPolicyMask, idx: usize) {
    if idx < 64 {
        mask.lo |= 1u64 << idx;
    } else {
        mask.hi |= 1u64 << (idx - 64);
    }
}

fn populate_policy_mask_map(bpf: &mut Ebpf, cfg: &CConfig) -> io::Result<()> {
    let mut masks: Vec<(u32, CapPolicyMask)> = Vec::new();
    for i in 0..cfg.n_rules as usize {
        let domain_id = cfg.rules[i].domain_id;
        if let Some((_, mask)) = masks.iter_mut().find(|(id, _)| *id == domain_id) {
            policy_mask_set(mask, i);
        } else {
            let mut mask = CapPolicyMask::default();
            policy_mask_set(&mut mask, i);
            masks.push((domain_id, mask));
        }
    }

    let mut policy_map: HashMap<_, u32, CapPolicyMask> = HashMap::try_from(
        bpf.map_mut("cap_policy")
            .ok_or_else(|| err("map cap_policy missing"))?,
    )
    .map_err(|e| err(format!("cap_policy: {e}")))?;
    for (domain_id, mask) in masks {
        policy_map
            .insert(domain_id, mask, 0)
            .map_err(|e| err(format!("cap_policy[{domain_id}]: {e}")))?;
    }
    Ok(())
}

// ── Hot-reload via cap_req ring buffer ─────────────────────────────

const CAP_REQ_RELOAD_UPDATE: i32 = -1;
const CAP_REQ_RELOAD_RULE: i32 = -2;
const CAP_REQ_RELOAD_COUNTS: i32 = -3;
const CAP_REQ_APPEND_UPDATE: i32 = -4;
const CAP_REQ_APPEND_RULE: i32 = -5;

#[repr(C)]
#[derive(Clone, Copy)]
struct ReloadUpdate {
    tag: i32,
    index: u32,
    entry: CUpdate,
}

#[repr(C)]
#[derive(Clone, Copy)]
struct ReloadRule {
    tag: i32,
    index: u32,
    entry: CRule,
}

#[repr(C)]
#[derive(Clone, Copy)]
struct ReloadCounts {
    tag: i32,
    n_rules: u32,
    n_updates: u32,
    _pad: u32,
}

#[repr(C)]
#[derive(Clone, Copy)]
struct AppendUpdate {
    tag: i32,
    caller_pid: i32,
    target_id: u32,
    new_scope_id: u32,
    required_mask: u64,
    entry: CUpdate,
}

#[repr(C)]
#[derive(Clone, Copy)]
struct AppendRule {
    tag: i32,
    caller_pid: i32,
    target_id: u32,
    new_scope_id: u32,
    required_mask: u64,
    entry: CRule,
}

/// A handle for hot-reloading policy rules into a running eBPF engine.
///
/// Holds only the `cap_req` user ring buffer fd (via a dup'd `OwnedFd`).
/// `Send + Sync` — safe to share across threads and the async MCP server.
pub struct ReloadHandle {
    cap_req_fd: Option<std::os::fd::OwnedFd>,
    cap_task_fd: std::os::fd::OwnedFd,
    cap_state_fd: std::os::fd::OwnedFd,
    cap_policy_fd: std::os::fd::OwnedFd,
    ts_counts_fd: std::os::fd::OwnedFd,
    append_lock: Mutex<()>,
    policy_features: u32,
}

unsafe impl Send for ReloadHandle {}
unsafe impl Sync for ReloadHandle {}

impl ReloadHandle {
    /// Returns `true` when the handle was created from a 5.10-compat ARRAY
    /// `cap_req` map.  Callers should fall back to
    /// `Loader::reload_policy_direct()`.
    pub fn is_compat(&self) -> bool {
        self.cap_req_fd.is_none()
    }

    fn submit_raw(&self, data: &[u8]) -> io::Result<()> {
        let fd = self
            .cap_req_fd
            .as_ref()
            .ok_or_else(|| {
                err("hot-reload via cap_req ringbuf not available on this kernel (5.10 compat)")
            })?
            .as_raw_fd();
        unsafe {
            let rb = libbpf_sys::user_ring_buffer__new(fd, std::ptr::null());
            if rb.is_null() {
                return Err(io::Error::last_os_error());
            }
            let sample = libbpf_sys::user_ring_buffer__reserve(rb, data.len() as u32);
            if sample.is_null() {
                let e = io::Error::last_os_error();
                libbpf_sys::user_ring_buffer__free(rb);
                return Err(e);
            }
            std::ptr::copy_nonoverlapping(data.as_ptr(), sample as *mut u8, data.len());
            libbpf_sys::user_ring_buffer__submit(rb, sample);
            libbpf_sys::user_ring_buffer__free(rb);
            libc::syscall(libc::SYS_getpid);
        }
        Ok(())
    }

    fn submit<T: Copy>(&self, val: &T) -> io::Result<()> {
        let bytes = unsafe {
            std::slice::from_raw_parts(val as *const T as *const u8, std::mem::size_of::<T>())
        };
        self.submit_raw(bytes)
    }

    fn count_slot(&self, slot: u32) -> io::Result<u32> {
        let mut value = 0u32;
        let rc = unsafe {
            libbpf_sys::bpf_map_lookup_elem(
                self.ts_counts_fd.as_raw_fd(),
                &slot as *const u32 as *const std::ffi::c_void,
                &mut value as *mut u32 as *mut std::ffi::c_void,
            )
        };
        if rc != 0 {
            return Err(io::Error::last_os_error());
        }
        Ok(value)
    }

    fn set_count_slot(&self, slot: u32, value: u32) -> io::Result<()> {
        let rc = unsafe {
            libbpf_sys::bpf_map_update_elem(
                self.ts_counts_fd.as_raw_fd(),
                &slot as *const u32 as *const std::ffi::c_void,
                &value as *const u32 as *const std::ffi::c_void,
                BPF_ANY,
            )
        };
        if rc != 0 {
            return Err(io::Error::last_os_error());
        }
        Ok(())
    }

    fn cap_policy_mask(&self, domain_id: u32) -> io::Result<Option<CapPolicyMask>> {
        let mut value = CapPolicyMask::default();
        let rc = unsafe {
            libbpf_sys::bpf_map_lookup_elem(
                self.cap_policy_fd.as_raw_fd(),
                &domain_id as *const u32 as *const std::ffi::c_void,
                &mut value as *mut CapPolicyMask as *mut std::ffi::c_void,
            )
        };
        if rc == 0 {
            return Ok(Some(value));
        }
        let e = io::Error::last_os_error();
        if e.raw_os_error() == Some(libc::ENOENT) {
            return Ok(None);
        }
        Err(e)
    }

    fn restore_cap_policy_mask(
        &self,
        domain_id: u32,
        before: Option<CapPolicyMask>,
    ) -> io::Result<()> {
        let rc = unsafe {
            match before {
                Some(mask) => libbpf_sys::bpf_map_update_elem(
                    self.cap_policy_fd.as_raw_fd(),
                    &domain_id as *const u32 as *const std::ffi::c_void,
                    &mask as *const CapPolicyMask as *const std::ffi::c_void,
                    BPF_ANY,
                ),
                None => libbpf_sys::bpf_map_delete_elem(
                    self.cap_policy_fd.as_raw_fd(),
                    &domain_id as *const u32 as *const std::ffi::c_void,
                ),
            }
        };
        if rc != 0 {
            let e = io::Error::last_os_error();
            if before.is_none() && e.raw_os_error() == Some(libc::ENOENT) {
                return Ok(());
            }
            return Err(e);
        }
        Ok(())
    }

    fn restore_append_state(
        &self,
        target_id: u32,
        rules_before: u32,
        updates_before: u32,
        policy_before: Option<CapPolicyMask>,
    ) -> io::Result<()> {
        let mut first_err = None;
        if let Err(e) = self.set_count_slot(0, rules_before) {
            first_err.get_or_insert(e);
        }
        if let Err(e) = self.set_count_slot(1, updates_before) {
            first_err.get_or_insert(e);
        }
        if let Err(e) = self.restore_cap_policy_mask(target_id, policy_before) {
            first_err.get_or_insert(e);
        }
        if let Some(e) = first_err {
            return Err(e);
        }
        Ok(())
    }

    fn submit_expect_count<T: Copy>(
        &self,
        val: &T,
        count_slot: u32,
        before: u32,
        what: &str,
    ) -> io::Result<()> {
        self.submit(val)?;
        for _ in 0..10 {
            let after = self.count_slot(count_slot)?;
            if after == before + 1 {
                return Ok(());
            }
            if after != before {
                return Err(err(format!(
                    "{what} changed count from {before} to {after}, expected {}",
                    before + 1
                )));
            }
            std::thread::sleep(std::time::Duration::from_millis(1));
        }
        Err(err(format!(
            "{what} was not admitted by the kernel; count remained {before}"
        )))
    }

    pub fn domain_for_pid(&self, pid: i32) -> io::Result<Option<u32>> {
        let tasks: HashMap<_, i32, u32> = hash_map_from_fd(&self.cap_task_fd)?;
        map_get_optional(&tasks, &pid, "lookup pid domain")
    }

    fn append_target_mask(caller_domain: u32, target_id: u32, target_state: &CapState) -> u64 {
        let mut mask = 0;
        if caller_domain == target_id {
            mask |= TARGET_SELF;
        }
        if target_state.parent == caller_domain {
            mask |= TARGET_CHILD;
        }
        mask
    }

    fn precheck_append_authority(
        &self,
        caller_pid: i32,
        target_id: u32,
        required_mask: u64,
        add_label_mask: u64,
        del_label_mask: u64,
        gate_mask: u64,
    ) -> io::Result<()> {
        let tasks: HashMap<_, i32, u32> = hash_map_from_fd(&self.cap_task_fd)?;
        let states: HashMap<_, u32, CapState> = hash_map_from_fd(&self.cap_state_fd)?;
        let caller_domain = map_get(&tasks, &caller_pid, "lookup caller domain")?;
        let source = map_get(&states, &caller_domain, "lookup caller cap_state")?;
        let target = map_get(&states, &target_id, "lookup target cap_state")?;
        let target_mask = Self::append_target_mask(caller_domain, target_id, &target);
        if target_mask & source.target_mask == 0 {
            return Err(err(format!(
                "caller domain {caller_domain} cannot target runtime domain {target_id}"
            )));
        }

        let mut needed = required_mask;
        if add_label_mask != 0 {
            needed |= AUTH_ADD_LABEL;
        }
        if del_label_mask != 0 {
            needed |= AUTH_DECLASSIFY;
        }
        if gate_mask != 0 {
            needed |= AUTH_REQUIRE_GATE;
        }
        if needed & !source.authority_mask != 0 {
            return Err(err(format!(
                "caller domain {caller_domain} lacks runtime authority 0x{:x}",
                needed & !source.authority_mask
            )));
        }
        let label_bits = add_label_mask | del_label_mask;
        if label_bits & !source.label_mask != 0 {
            return Err(err(format!(
                "caller domain {caller_domain} lacks label authority 0x{:x}",
                label_bits & !source.label_mask
            )));
        }
        Ok(())
    }

    /// Hot-reload a new compiled policy blob without restarting the engine.
    ///
    /// Sequence: quiesce (counts→0) → write updates → write rules → activate.
    /// Accumulated state (process labels, file labels, session gates) is preserved.
    pub fn reload_policy(&self, new_blob: &[u8]) -> io::Result<()> {
        if new_blob.len() != std::mem::size_of::<CConfig>() {
            return Err(err(format!(
                "reload config size mismatch: got {}, expected {}",
                new_blob.len(),
                std::mem::size_of::<CConfig>()
            )));
        }
        let cfg: Box<CConfig> =
            Box::new(unsafe { std::ptr::read_unaligned(new_blob.as_ptr() as *const CConfig) });
        validate_config(&cfg)?;
        validate_supported_features(&cfg, self.policy_features, "reload policy")?;
        let _guard = self
            .append_lock
            .lock()
            .map_err(|e| err(format!("reload lock poisoned: {e}")))?;

        // Phase 1: quiesce — set counts to 0 so the engine skips all rules.
        self.submit(&ReloadCounts {
            tag: CAP_REQ_RELOAD_COUNTS,
            n_rules: 0,
            n_updates: 0,
            _pad: 0,
        })?;

        // Phase 2: submit all update entries.
        for i in 0..cfg.n_updates {
            self.submit(&ReloadUpdate {
                tag: CAP_REQ_RELOAD_UPDATE,
                index: i,
                entry: cfg.updates[i as usize],
            })?;
        }

        // Phase 3: submit all rule entries.
        for i in 0..cfg.n_rules {
            self.submit(&ReloadRule {
                tag: CAP_REQ_RELOAD_RULE,
                index: i,
                entry: cfg.rules[i as usize],
            })?;
        }

        // Phase 4: activate — set real counts.
        self.submit(&ReloadCounts {
            tag: CAP_REQ_RELOAD_COUNTS,
            n_rules: cfg.n_rules,
            n_updates: cfg.n_updates,
            _pad: 0,
        })?;

        Ok(())
    }

    /// Append a precompiled policy delta through the kernel-admitted runtime path.
    ///
    /// Unlike `reload_policy`, this does not replace existing rules. Each update
    /// and rule is admitted by the BPF capability checker using the submitting
    /// pid's bound state. Updates that delete labels require `AUTH_DECLASSIFY`
    /// and label authority over every deleted bit, so runtime declassification is
    /// domain-local instead of a way to clear inherited higher-authority labels.
    pub fn append_policy_delta(
        &self,
        caller_pid: i32,
        target_id: u32,
        delta_blob: &[u8],
    ) -> io::Result<()> {
        self.append_policy_delta_with_rule_id_base(caller_pid, target_id, 0, delta_blob)
    }

    /// Same as `append_policy_delta`, but offsets appended rule ids before
    /// submission so userspace metadata can remain aligned with kernel events.
    pub fn append_policy_delta_with_rule_id_base(
        &self,
        caller_pid: i32,
        target_id: u32,
        rule_id_base: u32,
        delta_blob: &[u8],
    ) -> io::Result<()> {
        if caller_pid <= 0 || target_id == 0 {
            return Err(err("caller_pid and target_id must both be set"));
        }
        if delta_blob.len() != std::mem::size_of::<CConfig>() {
            return Err(err(format!(
                "delta config size mismatch: got {}, expected {}",
                delta_blob.len(),
                std::mem::size_of::<CConfig>()
            )));
        }
        let cfg: Box<CConfig> =
            Box::new(unsafe { std::ptr::read_unaligned(delta_blob.as_ptr() as *const CConfig) });
        validate_config(&cfg)?;
        validate_supported_features(&cfg, self.policy_features, "runtime policy delta")?;

        let _guard = self
            .append_lock
            .lock()
            .map_err(|e| err(format!("append lock poisoned: {e}")))?;
        let updates_before = self.count_slot(1)?;
        let rules_before = self.count_slot(0)?;
        if updates_before as usize + cfg.n_updates as usize > MAX_UPDATES {
            return Err(err(format!(
                "runtime policy delta has {} updates but only {} update slots remain",
                cfg.n_updates,
                MAX_UPDATES.saturating_sub(updates_before as usize)
            )));
        }
        if rules_before as usize + cfg.n_rules as usize > MAX_RULES {
            return Err(err(format!(
                "runtime policy delta has {} rules but only {} rule slots remain",
                cfg.n_rules,
                MAX_RULES.saturating_sub(rules_before as usize)
            )));
        }

        for i in 0..cfg.n_updates {
            let entry = cfg.updates[i as usize];
            self.precheck_append_authority(
                caller_pid,
                target_id,
                0,
                entry.add,
                entry.del,
                entry.gates | entry.invals,
            )
            .map_err(|e| err(format!("runtime policy delta update[{i}] rejected: {e}")))?;
        }

        for i in 0..cfg.n_rules {
            self.precheck_append_authority(caller_pid, target_id, AUTH_BIND_RULE, 0, 0, 0)
                .map_err(|e| err(format!("runtime policy delta rule[{i}] rejected: {e}")))?;
        }

        let policy_before = self.cap_policy_mask(target_id)?;
        let submitted = (|| -> io::Result<()> {
            for i in 0..cfg.n_updates {
                let entry = cfg.updates[i as usize];
                self.submit_expect_count(
                    &AppendUpdate {
                        tag: CAP_REQ_APPEND_UPDATE,
                        caller_pid,
                        target_id,
                        new_scope_id: 0,
                        required_mask: 0,
                        entry,
                    },
                    1,
                    updates_before + i,
                    &format!("runtime policy delta update[{i}]"),
                )?;
            }

            for i in 0..cfg.n_rules {
                let mut entry = cfg.rules[i as usize];
                entry.rule_id = entry.rule_id.saturating_add(rule_id_base);
                self.submit_expect_count(
                    &AppendRule {
                        tag: CAP_REQ_APPEND_RULE,
                        caller_pid,
                        target_id,
                        new_scope_id: 0,
                        required_mask: AUTH_BIND_RULE,
                        entry,
                    },
                    0,
                    rules_before + i,
                    &format!("runtime policy delta rule[{i}]"),
                )?;
            }
            Ok(())
        })();

        if let Err(e) = submitted {
            let rollback =
                self.restore_append_state(target_id, rules_before, updates_before, policy_before);
            if let Err(rollback_err) = rollback {
                return Err(err(format!(
                    "{e}; failed to roll back partial runtime policy delta: {rollback_err}"
                )));
            }
            return Err(err(format!(
                "{e}; rolled back partial runtime policy delta"
            )));
        }

        Ok(())
    }
}

fn cstr(buf: &[u8]) -> String {
    let end = buf.iter().position(|&b| b == 0).unwrap_or(buf.len());
    String::from_utf8_lossy(&buf[..end]).into_owned()
}

fn decode(e: &Event) -> Violation {
    let target = if e.conn_ip != 0 {
        let ip = e.conn_ip; // network order: bytes are a.b.c.d in memory
        format!(
            "{}.{}.{}.{}",
            ip & 0xff,
            (ip >> 8) & 0xff,
            (ip >> 16) & 0xff,
            (ip >> 24) & 0xff
        )
    } else {
        cstr(&e.filename)
    };
    let provenance = if e.prov_label != 0 {
        let target = if e.prov_ip != 0 {
            let ip = e.prov_ip;
            format!(
                "{}.{}.{}.{}",
                ip & 0xff,
                (ip >> 8) & 0xff,
                (ip >> 16) & 0xff,
                (ip >> 24) & 0xff
            )
        } else {
            cstr(&e.prov_target)
        };
        Some(Provenance {
            label: e.prov_label,
            timestamp_ns: e.prov_timestamp_ns,
            pid: e.prov_pid,
            op: e.prov_op,
            target,
        })
    } else {
        None
    };
    Violation {
        effect: e.effect,
        blocked: e.blocked != 0,
        killed: e.killed != 0,
        comm: cstr(&e.comm),
        pid: e.pid,
        ppid: e.ppid,
        target,
        rule_id: e.taint_rule_id,
        op: e.op,
        domain_id: e.domain_id,
        session_root: e.session_root,
        label: e.taint_label,
        matched_label: e.matched_label,
        matched_labels: e.matched_labels,
        provenance,
        timestamp_ns: e.timestamp_ns,
    }
}

/// Extract `(target_basename, bit_index)` pairs for every `op=EXEC` source
/// entry in a compiled policy blob. Pure function — no Loader, no kernel.
pub fn exec_sources_from_blob(blob: &[u8]) -> io::Result<Vec<(String, u8)>> {
    if blob.len() != std::mem::size_of::<CConfig>() {
        return Err(err(format!(
            "config size mismatch: got {}, expected {}",
            blob.len(),
            std::mem::size_of::<CConfig>()
        )));
    }
    let cfg: Box<CConfig> =
        Box::new(unsafe { std::ptr::read_unaligned(blob.as_ptr() as *const CConfig) });
    let n = (cfg.n_updates as usize).min(MAX_UPDATES);
    let mut out: Vec<(String, u8)> = Vec::with_capacity(n);
    for i in 0..n {
        let u = &cfg.updates[i];
        if u.op != OP_EXEC || u.add == 0 {
            continue;
        }
        let raw: &[u8] = &u.target;
        let end = raw.iter().position(|&b| b == 0).unwrap_or(raw.len());
        if let Ok(s) = std::str::from_utf8(&raw[..end]) {
            out.push((s.to_string(), u.add.trailing_zeros() as u8));
        }
    }
    Ok(out)
}

/// Sliding-window dedup for ringbuf events.
struct EventDedup {
    capacity: usize,
    set: HashSet<(i32, u64, u32)>,
    order: VecDeque<(i32, u64, u32)>,
}

impl EventDedup {
    fn new(capacity: usize) -> Self {
        Self {
            capacity,
            set: HashSet::with_capacity(capacity),
            order: VecDeque::with_capacity(capacity),
        }
    }

    fn admit(&mut self, key: (i32, u64, u32)) -> bool {
        if !self.set.insert(key) {
            return false;
        }
        self.order.push_back(key);
        if self.order.len() > self.capacity {
            if let Some(old) = self.order.pop_front() {
                self.set.remove(&old);
            }
        }
        true
    }
}

// ── Label remapping helpers ────────────────────────────────────────

/// Remap label bits according to a remapping table.
fn remap_labels(old_labels: u64, bit_remap: &StdHashMap<u8, Option<u8>>) -> u64 {
    let mut new_labels: u64 = 0;
    for old_bit in 0u8..64 {
        if (old_labels & (1u64 << old_bit)) == 0 {
            continue;
        }
        match bit_remap.get(&old_bit) {
            Some(Some(new_bit)) => {
                new_labels |= 1u64 << (*new_bit as u64);
            }
            Some(None) => {
                // deleted
            }
            None => {
                new_labels |= 1u64 << old_bit;
            }
        }
    }
    new_labels
}

/// Build a complete target→bit mapping from a CConfig (covering all op types).
fn build_full_bit_map(cfg: &CConfig) -> StdHashMap<String, u8> {
    let mut map = StdHashMap::new();
    let n = (cfg.n_updates as usize).min(MAX_UPDATES);
    for i in 0..n {
        let u = &cfg.updates[i];
        if u.add == 0 {
            continue;
        }
        let raw: &[u8] = &u.target;
        let end = raw.iter().position(|&b| b == 0).unwrap_or(raw.len());
        if let Ok(s) = std::str::from_utf8(&raw[..end]) {
            let key = format!("{}:{}", u.op, s);
            map.insert(key, u.add.trailing_zeros() as u8);
        }
    }
    map
}

#[cfg(test)]
mod tests {
    use super::*;

    const EFFECT_NOTIFY: u8 = 0;
    const EFFECT_KILL: u8 = 2;

    // The Rust ABI mirror must match the C struct sizes the object was built
    // with. These are the documented sizes from bpf/taint.h.
    #[test]
    fn abi_sizes() {
        assert_eq!(std::mem::size_of::<ProcState>(), 16);
        assert_eq!(std::mem::size_of::<CapState>(), 56);
        assert_eq!(std::mem::size_of::<DeltaRequest>(), 48);
        // CConfig mirrors bpf/taint.h exactly:
        // 8-byte header + 320 taint_update entries + 128 taint_rule entries.
        assert_eq!(std::mem::align_of::<CConfig>(), 8);
        assert_eq!(std::mem::size_of::<CUpdate>(), 144);
        assert_eq!(std::mem::size_of::<CRule>(), 224);
        assert_eq!(std::mem::size_of::<CConfig>(), 74_760);
        assert_eq!(std::mem::align_of::<Event>(), 8);
        assert_eq!(std::mem::size_of::<Event>(), 384);
        assert_eq!(
            std::mem::size_of::<CConfig>(),
            actplane_ifc_compiler::COMPILED_CONFIG_BLOB_SIZE,
            "engine/compiler CConfig size mismatch"
        );
    }

    #[test]
    fn duplicated_control_fds_are_close_on_exec() {
        let file = std::fs::File::open("/dev/null").expect("open /dev/null");
        let owned: OwnedFd = file.into();
        let dup = dup_owned_fd(&owned).expect("dup cloexec fd");

        let flags = unsafe { libc::fcntl(dup.as_raw_fd(), libc::F_GETFD) };
        assert!(flags >= 0, "F_GETFD failed: {}", io::Error::last_os_error());
        assert_ne!(
            flags & libc::FD_CLOEXEC,
            0,
            "duplicated control fd must not leak across exec"
        );
    }

    #[test]
    fn object_is_aligned_elf() {
        let b = object_bytes();
        assert_eq!(b.as_ptr() as usize % 8, 0, "object must be 8-aligned");
        assert_eq!(&b[..4], b"\x7fELF");
    }

    #[test]
    fn object_has_capability_user_ringbuf_path() {
        let b = object_bytes();
        for name in [
            b"cap_req".as_slice(),
            b"cap_state".as_slice(),
            b"cap_task".as_slice(),
            b"cap_policy".as_slice(),
            b"cap_drain_tick".as_slice(),
            b"trace_read".as_slice(),
            b"trace_read_exit".as_slice(),
            b"trace_write".as_slice(),
            b"trace_write_exit".as_slice(),
            b"trace_sendto".as_slice(),
            b"trace_recvfrom".as_slice(),
            b"trace_sendmsg".as_slice(),
            b"trace_recvmsg".as_slice(),
            b"trace_connect_exit".as_slice(),
            b"trace_close".as_slice(),
            b"trace_dup".as_slice(),
            b"trace_fcntl".as_slice(),
            b"trace_pipe".as_slice(),
            b"trace_socketpair".as_slice(),
            b"trace_bind".as_slice(),
            b"trace_accept".as_slice(),
            b"trace_sendfile64".as_slice(),
            b"trace_copy_file_range".as_slice(),
            b"trace_splice".as_slice(),
            b"enforce_socket_recvmsg".as_slice(),
            b"ts_fd".as_slice(),
            b"ts_sockfd".as_slice(),
            b"ts_connectpend".as_slice(),
            b"ts_iopend".as_slice(),
            b"ts_duppend".as_slice(),
            b"ts_fdcopypend".as_slice(),
            b"ts_pipepend".as_slice(),
            b"ts_socketpairpend".as_slice(),
            b"ts_unixsockpend".as_slice(),
            b"ts_acceptpend".as_slice(),
            b"stdio:stdin".as_slice(),
            b"stdio:stdout".as_slice(),
            b"ts_updates".as_slice(),
            b"ts_rules".as_slice(),
            b"ts_proc_domains".as_slice(),
            b"ts_exit_gates".as_slice(),
            b"exec_tail".as_slice(),
            b"exec_tp_update_simple".as_slice(),
            b"exec_tp_update_prefix".as_slice(),
            b"exec_tp_rule_simple".as_slice(),
            b"exec_tp_rule_complex".as_slice(),
        ] {
            assert!(
                b.windows(name.len()).any(|w| w == name),
                "object should contain {}",
                String::from_utf8_lossy(name)
            );
        }
    }

    #[test]
    fn default_hook_budget_keeps_advanced_tracepoints_off() {
        fn spec(name: &str) -> &'static TracepointSpec {
            TRACEPOINTS
                .iter()
                .find(|s| s.name == name)
                .expect("tracepoint spec")
        }

        let empty = HookBudget {
            features: 0,
            file_write: false,
            advanced_tracepoints: false,
        };
        assert!(tracepoint_needed(spec("handle_fork"), empty));
        assert!(!tracepoint_needed(spec("handle_exec"), empty));
        assert!(tracepoint_needed(spec("handle_exec_args"), empty));
        assert!(tracepoint_needed(spec("handle_exit"), empty));
        assert!(tracepoint_needed(spec("cap_drain_tick"), empty));
        assert!(!tracepoint_needed(spec("trace_openat"), empty));
        assert!(!tracepoint_needed(spec("trace_mmap"), empty));
        assert!(!tracepoint_needed(spec("trace_recvmsg"), empty));

        let file = HookBudget {
            features: FEAT_FILE_FLOW,
            file_write: false,
            advanced_tracepoints: false,
        };
        assert!(tracepoint_needed(spec("trace_openat"), file));
        assert!(tracepoint_needed(spec("trace_read_exit"), file));
        assert!(!tracepoint_needed(spec("trace_unlink"), file));
        assert!(!tracepoint_needed(spec("trace_pipe"), file));
        assert!(!tracepoint_needed(spec("trace_mmap"), file));

        let advanced_file = HookBudget {
            advanced_tracepoints: true,
            ..file
        };
        assert!(tracepoint_needed(spec("trace_pipe"), advanced_file));
        assert!(tracepoint_needed(spec("trace_mmap"), advanced_file));
        assert!(tracepoint_needed(spec("trace_recvmsg"), advanced_file));
    }

    #[test]
    fn file_flow_does_not_enable_lsm_file_hooks_without_block_file() {
        assert!(lsm_needed(
            "enforce_task_kill",
            false,
            false,
            false,
            false,
            false
        ));
        assert!(lsm_needed(
            "enforce_ptrace_access_check",
            false,
            false,
            false,
            false,
            false
        ));
        assert!(lsm_needed(
            "enforce_bpf_syscall",
            false,
            false,
            false,
            false,
            false
        ));
        assert!(!lsm_needed(
            "enforce_file_permission",
            false,
            false,
            false,
            false,
            false
        ));
        assert!(!lsm_needed(
            "enforce_mmap_file",
            false,
            false,
            false,
            false,
            true
        ));
        assert!(lsm_needed(
            "enforce_file_permission",
            false,
            true,
            false,
            false,
            false
        ));
        assert!(lsm_needed(
            "enforce_mmap_file",
            false,
            true,
            false,
            false,
            true
        ));
    }

    #[test]
    fn file_delta_requires_file_flow_budget() {
        let mut cfg: CConfig = unsafe { std::mem::zeroed() };
        cfg.n_updates = 1;
        cfg.updates[0].op = OP_OPEN;
        cfg.updates[0].m = 3; // TAINT_MATCH_ANY
        cfg.updates[0].add = 1;

        let err = validate_supported_features(&cfg, 0, "runtime policy delta")
            .expect_err("file source should require file-flow budget");
        assert!(
            err.to_string().contains("file source or sink hooks"),
            "{err}"
        );
        assert!(
            err.to_string().contains("ACTPLANE_RESERVE_FILE_FLOW=1"),
            "{err}"
        );
        assert!(
            err.to_string()
                .contains("MCP/watch/child-run already reserve file-flow"),
            "{err}"
        );
        validate_supported_features(&cfg, FEAT_FILE_FLOW, "runtime policy delta")
            .expect("file-flow budget admits file source");
    }

    #[test]
    fn block_delta_requires_matching_lsm_hook_profile() {
        let mut cfg: CConfig = unsafe { std::mem::zeroed() };
        cfg.n_rules = 1;
        cfg.rules[0].op = OP_EXEC;
        cfg.rules[0].effect = EFFECT_BLOCK;

        let err = validate_supported_features(&cfg, 0, "runtime policy delta")
            .expect_err("block exec should require bprm hook profile");
        assert!(err.to_string().contains("exec block hooks"), "{err}");
        assert!(
            err.to_string()
                .contains("block deltas require matching BPF-LSM hooks"),
            "{err}"
        );
        assert!(
            err.to_string()
                .contains("runtime reload/delta cannot attach new hooks"),
            "{err}"
        );
        validate_supported_features(&cfg, FEAT_BLOCK_EXEC, "runtime policy delta")
            .expect("block exec budget admits block exec rule");
    }

    #[test]
    fn path_matcher_delta_error_explains_loaded_policy_requirement() {
        let mut cfg: CConfig = unsafe { std::mem::zeroed() };
        cfg.n_rules = 1;
        cfg.rules[0].op = OP_OPEN;
        cfg.rules[0].m = M_CONTAINS;

        let err = validate_supported_features(&cfg, FEAT_FILE_FLOW, "runtime policy delta")
            .expect_err("path contains should require initial matcher support");
        let text = err.to_string();
        assert!(text.contains("path contains matches"), "{text}");
        assert!(
            text.contains("must be enabled by the policy used to load the engine"),
            "{text}"
        );
        assert!(text.contains("missing=0x"), "{text}");
    }

    #[test]
    fn argv_sensitive_exec_block_enables_lsm_hook() {
        let mut cfg: CConfig = unsafe { std::mem::zeroed() };
        cfg.n_rules = 1;
        cfg.rules[0].op = OP_EXEC;
        cfg.rules[0].effect = EFFECT_BLOCK;
        set_cstr(&mut cfg.rules[0].arg, "commit");

        assert_ne!(config_features(&cfg) & FEAT_BLOCK_EXEC, 0);
        validate_supported_features(&cfg, FEAT_BLOCK_EXEC, "runtime policy delta")
            .expect("argv-sensitive exec block rules now use the LSM hook via argv cache");
    }

    #[test]
    fn child_domain_state_is_monotonic_subset() {
        let parent = CapState {
            scope_id: 2,
            authority_mask: AUTH_BIND_RULE | AUTH_ADD_LABEL | AUTH_NARROW_SCOPE,
            target_mask: TARGET_CHILD,
            label_mask: 0b1010,
            ..CapState::default()
        };
        let child = child_domain_state(
            &parent,
            ChildDomainSpec {
                parent_pid: 100,
                parent_id: 100,
                child_id: 101,
                pid: 200,
                scope_id: 3,
                labels: 0b0010,
                authority_mask: AUTH_BIND_RULE,
                target_mask: TARGET_SELF,
                label_mask: 0b1000,
                ..ChildDomainSpec::default()
            },
        )
        .expect("valid child domain");
        assert_eq!(child.parent, 100);
        assert_eq!(child.scope_id, 3);
        assert_eq!(child.labels, 0b0010);
        assert_eq!(child.authority_mask, AUTH_BIND_RULE);
        assert_eq!(child.target_mask, TARGET_SELF);
        assert_eq!(child.label_mask, 0b1000);
    }

    #[test]
    fn child_domain_state_rejects_authority_widening() {
        let parent = CapState {
            scope_id: 4,
            authority_mask: AUTH_BIND_RULE,
            target_mask: TARGET_CHILD,
            label_mask: 0b0001,
            ..CapState::default()
        };
        let base = ChildDomainSpec {
            parent_pid: 100,
            parent_id: 100,
            child_id: 101,
            pid: 200,
            scope_id: 5,
            authority_mask: AUTH_BIND_RULE,
            target_mask: TARGET_SELF,
            ..ChildDomainSpec::default()
        };

        assert!(child_domain_state(
            &CapState {
                target_mask: 0,
                ..parent
            },
            base
        )
        .is_err());
        assert!(child_domain_state(
            &CapState {
                authority_mask: 0,
                ..parent
            },
            base
        )
        .is_err());
        assert!(child_domain_state(
            &parent,
            ChildDomainSpec {
                scope_id: 3,
                ..base
            }
        )
        .is_err());
        assert!(child_domain_state(
            &parent,
            ChildDomainSpec {
                authority_mask: AUTH_BIND_RULE | AUTH_ADD_LABEL,
                ..base
            }
        )
        .is_err());
        assert!(child_domain_state(
            &parent,
            ChildDomainSpec {
                labels: 0b0010,
                ..base
            }
        )
        .is_err());
    }

    #[test]
    fn reload_struct_layout() {
        assert_eq!(
            std::mem::size_of::<ReloadUpdate>(),
            8 + std::mem::size_of::<CUpdate>()
        );
        assert_eq!(
            std::mem::size_of::<ReloadRule>(),
            8 + std::mem::size_of::<CRule>()
        );
        assert_eq!(std::mem::size_of::<ReloadCounts>(), 16);
        assert_eq!(
            std::mem::size_of::<AppendUpdate>(),
            24 + std::mem::size_of::<CUpdate>()
        );
        assert_eq!(
            std::mem::size_of::<AppendRule>(),
            24 + std::mem::size_of::<CRule>()
        );
    }

    #[test]
    fn cap_policy_mask_tracks_full_rule_index_range() {
        let mut mask = CapPolicyMask::default();
        policy_mask_set(&mut mask, 0);
        policy_mask_set(&mut mask, 63);
        policy_mask_set(&mut mask, 64);
        policy_mask_set(&mut mask, MAX_RULES - 1);

        assert_eq!(mask.lo, 1 | (1u64 << 63));
        assert_eq!(mask.hi, 1 | (1u64 << 63));
    }

    fn set_cstr<const N: usize>(dst: &mut [u8; N], s: &str) {
        let bytes = s.as_bytes();
        let n = bytes.len().min(N.saturating_sub(1));
        dst[..n].copy_from_slice(&bytes[..n]);
    }

    fn config_blob(cfg: &CConfig) -> Vec<u8> {
        unsafe {
            std::slice::from_raw_parts(
                cfg as *const CConfig as *const u8,
                std::mem::size_of::<CConfig>(),
            )
            .to_vec()
        }
    }

    fn empty_config_blob() -> Vec<u8> {
        let cfg: CConfig = unsafe { std::mem::zeroed() };
        config_blob(&cfg)
    }

    fn notify_exec_config_blob(name: &str) -> Vec<u8> {
        let mut cfg: CConfig = unsafe { std::mem::zeroed() };
        cfg.n_rules = 1;
        cfg.rules[0].op = OP_EXEC;
        cfg.rules[0].m = 0; // TAINT_MATCH_EXACT
        cfg.rules[0].effect = EFFECT_NOTIFY;
        cfg.rules[0].rule_id = 0;
        set_cstr(&mut cfg.rules[0].target, name);
        config_blob(&cfg)
    }

    fn exec_arg_rule_config_blob(name: &str, arg: &str, effect: u8) -> Vec<u8> {
        let mut cfg: CConfig = unsafe { std::mem::zeroed() };
        cfg.n_rules = 1;
        cfg.rules[0].op = OP_EXEC;
        cfg.rules[0].m = 0; // TAINT_MATCH_EXACT
        cfg.rules[0].effect = effect;
        cfg.rules[0].rule_id = 0;
        set_cstr(&mut cfg.rules[0].target, name);
        set_cstr(&mut cfg.rules[0].arg, arg);
        config_blob(&cfg)
    }

    fn source_open_any_config_blob(label: u64) -> Vec<u8> {
        let mut cfg: CConfig = unsafe { std::mem::zeroed() };
        cfg.n_updates = 1;
        cfg.updates[0].op = OP_OPEN;
        cfg.updates[0].m = 3; // TAINT_MATCH_ANY
        cfg.updates[0].add = label;
        config_blob(&cfg)
    }

    fn source_open_exact_config_blob(label: u64, path: &str) -> Vec<u8> {
        assert!(path.len() < PAT, "test source path too long for CUpdate");
        let mut cfg: CConfig = unsafe { std::mem::zeroed() };
        cfg.n_updates = 1;
        cfg.updates[0].op = OP_OPEN;
        cfg.updates[0].m = 0; // TAINT_MATCH_EXACT
        cfg.updates[0].add = label;
        set_cstr(&mut cfg.updates[0].target, path);
        config_blob(&cfg)
    }

    fn source_open_then_notify_exec_config_blob(path: &str, name: &str, label: u64) -> Vec<u8> {
        assert!(path.len() < PAT, "test source path too long for CUpdate");
        let mut cfg: CConfig = unsafe { std::mem::zeroed() };
        cfg.n_updates = 1;
        cfg.updates[0].op = OP_OPEN;
        cfg.updates[0].m = 0; // TAINT_MATCH_EXACT
        cfg.updates[0].add = label;
        set_cstr(&mut cfg.updates[0].target, path);
        cfg.n_rules = 1;
        cfg.rules[0].op = OP_EXEC;
        cfg.rules[0].m = 0; // TAINT_MATCH_EXACT
        cfg.rules[0].effect = EFFECT_NOTIFY;
        cfg.rules[0].rule_id = 0;
        cfg.rules[0].req = label;
        set_cstr(&mut cfg.rules[0].target, name);
        config_blob(&cfg)
    }

    fn source_open_then_notify_open_config_blob(source: &str, sink: &str, label: u64) -> Vec<u8> {
        assert!(source.len() < PAT, "test source path too long for CUpdate");
        assert!(sink.len() < PAT, "test sink path too long for CRule");
        let mut cfg: CConfig = unsafe { std::mem::zeroed() };
        cfg.n_updates = 1;
        cfg.updates[0].op = OP_OPEN;
        cfg.updates[0].m = 0; // TAINT_MATCH_EXACT
        cfg.updates[0].add = label;
        set_cstr(&mut cfg.updates[0].target, source);
        cfg.n_rules = 1;
        cfg.rules[0].op = OP_OPEN;
        cfg.rules[0].m = 0; // TAINT_MATCH_EXACT
        cfg.rules[0].effect = EFFECT_NOTIFY;
        cfg.rules[0].rule_id = 0;
        cfg.rules[0].req = label;
        set_cstr(&mut cfg.rules[0].target, sink);
        config_blob(&cfg)
    }

    fn declassify_exec_config_blob(name: &str, label: u64) -> Vec<u8> {
        let mut cfg: CConfig = unsafe { std::mem::zeroed() };
        cfg.n_updates = 1;
        cfg.updates[0].op = OP_EXEC;
        cfg.updates[0].m = 0; // TAINT_MATCH_EXACT
        cfg.updates[0].del = label;
        set_cstr(&mut cfg.updates[0].target, name);
        config_blob(&cfg)
    }

    fn source_recv_then_notify_exec_config_blob(ipv4: u32, name: &str, label: u64) -> Vec<u8> {
        let mut cfg: CConfig = unsafe { std::mem::zeroed() };
        cfg.n_updates = 1;
        cfg.updates[0].op = OP_RECV;
        cfg.updates[0].m = 3; // TAINT_MATCH_ANY
        cfg.updates[0].add = label;
        cfg.updates[0].ipv4 = ipv4;
        cfg.updates[0].ipv4_mask = u32::MAX;
        cfg.n_rules = 1;
        cfg.rules[0].op = OP_EXEC;
        cfg.rules[0].m = 0; // TAINT_MATCH_EXACT
        cfg.rules[0].effect = EFFECT_NOTIFY;
        cfg.rules[0].rule_id = 0;
        cfg.rules[0].req = label;
        set_cstr(&mut cfg.rules[0].target, name);
        config_blob(&cfg)
    }

    fn block_recv_endpoint_config_blob(ipv4: u32, label: u64) -> Vec<u8> {
        let mut cfg: CConfig = unsafe { std::mem::zeroed() };
        cfg.n_rules = 1;
        cfg.rules[0].op = OP_RECV;
        cfg.rules[0].m = 3; // TAINT_MATCH_ANY
        cfg.rules[0].effect = EFFECT_BLOCK;
        cfg.rules[0].rule_id = 0;
        cfg.rules[0].req = label;
        cfg.rules[0].ipv4 = ipv4;
        cfg.rules[0].ipv4_mask = u32::MAX;
        config_blob(&cfg)
    }

    fn notify_exec_if_label_config_blob(name: &str, label: u64) -> Vec<u8> {
        let mut cfg: CConfig = unsafe { std::mem::zeroed() };
        cfg.n_rules = 1;
        cfg.rules[0].op = OP_EXEC;
        cfg.rules[0].m = 0; // TAINT_MATCH_EXACT
        cfg.rules[0].effect = EFFECT_NOTIFY;
        cfg.rules[0].rule_id = 0;
        cfg.rules[0].req = label;
        set_cstr(&mut cfg.rules[0].target, name);
        config_blob(&cfg)
    }

    fn block_write_paths_and_prefix_config_blob(
        paths: &[String],
        prefix: &str,
        label: u64,
    ) -> Vec<u8> {
        assert!(paths.len() < MAX_RULES);
        assert!(prefix.len() < PAT, "test prefix too long for CRule");
        let mut cfg: CConfig = unsafe { std::mem::zeroed() };
        cfg.n_rules = paths.len() as u32 + 1;
        for (idx, path) in paths.iter().enumerate() {
            assert!(path.len() < PAT, "test write path too long for CRule");
            cfg.rules[idx].op = OP_WRITE;
            cfg.rules[idx].m = 0; // TAINT_MATCH_EXACT
            cfg.rules[idx].effect = EFFECT_BLOCK;
            cfg.rules[idx].rule_id = idx as u32;
            cfg.rules[idx].req = label;
            set_cstr(&mut cfg.rules[idx].target, path);
        }
        let prefix_idx = paths.len();
        cfg.rules[prefix_idx].op = OP_WRITE;
        cfg.rules[prefix_idx].m = 1; // TAINT_MATCH_PREFIX
        cfg.rules[prefix_idx].effect = EFFECT_BLOCK;
        cfg.rules[prefix_idx].rule_id = prefix_idx as u32;
        cfg.rules[prefix_idx].req = label;
        set_cstr(&mut cfg.rules[prefix_idx].target, prefix);
        config_blob(&cfg)
    }

    fn spawn_stopped_exec(path: &std::path::Path) -> std::process::Child {
        std::process::Command::new("/bin/sh")
            .arg("-c")
            .arg("kill -STOP $$; exec \"$@\"")
            .arg("actplane-domain-test")
            .arg(path)
            .spawn()
            .expect("spawn stopped executable")
    }

    fn spawn_stopped_shell(script: &str) -> std::process::Child {
        std::process::Command::new("/bin/sh")
            .arg("-c")
            .arg(format!("kill -STOP $$; {script}"))
            .spawn()
            .expect("spawn stopped shell")
    }

    fn wait_until_stopped(child: &std::process::Child) {
        let pid = child.id() as i32;
        let stat_path = format!("/proc/{pid}/stat");
        for _ in 0..200 {
            if let Ok(stat) = std::fs::read_to_string(&stat_path) {
                if let Some((_, rest)) = stat.rsplit_once(") ") {
                    match rest.chars().next() {
                        Some('T') | Some('t') => return,
                        Some('Z') => panic!("child {pid} exited before reaching stopped state"),
                        _ => {}
                    }
                }
            }
            std::thread::sleep(std::time::Duration::from_millis(10));
        }
        panic!("child {pid} did not reach stopped state before resume");
    }

    fn resume_and_wait(child: &mut std::process::Child) {
        let pid = child.id() as i32;
        wait_until_stopped(child);
        let rc = unsafe { libc::kill(pid, libc::SIGCONT) };
        assert_eq!(rc, 0, "resume child {pid}");
        let status = child.wait().expect("wait child");
        assert!(status.success(), "child status {status:?}");
    }

    fn wait_child_timeout(
        child: &mut std::process::Child,
        timeout: std::time::Duration,
    ) -> Option<std::process::ExitStatus> {
        let deadline = std::time::Instant::now() + timeout;
        loop {
            match child.try_wait() {
                Ok(Some(status)) => return Some(status),
                Ok(None) => {
                    if std::time::Instant::now() >= deadline {
                        return None;
                    }
                    std::thread::sleep(std::time::Duration::from_millis(10));
                }
                Err(e) => panic!("try_wait child: {e}"),
            }
        }
    }

    fn mmap_shared_writer_script(src: &str, out: &str) -> String {
        format!(
            r#"import mmap, os, signal
os.kill(os.getpid(), signal.SIGSTOP)
out_fd = os.open({out:?}, os.O_RDWR | os.O_CREAT | os.O_TRUNC, 0o600)
try:
    os.ftruncate(out_fd, 4096)
    with open({src:?}, "rb") as src:
        src.read()
    mm = mmap.mmap(out_fd, 4096, access=mmap.ACCESS_WRITE)
    try:
        mm[:6] = b"copied"
        mm.flush()
    finally:
        mm.close()
finally:
    os.close(out_fd)
"#
        )
    }

    fn mprotect_shared_writer_script(src: &str, out: &str) -> String {
        format!(
            r#"import ctypes, os, signal
os.kill(os.getpid(), signal.SIGSTOP)
libc = ctypes.CDLL(None, use_errno=True)
PROT_READ = 1
PROT_WRITE = 2
MAP_SHARED = 1
MS_SYNC = 4
size = 4096
libc.mmap.restype = ctypes.c_void_p
libc.mmap.argtypes = [ctypes.c_void_p, ctypes.c_size_t, ctypes.c_int, ctypes.c_int, ctypes.c_int, ctypes.c_long]
libc.mprotect.argtypes = [ctypes.c_void_p, ctypes.c_size_t, ctypes.c_int]
libc.msync.argtypes = [ctypes.c_void_p, ctypes.c_size_t, ctypes.c_int]
libc.munmap.argtypes = [ctypes.c_void_p, ctypes.c_size_t]
out_fd = os.open({out:?}, os.O_RDWR | os.O_CREAT | os.O_TRUNC, 0o600)
try:
    os.ftruncate(out_fd, size)
    addr = libc.mmap(None, size, PROT_READ, MAP_SHARED, out_fd, 0)
    if addr is None or addr == ctypes.c_void_p(-1).value:
        raise OSError(ctypes.get_errno(), "mmap failed")
    try:
        with open({src:?}, "rb") as src:
            src.read()
        if libc.mprotect(ctypes.c_void_p(addr), size, PROT_READ | PROT_WRITE) != 0:
            raise OSError(ctypes.get_errno(), "mprotect failed")
        ctypes.memmove(ctypes.c_void_p(addr), b"copied\n", 7)
        if libc.msync(ctypes.c_void_p(addr), size, MS_SYNC) != 0:
            raise OSError(ctypes.get_errno(), "msync failed")
    finally:
        libc.munmap(ctypes.c_void_p(addr), size)
finally:
    os.close(out_fd)
"#
        )
    }

    fn mremap_shared_writer_script(src: &str, out: &str) -> String {
        format!(
            r#"import ctypes, os, signal
os.kill(os.getpid(), signal.SIGSTOP)
libc = ctypes.CDLL(None, use_errno=True)
PROT_READ = 1
PROT_WRITE = 2
MAP_SHARED = 1
MS_SYNC = 4
size = 4096
libc.mmap.restype = ctypes.c_void_p
libc.mmap.argtypes = [ctypes.c_void_p, ctypes.c_size_t, ctypes.c_int, ctypes.c_int, ctypes.c_int, ctypes.c_long]
libc.mremap.restype = ctypes.c_void_p
libc.mremap.argtypes = [ctypes.c_void_p, ctypes.c_size_t, ctypes.c_size_t, ctypes.c_int]
libc.msync.argtypes = [ctypes.c_void_p, ctypes.c_size_t, ctypes.c_int]
libc.munmap.argtypes = [ctypes.c_void_p, ctypes.c_size_t]
out_fd = os.open({out:?}, os.O_RDWR | os.O_CREAT | os.O_TRUNC, 0o600)
try:
    os.ftruncate(out_fd, size)
    addr = libc.mmap(None, size, PROT_READ | PROT_WRITE, MAP_SHARED, out_fd, 0)
    if addr is None or addr == ctypes.c_void_p(-1).value:
        raise OSError(ctypes.get_errno(), "mmap failed")
    mapped = addr
    try:
        with open({src:?}, "rb") as src:
            src.read()
        remapped = libc.mremap(ctypes.c_void_p(addr), size, size, 0)
        if remapped is None or remapped == ctypes.c_void_p(-1).value:
            raise OSError(ctypes.get_errno(), "mremap failed")
        mapped = remapped
        ctypes.memmove(ctypes.c_void_p(mapped), b"copied\n", 7)
        if libc.msync(ctypes.c_void_p(mapped), size, MS_SYNC) != 0:
            raise OSError(ctypes.get_errno(), "msync failed")
    finally:
        libc.munmap(ctypes.c_void_p(mapped), size)
finally:
    os.close(out_fd)
"#
        )
    }

    fn mprotect_read_upgrade_exec_script(src: &str, hit: &str) -> String {
        format!(
            r#"import ctypes, os, signal
libc = ctypes.CDLL(None, use_errno=True)
PROT_NONE = 0
PROT_READ = 1
MAP_PRIVATE = 2
size = 4096
libc.mmap.restype = ctypes.c_void_p
libc.mmap.argtypes = [ctypes.c_void_p, ctypes.c_size_t, ctypes.c_int, ctypes.c_int, ctypes.c_int, ctypes.c_long]
libc.mprotect.argtypes = [ctypes.c_void_p, ctypes.c_size_t, ctypes.c_int]
libc.munmap.argtypes = [ctypes.c_void_p, ctypes.c_size_t]
fd = os.open({src:?}, os.O_RDONLY)
os.kill(os.getpid(), signal.SIGSTOP)
try:
    addr = libc.mmap(None, size, PROT_NONE, MAP_PRIVATE, fd, 0)
    if addr is None or addr == ctypes.c_void_p(-1).value:
        raise OSError(ctypes.get_errno(), "mmap failed")
    try:
        if libc.mprotect(ctypes.c_void_p(addr), size, PROT_READ) != 0:
            raise OSError(ctypes.get_errno(), "mprotect failed")
        data = ctypes.string_at(ctypes.c_void_p(addr), 1)
        if data != b"s":
            raise RuntimeError("unexpected mapped data")
    finally:
        libc.munmap(ctypes.c_void_p(addr), size)
finally:
    os.close(fd)
os.execv({hit:?}, [{hit:?}])
"#
        )
    }

    fn mprotect_two_mapping_read_upgrade_exec_script(
        src_a: &str,
        src_b: &str,
        hit: &str,
    ) -> String {
        format!(
            r#"import ctypes, os, signal
libc = ctypes.CDLL(None, use_errno=True)
PROT_NONE = 0
PROT_READ = 1
MAP_PRIVATE = 2
size = 4096
libc.mmap.restype = ctypes.c_void_p
libc.mmap.argtypes = [ctypes.c_void_p, ctypes.c_size_t, ctypes.c_int, ctypes.c_int, ctypes.c_int, ctypes.c_long]
libc.mprotect.argtypes = [ctypes.c_void_p, ctypes.c_size_t, ctypes.c_int]
libc.munmap.argtypes = [ctypes.c_void_p, ctypes.c_size_t]
fd_a = os.open({src_a:?}, os.O_RDONLY)
fd_b = os.open({src_b:?}, os.O_RDONLY)
os.kill(os.getpid(), signal.SIGSTOP)
addr_a = libc.mmap(None, size, PROT_NONE, MAP_PRIVATE, fd_a, 0)
addr_b = libc.mmap(None, size, PROT_NONE, MAP_PRIVATE, fd_b, 0)
if addr_a is None or addr_a == ctypes.c_void_p(-1).value:
    raise OSError(ctypes.get_errno(), "mmap src_a failed")
if addr_b is None or addr_b == ctypes.c_void_p(-1).value:
    raise OSError(ctypes.get_errno(), "mmap src_b failed")
try:
    if libc.mprotect(ctypes.c_void_p(addr_a), size, PROT_READ) != 0:
        raise OSError(ctypes.get_errno(), "mprotect src_a failed")
    data = ctypes.string_at(ctypes.c_void_p(addr_a), 1)
    if data != b"s":
        raise RuntimeError("unexpected mapped data")
finally:
    libc.munmap(ctypes.c_void_p(addr_b), size)
    libc.munmap(ctypes.c_void_p(addr_a), size)
    os.close(fd_b)
    os.close(fd_a)
os.execv({hit:?}, [{hit:?}])
"#
        )
    }

    fn percentile(sorted: &[std::time::Duration], pct: f64) -> std::time::Duration {
        assert!(!sorted.is_empty());
        let idx = ((sorted.len() - 1) as f64 * pct).round() as usize;
        sorted[idx.min(sorted.len() - 1)]
    }

    fn summarize_durations(name: &str, durs: &[std::time::Duration]) {
        let mut sorted = durs.to_vec();
        sorted.sort_unstable();
        let total_ns: u128 = durs.iter().map(|d| d.as_nanos()).sum();
        let mean_us = total_ns as f64 / durs.len() as f64 / 1000.0;
        println!(
            "{name}: n={} mean={:.2}us p50={:.2}us p90={:.2}us p99={:.2}us min={:.2}us max={:.2}us",
            durs.len(),
            mean_us,
            percentile(&sorted, 0.50).as_secs_f64() * 1_000_000.0,
            percentile(&sorted, 0.90).as_secs_f64() * 1_000_000.0,
            percentile(&sorted, 0.99).as_secs_f64() * 1_000_000.0,
            sorted[0].as_secs_f64() * 1_000_000.0,
            sorted[sorted.len() - 1].as_secs_f64() * 1_000_000.0,
        );
    }

    struct LiveBpfTestGuard {
        _lock: std::sync::MutexGuard<'static, ()>,
        old_hook_profile: Option<std::ffi::OsString>,
    }

    impl Drop for LiveBpfTestGuard {
        fn drop(&mut self) {
            if let Some(v) = self.old_hook_profile.take() {
                std::env::set_var("ACTPLANE_HOOK_PROFILE", v);
            } else {
                std::env::remove_var("ACTPLANE_HOOK_PROFILE");
            }
        }
    }

    fn live_bpf_test_guard() -> LiveBpfTestGuard {
        static LIVE_BPF_TEST_LOCK: std::sync::OnceLock<std::sync::Mutex<()>> =
            std::sync::OnceLock::new();
        let lock = LIVE_BPF_TEST_LOCK
            .get_or_init(|| std::sync::Mutex::new(()))
            .lock()
            .expect("live BPF test lock");
        let old_hook_profile = std::env::var_os("ACTPLANE_HOOK_PROFILE");
        std::env::set_var("ACTPLANE_HOOK_PROFILE", "full");
        LiveBpfTestGuard {
            _lock: lock,
            old_hook_profile,
        }
    }

    #[test]
    #[ignore = "requires root/CAP_BPF and loads live eBPF programs"]
    fn reload_policy_latency_smoke() {
        let empty = empty_config_blob();
        let policy_a = notify_exec_config_blob("aprl_a");
        let policy_b = notify_exec_config_blob("aprl_b");
        let policy_hit = notify_exec_config_blob("aprlhit");

        let mut loader = Loader::load(&empty).expect("load eBPF engine");
        loader
            .seed_label(std::process::id() as i32, 1)
            .expect("seed current test domain");
        let handle = loader.reload_handle().expect("reload handle");
        let stop = std::sync::Arc::new(std::sync::atomic::AtomicBool::new(false));
        let (tx, rx) = std::sync::mpsc::channel();
        let run_stop = std::sync::Arc::clone(&stop);
        let run_thread = std::thread::spawn(move || {
            loader.run(&run_stop, |v| {
                let _ = tx.send((std::time::Instant::now(), v));
            })
        });

        std::thread::sleep(std::time::Duration::from_millis(50));
        for i in 0..10 {
            let blob = if i % 2 == 0 { &policy_a } else { &policy_b };
            handle.reload_policy(blob).expect("warm reload");
        }

        let mut reload_durs = Vec::new();
        for i in 0..200 {
            let blob = if i % 2 == 0 { &policy_a } else { &policy_b };
            let start = std::time::Instant::now();
            handle.reload_policy(blob).expect("measured reload");
            reload_durs.push(start.elapsed());
        }

        let tmp =
            std::env::temp_dir().join(format!("actplane-reload-bench-{}", std::process::id()));
        std::fs::create_dir_all(&tmp).expect("create temp dir");
        let hit_path = tmp.join("aprlhit");
        std::fs::copy("/bin/true", &hit_path).expect("copy /bin/true");

        let mut effect_durs = Vec::new();
        let mut reload_only_durs = Vec::new();
        for _ in 0..50 {
            let start = std::time::Instant::now();
            handle
                .reload_policy(&policy_hit)
                .expect("reload hit policy");
            reload_only_durs.push(start.elapsed());
            let status = std::process::Command::new(&hit_path)
                .status()
                .expect("run matching executable");
            assert!(status.success());
            let (event_at, v) = rx
                .recv_timeout(std::time::Duration::from_secs(2))
                .expect("violation after reload");
            assert!(v.target.ends_with("aprlhit"), "target was {}", v.target);
            effect_durs.push(event_at.duration_since(start));
        }

        stop.store(true, std::sync::atomic::Ordering::Relaxed);
        run_thread
            .join()
            .expect("join ring thread")
            .expect("run loop");
        let _ = std::fs::remove_dir_all(&tmp);

        println!("reload path: 1 rule, 0 updates, counts quiesce + rule + counts activate");
        summarize_durations("reload_policy_submit_to_drain", &reload_durs);
        summarize_durations("reload_policy_before_effect_samples", &reload_only_durs);
        summarize_durations("reload_to_observed_exec_violation", &effect_durs);
    }

    #[test]
    #[ignore = "requires root/CAP_BPF and loads live eBPF programs"]
    fn unseeded_processes_do_not_match_global_policy() {
        let policy = notify_exec_config_blob("apoutside");
        let mut loader = Loader::load(&policy).expect("load eBPF engine");

        let stop = std::sync::Arc::new(std::sync::atomic::AtomicBool::new(false));
        let (tx, rx) = std::sync::mpsc::channel();
        let run_stop = std::sync::Arc::clone(&stop);
        let run_thread = std::thread::spawn(move || {
            loader.run(&run_stop, |v| {
                let _ = tx.send(v);
            })
        });
        std::thread::sleep(std::time::Duration::from_millis(50));

        let tmp =
            std::env::temp_dir().join(format!("actplane-unseeded-smoke-{}", std::process::id()));
        std::fs::create_dir_all(&tmp).expect("create temp dir");
        let hit_path = tmp.join("apoutside");
        std::fs::copy("/bin/true", &hit_path).expect("copy /bin/true");
        let status = std::process::Command::new(&hit_path)
            .status()
            .expect("run matching executable");
        assert!(status.success());

        assert!(
            rx.recv_timeout(std::time::Duration::from_millis(250))
                .is_err(),
            "unseeded process matched a global rule"
        );

        stop.store(true, std::sync::atomic::Ordering::Relaxed);
        run_thread
            .join()
            .expect("join ring thread")
            .expect("run loop");
        let _ = std::fs::remove_dir_all(&tmp);
    }

    #[test]
    #[ignore = "requires root/CAP_BPF and loads live eBPF programs"]
    fn exec_argv_notify_matches_token_smoke() {
        let policy = exec_arg_rule_config_blob("apargnotify", "needle", EFFECT_NOTIFY);
        let mut loader = Loader::load(&policy).expect("load eBPF engine");
        loader
            .seed_label(std::process::id() as i32, 1)
            .expect("seed current test domain");

        let stop = std::sync::Arc::new(std::sync::atomic::AtomicBool::new(false));
        let (tx, rx) = std::sync::mpsc::channel();
        let run_stop = std::sync::Arc::clone(&stop);
        let run_thread = std::thread::spawn(move || {
            loader.run(&run_stop, |v| {
                let _ = tx.send(v);
            })
        });
        std::thread::sleep(std::time::Duration::from_millis(50));

        let tmp = std::env::temp_dir().join(format!("actplane-argv-notify-{}", std::process::id()));
        std::fs::create_dir_all(&tmp).expect("create temp dir");
        let hit_path = tmp.join("apargnotify");
        std::fs::copy("/bin/true", &hit_path).expect("copy /bin/true");

        let miss = std::process::Command::new(&hit_path)
            .arg("other")
            .status()
            .expect("run non-matching argv executable");
        assert!(miss.success());
        assert!(
            rx.recv_timeout(std::time::Duration::from_millis(250))
                .is_err(),
            "non-matching argv token reported a violation"
        );

        let hit = std::process::Command::new(&hit_path)
            .arg("needle")
            .status()
            .expect("run matching argv executable");
        assert!(hit.success());
        let v = rx
            .recv_timeout(std::time::Duration::from_secs(2))
            .expect("argv-sensitive notify violation");
        assert_eq!(v.effect, EFFECT_NOTIFY as u32);
        assert!(!v.blocked, "notify should not block: {v:?}");
        assert!(!v.killed, "notify should not kill: {v:?}");
        assert!(v.target.ends_with("apargnotify"), "target was {}", v.target);

        stop.store(true, std::sync::atomic::Ordering::Relaxed);
        run_thread
            .join()
            .expect("join ring thread")
            .expect("run loop");
        let _ = std::fs::remove_dir_all(&tmp);
    }

    #[test]
    #[ignore = "requires root/CAP_BPF and loads live eBPF programs"]
    fn exec_argv_kill_terminates_post_exec_smoke() {
        let policy = exec_arg_rule_config_blob("apargkill", "needle", EFFECT_KILL);
        let mut loader = Loader::load(&policy).expect("load eBPF engine");
        loader
            .seed_label(std::process::id() as i32, 1)
            .expect("seed current test domain");

        let stop = std::sync::Arc::new(std::sync::atomic::AtomicBool::new(false));
        let (tx, rx) = std::sync::mpsc::channel();
        let run_stop = std::sync::Arc::clone(&stop);
        let run_thread = std::thread::spawn(move || {
            loader.run(&run_stop, |v| {
                let _ = tx.send(v);
            })
        });
        std::thread::sleep(std::time::Duration::from_millis(50));

        let tmp = std::env::temp_dir().join(format!("actplane-argv-kill-{}", std::process::id()));
        std::fs::create_dir_all(&tmp).expect("create temp dir");
        let hit_path = tmp.join("apargkill");
        std::fs::copy("/bin/sh", &hit_path).expect("copy /bin/sh");

        let mut child = std::process::Command::new(&hit_path)
            .arg("-c")
            .arg("sleep 30")
            .arg("needle")
            .spawn()
            .expect("spawn matching argv executable");
        let status = match wait_child_timeout(&mut child, std::time::Duration::from_secs(2)) {
            Some(status) => status,
            None => {
                let _ = child.kill();
                let _ = child.wait();
                panic!("argv-sensitive kill rule did not terminate child");
            }
        };
        assert!(!status.success(), "kill rule child status {status:?}");

        let v = rx
            .recv_timeout(std::time::Duration::from_secs(2))
            .expect("argv-sensitive kill violation");
        assert_eq!(v.effect, EFFECT_KILL as u32);
        assert!(v.killed, "kill violation did not set killed=true: {v:?}");
        assert!(
            !v.blocked,
            "argv-sensitive kill should come from post-exec tracepoint: {v:?}"
        );
        assert!(v.target.ends_with("apargkill"), "target was {}", v.target);

        stop.store(true, std::sync::atomic::Ordering::Relaxed);
        run_thread
            .join()
            .expect("join ring thread")
            .expect("run loop");
        let _ = std::fs::remove_dir_all(&tmp);
    }

    #[test]
    #[ignore = "requires root/CAP_BPF, active BPF LSM, and loads live eBPF programs"]
    fn lsm_path_write_hooks_block_unlink_rename_and_truncate_smoke() {
        if !bpf_lsm_active() {
            eprintln!("skipping LSM path write hook smoke: bpf LSM is not active");
            return;
        }
        let label = 1;
        let caller_pid = std::process::id() as i32;
        let tmp = std::env::temp_dir().join(format!(
            "actplane-lsm-path-write-smoke-{}",
            std::process::id()
        ));
        std::fs::create_dir_all(&tmp).expect("create temp dir");
        let unlink_path = tmp.join("protected-unlink");
        let truncate_path = tmp.join("protected-truncate");
        let rename_old_path = tmp.join("protected-rename-old");
        let rename_new_path = tmp.join("protected-rename-new");
        let long_dir = tmp.join("parent-name-longer-than-sixty-three-bytes-for-dentry-append");
        let long_unlink_path = long_dir.join("ap-longhook");
        assert!(
            long_dir.to_string_lossy().len() > 63,
            "test parent path must cross the dentry append offset bucket"
        );
        std::fs::create_dir_all(&long_dir).expect("create long parent dir");
        std::fs::write(&unlink_path, "keep unlink\n").expect("write unlink file");
        std::fs::write(&truncate_path, "keep truncate\n").expect("write truncate file");
        std::fs::write(&rename_old_path, "keep rename\n").expect("write rename file");
        std::fs::write(&long_unlink_path, "keep long unlink\n").expect("write long unlink file");

        let protected = vec![
            unlink_path.to_string_lossy().to_string(),
            truncate_path.to_string_lossy().to_string(),
            rename_old_path.to_string_lossy().to_string(),
            long_unlink_path.to_string_lossy().to_string(),
        ];
        let long_prefix: String = protected[3].chars().take(PAT - 1).collect();
        let policy = block_write_paths_and_prefix_config_blob(&protected[..3], &long_prefix, label);
        let mut loader = Loader::load(&policy).expect("load eBPF engine");
        assert!(
            loader.enforce_mode(),
            "BPF LSM should be active for this test"
        );
        loader
            .seed_label(caller_pid, label)
            .expect("seed current test domain");

        let stop = std::sync::Arc::new(std::sync::atomic::AtomicBool::new(false));
        let (tx, rx) = std::sync::mpsc::channel();
        let run_stop = std::sync::Arc::clone(&stop);
        let run_thread = std::thread::spawn(move || {
            loader.run(&run_stop, |v| {
                let _ = tx.send(v);
            })
        });
        std::thread::sleep(std::time::Duration::from_millis(50));

        let unlink_err = std::fs::remove_file(&unlink_path).expect_err("unlink should be blocked");
        assert_eq!(unlink_err.raw_os_error(), Some(libc::EPERM));
        assert!(unlink_path.is_file(), "blocked unlink removed the file");

        let truncate_c = std::ffi::CString::new(truncate_path.to_string_lossy().as_bytes())
            .expect("truncate path cstring");
        let truncate_rc = unsafe { libc::truncate(truncate_c.as_ptr(), 0) };
        assert_eq!(truncate_rc, -1, "truncate should fail");
        assert_eq!(
            std::io::Error::last_os_error().raw_os_error(),
            Some(libc::EPERM)
        );
        let truncate_contents =
            std::fs::read_to_string(&truncate_path).expect("read truncate file");
        assert_eq!(truncate_contents, "keep truncate\n");

        let rename_err = std::fs::rename(&rename_old_path, &rename_new_path)
            .expect_err("rename should be blocked");
        assert_eq!(rename_err.raw_os_error(), Some(libc::EPERM));
        assert!(rename_old_path.is_file(), "blocked rename removed old path");
        assert!(!rename_new_path.exists(), "blocked rename created new path");

        let long_unlink_err =
            std::fs::remove_file(&long_unlink_path).expect_err("long unlink should be blocked");
        assert_eq!(long_unlink_err.raw_os_error(), Some(libc::EPERM));
        assert!(
            long_unlink_path.is_file(),
            "blocked long-parent unlink removed the file"
        );

        let mut seen = Vec::new();
        for _ in 0..4 {
            let v = rx
                .recv_timeout(std::time::Duration::from_secs(2))
                .expect("blocked write violation event");
            assert!(v.blocked, "violation was not marked blocked: {v:?}");
            seen.push(v.target);
        }
        for path in &protected {
            assert!(
                seen.iter().any(|target| target == path),
                "missing blocked event for {path}; saw {seen:?}"
            );
        }

        stop.store(true, std::sync::atomic::Ordering::Relaxed);
        run_thread
            .join()
            .expect("join ring thread")
            .expect("run loop");
        let _ = std::fs::remove_dir_all(&tmp);
    }

    #[test]
    #[ignore = "requires root/CAP_BPF, active BPF LSM, and loads live eBPF programs"]
    fn lsm_fd_write_after_late_read_propagates_file_label_smoke() {
        if !bpf_lsm_active() {
            eprintln!("skipping LSM fd write flow smoke: bpf LSM is not active");
            return;
        }
        let _guard = live_bpf_test_guard();
        let label = 1;
        let caller_pid = std::process::id() as i32;
        let tmp = std::env::temp_dir().join(format!("actplane-lsm-fd-flow-{}", std::process::id()));
        std::fs::create_dir_all(&tmp).expect("create temp dir");
        let src_path = tmp.join("src");
        let out_path = tmp.join("out");
        let hit_path = tmp.join("apfdhit");
        std::fs::write(&src_path, "secret\n").expect("write source");
        std::fs::copy("/bin/true", &hit_path).expect("copy /bin/true");

        let src = src_path.to_string_lossy().to_string();
        let out = out_path.to_string_lossy().to_string();
        let hit = hit_path.to_string_lossy().to_string();
        let policy = source_open_then_notify_exec_config_blob(&src, "apfdhit", label);
        let mut loader = Loader::load(&policy).expect("load eBPF engine");
        assert!(
            loader.enforce_mode(),
            "BPF LSM should be active for this test"
        );
        loader
            .bind_state(
                caller_pid,
                caller_pid as u32,
                CapState {
                    scope_id: 1,
                    target_mask: TARGET_SELF | TARGET_CHILD,
                    ..CapState::default()
                },
            )
            .expect("bind active unlabeled caller domain");

        let mut writer =
            spawn_stopped_shell(&format!("exec 3> {out}; read _ < {src}; echo copied >&3"));
        let mut reader = spawn_stopped_shell(&format!("read _ < {out}; exec {hit}"));

        let stop = std::sync::Arc::new(std::sync::atomic::AtomicBool::new(false));
        let (tx, rx) = std::sync::mpsc::channel();
        let run_stop = std::sync::Arc::clone(&stop);
        let run_thread = std::thread::spawn(move || {
            loader.run(&run_stop, |v| {
                let _ = tx.send(v);
            })
        });
        std::thread::sleep(std::time::Duration::from_millis(50));

        resume_and_wait(&mut writer);
        assert!(
            out_path.is_file(),
            "writer did not create the output file before reader ran"
        );
        while rx.try_recv().is_ok() {}

        resume_and_wait(&mut reader);
        let v = rx
            .recv_timeout(std::time::Duration::from_secs(2))
            .expect("reader should inherit label written through an already-open fd");
        assert!(v.target.ends_with("apfdhit"), "target was {}", v.target);

        stop.store(true, std::sync::atomic::Ordering::Relaxed);
        run_thread
            .join()
            .expect("join ring thread")
            .expect("run loop");
        let _ = std::fs::remove_dir_all(&tmp);
    }

    #[test]
    #[ignore = "requires root/CAP_BPF and loads live eBPF programs"]
    fn object_label_open_violation_reports_file_provenance_smoke() {
        let _guard = live_bpf_test_guard();
        let label = 1;
        let caller_pid = std::process::id() as i32;
        let tmp =
            std::env::temp_dir().join(format!("actplane-object-prov-smoke-{}", std::process::id()));
        std::fs::create_dir_all(&tmp).expect("create temp dir");
        let src_path = tmp.join("src");
        let shared_path = tmp.join("shared");
        std::fs::write(&src_path, "secret\n").expect("write source");

        let src = src_path.to_string_lossy().to_string();
        let shared = shared_path.to_string_lossy().to_string();
        let policy = source_open_then_notify_open_config_blob(&src, &shared, label);
        let mut loader = Loader::load(&policy).expect("load eBPF engine");
        loader
            .bind_state(
                caller_pid,
                caller_pid as u32,
                CapState {
                    scope_id: 1,
                    target_mask: TARGET_SELF | TARGET_CHILD,
                    ..CapState::default()
                },
            )
            .expect("bind active unlabeled caller domain");

        let mut writer =
            spawn_stopped_shell(&format!("read _ < {src}; printf 'copied\\n' > {shared}"));
        let mut reader = spawn_stopped_shell(&format!("read _ < {shared}"));

        let stop = std::sync::Arc::new(std::sync::atomic::AtomicBool::new(false));
        let (tx, rx) = std::sync::mpsc::channel();
        let run_stop = std::sync::Arc::clone(&stop);
        let run_thread = std::thread::spawn(move || {
            loader.run(&run_stop, |v| {
                let _ = tx.send(v);
            })
        });
        std::thread::sleep(std::time::Duration::from_millis(50));

        resume_and_wait(&mut writer);
        assert!(
            shared_path.is_file(),
            "writer did not create the shared file before reader ran"
        );
        while rx.try_recv().is_ok() {}

        resume_and_wait(&mut reader);
        let v = rx
            .recv_timeout(std::time::Duration::from_secs(2))
            .expect("open sink should report the file object's provenance");
        assert_eq!(v.op, OP_OPEN as u32);
        assert_eq!(v.matched_label, label);
        assert_eq!(v.matched_labels, label);
        assert!(
            v.target.ends_with("/shared") || v.target == shared,
            "target was {}",
            v.target
        );
        let provenance = v.provenance.expect("file object provenance");
        assert_eq!(provenance.label, label);
        assert_eq!(provenance.op, OP_OPEN as u32);
        assert!(
            provenance.target.ends_with("/src") || provenance.target == src,
            "provenance target was {}",
            provenance.target
        );

        stop.store(true, std::sync::atomic::Ordering::Relaxed);
        run_thread
            .join()
            .expect("join ring thread")
            .expect("run loop");
        let _ = std::fs::remove_dir_all(&tmp);
    }

    #[test]
    #[ignore = "requires root/CAP_BPF, active BPF LSM, and loads live eBPF programs"]
    fn lsm_mmap_shared_write_after_late_read_propagates_file_label_smoke() {
        if !bpf_lsm_active() {
            eprintln!("skipping LSM mmap write flow smoke: bpf LSM is not active");
            return;
        }
        let _guard = live_bpf_test_guard();
        if std::process::Command::new("python3")
            .arg("-c")
            .arg("import mmap")
            .stdout(std::process::Stdio::null())
            .stderr(std::process::Stdio::null())
            .status()
            .is_err()
        {
            eprintln!("skipping LSM mmap write flow smoke: python3 mmap is not available");
            return;
        }

        let label = 1;
        let caller_pid = std::process::id() as i32;
        let tmp = std::env::temp_dir().join(format!("actplane-lsm-mmap-{}", std::process::id()));
        std::fs::create_dir_all(&tmp).expect("create temp dir");
        let src_path = tmp.join("src");
        let out_path = tmp.join("out");
        let hit_path = tmp.join("apmaphit");
        std::fs::write(&src_path, "secret\n").expect("write source");
        std::fs::copy("/bin/true", &hit_path).expect("copy /bin/true");

        let src = src_path.to_string_lossy().to_string();
        let out = out_path.to_string_lossy().to_string();
        let hit = hit_path.to_string_lossy().to_string();
        let policy = source_open_then_notify_exec_config_blob(&src, "apmaphit", label);
        let mut loader = Loader::load(&policy).expect("load eBPF engine");
        assert!(
            loader.enforce_mode(),
            "BPF LSM should be active for this test"
        );
        loader
            .bind_state(
                caller_pid,
                caller_pid as u32,
                CapState {
                    scope_id: 1,
                    target_mask: TARGET_SELF | TARGET_CHILD,
                    ..CapState::default()
                },
            )
            .expect("bind active unlabeled caller domain");

        let mut writer = std::process::Command::new("python3")
            .arg("-c")
            .arg(mmap_shared_writer_script(&src, &out))
            .spawn()
            .expect("spawn stopped mmap writer");
        let mut reader = spawn_stopped_shell(&format!("read _ < {out}; exec {hit}"));

        let stop = std::sync::Arc::new(std::sync::atomic::AtomicBool::new(false));
        let (tx, rx) = std::sync::mpsc::channel();
        let run_stop = std::sync::Arc::clone(&stop);
        let run_thread = std::thread::spawn(move || {
            loader.run(&run_stop, |v| {
                let _ = tx.send(v);
            })
        });
        std::thread::sleep(std::time::Duration::from_millis(50));

        resume_and_wait(&mut writer);
        assert!(
            out_path.is_file(),
            "writer did not create the output file before reader ran"
        );
        while rx.try_recv().is_ok() {}

        resume_and_wait(&mut reader);
        let v = rx
            .recv_timeout(std::time::Duration::from_secs(2))
            .expect("LSM reader should inherit label written through shared mmap");
        assert!(v.target.ends_with("apmaphit"), "target was {}", v.target);

        stop.store(true, std::sync::atomic::Ordering::Relaxed);
        run_thread
            .join()
            .expect("join ring thread")
            .expect("run loop");
        let _ = std::fs::remove_dir_all(&tmp);
    }

    #[test]
    #[ignore = "requires root/CAP_BPF, active BPF LSM, and loads live eBPF programs"]
    fn lsm_mprotect_shared_write_after_late_read_propagates_file_label_smoke() {
        if !bpf_lsm_active() {
            eprintln!("skipping LSM mprotect write flow smoke: bpf LSM is not active");
            return;
        }
        let _guard = live_bpf_test_guard();
        if std::process::Command::new("python3")
            .arg("-c")
            .arg("import ctypes")
            .stdout(std::process::Stdio::null())
            .stderr(std::process::Stdio::null())
            .status()
            .is_err()
        {
            eprintln!("skipping LSM mprotect write flow smoke: python3 ctypes is not available");
            return;
        }

        let label = 1;
        let caller_pid = std::process::id() as i32;
        let tmp =
            std::env::temp_dir().join(format!("actplane-lsm-mprotect-{}", std::process::id()));
        std::fs::create_dir_all(&tmp).expect("create temp dir");
        let src_path = tmp.join("src");
        let out_path = tmp.join("out");
        let hit_path = tmp.join("apmprothit");
        std::fs::write(&src_path, "secret\n").expect("write source");
        std::fs::copy("/bin/true", &hit_path).expect("copy /bin/true");

        let src = src_path.to_string_lossy().to_string();
        let out = out_path.to_string_lossy().to_string();
        let hit = hit_path.to_string_lossy().to_string();
        let policy = source_open_then_notify_exec_config_blob(&src, "apmprothit", label);
        let mut loader = Loader::load(&policy).expect("load eBPF engine");
        assert!(
            loader.enforce_mode(),
            "BPF LSM should be active for this test"
        );
        loader
            .bind_state(
                caller_pid,
                caller_pid as u32,
                CapState {
                    scope_id: 1,
                    target_mask: TARGET_SELF | TARGET_CHILD,
                    ..CapState::default()
                },
            )
            .expect("bind active unlabeled caller domain");

        let mut writer = std::process::Command::new("python3")
            .arg("-c")
            .arg(mprotect_shared_writer_script(&src, &out))
            .spawn()
            .expect("spawn stopped mprotect writer");
        let mut reader = spawn_stopped_shell(&format!("read _ < {out}; exec {hit}"));

        let stop = std::sync::Arc::new(std::sync::atomic::AtomicBool::new(false));
        let (tx, rx) = std::sync::mpsc::channel();
        let run_stop = std::sync::Arc::clone(&stop);
        let run_thread = std::thread::spawn(move || {
            loader.run(&run_stop, |v| {
                let _ = tx.send(v);
            })
        });
        std::thread::sleep(std::time::Duration::from_millis(50));

        resume_and_wait(&mut writer);
        assert!(
            out_path.is_file(),
            "writer did not create the output file before reader ran"
        );
        while rx.try_recv().is_ok() {}

        resume_and_wait(&mut reader);
        let v = rx
            .recv_timeout(std::time::Duration::from_secs(2))
            .expect("LSM reader should inherit label written after mprotect");
        assert!(v.target.ends_with("apmprothit"), "target was {}", v.target);

        stop.store(true, std::sync::atomic::Ordering::Relaxed);
        run_thread
            .join()
            .expect("join ring thread")
            .expect("run loop");
        let _ = std::fs::remove_dir_all(&tmp);
    }

    #[test]
    #[ignore = "requires root/CAP_BPF, active BPF LSM, and loads live eBPF programs"]
    fn lsm_mprotect_read_upgrade_source_taints_subject_smoke() {
        if !bpf_lsm_active() {
            eprintln!("skipping LSM mprotect read-upgrade smoke: bpf LSM is not active");
            return;
        }
        let _guard = live_bpf_test_guard();
        if std::process::Command::new("python3")
            .arg("-c")
            .arg("import ctypes")
            .stdout(std::process::Stdio::null())
            .stderr(std::process::Stdio::null())
            .status()
            .is_err()
        {
            eprintln!("skipping LSM mprotect read-upgrade smoke: python3 ctypes is not available");
            return;
        }

        let label = 1;
        let tmp =
            std::env::temp_dir().join(format!("actplane-lsm-mprotect-read-{}", std::process::id()));
        std::fs::create_dir_all(&tmp).expect("create temp dir");
        let src_path = tmp.join("src");
        let hit_path = tmp.join("apmprdhit");
        std::fs::write(&src_path, "secret\n").expect("write source");
        std::fs::copy("/bin/true", &hit_path).expect("copy /bin/true");

        let src = src_path.to_string_lossy().to_string();
        let hit = hit_path.to_string_lossy().to_string();
        let mut subject = std::process::Command::new("python3")
            .arg("-c")
            .arg(mprotect_read_upgrade_exec_script(&src, &hit))
            .spawn()
            .expect("spawn stopped mprotect read-upgrade subject");
        wait_until_stopped(&subject);

        let policy = source_open_then_notify_exec_config_blob("src", "apmprdhit", label);
        let mut loader = Loader::load(&policy).expect("load eBPF engine");
        assert!(
            loader.enforce_mode(),
            "BPF LSM should be active for this test"
        );
        let subject_pid = subject.id() as i32;
        loader
            .bind_state(
                subject_pid,
                subject_pid as u32,
                CapState {
                    scope_id: 1,
                    target_mask: TARGET_SELF | TARGET_CHILD,
                    ..CapState::default()
                },
            )
            .expect("bind stopped subject domain");

        let stop = std::sync::Arc::new(std::sync::atomic::AtomicBool::new(false));
        let (tx, rx) = std::sync::mpsc::channel();
        let run_stop = std::sync::Arc::clone(&stop);
        let run_thread = std::thread::spawn(move || {
            loader.run(&run_stop, |v| {
                let _ = tx.send(v);
            })
        });
        std::thread::sleep(std::time::Duration::from_millis(50));

        resume_and_wait(&mut subject);
        let v = rx
            .recv_timeout(std::time::Duration::from_secs(2))
            .expect("LSM subject should inherit source label on mprotect read upgrade");
        assert!(v.target.ends_with("apmprdhit"), "target was {}", v.target);

        stop.store(true, std::sync::atomic::Ordering::Relaxed);
        run_thread
            .join()
            .expect("join ring thread")
            .expect("run loop");
        let _ = std::fs::remove_dir_all(&tmp);
    }

    #[test]
    #[ignore = "requires root/CAP_BPF and loads live eBPF tracepoints"]
    fn tracepoint_fd_write_after_late_read_propagates_file_label_smoke() {
        let _guard = live_bpf_test_guard();
        let label = 1;
        let caller_pid = std::process::id() as i32;
        let tmp = std::env::temp_dir().join(format!("actplane-tp-fd-flow-{}", std::process::id()));
        std::fs::create_dir_all(&tmp).expect("create temp dir");
        let src_path = tmp.join("src");
        let out_path = tmp.join("out");
        let hit_path = tmp.join("aptpfdhit");
        std::fs::write(&src_path, "secret\n").expect("write source");
        std::fs::copy("/bin/true", &hit_path).expect("copy /bin/true");

        let src = src_path.to_string_lossy().to_string();
        let out = out_path.to_string_lossy().to_string();
        let hit = hit_path.to_string_lossy().to_string();
        let policy = source_open_then_notify_exec_config_blob(&src, "aptpfdhit", label);
        let old_force = std::env::var_os("ACTPLANE_FORCE_TRACEPOINT");
        std::env::set_var("ACTPLANE_FORCE_TRACEPOINT", "1");
        let loaded = Loader::load(&policy);
        if let Some(v) = old_force {
            std::env::set_var("ACTPLANE_FORCE_TRACEPOINT", v);
        } else {
            std::env::remove_var("ACTPLANE_FORCE_TRACEPOINT");
        }
        let mut loader = loaded.expect("load tracepoint eBPF engine");
        assert!(
            !loader.enforce_mode(),
            "forced tracepoint mode should not attach BPF LSM"
        );
        loader
            .bind_state(
                caller_pid,
                caller_pid as u32,
                CapState {
                    scope_id: 1,
                    target_mask: TARGET_SELF | TARGET_CHILD,
                    ..CapState::default()
                },
            )
            .expect("bind active unlabeled caller domain");

        let mut writer =
            spawn_stopped_shell(&format!("exec 3> {out}; read _ < {src}; echo copied >&3"));
        let mut reader = spawn_stopped_shell(&format!("read _ < {out}; exec {hit}"));

        let stop = std::sync::Arc::new(std::sync::atomic::AtomicBool::new(false));
        let (tx, rx) = std::sync::mpsc::channel();
        let run_stop = std::sync::Arc::clone(&stop);
        let run_thread = std::thread::spawn(move || {
            loader.run(&run_stop, |v| {
                let _ = tx.send(v);
            })
        });
        std::thread::sleep(std::time::Duration::from_millis(50));

        resume_and_wait(&mut writer);
        assert!(
            out_path.is_file(),
            "writer did not create the output file before reader ran"
        );
        while rx.try_recv().is_ok() {}

        resume_and_wait(&mut reader);
        let v = rx
            .recv_timeout(std::time::Duration::from_secs(2))
            .expect("tracepoint reader should inherit label written through an already-open fd");
        assert!(v.target.ends_with("aptpfdhit"), "target was {}", v.target);

        stop.store(true, std::sync::atomic::Ordering::Relaxed);
        run_thread
            .join()
            .expect("join ring thread")
            .expect("run loop");
        let _ = std::fs::remove_dir_all(&tmp);
    }

    #[test]
    #[ignore = "requires root/CAP_BPF and loads live eBPF tracepoints"]
    fn tracepoint_mmap_shared_write_after_late_read_propagates_file_label_smoke() {
        let _guard = live_bpf_test_guard();
        if std::process::Command::new("python3")
            .arg("-c")
            .arg("import mmap")
            .stdout(std::process::Stdio::null())
            .stderr(std::process::Stdio::null())
            .status()
            .is_err()
        {
            eprintln!("skipping tracepoint mmap write flow smoke: python3 mmap is not available");
            return;
        }

        let label = 1;
        let caller_pid = std::process::id() as i32;
        let tmp = std::env::temp_dir().join(format!("actplane-tp-mmap-{}", std::process::id()));
        std::fs::create_dir_all(&tmp).expect("create temp dir");
        let src_path = tmp.join("src");
        let out_path = tmp.join("out");
        let hit_path = tmp.join("aptpmaphit");
        std::fs::write(&src_path, "secret\n").expect("write source");
        std::fs::copy("/bin/true", &hit_path).expect("copy /bin/true");

        let src = src_path.to_string_lossy().to_string();
        let out = out_path.to_string_lossy().to_string();
        let hit = hit_path.to_string_lossy().to_string();
        let policy = source_open_then_notify_exec_config_blob(&src, "aptpmaphit", label);
        let old_force = std::env::var_os("ACTPLANE_FORCE_TRACEPOINT");
        std::env::set_var("ACTPLANE_FORCE_TRACEPOINT", "1");
        let loaded = Loader::load(&policy);
        if let Some(v) = old_force {
            std::env::set_var("ACTPLANE_FORCE_TRACEPOINT", v);
        } else {
            std::env::remove_var("ACTPLANE_FORCE_TRACEPOINT");
        }
        let mut loader = loaded.expect("load tracepoint eBPF engine");
        assert!(
            !loader.enforce_mode(),
            "forced tracepoint mode should not attach BPF LSM"
        );
        loader
            .bind_state(
                caller_pid,
                caller_pid as u32,
                CapState {
                    scope_id: 1,
                    target_mask: TARGET_SELF | TARGET_CHILD,
                    ..CapState::default()
                },
            )
            .expect("bind active unlabeled caller domain");

        let mut writer = std::process::Command::new("python3")
            .arg("-c")
            .arg(mmap_shared_writer_script(&src, &out))
            .spawn()
            .expect("spawn stopped mmap writer");
        let mut reader = spawn_stopped_shell(&format!("read _ < {out}; exec {hit}"));

        let stop = std::sync::Arc::new(std::sync::atomic::AtomicBool::new(false));
        let (tx, rx) = std::sync::mpsc::channel();
        let run_stop = std::sync::Arc::clone(&stop);
        let run_thread = std::thread::spawn(move || {
            loader.run(&run_stop, |v| {
                let _ = tx.send(v);
            })
        });
        std::thread::sleep(std::time::Duration::from_millis(50));

        resume_and_wait(&mut writer);
        assert!(
            out_path.is_file(),
            "writer did not create the output file before reader ran"
        );
        while rx.try_recv().is_ok() {}

        resume_and_wait(&mut reader);
        let v = rx
            .recv_timeout(std::time::Duration::from_secs(2))
            .expect("tracepoint reader should inherit label written through shared mmap");
        assert!(v.target.ends_with("aptpmaphit"), "target was {}", v.target);

        stop.store(true, std::sync::atomic::Ordering::Relaxed);
        run_thread
            .join()
            .expect("join ring thread")
            .expect("run loop");
        let _ = std::fs::remove_dir_all(&tmp);
    }

    #[test]
    #[ignore = "requires root/CAP_BPF and loads live eBPF tracepoints"]
    fn tracepoint_mprotect_shared_write_after_late_read_propagates_file_label_smoke() {
        let _guard = live_bpf_test_guard();
        if std::process::Command::new("python3")
            .arg("-c")
            .arg("import ctypes")
            .stdout(std::process::Stdio::null())
            .stderr(std::process::Stdio::null())
            .status()
            .is_err()
        {
            eprintln!(
                "skipping tracepoint mprotect write flow smoke: python3 ctypes is not available"
            );
            return;
        }

        let label = 1;
        let caller_pid = std::process::id() as i32;
        let tmp = std::env::temp_dir().join(format!("actplane-tp-mprotect-{}", std::process::id()));
        std::fs::create_dir_all(&tmp).expect("create temp dir");
        let src_path = tmp.join("src");
        let out_path = tmp.join("out");
        let hit_path = tmp.join("aptpmprothit");
        std::fs::write(&src_path, "secret\n").expect("write source");
        std::fs::copy("/bin/true", &hit_path).expect("copy /bin/true");

        let src = src_path.to_string_lossy().to_string();
        let out = out_path.to_string_lossy().to_string();
        let hit = hit_path.to_string_lossy().to_string();
        let policy = source_open_then_notify_exec_config_blob(&src, "aptpmprothit", label);
        let old_force = std::env::var_os("ACTPLANE_FORCE_TRACEPOINT");
        std::env::set_var("ACTPLANE_FORCE_TRACEPOINT", "1");
        let loaded = Loader::load(&policy);
        if let Some(v) = old_force {
            std::env::set_var("ACTPLANE_FORCE_TRACEPOINT", v);
        } else {
            std::env::remove_var("ACTPLANE_FORCE_TRACEPOINT");
        }
        let mut loader = loaded.expect("load tracepoint eBPF engine");
        assert!(
            !loader.enforce_mode(),
            "forced tracepoint mode should not attach BPF LSM"
        );
        loader
            .bind_state(
                caller_pid,
                caller_pid as u32,
                CapState {
                    scope_id: 1,
                    target_mask: TARGET_SELF | TARGET_CHILD,
                    ..CapState::default()
                },
            )
            .expect("bind active unlabeled caller domain");

        let mut writer = std::process::Command::new("python3")
            .arg("-c")
            .arg(mprotect_shared_writer_script(&src, &out))
            .spawn()
            .expect("spawn stopped mprotect writer");
        let mut reader = spawn_stopped_shell(&format!("read _ < {out}; exec {hit}"));

        let stop = std::sync::Arc::new(std::sync::atomic::AtomicBool::new(false));
        let (tx, rx) = std::sync::mpsc::channel();
        let run_stop = std::sync::Arc::clone(&stop);
        let run_thread = std::thread::spawn(move || {
            loader.run(&run_stop, |v| {
                let _ = tx.send(v);
            })
        });
        std::thread::sleep(std::time::Duration::from_millis(50));

        resume_and_wait(&mut writer);
        assert!(
            out_path.is_file(),
            "writer did not create the output file before reader ran"
        );
        while rx.try_recv().is_ok() {}

        resume_and_wait(&mut reader);
        let v = rx
            .recv_timeout(std::time::Duration::from_secs(2))
            .expect("tracepoint reader should inherit label written after mprotect");
        assert!(
            v.target.ends_with("aptpmprothit"),
            "target was {}",
            v.target
        );

        stop.store(true, std::sync::atomic::Ordering::Relaxed);
        run_thread
            .join()
            .expect("join ring thread")
            .expect("run loop");
        let _ = std::fs::remove_dir_all(&tmp);
    }

    #[test]
    #[ignore = "requires root/CAP_BPF and loads live eBPF tracepoints"]
    fn tracepoint_mprotect_read_upgrade_source_taints_subject_smoke() {
        let _guard = live_bpf_test_guard();
        if std::process::Command::new("python3")
            .arg("-c")
            .arg("import ctypes")
            .stdout(std::process::Stdio::null())
            .stderr(std::process::Stdio::null())
            .status()
            .is_err()
        {
            eprintln!(
                "skipping tracepoint mprotect read-upgrade smoke: python3 ctypes is not available"
            );
            return;
        }

        let label = 1;
        let tmp =
            std::env::temp_dir().join(format!("actplane-tp-mprotect-read-{}", std::process::id()));
        std::fs::create_dir_all(&tmp).expect("create temp dir");
        let src_path = tmp.join("src");
        let hit_path = tmp.join("aptpmprdhit");
        std::fs::write(&src_path, "secret\n").expect("write source");
        std::fs::copy("/bin/true", &hit_path).expect("copy /bin/true");

        let src = src_path.to_string_lossy().to_string();
        let hit = hit_path.to_string_lossy().to_string();
        let mut subject = std::process::Command::new("python3")
            .arg("-c")
            .arg(mprotect_read_upgrade_exec_script(&src, &hit))
            .spawn()
            .expect("spawn stopped mprotect read-upgrade subject");
        wait_until_stopped(&subject);

        let policy = source_open_then_notify_exec_config_blob("src", "aptpmprdhit", label);
        let old_force = std::env::var_os("ACTPLANE_FORCE_TRACEPOINT");
        std::env::set_var("ACTPLANE_FORCE_TRACEPOINT", "1");
        let loaded = Loader::load(&policy);
        if let Some(v) = old_force {
            std::env::set_var("ACTPLANE_FORCE_TRACEPOINT", v);
        } else {
            std::env::remove_var("ACTPLANE_FORCE_TRACEPOINT");
        }
        let mut loader = loaded.expect("load tracepoint eBPF engine");
        assert!(
            !loader.enforce_mode(),
            "forced tracepoint mode should not attach BPF LSM"
        );
        let subject_pid = subject.id() as i32;
        loader
            .bind_state(
                subject_pid,
                subject_pid as u32,
                CapState {
                    scope_id: 1,
                    target_mask: TARGET_SELF | TARGET_CHILD,
                    ..CapState::default()
                },
            )
            .expect("bind stopped subject domain");

        let stop = std::sync::Arc::new(std::sync::atomic::AtomicBool::new(false));
        let (tx, rx) = std::sync::mpsc::channel();
        let run_stop = std::sync::Arc::clone(&stop);
        let run_thread = std::thread::spawn(move || {
            loader.run(&run_stop, |v| {
                let _ = tx.send(v);
            })
        });
        std::thread::sleep(std::time::Duration::from_millis(50));

        resume_and_wait(&mut subject);
        let v = rx
            .recv_timeout(std::time::Duration::from_secs(2))
            .expect("tracepoint subject should inherit source label on mprotect read upgrade");
        assert!(v.target.ends_with("aptpmprdhit"), "target was {}", v.target);

        stop.store(true, std::sync::atomic::Ordering::Relaxed);
        run_thread
            .join()
            .expect("join ring thread")
            .expect("run loop");
        let _ = std::fs::remove_dir_all(&tmp);
    }

    #[test]
    #[ignore = "requires root/CAP_BPF and loads live eBPF tracepoints"]
    fn tracepoint_mmap_exact_start_tracks_multiple_mappings_smoke() {
        let _guard = live_bpf_test_guard();
        if std::process::Command::new("python3")
            .arg("-c")
            .arg("import ctypes")
            .stdout(std::process::Stdio::null())
            .stderr(std::process::Stdio::null())
            .status()
            .is_err()
        {
            eprintln!(
                "skipping tracepoint mmap multi-mapping smoke: python3 ctypes is not available"
            );
            return;
        }

        let label = 1;
        let tmp =
            std::env::temp_dir().join(format!("actplane-tp-mmap-multi-{}", std::process::id()));
        std::fs::create_dir_all(&tmp).expect("create temp dir");
        let src_a_path = tmp.join("src_a");
        let src_b_path = tmp.join("src_b");
        let hit_path = tmp.join("aptpmmultihit");
        std::fs::write(&src_a_path, "secret-a\n").expect("write source a");
        std::fs::write(&src_b_path, "secret-b\n").expect("write source b");
        std::fs::copy("/bin/true", &hit_path).expect("copy /bin/true");

        let src_a = src_a_path.to_string_lossy().to_string();
        let src_b = src_b_path.to_string_lossy().to_string();
        let hit = hit_path.to_string_lossy().to_string();
        let mut subject = std::process::Command::new("python3")
            .arg("-c")
            .arg(mprotect_two_mapping_read_upgrade_exec_script(
                &src_a, &src_b, &hit,
            ))
            .spawn()
            .expect("spawn stopped mmap multi-mapping subject");
        wait_until_stopped(&subject);

        let policy = source_open_then_notify_exec_config_blob("src_a", "aptpmmultihit", label);
        let old_force = std::env::var_os("ACTPLANE_FORCE_TRACEPOINT");
        std::env::set_var("ACTPLANE_FORCE_TRACEPOINT", "1");
        let loaded = Loader::load(&policy);
        if let Some(v) = old_force {
            std::env::set_var("ACTPLANE_FORCE_TRACEPOINT", v);
        } else {
            std::env::remove_var("ACTPLANE_FORCE_TRACEPOINT");
        }
        let mut loader = loaded.expect("load tracepoint eBPF engine");
        assert!(
            !loader.enforce_mode(),
            "forced tracepoint mode should not attach BPF LSM"
        );
        let subject_pid = subject.id() as i32;
        loader
            .bind_state(
                subject_pid,
                subject_pid as u32,
                CapState {
                    scope_id: 1,
                    target_mask: TARGET_SELF | TARGET_CHILD,
                    ..CapState::default()
                },
            )
            .expect("bind stopped subject domain");

        let stop = std::sync::Arc::new(std::sync::atomic::AtomicBool::new(false));
        let (tx, rx) = std::sync::mpsc::channel();
        let run_stop = std::sync::Arc::clone(&stop);
        let run_thread = std::thread::spawn(move || {
            loader.run(&run_stop, |v| {
                let _ = tx.send(v);
            })
        });
        std::thread::sleep(std::time::Duration::from_millis(50));

        resume_and_wait(&mut subject);
        let v = rx
            .recv_timeout(std::time::Duration::from_secs(2))
            .expect("tracepoint subject should keep the older mmap record after a later mmap");
        assert!(
            v.target.ends_with("aptpmmultihit"),
            "target was {}",
            v.target
        );

        stop.store(true, std::sync::atomic::Ordering::Relaxed);
        run_thread
            .join()
            .expect("join ring thread")
            .expect("run loop");
        let _ = std::fs::remove_dir_all(&tmp);
    }

    #[test]
    #[ignore = "requires root/CAP_BPF and loads live eBPF tracepoints"]
    fn tracepoint_mremap_shared_write_after_late_read_propagates_file_label_smoke() {
        let _guard = live_bpf_test_guard();
        if std::process::Command::new("python3")
            .arg("-c")
            .arg("import ctypes")
            .stdout(std::process::Stdio::null())
            .stderr(std::process::Stdio::null())
            .status()
            .is_err()
        {
            eprintln!(
                "skipping tracepoint mremap write flow smoke: python3 ctypes is not available"
            );
            return;
        }

        let label = 1;
        let caller_pid = std::process::id() as i32;
        let tmp = std::env::temp_dir().join(format!("actplane-tp-mremap-{}", std::process::id()));
        std::fs::create_dir_all(&tmp).expect("create temp dir");
        let src_path = tmp.join("src");
        let out_path = tmp.join("out");
        let hit_path = tmp.join("aptpmremaphit");
        std::fs::write(&src_path, "secret\n").expect("write source");
        std::fs::copy("/bin/true", &hit_path).expect("copy /bin/true");

        let src = src_path.to_string_lossy().to_string();
        let out = out_path.to_string_lossy().to_string();
        let hit = hit_path.to_string_lossy().to_string();
        let policy = source_open_then_notify_exec_config_blob(&src, "aptpmremaphit", label);
        let old_force = std::env::var_os("ACTPLANE_FORCE_TRACEPOINT");
        std::env::set_var("ACTPLANE_FORCE_TRACEPOINT", "1");
        let loaded = Loader::load(&policy);
        if let Some(v) = old_force {
            std::env::set_var("ACTPLANE_FORCE_TRACEPOINT", v);
        } else {
            std::env::remove_var("ACTPLANE_FORCE_TRACEPOINT");
        }
        let mut loader = loaded.expect("load tracepoint eBPF engine");
        assert!(
            !loader.enforce_mode(),
            "forced tracepoint mode should not attach BPF LSM"
        );
        loader
            .bind_state(
                caller_pid,
                caller_pid as u32,
                CapState {
                    scope_id: 1,
                    target_mask: TARGET_SELF | TARGET_CHILD,
                    ..CapState::default()
                },
            )
            .expect("bind active unlabeled caller domain");

        let mut writer = std::process::Command::new("python3")
            .arg("-c")
            .arg(mremap_shared_writer_script(&src, &out))
            .spawn()
            .expect("spawn stopped mremap writer");
        let mut reader = spawn_stopped_shell(&format!("read _ < {out}; exec {hit}"));

        let stop = std::sync::Arc::new(std::sync::atomic::AtomicBool::new(false));
        let (tx, rx) = std::sync::mpsc::channel();
        let run_stop = std::sync::Arc::clone(&stop);
        let run_thread = std::thread::spawn(move || {
            loader.run(&run_stop, |v| {
                let _ = tx.send(v);
            })
        });
        std::thread::sleep(std::time::Duration::from_millis(50));

        resume_and_wait(&mut writer);
        assert!(
            out_path.is_file(),
            "writer did not create the output file before reader ran"
        );
        while rx.try_recv().is_ok() {}

        resume_and_wait(&mut reader);
        let v = rx
            .recv_timeout(std::time::Duration::from_secs(2))
            .expect("tracepoint reader should inherit label written after mremap");
        assert!(
            v.target.ends_with("aptpmremaphit"),
            "target was {}",
            v.target
        );

        stop.store(true, std::sync::atomic::Ordering::Relaxed);
        run_thread
            .join()
            .expect("join ring thread")
            .expect("run loop");
        let _ = std::fs::remove_dir_all(&tmp);
    }

    #[test]
    #[ignore = "requires root/CAP_BPF and loads live eBPF tracepoints"]
    fn tracepoint_fcntl_dup_fd_flow_smoke() {
        let _guard = live_bpf_test_guard();
        if std::process::Command::new("python3")
            .arg("--version")
            .stdout(std::process::Stdio::null())
            .stderr(std::process::Stdio::null())
            .status()
            .is_err()
        {
            eprintln!("skipping fcntl fd-flow smoke: python3 is not available");
            return;
        }

        let label = 1;
        let caller_pid = std::process::id() as i32;
        let tmp = std::env::temp_dir().join(format!("actplane-tp-fcntl-{}", std::process::id()));
        std::fs::create_dir_all(&tmp).expect("create temp dir");
        let src_path = tmp.join("src");
        let out_path = tmp.join("out");
        let hit_path = tmp.join("aptpfcntlhit");
        std::fs::write(&src_path, "secret\n").expect("write source");
        std::fs::copy("/bin/true", &hit_path).expect("copy /bin/true");

        let src = src_path.to_string_lossy().to_string();
        let out = out_path.to_string_lossy().to_string();
        let hit = hit_path.to_string_lossy().to_string();
        let policy = source_open_then_notify_exec_config_blob(&src, "aptpfcntlhit", label);
        let old_force = std::env::var_os("ACTPLANE_FORCE_TRACEPOINT");
        std::env::set_var("ACTPLANE_FORCE_TRACEPOINT", "1");
        let loaded = Loader::load(&policy);
        if let Some(v) = old_force {
            std::env::set_var("ACTPLANE_FORCE_TRACEPOINT", v);
        } else {
            std::env::remove_var("ACTPLANE_FORCE_TRACEPOINT");
        }
        let mut loader = loaded.expect("load tracepoint eBPF engine");
        assert!(
            !loader.enforce_mode(),
            "forced tracepoint mode should not attach BPF LSM"
        );
        loader
            .bind_state(
                caller_pid,
                caller_pid as u32,
                CapState {
                    scope_id: 1,
                    target_mask: TARGET_SELF | TARGET_CHILD,
                    ..CapState::default()
                },
            )
            .expect("bind active unlabeled caller domain");

        let py = format!(
            r#"import fcntl, os, signal
os.kill(os.getpid(), signal.SIGSTOP)
out_fd = os.open({out:?}, os.O_WRONLY | os.O_CREAT | os.O_TRUNC, 0o600)
with open({src:?}, "rb") as src:
    src.read()
dup_fd = fcntl.fcntl(out_fd, fcntl.F_DUPFD, 10)
os.write(dup_fd, b"copied\n")
os.close(dup_fd)
os.close(out_fd)
"#
        );
        let mut writer = std::process::Command::new("python3")
            .arg("-c")
            .arg(py)
            .spawn()
            .expect("spawn stopped fcntl writer");
        let mut reader = spawn_stopped_shell(&format!("read _ < {out}; exec {hit}"));

        let stop = std::sync::Arc::new(std::sync::atomic::AtomicBool::new(false));
        let (tx, rx) = std::sync::mpsc::channel();
        let run_stop = std::sync::Arc::clone(&stop);
        let run_thread = std::thread::spawn(move || {
            loader.run(&run_stop, |v| {
                let _ = tx.send(v);
            })
        });
        std::thread::sleep(std::time::Duration::from_millis(50));

        resume_and_wait(&mut writer);
        assert!(
            out_path.is_file(),
            "writer did not create the output file before reader ran"
        );
        while rx.try_recv().is_ok() {}

        resume_and_wait(&mut reader);
        let v = rx
            .recv_timeout(std::time::Duration::from_secs(2))
            .expect("tracepoint reader should inherit label written through fcntl-duplicated fd");
        assert!(
            v.target.ends_with("aptpfcntlhit"),
            "target was {}",
            v.target
        );

        stop.store(true, std::sync::atomic::Ordering::Relaxed);
        run_thread
            .join()
            .expect("join ring thread")
            .expect("run loop");
        let _ = std::fs::remove_dir_all(&tmp);
    }

    #[test]
    #[ignore = "requires root/CAP_BPF and loads live eBPF tracepoints"]
    fn tracepoint_sendfile_fd_flow_smoke() {
        let _guard = live_bpf_test_guard();
        let python_has_sendfile = std::process::Command::new("python3")
            .arg("-c")
            .arg("import os; raise SystemExit(0 if hasattr(os, 'sendfile') else 1)")
            .stdout(std::process::Stdio::null())
            .stderr(std::process::Stdio::null())
            .status()
            .map(|s| s.success())
            .unwrap_or(false);
        if !python_has_sendfile {
            eprintln!("skipping sendfile fd-flow smoke: python3 os.sendfile is not available");
            return;
        }

        let label = 1;
        let caller_pid = std::process::id() as i32;
        let tmp = std::env::temp_dir().join(format!("actplane-tp-sendfile-{}", std::process::id()));
        std::fs::create_dir_all(&tmp).expect("create temp dir");
        let src_path = tmp.join("src");
        let out_path = tmp.join("out");
        let hit_path = tmp.join("aptpsendfilehit");
        std::fs::write(&src_path, "secret\n").expect("write source");
        std::fs::copy("/bin/true", &hit_path).expect("copy /bin/true");

        let src = src_path.to_string_lossy().to_string();
        let out = out_path.to_string_lossy().to_string();
        let hit = hit_path.to_string_lossy().to_string();
        let policy = source_open_then_notify_exec_config_blob(&src, "aptpsendfilehit", label);
        let old_force = std::env::var_os("ACTPLANE_FORCE_TRACEPOINT");
        std::env::set_var("ACTPLANE_FORCE_TRACEPOINT", "1");
        let loaded = Loader::load(&policy);
        if let Some(v) = old_force {
            std::env::set_var("ACTPLANE_FORCE_TRACEPOINT", v);
        } else {
            std::env::remove_var("ACTPLANE_FORCE_TRACEPOINT");
        }
        let mut loader = loaded.expect("load tracepoint eBPF engine");
        assert!(
            !loader.enforce_mode(),
            "forced tracepoint mode should not attach BPF LSM"
        );
        loader
            .bind_state(
                caller_pid,
                caller_pid as u32,
                CapState {
                    scope_id: 1,
                    target_mask: TARGET_SELF | TARGET_CHILD,
                    ..CapState::default()
                },
            )
            .expect("bind active unlabeled caller domain");

        let py = format!(
            r#"import os, signal
os.kill(os.getpid(), signal.SIGSTOP)
out_fd = os.open({out:?}, os.O_WRONLY | os.O_CREAT | os.O_TRUNC, 0o600)
src_fd = os.open({src:?}, os.O_RDONLY)
try:
    os.sendfile(out_fd, src_fd, 0, 4096)
finally:
    os.close(src_fd)
    os.close(out_fd)
"#
        );
        let mut writer = std::process::Command::new("python3")
            .arg("-c")
            .arg(py)
            .spawn()
            .expect("spawn stopped sendfile writer");
        let mut reader = spawn_stopped_shell(&format!("read _ < {out}; exec {hit}"));

        let stop = std::sync::Arc::new(std::sync::atomic::AtomicBool::new(false));
        let (tx, rx) = std::sync::mpsc::channel();
        let run_stop = std::sync::Arc::clone(&stop);
        let run_thread = std::thread::spawn(move || {
            loader.run(&run_stop, |v| {
                let _ = tx.send(v);
            })
        });
        std::thread::sleep(std::time::Duration::from_millis(50));

        resume_and_wait(&mut writer);
        assert!(
            out_path.is_file(),
            "writer did not create the output file before reader ran"
        );
        while rx.try_recv().is_ok() {}

        resume_and_wait(&mut reader);
        let v = rx
            .recv_timeout(std::time::Duration::from_secs(2))
            .expect("tracepoint reader should inherit label written through sendfile");
        assert!(
            v.target.ends_with("aptpsendfilehit"),
            "target was {}",
            v.target
        );

        stop.store(true, std::sync::atomic::Ordering::Relaxed);
        run_thread
            .join()
            .expect("join ring thread")
            .expect("run loop");
        let _ = std::fs::remove_dir_all(&tmp);
    }

    #[test]
    #[ignore = "requires root/CAP_BPF and loads live eBPF tracepoints"]
    fn tracepoint_copy_file_range_fd_flow_smoke() {
        let _guard = live_bpf_test_guard();
        let python_has_copy_file_range = std::process::Command::new("python3")
            .arg("-c")
            .arg("import os; raise SystemExit(0 if hasattr(os, 'copy_file_range') else 1)")
            .stdout(std::process::Stdio::null())
            .stderr(std::process::Stdio::null())
            .status()
            .map(|s| s.success())
            .unwrap_or(false);
        if !python_has_copy_file_range {
            eprintln!(
                "skipping copy_file_range fd-flow smoke: python3 os.copy_file_range is not available"
            );
            return;
        }

        let label = 1;
        let caller_pid = std::process::id() as i32;
        let tmp = std::env::temp_dir().join(format!(
            "actplane-tp-copy-file-range-{}",
            std::process::id()
        ));
        std::fs::create_dir_all(&tmp).expect("create temp dir");
        let src_path = tmp.join("src");
        let out_path = tmp.join("out");
        let hit_path = tmp.join("aptpcfrhit");
        std::fs::write(&src_path, "secret\n").expect("write source");
        std::fs::copy("/bin/true", &hit_path).expect("copy /bin/true");

        let src = src_path.to_string_lossy().to_string();
        let out = out_path.to_string_lossy().to_string();
        let hit = hit_path.to_string_lossy().to_string();
        let policy = source_open_then_notify_exec_config_blob(&src, "aptpcfrhit", label);
        let old_force = std::env::var_os("ACTPLANE_FORCE_TRACEPOINT");
        std::env::set_var("ACTPLANE_FORCE_TRACEPOINT", "1");
        let loaded = Loader::load(&policy);
        if let Some(v) = old_force {
            std::env::set_var("ACTPLANE_FORCE_TRACEPOINT", v);
        } else {
            std::env::remove_var("ACTPLANE_FORCE_TRACEPOINT");
        }
        let mut loader = loaded.expect("load tracepoint eBPF engine");
        assert!(
            !loader.enforce_mode(),
            "forced tracepoint mode should not attach BPF LSM"
        );
        loader
            .bind_state(
                caller_pid,
                caller_pid as u32,
                CapState {
                    scope_id: 1,
                    target_mask: TARGET_SELF | TARGET_CHILD,
                    ..CapState::default()
                },
            )
            .expect("bind active unlabeled caller domain");

        let py = format!(
            r#"import os, signal
os.kill(os.getpid(), signal.SIGSTOP)
out_fd = os.open({out:?}, os.O_WRONLY | os.O_CREAT | os.O_TRUNC, 0o600)
src_fd = os.open({src:?}, os.O_RDONLY)
try:
    os.copy_file_range(src_fd, out_fd, 4096)
finally:
    os.close(src_fd)
    os.close(out_fd)
"#
        );
        let mut writer = std::process::Command::new("python3")
            .arg("-c")
            .arg(py)
            .spawn()
            .expect("spawn stopped copy_file_range writer");
        let mut reader = spawn_stopped_shell(&format!("read _ < {out}; exec {hit}"));

        let stop = std::sync::Arc::new(std::sync::atomic::AtomicBool::new(false));
        let (tx, rx) = std::sync::mpsc::channel();
        let run_stop = std::sync::Arc::clone(&stop);
        let run_thread = std::thread::spawn(move || {
            loader.run(&run_stop, |v| {
                let _ = tx.send(v);
            })
        });
        std::thread::sleep(std::time::Duration::from_millis(50));

        resume_and_wait(&mut writer);
        assert!(
            out_path.is_file(),
            "writer did not create the output file before reader ran"
        );
        while rx.try_recv().is_ok() {}

        resume_and_wait(&mut reader);
        let v = rx
            .recv_timeout(std::time::Duration::from_secs(2))
            .expect("tracepoint reader should inherit label written through copy_file_range");
        assert!(v.target.ends_with("aptpcfrhit"), "target was {}", v.target);

        stop.store(true, std::sync::atomic::Ordering::Relaxed);
        run_thread
            .join()
            .expect("join ring thread")
            .expect("run loop");
        let _ = std::fs::remove_dir_all(&tmp);
    }

    #[test]
    #[ignore = "requires root/CAP_BPF and loads live eBPF tracepoints"]
    fn tracepoint_pipe_fork_fd_flow_smoke() {
        let _guard = live_bpf_test_guard();
        if std::process::Command::new("python3")
            .arg("--version")
            .stdout(std::process::Stdio::null())
            .stderr(std::process::Stdio::null())
            .status()
            .is_err()
        {
            eprintln!("skipping pipe fd-flow smoke: python3 is not available");
            return;
        }

        let label = 1;
        let caller_pid = std::process::id() as i32;
        let tmp = std::env::temp_dir().join(format!("actplane-tp-pipe-{}", std::process::id()));
        std::fs::create_dir_all(&tmp).expect("create temp dir");
        let src_path = tmp.join("src");
        let hit_path = tmp.join("aptppipehit");
        std::fs::write(&src_path, "secret\n").expect("write source");
        std::fs::copy("/bin/true", &hit_path).expect("copy /bin/true");

        let src = src_path.to_string_lossy().to_string();
        let hit = hit_path.to_string_lossy().to_string();
        let policy = source_open_then_notify_exec_config_blob(&src, "aptppipehit", label);
        let old_force = std::env::var_os("ACTPLANE_FORCE_TRACEPOINT");
        std::env::set_var("ACTPLANE_FORCE_TRACEPOINT", "1");
        let loaded = Loader::load(&policy);
        if let Some(v) = old_force {
            std::env::set_var("ACTPLANE_FORCE_TRACEPOINT", v);
        } else {
            std::env::remove_var("ACTPLANE_FORCE_TRACEPOINT");
        }
        let mut loader = loaded.expect("load tracepoint eBPF engine");
        assert!(
            !loader.enforce_mode(),
            "forced tracepoint mode should not attach BPF LSM"
        );
        loader
            .bind_state(
                caller_pid,
                caller_pid as u32,
                CapState {
                    scope_id: 1,
                    target_mask: TARGET_SELF | TARGET_CHILD,
                    ..CapState::default()
                },
            )
            .expect("bind active unlabeled caller domain");

        let py = format!(
            r#"import os, signal, sys
os.kill(os.getpid(), signal.SIGSTOP)
r_fd, w_fd = os.pipe()
pid = os.fork()
if pid == 0:
    os.close(w_fd)
    os.read(r_fd, 4096)
    os.close(r_fd)
    os.execv({hit:?}, [{hit:?}])
os.close(r_fd)
with open({src:?}, "rb") as src:
    src.read()
os.write(w_fd, b"x")
os.close(w_fd)
_, status = os.waitpid(pid, 0)
raise SystemExit(0 if status == 0 else 1)
"#
        );
        let mut actor = std::process::Command::new("python3")
            .arg("-c")
            .arg(py)
            .spawn()
            .expect("spawn stopped pipe actor");

        let stop = std::sync::Arc::new(std::sync::atomic::AtomicBool::new(false));
        let (tx, rx) = std::sync::mpsc::channel();
        let run_stop = std::sync::Arc::clone(&stop);
        let run_thread = std::thread::spawn(move || {
            loader.run(&run_stop, |v| {
                let _ = tx.send(v);
            })
        });
        std::thread::sleep(std::time::Duration::from_millis(50));
        while rx.try_recv().is_ok() {}

        resume_and_wait(&mut actor);
        let v = rx
            .recv_timeout(std::time::Duration::from_secs(2))
            .expect("forked child should inherit label read through pipe");
        assert!(v.target.ends_with("aptppipehit"), "target was {}", v.target);

        stop.store(true, std::sync::atomic::Ordering::Relaxed);
        run_thread
            .join()
            .expect("join ring thread")
            .expect("run loop");
        let _ = std::fs::remove_dir_all(&tmp);
    }

    #[test]
    #[ignore = "requires root/CAP_BPF and loads live eBPF tracepoints"]
    fn tracepoint_socketpair_fork_fd_flow_smoke() {
        let _guard = live_bpf_test_guard();
        if std::process::Command::new("python3")
            .arg("--version")
            .stdout(std::process::Stdio::null())
            .stderr(std::process::Stdio::null())
            .status()
            .is_err()
        {
            eprintln!("skipping socketpair fd-flow smoke: python3 is not available");
            return;
        }

        let label = 1;
        let caller_pid = std::process::id() as i32;
        let tmp =
            std::env::temp_dir().join(format!("actplane-tp-socketpair-{}", std::process::id()));
        std::fs::create_dir_all(&tmp).expect("create temp dir");
        let src_path = tmp.join("src");
        let hit_path = tmp.join("aptpsockhit");
        std::fs::write(&src_path, "secret\n").expect("write source");
        std::fs::copy("/bin/true", &hit_path).expect("copy /bin/true");

        let src = src_path.to_string_lossy().to_string();
        let hit = hit_path.to_string_lossy().to_string();
        let policy = source_open_then_notify_exec_config_blob(&src, "aptpsockhit", label);
        let old_force = std::env::var_os("ACTPLANE_FORCE_TRACEPOINT");
        std::env::set_var("ACTPLANE_FORCE_TRACEPOINT", "1");
        let loaded = Loader::load(&policy);
        if let Some(v) = old_force {
            std::env::set_var("ACTPLANE_FORCE_TRACEPOINT", v);
        } else {
            std::env::remove_var("ACTPLANE_FORCE_TRACEPOINT");
        }
        let mut loader = loaded.expect("load tracepoint eBPF engine");
        assert!(
            !loader.enforce_mode(),
            "forced tracepoint mode should not attach BPF LSM"
        );
        loader
            .bind_state(
                caller_pid,
                caller_pid as u32,
                CapState {
                    scope_id: 1,
                    target_mask: TARGET_SELF | TARGET_CHILD,
                    ..CapState::default()
                },
            )
            .expect("bind active unlabeled caller domain");

        let py = format!(
            r#"import os, signal, socket
os.kill(os.getpid(), signal.SIGSTOP)
left, right = socket.socketpair()
pid = os.fork()
if pid == 0:
    left.close()
    right.recv(4096)
    right.close()
    os.execv({hit:?}, [{hit:?}])
right.close()
with open({src:?}, "rb") as src:
    src.read()
left.send(b"x")
left.close()
_, status = os.waitpid(pid, 0)
raise SystemExit(0 if status == 0 else 1)
"#
        );
        let mut actor = std::process::Command::new("python3")
            .arg("-c")
            .arg(py)
            .spawn()
            .expect("spawn stopped socketpair actor");

        let stop = std::sync::Arc::new(std::sync::atomic::AtomicBool::new(false));
        let (tx, rx) = std::sync::mpsc::channel();
        let run_stop = std::sync::Arc::clone(&stop);
        let run_thread = std::thread::spawn(move || {
            loader.run(&run_stop, |v| {
                let _ = tx.send(v);
            })
        });
        std::thread::sleep(std::time::Duration::from_millis(50));
        while rx.try_recv().is_ok() {}

        resume_and_wait(&mut actor);
        let v = rx
            .recv_timeout(std::time::Duration::from_secs(2))
            .expect("forked child should inherit label received through socketpair");
        assert!(v.target.ends_with("aptpsockhit"), "target was {}", v.target);

        stop.store(true, std::sync::atomic::Ordering::Relaxed);
        run_thread
            .join()
            .expect("join ring thread")
            .expect("run loop");
        let _ = std::fs::remove_dir_all(&tmp);
    }

    #[test]
    #[ignore = "requires root/CAP_BPF and loads live eBPF tracepoints"]
    fn tracepoint_unix_path_socket_fork_fd_flow_smoke() {
        let _guard = live_bpf_test_guard();
        if std::process::Command::new("python3")
            .arg("--version")
            .stdout(std::process::Stdio::null())
            .stderr(std::process::Stdio::null())
            .status()
            .is_err()
        {
            eprintln!("skipping pathname unix socket smoke: python3 is not available");
            return;
        }

        let label = 1;
        let caller_pid = std::process::id() as i32;
        let tmp = std::env::temp_dir().join(format!("actplane-tp-unix-{}", std::process::id()));
        std::fs::create_dir_all(&tmp).expect("create temp dir");
        let src_path = tmp.join("src");
        let sock_path = tmp.join("sock");
        let hit_path = tmp.join("aptpunixhit");
        std::fs::write(&src_path, "secret\n").expect("write source");
        std::fs::copy("/bin/true", &hit_path).expect("copy /bin/true");

        let src = src_path.to_string_lossy().to_string();
        let sock = sock_path.to_string_lossy().to_string();
        let hit = hit_path.to_string_lossy().to_string();
        let policy = source_open_then_notify_exec_config_blob(&src, "aptpunixhit", label);
        let old_force = std::env::var_os("ACTPLANE_FORCE_TRACEPOINT");
        std::env::set_var("ACTPLANE_FORCE_TRACEPOINT", "1");
        let loaded = Loader::load(&policy);
        if let Some(v) = old_force {
            std::env::set_var("ACTPLANE_FORCE_TRACEPOINT", v);
        } else {
            std::env::remove_var("ACTPLANE_FORCE_TRACEPOINT");
        }
        let mut loader = loaded.expect("load tracepoint eBPF engine");
        assert!(
            !loader.enforce_mode(),
            "forced tracepoint mode should not attach BPF LSM"
        );
        loader
            .bind_state(
                caller_pid,
                caller_pid as u32,
                CapState {
                    scope_id: 1,
                    target_mask: TARGET_SELF | TARGET_CHILD,
                    ..CapState::default()
                },
            )
            .expect("bind active unlabeled caller domain");

        let py = format!(
            r#"import os, signal, socket, time
os.kill(os.getpid(), signal.SIGSTOP)
try:
    os.unlink({sock:?})
except FileNotFoundError:
    pass
srv = socket.socket(socket.AF_UNIX, socket.SOCK_STREAM)
srv.bind({sock:?})
srv.listen(1)
pid = os.fork()
if pid == 0:
    conn, _ = srv.accept()
    conn.recv(4096)
    conn.close()
    srv.close()
    os.execv({hit:?}, [{hit:?}])
with open({src:?}, "rb") as src:
    src.read()
cli = socket.socket(socket.AF_UNIX, socket.SOCK_STREAM)
for _ in range(100):
    try:
        cli.connect({sock:?})
        break
    except OSError:
        time.sleep(0.01)
else:
    raise RuntimeError("connect failed")
cli.sendall(b"x")
cli.close()
srv.close()
_, status = os.waitpid(pid, 0)
raise SystemExit(0 if status == 0 else 1)
"#
        );
        let mut actor = std::process::Command::new("python3")
            .arg("-c")
            .arg(py)
            .spawn()
            .expect("spawn stopped pathname unix socket actor");

        let stop = std::sync::Arc::new(std::sync::atomic::AtomicBool::new(false));
        let (tx, rx) = std::sync::mpsc::channel();
        let run_stop = std::sync::Arc::clone(&stop);
        let run_thread = std::thread::spawn(move || {
            loader.run(&run_stop, |v| {
                let _ = tx.send(v);
            })
        });
        std::thread::sleep(std::time::Duration::from_millis(50));
        while rx.try_recv().is_ok() {}

        resume_and_wait(&mut actor);
        let v = rx
            .recv_timeout(std::time::Duration::from_secs(2))
            .expect("forked child should inherit label received through pathname unix socket");
        assert!(v.target.ends_with("aptpunixhit"), "target was {}", v.target);

        stop.store(true, std::sync::atomic::Ordering::Relaxed);
        run_thread
            .join()
            .expect("join ring thread")
            .expect("run loop");
        let _ = std::fs::remove_dir_all(&tmp);
    }

    #[test]
    #[ignore = "requires root/CAP_BPF and loads live eBPF tracepoints"]
    fn tracepoint_unix_abstract_socket_fork_fd_flow_smoke() {
        let _guard = live_bpf_test_guard();
        if std::process::Command::new("python3")
            .arg("--version")
            .stdout(std::process::Stdio::null())
            .stderr(std::process::Stdio::null())
            .status()
            .is_err()
        {
            eprintln!("skipping abstract unix socket smoke: python3 is not available");
            return;
        }

        let label = 1;
        let caller_pid = std::process::id() as i32;
        let tmp = std::env::temp_dir().join(format!("actplane-tp-unix-abs-{}", std::process::id()));
        std::fs::create_dir_all(&tmp).expect("create temp dir");
        let src_path = tmp.join("src");
        let hit_path = tmp.join("aptpabshit");
        std::fs::write(&src_path, "secret\n").expect("write source");
        std::fs::copy("/bin/true", &hit_path).expect("copy /bin/true");

        let src = src_path.to_string_lossy().to_string();
        let hit = hit_path.to_string_lossy().to_string();
        let addr = format!("\0actplane-abstract-{}", std::process::id());
        let policy = source_open_then_notify_exec_config_blob(&src, "aptpabshit", label);
        let old_force = std::env::var_os("ACTPLANE_FORCE_TRACEPOINT");
        std::env::set_var("ACTPLANE_FORCE_TRACEPOINT", "1");
        let loaded = Loader::load(&policy);
        if let Some(v) = old_force {
            std::env::set_var("ACTPLANE_FORCE_TRACEPOINT", v);
        } else {
            std::env::remove_var("ACTPLANE_FORCE_TRACEPOINT");
        }
        let mut loader = loaded.expect("load tracepoint eBPF engine");
        assert!(
            !loader.enforce_mode(),
            "forced tracepoint mode should not attach BPF LSM"
        );
        loader
            .bind_state(
                caller_pid,
                caller_pid as u32,
                CapState {
                    scope_id: 1,
                    target_mask: TARGET_SELF | TARGET_CHILD,
                    ..CapState::default()
                },
            )
            .expect("bind active unlabeled caller domain");

        let py = format!(
            r#"import os, signal, socket, time
addr = {addr:?}
os.kill(os.getpid(), signal.SIGSTOP)
srv = socket.socket(socket.AF_UNIX, socket.SOCK_STREAM)
srv.bind(addr)
srv.listen(1)
pid = os.fork()
if pid == 0:
    conn, _ = srv.accept()
    conn.recv(4096)
    conn.close()
    srv.close()
    os.execv({hit:?}, [{hit:?}])
with open({src:?}, "rb") as src:
    src.read()
cli = socket.socket(socket.AF_UNIX, socket.SOCK_STREAM)
for _ in range(100):
    try:
        cli.connect(addr)
        break
    except OSError:
        time.sleep(0.01)
else:
    raise RuntimeError("connect failed")
cli.sendall(b"x")
cli.close()
srv.close()
_, status = os.waitpid(pid, 0)
raise SystemExit(0 if status == 0 else 1)
"#
        );
        let mut actor = std::process::Command::new("python3")
            .arg("-c")
            .arg(py)
            .spawn()
            .expect("spawn stopped abstract unix socket actor");

        let stop = std::sync::Arc::new(std::sync::atomic::AtomicBool::new(false));
        let (tx, rx) = std::sync::mpsc::channel();
        let run_stop = std::sync::Arc::clone(&stop);
        let run_thread = std::thread::spawn(move || {
            loader.run(&run_stop, |v| {
                let _ = tx.send(v);
            })
        });
        std::thread::sleep(std::time::Duration::from_millis(50));
        while rx.try_recv().is_ok() {}

        resume_and_wait(&mut actor);
        let v = rx
            .recv_timeout(std::time::Duration::from_secs(2))
            .expect("forked child should inherit label received through abstract unix socket");
        assert!(v.target.ends_with("aptpabshit"), "target was {}", v.target);

        stop.store(true, std::sync::atomic::Ordering::Relaxed);
        run_thread
            .join()
            .expect("join ring thread")
            .expect("run loop");
        let _ = std::fs::remove_dir_all(&tmp);
    }

    #[test]
    #[ignore = "requires root/CAP_BPF and loads live eBPF tracepoints"]
    fn tracepoint_unix_dgram_sendto_path_flow_smoke() {
        let _guard = live_bpf_test_guard();
        if std::process::Command::new("python3")
            .arg("--version")
            .stdout(std::process::Stdio::null())
            .stderr(std::process::Stdio::null())
            .status()
            .is_err()
        {
            eprintln!("skipping pathname unix datagram smoke: python3 is not available");
            return;
        }

        let label = 1;
        let caller_pid = std::process::id() as i32;
        let tmp =
            std::env::temp_dir().join(format!("actplane-tp-unix-dgram-{}", std::process::id()));
        std::fs::create_dir_all(&tmp).expect("create temp dir");
        let src_path = tmp.join("src");
        let sock_path = tmp.join("sock");
        let hit_path = tmp.join("aptpdgrhit");
        std::fs::write(&src_path, "secret\n").expect("write source");
        std::fs::copy("/bin/true", &hit_path).expect("copy /bin/true");

        let src = src_path.to_string_lossy().to_string();
        let sock = sock_path.to_string_lossy().to_string();
        let hit = hit_path.to_string_lossy().to_string();
        let policy = source_open_then_notify_exec_config_blob(&src, "aptpdgrhit", label);
        let old_force = std::env::var_os("ACTPLANE_FORCE_TRACEPOINT");
        std::env::set_var("ACTPLANE_FORCE_TRACEPOINT", "1");
        let loaded = Loader::load(&policy);
        if let Some(v) = old_force {
            std::env::set_var("ACTPLANE_FORCE_TRACEPOINT", v);
        } else {
            std::env::remove_var("ACTPLANE_FORCE_TRACEPOINT");
        }
        let mut loader = loaded.expect("load tracepoint eBPF engine");
        assert!(
            !loader.enforce_mode(),
            "forced tracepoint mode should not attach BPF LSM"
        );
        loader
            .bind_state(
                caller_pid,
                caller_pid as u32,
                CapState {
                    scope_id: 1,
                    target_mask: TARGET_SELF | TARGET_CHILD,
                    ..CapState::default()
                },
            )
            .expect("bind active unlabeled caller domain");

        let py = format!(
            r#"import os, signal, socket
os.kill(os.getpid(), signal.SIGSTOP)
try:
    os.unlink({sock:?})
except FileNotFoundError:
    pass
srv = socket.socket(socket.AF_UNIX, socket.SOCK_DGRAM)
srv.bind({sock:?})
pid = os.fork()
if pid == 0:
    srv.recvfrom(4096)
    srv.close()
    os.execv({hit:?}, [{hit:?}])
with open({src:?}, "rb") as src:
    src.read()
cli = socket.socket(socket.AF_UNIX, socket.SOCK_DGRAM)
cli.sendto(b"x", {sock:?})
cli.close()
srv.close()
_, status = os.waitpid(pid, 0)
raise SystemExit(0 if status == 0 else 1)
"#
        );
        let mut actor = std::process::Command::new("python3")
            .arg("-c")
            .arg(py)
            .spawn()
            .expect("spawn stopped pathname unix datagram actor");

        let stop = std::sync::Arc::new(std::sync::atomic::AtomicBool::new(false));
        let (tx, rx) = std::sync::mpsc::channel();
        let run_stop = std::sync::Arc::clone(&stop);
        let run_thread = std::thread::spawn(move || {
            loader.run(&run_stop, |v| {
                let _ = tx.send(v);
            })
        });
        std::thread::sleep(std::time::Duration::from_millis(50));
        while rx.try_recv().is_ok() {}

        resume_and_wait(&mut actor);
        let v = rx
            .recv_timeout(std::time::Duration::from_secs(2))
            .expect("forked child should inherit label received through unix datagram sendto");
        assert!(v.target.ends_with("aptpdgrhit"), "target was {}", v.target);

        stop.store(true, std::sync::atomic::Ordering::Relaxed);
        run_thread
            .join()
            .expect("join ring thread")
            .expect("run loop");
        let _ = std::fs::remove_dir_all(&tmp);
    }

    #[test]
    #[ignore = "requires root/CAP_BPF and loads live eBPF tracepoints"]
    fn tracepoint_unix_dgram_sendmsg_abstract_flow_smoke() {
        let _guard = live_bpf_test_guard();
        let python_has_sendmsg = std::process::Command::new("python3")
            .arg("-c")
            .arg("import socket; raise SystemExit(0 if hasattr(socket.socket, 'sendmsg') else 1)")
            .stdout(std::process::Stdio::null())
            .stderr(std::process::Stdio::null())
            .status()
            .map(|s| s.success())
            .unwrap_or(false);
        if !python_has_sendmsg {
            eprintln!("skipping abstract unix datagram sendmsg smoke: python3 socket.sendmsg is unavailable");
            return;
        }

        let label = 1;
        let caller_pid = std::process::id() as i32;
        let tmp = std::env::temp_dir().join(format!("actplane-tp-unix-msg-{}", std::process::id()));
        std::fs::create_dir_all(&tmp).expect("create temp dir");
        let src_path = tmp.join("src");
        let hit_path = tmp.join("aptpmsgdhit");
        std::fs::write(&src_path, "secret\n").expect("write source");
        std::fs::copy("/bin/true", &hit_path).expect("copy /bin/true");

        let src = src_path.to_string_lossy().to_string();
        let hit = hit_path.to_string_lossy().to_string();
        let addr = format!("\0actplane-dgram-{}", std::process::id());
        let policy = source_open_then_notify_exec_config_blob(&src, "aptpmsgdhit", label);
        let old_force = std::env::var_os("ACTPLANE_FORCE_TRACEPOINT");
        std::env::set_var("ACTPLANE_FORCE_TRACEPOINT", "1");
        let loaded = Loader::load(&policy);
        if let Some(v) = old_force {
            std::env::set_var("ACTPLANE_FORCE_TRACEPOINT", v);
        } else {
            std::env::remove_var("ACTPLANE_FORCE_TRACEPOINT");
        }
        let mut loader = loaded.expect("load tracepoint eBPF engine");
        assert!(
            !loader.enforce_mode(),
            "forced tracepoint mode should not attach BPF LSM"
        );
        loader
            .bind_state(
                caller_pid,
                caller_pid as u32,
                CapState {
                    scope_id: 1,
                    target_mask: TARGET_SELF | TARGET_CHILD,
                    ..CapState::default()
                },
            )
            .expect("bind active unlabeled caller domain");

        let py = format!(
            r#"import os, signal, socket
addr = {addr:?}
os.kill(os.getpid(), signal.SIGSTOP)
srv = socket.socket(socket.AF_UNIX, socket.SOCK_DGRAM)
srv.bind(addr)
pid = os.fork()
if pid == 0:
    srv.recvmsg(4096)
    srv.close()
    os.execv({hit:?}, [{hit:?}])
with open({src:?}, "rb") as src:
    src.read()
cli = socket.socket(socket.AF_UNIX, socket.SOCK_DGRAM)
cli.sendmsg([b"x"], [], 0, addr)
cli.close()
srv.close()
_, status = os.waitpid(pid, 0)
raise SystemExit(0 if status == 0 else 1)
"#
        );
        let mut actor = std::process::Command::new("python3")
            .arg("-c")
            .arg(py)
            .spawn()
            .expect("spawn stopped abstract unix datagram actor");

        let stop = std::sync::Arc::new(std::sync::atomic::AtomicBool::new(false));
        let (tx, rx) = std::sync::mpsc::channel();
        let run_stop = std::sync::Arc::clone(&stop);
        let run_thread = std::thread::spawn(move || {
            loader.run(&run_stop, |v| {
                let _ = tx.send(v);
            })
        });
        std::thread::sleep(std::time::Duration::from_millis(50));
        while rx.try_recv().is_ok() {}

        resume_and_wait(&mut actor);
        let v = rx
            .recv_timeout(std::time::Duration::from_secs(2))
            .expect("forked child should inherit label received through unix datagram sendmsg");
        assert!(v.target.ends_with("aptpmsgdhit"), "target was {}", v.target);

        stop.store(true, std::sync::atomic::Ordering::Relaxed);
        run_thread
            .join()
            .expect("join ring thread")
            .expect("run loop");
        let _ = std::fs::remove_dir_all(&tmp);
    }

    #[test]
    #[ignore = "requires root/CAP_BPF and loads live eBPF tracepoints"]
    fn tracepoint_scm_rights_received_fd_flow_smoke() {
        let _guard = live_bpf_test_guard();
        let python_has_scm_rights = std::process::Command::new("python3")
            .arg("-c")
            .arg("import array, socket; raise SystemExit(0 if hasattr(socket, 'SCM_RIGHTS') and hasattr(socket.socket, 'sendmsg') and hasattr(socket.socket, 'recvmsg') else 1)")
            .stdout(std::process::Stdio::null())
            .stderr(std::process::Stdio::null())
            .status()
            .map(|s| s.success())
            .unwrap_or(false);
        if !python_has_scm_rights {
            eprintln!("skipping SCM_RIGHTS smoke: python3 sendmsg/recvmsg is unavailable");
            return;
        }

        let label = 1;
        let caller_pid = std::process::id() as i32;
        let tmp = std::env::temp_dir().join(format!("actplane-tp-scm-{}", std::process::id()));
        std::fs::create_dir_all(&tmp).expect("create temp dir");
        let src_path = tmp.join("src");
        let out_path = tmp.join("out");
        let hit_path = tmp.join("aptpscmhit");
        std::fs::write(&src_path, "secret\n").expect("write source");
        std::fs::copy("/bin/true", &hit_path).expect("copy /bin/true");

        let src = src_path.to_string_lossy().to_string();
        let out = out_path.to_string_lossy().to_string();
        let hit = hit_path.to_string_lossy().to_string();
        let policy = source_open_then_notify_exec_config_blob(&src, "aptpscmhit", label);
        let old_force = std::env::var_os("ACTPLANE_FORCE_TRACEPOINT");
        std::env::set_var("ACTPLANE_FORCE_TRACEPOINT", "1");
        let loaded = Loader::load(&policy);
        if let Some(v) = old_force {
            std::env::set_var("ACTPLANE_FORCE_TRACEPOINT", v);
        } else {
            std::env::remove_var("ACTPLANE_FORCE_TRACEPOINT");
        }
        let mut loader = loaded.expect("load tracepoint eBPF engine");
        assert!(
            !loader.enforce_mode(),
            "forced tracepoint mode should not attach BPF LSM"
        );
        loader
            .bind_state(
                caller_pid,
                caller_pid as u32,
                CapState {
                    scope_id: 1,
                    target_mask: TARGET_SELF | TARGET_CHILD,
                    ..CapState::default()
                },
            )
            .expect("bind active unlabeled caller domain");

        let py = format!(
            r#"import array, os, signal, socket
os.kill(os.getpid(), signal.SIGSTOP)
left, right = socket.socketpair()
out_fd = os.open({out:?}, os.O_WRONLY | os.O_CREAT | os.O_TRUNC, 0o600)
pid = os.fork()
if pid == 0:
    left.close()
    fds = array.array("i")
    msg, ancdata, flags, addr = right.recvmsg(1, socket.CMSG_SPACE(fds.itemsize))
    for level, ctype, data in ancdata:
        if level == socket.SOL_SOCKET and ctype == socket.SCM_RIGHTS:
            fds.frombytes(data[:fds.itemsize])
            break
    if not fds:
        raise RuntimeError("missing received fd")
    os.write(fds[0], b"copied\n")
    os.close(fds[0])
    right.close()
    raise SystemExit(0)
right.close()
with open({src:?}, "rb") as src:
    src.read()
fds = array.array("i", [out_fd])
left.sendmsg([b"x"], [(socket.SOL_SOCKET, socket.SCM_RIGHTS, fds)])
left.close()
os.close(out_fd)
_, status = os.waitpid(pid, 0)
raise SystemExit(0 if status == 0 else 1)
"#
        );
        let mut actor = std::process::Command::new("python3")
            .arg("-c")
            .arg(py)
            .spawn()
            .expect("spawn stopped SCM_RIGHTS actor");

        let stop = std::sync::Arc::new(std::sync::atomic::AtomicBool::new(false));
        let (tx, rx) = std::sync::mpsc::channel();
        let run_stop = std::sync::Arc::clone(&stop);
        let run_thread = std::thread::spawn(move || {
            loader.run(&run_stop, |v| {
                let _ = tx.send(v);
            })
        });
        std::thread::sleep(std::time::Duration::from_millis(50));
        while rx.try_recv().is_ok() {}

        resume_and_wait(&mut actor);
        assert!(
            out_path.is_file(),
            "SCM_RIGHTS actor did not create the output file"
        );
        while rx.try_recv().is_ok() {}

        let mut reader = spawn_stopped_shell(&format!("read _ < {out}; exec {hit}"));
        resume_and_wait(&mut reader);
        let v = rx
            .recv_timeout(std::time::Duration::from_secs(2))
            .expect("reader should inherit label written through SCM_RIGHTS fd");
        assert!(v.target.ends_with("aptpscmhit"), "target was {}", v.target);

        stop.store(true, std::sync::atomic::Ordering::Relaxed);
        run_thread
            .join()
            .expect("join ring thread")
            .expect("run loop");
        let _ = std::fs::remove_dir_all(&tmp);
    }

    #[test]
    #[ignore = "requires root/CAP_BPF and loads live eBPF tracepoints"]
    fn tracepoint_scm_rights_fifth_fd_flow_smoke() {
        let _guard = live_bpf_test_guard();
        let python_has_scm_rights = std::process::Command::new("python3")
            .arg("-c")
            .arg("import array, socket; raise SystemExit(0 if hasattr(socket, 'SCM_RIGHTS') and hasattr(socket.socket, 'sendmsg') and hasattr(socket.socket, 'recvmsg') else 1)")
            .stdout(std::process::Stdio::null())
            .stderr(std::process::Stdio::null())
            .status()
            .map(|s| s.success())
            .unwrap_or(false);
        if !python_has_scm_rights {
            eprintln!("skipping SCM_RIGHTS fifth-fd smoke: python3 sendmsg/recvmsg is unavailable");
            return;
        }

        let label = 1;
        let caller_pid = std::process::id() as i32;
        let tmp = std::env::temp_dir().join(format!("actplane-tp-scm5-{}", std::process::id()));
        std::fs::create_dir_all(&tmp).expect("create temp dir");
        let src_path = tmp.join("src");
        let out_path = tmp.join("out");
        let hit_path = tmp.join("aptpscm5hit");
        std::fs::write(&src_path, "secret\n").expect("write source");
        std::fs::copy("/bin/true", &hit_path).expect("copy /bin/true");

        let src = src_path.to_string_lossy().to_string();
        let out = out_path.to_string_lossy().to_string();
        let hit = hit_path.to_string_lossy().to_string();
        let policy = source_open_then_notify_exec_config_blob(&src, "aptpscm5hit", label);
        let old_force = std::env::var_os("ACTPLANE_FORCE_TRACEPOINT");
        std::env::set_var("ACTPLANE_FORCE_TRACEPOINT", "1");
        let loaded = Loader::load(&policy);
        if let Some(v) = old_force {
            std::env::set_var("ACTPLANE_FORCE_TRACEPOINT", v);
        } else {
            std::env::remove_var("ACTPLANE_FORCE_TRACEPOINT");
        }
        let mut loader = loaded.expect("load tracepoint eBPF engine");
        assert!(
            !loader.enforce_mode(),
            "forced tracepoint mode should not attach BPF LSM"
        );
        loader
            .bind_state(
                caller_pid,
                caller_pid as u32,
                CapState {
                    scope_id: 1,
                    target_mask: TARGET_SELF | TARGET_CHILD,
                    ..CapState::default()
                },
            )
            .expect("bind active unlabeled caller domain");

        let py = format!(
            r#"import array, os, signal, socket
os.kill(os.getpid(), signal.SIGSTOP)
left, right = socket.socketpair()
dummy_fds = [os.open(os.devnull, os.O_WRONLY) for _ in range(4)]
out_fd = os.open({out:?}, os.O_WRONLY | os.O_CREAT | os.O_TRUNC, 0o600)
pid = os.fork()
if pid == 0:
    left.close()
    fds = array.array("i")
    msg, ancdata, flags, addr = right.recvmsg(1, socket.CMSG_SPACE(fds.itemsize * 5))
    for level, ctype, data in ancdata:
        if level == socket.SOL_SOCKET and ctype == socket.SCM_RIGHTS:
            fds.frombytes(data[:fds.itemsize * 5])
            break
    if len(fds) < 5:
        raise RuntimeError("missing received fd batch")
    os.write(fds[4], b"copied\n")
    for fd in fds:
        os.close(fd)
    right.close()
    raise SystemExit(0)
right.close()
with open({src:?}, "rb") as src:
    src.read()
fds = array.array("i", dummy_fds + [out_fd])
left.sendmsg([b"x"], [(socket.SOL_SOCKET, socket.SCM_RIGHTS, fds)])
left.close()
for fd in dummy_fds:
    os.close(fd)
os.close(out_fd)
_, status = os.waitpid(pid, 0)
raise SystemExit(0 if status == 0 else 1)
"#
        );
        let mut actor = std::process::Command::new("python3")
            .arg("-c")
            .arg(py)
            .spawn()
            .expect("spawn stopped SCM_RIGHTS fifth-fd actor");

        let stop = std::sync::Arc::new(std::sync::atomic::AtomicBool::new(false));
        let (tx, rx) = std::sync::mpsc::channel();
        let run_stop = std::sync::Arc::clone(&stop);
        let run_thread = std::thread::spawn(move || {
            loader.run(&run_stop, |v| {
                let _ = tx.send(v);
            })
        });
        std::thread::sleep(std::time::Duration::from_millis(50));
        while rx.try_recv().is_ok() {}

        resume_and_wait(&mut actor);
        assert!(
            out_path.is_file(),
            "SCM_RIGHTS fifth-fd actor did not create the output file"
        );
        while rx.try_recv().is_ok() {}

        let mut reader = spawn_stopped_shell(&format!("read _ < {out}; exec {hit}"));
        resume_and_wait(&mut reader);
        let v = rx
            .recv_timeout(std::time::Duration::from_secs(2))
            .expect("reader should inherit label written through fifth SCM_RIGHTS fd");
        assert!(v.target.ends_with("aptpscm5hit"), "target was {}", v.target);

        stop.store(true, std::sync::atomic::Ordering::Relaxed);
        run_thread
            .join()
            .expect("join ring thread")
            .expect("run loop");
        let _ = std::fs::remove_dir_all(&tmp);
    }

    #[test]
    #[ignore = "requires root/CAP_BPF and loads live eBPF tracepoints"]
    fn tracepoint_scm_rights_socketpair_fd_identity_smoke() {
        let _guard = live_bpf_test_guard();
        let python_has_scm_rights = std::process::Command::new("python3")
            .arg("-c")
            .arg("import array, socket; raise SystemExit(0 if hasattr(socket, 'SCM_RIGHTS') and hasattr(socket.socket, 'sendmsg') and hasattr(socket.socket, 'recvmsg') else 1)")
            .stdout(std::process::Stdio::null())
            .stderr(std::process::Stdio::null())
            .status()
            .map(|s| s.success())
            .unwrap_or(false);
        if !python_has_scm_rights {
            eprintln!(
                "skipping SCM_RIGHTS socketpair smoke: python3 sendmsg/recvmsg is unavailable"
            );
            return;
        }

        let label = 1;
        let caller_pid = std::process::id() as i32;
        let tmp = std::env::temp_dir().join(format!("actplane-tp-scm-sock-{}", std::process::id()));
        std::fs::create_dir_all(&tmp).expect("create temp dir");
        let src_path = tmp.join("src");
        let hit_path = tmp.join("aptpscmsockhit");
        std::fs::write(&src_path, "secret\n").expect("write source");
        std::fs::copy("/bin/true", &hit_path).expect("copy /bin/true");

        let src = src_path.to_string_lossy().to_string();
        let hit = hit_path.to_string_lossy().to_string();
        let policy = source_open_then_notify_exec_config_blob(&src, "aptpscmsockhit", label);
        let old_force = std::env::var_os("ACTPLANE_FORCE_TRACEPOINT");
        std::env::set_var("ACTPLANE_FORCE_TRACEPOINT", "1");
        let loaded = Loader::load(&policy);
        if let Some(v) = old_force {
            std::env::set_var("ACTPLANE_FORCE_TRACEPOINT", v);
        } else {
            std::env::remove_var("ACTPLANE_FORCE_TRACEPOINT");
        }
        let mut loader = loaded.expect("load tracepoint eBPF engine");
        assert!(
            !loader.enforce_mode(),
            "forced tracepoint mode should not attach BPF LSM"
        );
        loader
            .bind_state(
                caller_pid,
                caller_pid as u32,
                CapState {
                    scope_id: 1,
                    target_mask: TARGET_SELF | TARGET_CHILD,
                    ..CapState::default()
                },
            )
            .expect("bind active unlabeled caller domain");

        let py = format!(
            r#"import array, os, signal, socket
os.kill(os.getpid(), signal.SIGSTOP)
ctrl_parent, ctrl_child = socket.socketpair()
pid = os.fork()
if pid == 0:
    ctrl_parent.close()
    fds = array.array("i")
    msg, ancdata, flags, addr = ctrl_child.recvmsg(1, socket.CMSG_SPACE(fds.itemsize))
    for level, ctype, data in ancdata:
        if level == socket.SOL_SOCKET and ctype == socket.SCM_RIGHTS:
            fds.frombytes(data[:fds.itemsize])
            break
    if not fds:
        raise RuntimeError("missing received socket fd")
    os.read(fds[0], 1)
    os.execv({hit:?}, [{hit:?}])
ctrl_child.close()
data_parent, data_child = socket.socketpair()
fds = array.array("i", [data_child.fileno()])
ctrl_parent.sendmsg([b"x"], [(socket.SOL_SOCKET, socket.SCM_RIGHTS, fds)])
data_child.close()
with open({src:?}, "rb") as src:
    src.read()
data_parent.send(b"y")
data_parent.close()
ctrl_parent.close()
_, status = os.waitpid(pid, 0)
raise SystemExit(0 if os.WIFEXITED(status) and os.WEXITSTATUS(status) == 0 else 1)
"#
        );
        let mut actor = std::process::Command::new("python3")
            .arg("-c")
            .arg(py)
            .spawn()
            .expect("spawn stopped SCM_RIGHTS socketpair actor");

        let stop = std::sync::Arc::new(std::sync::atomic::AtomicBool::new(false));
        let (tx, rx) = std::sync::mpsc::channel();
        let run_stop = std::sync::Arc::clone(&stop);
        let run_thread = std::thread::spawn(move || {
            loader.run(&run_stop, |v| {
                let _ = tx.send(v);
            })
        });
        std::thread::sleep(std::time::Duration::from_millis(50));
        while rx.try_recv().is_ok() {}

        resume_and_wait(&mut actor);
        let v = rx
            .recv_timeout(std::time::Duration::from_secs(2))
            .expect("child should inherit label through SCM_RIGHTS socketpair fd");
        assert!(
            v.target.ends_with("aptpscmsockhit"),
            "target was {}",
            v.target
        );

        stop.store(true, std::sync::atomic::Ordering::Relaxed);
        run_thread
            .join()
            .expect("join ring thread")
            .expect("run loop");
        let _ = std::fs::remove_dir_all(&tmp);
    }

    #[test]
    #[ignore = "requires root/CAP_BPF and loads live eBPF tracepoints"]
    fn tracepoint_recv_endpoint_source_taints_reader_smoke() {
        let _guard = live_bpf_test_guard();
        if std::process::Command::new("python3")
            .arg("--version")
            .stdout(std::process::Stdio::null())
            .stderr(std::process::Stdio::null())
            .status()
            .is_err()
        {
            eprintln!("skipping tracepoint recv flow smoke: python3 is not available");
            return;
        }

        let label = 1;
        let caller_pid = std::process::id() as i32;
        let tmp = std::env::temp_dir().join(format!("actplane-tp-recv-{}", std::process::id()));
        std::fs::create_dir_all(&tmp).expect("create temp dir");
        let hit_path = tmp.join("aptprecvhit");
        std::fs::copy("/bin/true", &hit_path).expect("copy /bin/true");
        let hit = hit_path.to_string_lossy().to_string();
        let listener = std::net::TcpListener::bind(("127.0.0.1", 0)).expect("bind listener");
        let port = listener.local_addr().expect("listener addr").port();
        let loopback = 127u32 | (1u32 << 24);
        let policy = source_recv_then_notify_exec_config_blob(loopback, "aptprecvhit", label);
        let old_force = std::env::var_os("ACTPLANE_FORCE_TRACEPOINT");
        std::env::set_var("ACTPLANE_FORCE_TRACEPOINT", "1");
        let loaded = Loader::load(&policy);
        if let Some(v) = old_force {
            std::env::set_var("ACTPLANE_FORCE_TRACEPOINT", v);
        } else {
            std::env::remove_var("ACTPLANE_FORCE_TRACEPOINT");
        }
        let mut loader = loaded.expect("load tracepoint eBPF engine");
        assert!(
            !loader.enforce_mode(),
            "forced tracepoint mode should not attach BPF LSM"
        );
        loader
            .bind_state(
                caller_pid,
                caller_pid as u32,
                CapState {
                    scope_id: 1,
                    target_mask: TARGET_SELF | TARGET_CHILD,
                    ..CapState::default()
                },
            )
            .expect("bind active unlabeled caller domain");

        let py = format!(
            r#"import os, signal, socket
os.kill(os.getpid(), signal.SIGSTOP)
s = socket.create_connection(("127.0.0.1", {port}))
s.recv(4096)
s.close()
os.execv({hit:?}, [{hit:?}])
"#
        );
        let mut reader = std::process::Command::new("python3")
            .arg("-c")
            .arg(py)
            .spawn()
            .expect("spawn stopped tracepoint TCP reader");

        let stop = std::sync::Arc::new(std::sync::atomic::AtomicBool::new(false));
        let (tx, rx) = std::sync::mpsc::channel();
        let run_stop = std::sync::Arc::clone(&stop);
        let run_thread = std::thread::spawn(move || {
            loader.run(&run_stop, |v| {
                let _ = tx.send(v);
            })
        });
        std::thread::sleep(std::time::Duration::from_millis(50));

        wait_until_stopped(&reader);
        let rc = unsafe { libc::kill(reader.id() as i32, libc::SIGCONT) };
        assert_eq!(rc, 0, "resume tracepoint TCP reader");
        let (mut stream, _addr) = listener.accept().expect("accept TCP reader");
        use std::io::Write;
        stream
            .write_all(b"network-secret\n")
            .expect("write to reader");
        drop(stream);
        let status = reader.wait().expect("wait TCP reader");
        assert!(status.success(), "reader status {status:?}");

        let v = rx
            .recv_timeout(std::time::Duration::from_secs(2))
            .expect("tracepoint recv source should taint reader before exec");
        assert!(v.target.ends_with("aptprecvhit"), "target was {}", v.target);

        stop.store(true, std::sync::atomic::Ordering::Relaxed);
        run_thread
            .join()
            .expect("join ring thread")
            .expect("run loop");
        let _ = std::fs::remove_dir_all(&tmp);
    }

    #[test]
    #[ignore = "requires root/CAP_BPF and loads live eBPF tracepoints"]
    fn tracepoint_udp_recvfrom_endpoint_source_taints_reader_smoke() {
        let _guard = live_bpf_test_guard();
        if std::process::Command::new("python3")
            .arg("--version")
            .stdout(std::process::Stdio::null())
            .stderr(std::process::Stdio::null())
            .status()
            .is_err()
        {
            eprintln!("skipping tracepoint UDP recvfrom smoke: python3 is not available");
            return;
        }

        let label = 1;
        let caller_pid = std::process::id() as i32;
        let tmp = std::env::temp_dir().join(format!("actplane-tp-udp-{}", std::process::id()));
        std::fs::create_dir_all(&tmp).expect("create temp dir");
        let hit_path = tmp.join("aptpudphit");
        let port_path = tmp.join("port");
        std::fs::copy("/bin/true", &hit_path).expect("copy /bin/true");
        let hit = hit_path.to_string_lossy().to_string();
        let port_file = port_path.to_string_lossy().to_string();
        let loopback = 127u32 | (1u32 << 24);
        let policy = source_recv_then_notify_exec_config_blob(loopback, "aptpudphit", label);
        let old_force = std::env::var_os("ACTPLANE_FORCE_TRACEPOINT");
        std::env::set_var("ACTPLANE_FORCE_TRACEPOINT", "1");
        let loaded = Loader::load(&policy);
        if let Some(v) = old_force {
            std::env::set_var("ACTPLANE_FORCE_TRACEPOINT", v);
        } else {
            std::env::remove_var("ACTPLANE_FORCE_TRACEPOINT");
        }
        let mut loader = loaded.expect("load tracepoint eBPF engine");
        assert!(
            !loader.enforce_mode(),
            "forced tracepoint mode should not attach BPF LSM"
        );
        loader
            .bind_state(
                caller_pid,
                caller_pid as u32,
                CapState {
                    scope_id: 1,
                    target_mask: TARGET_SELF | TARGET_CHILD,
                    ..CapState::default()
                },
            )
            .expect("bind active unlabeled caller domain");

        let py = format!(
            r#"import os, signal, socket
s = socket.socket(socket.AF_INET, socket.SOCK_DGRAM)
s.bind(("127.0.0.1", 0))
with open({port_file:?}, "w", encoding="utf-8") as f:
    f.write(str(s.getsockname()[1]))
    f.flush()
os.kill(os.getpid(), signal.SIGSTOP)
s.recvfrom(4096)
s.close()
os.execv({hit:?}, [{hit:?}])
"#
        );
        let mut reader = std::process::Command::new("python3")
            .arg("-c")
            .arg(py)
            .spawn()
            .expect("spawn stopped tracepoint UDP reader");

        let mut port = None;
        for _ in 0..200 {
            match std::fs::read_to_string(&port_path) {
                Ok(s) => {
                    port = Some(s.trim().parse::<u16>().expect("parse UDP port"));
                    break;
                }
                Err(_) => std::thread::sleep(std::time::Duration::from_millis(10)),
            }
        }
        let port = port.expect("UDP reader should publish bound port");

        let stop = std::sync::Arc::new(std::sync::atomic::AtomicBool::new(false));
        let (tx, rx) = std::sync::mpsc::channel();
        let run_stop = std::sync::Arc::clone(&stop);
        let run_thread = std::thread::spawn(move || {
            loader.run(&run_stop, |v| {
                let _ = tx.send(v);
            })
        });
        std::thread::sleep(std::time::Duration::from_millis(50));
        while rx.try_recv().is_ok() {}

        let sender = std::net::UdpSocket::bind(("127.0.0.1", 0)).expect("bind UDP sender");
        sender
            .send_to(b"network-secret\n", ("127.0.0.1", port))
            .expect("send UDP packet");
        resume_and_wait(&mut reader);

        let v = rx
            .recv_timeout(std::time::Duration::from_secs(2))
            .expect("tracepoint UDP recvfrom source should taint reader before exec");
        assert!(v.target.ends_with("aptpudphit"), "target was {}", v.target);

        stop.store(true, std::sync::atomic::Ordering::Relaxed);
        run_thread
            .join()
            .expect("join ring thread")
            .expect("run loop");
        let _ = std::fs::remove_dir_all(&tmp);
    }

    #[test]
    #[ignore = "requires root/CAP_BPF and loads live eBPF tracepoints"]
    fn tracepoint_udp_recvmsg_endpoint_source_taints_reader_smoke() {
        let _guard = live_bpf_test_guard();
        let python_has_recvmsg = std::process::Command::new("python3")
            .arg("-c")
            .arg("import socket; raise SystemExit(0 if hasattr(socket.socket, 'recvmsg') else 1)")
            .stdout(std::process::Stdio::null())
            .stderr(std::process::Stdio::null())
            .status()
            .map(|s| s.success())
            .unwrap_or(false);
        if !python_has_recvmsg {
            eprintln!(
                "skipping tracepoint UDP recvmsg smoke: python3 socket.recvmsg is unavailable"
            );
            return;
        }

        let label = 1;
        let caller_pid = std::process::id() as i32;
        let tmp = std::env::temp_dir().join(format!("actplane-tp-udpmsg-{}", std::process::id()));
        std::fs::create_dir_all(&tmp).expect("create temp dir");
        let hit_path = tmp.join("aptpmsghit");
        let port_path = tmp.join("port");
        std::fs::copy("/bin/true", &hit_path).expect("copy /bin/true");
        let hit = hit_path.to_string_lossy().to_string();
        let port_file = port_path.to_string_lossy().to_string();
        let loopback = 127u32 | (1u32 << 24);
        let policy = source_recv_then_notify_exec_config_blob(loopback, "aptpmsghit", label);
        let old_force = std::env::var_os("ACTPLANE_FORCE_TRACEPOINT");
        std::env::set_var("ACTPLANE_FORCE_TRACEPOINT", "1");
        let loaded = Loader::load(&policy);
        if let Some(v) = old_force {
            std::env::set_var("ACTPLANE_FORCE_TRACEPOINT", v);
        } else {
            std::env::remove_var("ACTPLANE_FORCE_TRACEPOINT");
        }
        let mut loader = loaded.expect("load tracepoint eBPF engine");
        assert!(
            !loader.enforce_mode(),
            "forced tracepoint mode should not attach BPF LSM"
        );
        loader
            .bind_state(
                caller_pid,
                caller_pid as u32,
                CapState {
                    scope_id: 1,
                    target_mask: TARGET_SELF | TARGET_CHILD,
                    ..CapState::default()
                },
            )
            .expect("bind active unlabeled caller domain");

        let py = format!(
            r#"import os, signal, socket
s = socket.socket(socket.AF_INET, socket.SOCK_DGRAM)
s.bind(("127.0.0.1", 0))
with open({port_file:?}, "w", encoding="utf-8") as f:
    f.write(str(s.getsockname()[1]))
    f.flush()
os.kill(os.getpid(), signal.SIGSTOP)
s.recvmsg(4096)
s.close()
os.execv({hit:?}, [{hit:?}])
"#
        );
        let mut reader = std::process::Command::new("python3")
            .arg("-c")
            .arg(py)
            .spawn()
            .expect("spawn stopped tracepoint UDP recvmsg reader");

        let mut port = None;
        for _ in 0..200 {
            match std::fs::read_to_string(&port_path) {
                Ok(s) => {
                    port = Some(s.trim().parse::<u16>().expect("parse UDP port"));
                    break;
                }
                Err(_) => std::thread::sleep(std::time::Duration::from_millis(10)),
            }
        }
        let port = port.expect("UDP recvmsg reader should publish bound port");

        let stop = std::sync::Arc::new(std::sync::atomic::AtomicBool::new(false));
        let (tx, rx) = std::sync::mpsc::channel();
        let run_stop = std::sync::Arc::clone(&stop);
        let run_thread = std::thread::spawn(move || {
            loader.run(&run_stop, |v| {
                let _ = tx.send(v);
            })
        });
        std::thread::sleep(std::time::Duration::from_millis(50));
        while rx.try_recv().is_ok() {}

        let sender = std::net::UdpSocket::bind(("127.0.0.1", 0)).expect("bind UDP sender");
        sender
            .send_to(b"network-secret\n", ("127.0.0.1", port))
            .expect("send UDP packet");
        resume_and_wait(&mut reader);

        let v = rx
            .recv_timeout(std::time::Duration::from_secs(2))
            .expect("tracepoint UDP recvmsg source should taint reader before exec");
        assert!(v.target.ends_with("aptpmsghit"), "target was {}", v.target);

        stop.store(true, std::sync::atomic::Ordering::Relaxed);
        run_thread
            .join()
            .expect("join ring thread")
            .expect("run loop");
        let _ = std::fs::remove_dir_all(&tmp);
    }

    #[test]
    #[ignore = "requires root/CAP_BPF and loads live eBPF tracepoints"]
    fn tracepoint_splice_fd_flow_smoke() {
        let _guard = live_bpf_test_guard();
        let python_has_splice = std::process::Command::new("python3")
            .arg("-c")
            .arg("import os; raise SystemExit(0 if hasattr(os, 'splice') else 1)")
            .stdout(std::process::Stdio::null())
            .stderr(std::process::Stdio::null())
            .status()
            .map(|s| s.success())
            .unwrap_or(false);
        if !python_has_splice {
            eprintln!("skipping splice fd-flow smoke: python3 os.splice is not available");
            return;
        }

        let label = 1;
        let caller_pid = std::process::id() as i32;
        let tmp = std::env::temp_dir().join(format!("actplane-tp-splice-{}", std::process::id()));
        std::fs::create_dir_all(&tmp).expect("create temp dir");
        let src_path = tmp.join("src");
        let out_path = tmp.join("out");
        let hit_path = tmp.join("aptpsplicehit");
        std::fs::write(&src_path, "secret\n").expect("write source");
        std::fs::copy("/bin/true", &hit_path).expect("copy /bin/true");

        let src = src_path.to_string_lossy().to_string();
        let out = out_path.to_string_lossy().to_string();
        let hit = hit_path.to_string_lossy().to_string();
        let policy = source_open_then_notify_exec_config_blob(&src, "aptpsplicehit", label);
        let old_force = std::env::var_os("ACTPLANE_FORCE_TRACEPOINT");
        std::env::set_var("ACTPLANE_FORCE_TRACEPOINT", "1");
        let loaded = Loader::load(&policy);
        if let Some(v) = old_force {
            std::env::set_var("ACTPLANE_FORCE_TRACEPOINT", v);
        } else {
            std::env::remove_var("ACTPLANE_FORCE_TRACEPOINT");
        }
        let mut loader = loaded.expect("load tracepoint eBPF engine");
        assert!(
            !loader.enforce_mode(),
            "forced tracepoint mode should not attach BPF LSM"
        );
        loader
            .bind_state(
                caller_pid,
                caller_pid as u32,
                CapState {
                    scope_id: 1,
                    target_mask: TARGET_SELF | TARGET_CHILD,
                    ..CapState::default()
                },
            )
            .expect("bind active unlabeled caller domain");

        let py = format!(
            r#"import os, signal
os.kill(os.getpid(), signal.SIGSTOP)
r_fd, w_fd = os.pipe()
out_fd = os.open({out:?}, os.O_WRONLY | os.O_CREAT | os.O_TRUNC, 0o600)
src_fd = os.open({src:?}, os.O_RDONLY)
try:
    os.splice(src_fd, w_fd, 4096)
    os.close(w_fd)
    w_fd = -1
    os.splice(r_fd, out_fd, 4096)
finally:
    for fd in (src_fd, out_fd, r_fd, w_fd):
        if fd >= 0:
            try:
                os.close(fd)
            except OSError:
                pass
"#
        );
        let mut writer = std::process::Command::new("python3")
            .arg("-c")
            .arg(py)
            .spawn()
            .expect("spawn stopped splice writer");
        let mut reader = spawn_stopped_shell(&format!("read _ < {out}; exec {hit}"));

        let stop = std::sync::Arc::new(std::sync::atomic::AtomicBool::new(false));
        let (tx, rx) = std::sync::mpsc::channel();
        let run_stop = std::sync::Arc::clone(&stop);
        let run_thread = std::thread::spawn(move || {
            loader.run(&run_stop, |v| {
                let _ = tx.send(v);
            })
        });
        std::thread::sleep(std::time::Duration::from_millis(50));

        resume_and_wait(&mut writer);
        assert!(
            out_path.is_file(),
            "writer did not create the output file before reader ran"
        );
        while rx.try_recv().is_ok() {}

        resume_and_wait(&mut reader);
        let v = rx
            .recv_timeout(std::time::Duration::from_secs(2))
            .expect("tracepoint reader should inherit label written through splice");
        assert!(
            v.target.ends_with("aptpsplicehit"),
            "target was {}",
            v.target
        );

        stop.store(true, std::sync::atomic::Ordering::Relaxed);
        run_thread
            .join()
            .expect("join ring thread")
            .expect("run loop");
        let _ = std::fs::remove_dir_all(&tmp);
    }

    #[test]
    #[ignore = "requires root/CAP_BPF, active BPF LSM, and loads live eBPF programs"]
    fn lsm_recv_endpoint_source_taints_reader_smoke() {
        if !bpf_lsm_active() {
            eprintln!("skipping LSM recv flow smoke: bpf LSM is not active");
            return;
        }
        if !std::path::Path::new("/bin/bash").exists() {
            eprintln!("skipping LSM recv flow smoke: /bin/bash is not available");
            return;
        }

        let label = 1;
        let caller_pid = std::process::id() as i32;
        let tmp = std::env::temp_dir().join(format!("actplane-lsm-recv-{}", std::process::id()));
        std::fs::create_dir_all(&tmp).expect("create temp dir");
        let hit_path = tmp.join("aprecvhit");
        std::fs::copy("/bin/true", &hit_path).expect("copy /bin/true");
        let hit = hit_path.to_string_lossy().to_string();
        let listener = std::net::TcpListener::bind(("127.0.0.1", 0)).expect("bind listener");
        let port = listener.local_addr().expect("listener addr").port();
        let loopback = 127u32 | (1u32 << 24);
        let policy = source_recv_then_notify_exec_config_blob(loopback, "aprecvhit", label);
        let mut loader = Loader::load(&policy).expect("load eBPF engine");
        assert!(
            loader.enforce_mode(),
            "BPF LSM should be active for this test"
        );
        loader
            .bind_state(
                caller_pid,
                caller_pid as u32,
                CapState {
                    scope_id: 1,
                    target_mask: TARGET_SELF | TARGET_CHILD,
                    ..CapState::default()
                },
            )
            .expect("bind active unlabeled caller domain");

        let mut reader = std::process::Command::new("/bin/bash")
            .arg("-c")
            .arg(format!(
                "kill -STOP $$; exec 3<>/dev/tcp/127.0.0.1/{port}; IFS= read -r _ <&3; exec {hit}"
            ))
            .spawn()
            .expect("spawn stopped TCP reader");

        let stop = std::sync::Arc::new(std::sync::atomic::AtomicBool::new(false));
        let (tx, rx) = std::sync::mpsc::channel();
        let run_stop = std::sync::Arc::clone(&stop);
        let run_thread = std::thread::spawn(move || {
            loader.run(&run_stop, |v| {
                let _ = tx.send(v);
            })
        });
        std::thread::sleep(std::time::Duration::from_millis(50));

        wait_until_stopped(&reader);
        let rc = unsafe { libc::kill(reader.id() as i32, libc::SIGCONT) };
        assert_eq!(rc, 0, "resume TCP reader");
        let (mut stream, _addr) = listener.accept().expect("accept TCP reader");
        use std::io::Write;
        stream
            .write_all(b"network-secret\n")
            .expect("write to reader");
        drop(stream);
        let status = reader.wait().expect("wait TCP reader");
        assert!(status.success(), "reader status {status:?}");

        let v = rx
            .recv_timeout(std::time::Duration::from_secs(2))
            .expect("recv source should taint reader before exec");
        assert!(v.target.ends_with("aprecvhit"), "target was {}", v.target);
        assert_eq!(v.rule_id, 0);

        stop.store(true, std::sync::atomic::Ordering::Relaxed);
        run_thread
            .join()
            .expect("join ring thread")
            .expect("run loop");
        let _ = std::fs::remove_dir_all(&tmp);
    }

    #[test]
    #[ignore = "requires root/CAP_BPF, active BPF LSM, and loads live eBPF programs"]
    fn lsm_recv_endpoint_block_smoke() {
        if !bpf_lsm_active() {
            eprintln!("skipping LSM recv block smoke: bpf LSM is not active");
            return;
        }
        if !std::path::Path::new("/bin/bash").exists() {
            eprintln!("skipping LSM recv block smoke: /bin/bash is not available");
            return;
        }

        let label = 1;
        let caller_pid = std::process::id() as i32;
        let listener = std::net::TcpListener::bind(("127.0.0.1", 0)).expect("bind listener");
        let port = listener.local_addr().expect("listener addr").port();
        let loopback = 127u32 | (1u32 << 24);
        let policy = block_recv_endpoint_config_blob(loopback, label);
        let mut loader = Loader::load(&policy).expect("load eBPF engine");
        assert!(
            loader.enforce_mode(),
            "BPF LSM should be active for this test"
        );
        loader
            .seed_label(caller_pid, label)
            .expect("seed labeled caller domain");

        let mut reader = std::process::Command::new("/bin/bash")
            .arg("-c")
            .arg(format!(
                "kill -STOP $$; exec 3<>/dev/tcp/127.0.0.1/{port}; if IFS= read -r _ <&3; then exit 42; else exit 0; fi"
            ))
            .spawn()
            .expect("spawn stopped TCP reader");

        let stop = std::sync::Arc::new(std::sync::atomic::AtomicBool::new(false));
        let (tx, rx) = std::sync::mpsc::channel();
        let run_stop = std::sync::Arc::clone(&stop);
        let run_thread = std::thread::spawn(move || {
            loader.run(&run_stop, |v| {
                let _ = tx.send(v);
            })
        });
        std::thread::sleep(std::time::Duration::from_millis(50));

        wait_until_stopped(&reader);
        let rc = unsafe { libc::kill(reader.id() as i32, libc::SIGCONT) };
        assert_eq!(rc, 0, "resume TCP reader");
        let (mut stream, _addr) = listener.accept().expect("accept TCP reader");
        use std::io::Write;
        let _ = stream.write_all(b"network-secret\n");
        drop(stream);
        let status = reader.wait().expect("wait TCP reader");
        assert!(status.success(), "reader status {status:?}");

        let v = rx
            .recv_timeout(std::time::Duration::from_secs(2))
            .expect("recv rule should block reader");
        assert!(v.blocked, "recv violation was not blocked: {v:?}");
        assert_eq!(v.rule_id, 0);

        stop.store(true, std::sync::atomic::Ordering::Relaxed);
        run_thread
            .join()
            .expect("join ring thread")
            .expect("run loop");
    }

    #[test]
    #[ignore = "requires root/CAP_BPF and loads live eBPF programs"]
    fn sibling_domain_file_labels_do_not_collide_smoke() {
        let _guard = live_bpf_test_guard();
        let empty = empty_config_blob();
        let caller_pid = std::process::id() as i32;
        let domain_a = 300;
        let domain_b = 400;
        let label = 1;

        let tmp = std::env::temp_dir().join(format!(
            "actplane-label-domain-smoke-{}",
            std::process::id()
        ));
        std::fs::create_dir_all(&tmp).expect("create temp dir");
        let src_path = tmp.join("src");
        let shared_path = tmp.join("shared");
        let hit_path = tmp.join("aplabel");
        std::fs::write(&src_path, "secret\n").expect("write source");
        std::fs::copy("/bin/true", &hit_path).expect("copy /bin/true");

        let src = src_path.to_string_lossy().to_string();
        let shared = shared_path.to_string_lossy().to_string();
        let hit = hit_path.to_string_lossy().to_string();
        let policy_a_source = source_open_any_config_blob(label);
        let policy_b_rule = notify_exec_if_label_config_blob("aplabel", label);
        let policy_b_source = source_open_any_config_blob(label);

        let mut loader = Loader::load_with_hook_reserve(&empty, HookReserve::runtime_file_delta())
            .expect("load eBPF engine with file-flow reserve");
        let domain_a_state = CapState {
            scope_id: 1,
            authority_mask: AUTH_ADD_LABEL,
            target_mask: TARGET_SELF,
            label_mask: label,
            ..CapState::default()
        };
        let domain_b_state = CapState {
            scope_id: 1,
            authority_mask: AUTH_ADD_LABEL | AUTH_BIND_RULE,
            target_mask: TARGET_SELF,
            label_mask: label,
            ..CapState::default()
        };

        loader
            .bind_state(caller_pid, domain_a, domain_a_state)
            .expect("bind caller to domain A");
        let handle = loader.reload_handle().expect("reload handle");
        handle
            .append_policy_delta(caller_pid, domain_a, &policy_a_source)
            .expect("append domain A source");

        loader
            .bind_state(caller_pid, domain_b, domain_b_state)
            .expect("bind caller to domain B");
        handle
            .append_policy_delta(caller_pid, domain_b, &policy_b_rule)
            .expect("append domain B rule");

        let mut writer = spawn_stopped_shell(&format!("cat {src} > {shared}"));
        loader
            .bind_state(writer.id() as i32, domain_a, domain_a_state)
            .expect("bind writer to domain A");

        let mut sibling_reader = spawn_stopped_shell(&format!("read _ < {shared}; exec {hit}"));
        loader
            .bind_state(sibling_reader.id() as i32, domain_b, domain_b_state)
            .expect("bind reader to domain B");

        let mut local_reader = spawn_stopped_shell(&format!("read _ < {shared}; exec {hit}"));
        loader
            .bind_state(local_reader.id() as i32, domain_b, domain_b_state)
            .expect("bind local reader to domain B");

        let stop = std::sync::Arc::new(std::sync::atomic::AtomicBool::new(false));
        let (tx, rx) = std::sync::mpsc::channel();
        let run_stop = std::sync::Arc::clone(&stop);
        let run_thread = std::thread::spawn(move || {
            loader.run(&run_stop, |v| {
                let _ = tx.send(v);
            })
        });
        std::thread::sleep(std::time::Duration::from_millis(50));

        resume_and_wait(&mut writer);
        resume_and_wait(&mut sibling_reader);
        assert!(
            rx.recv_timeout(std::time::Duration::from_millis(300))
                .is_err(),
            "domain B observed domain A's file label bit"
        );

        while rx.try_recv().is_ok() {}
        handle
            .append_policy_delta(caller_pid, domain_b, &policy_b_source)
            .expect("append domain B source");
        resume_and_wait(&mut local_reader);
        let v = rx
            .recv_timeout(std::time::Duration::from_secs(2))
            .expect("domain B local source violation");
        assert!(v.target.ends_with("aplabel"), "target was {}", v.target);

        stop.store(true, std::sync::atomic::Ordering::Relaxed);
        run_thread
            .join()
            .expect("join ring thread")
            .expect("run loop");
        let _ = std::fs::remove_dir_all(&tmp);
    }

    #[test]
    #[ignore = "requires root/CAP_BPF and loads live eBPF programs"]
    fn binding_domain_resets_inherited_process_labels_smoke() {
        let empty = empty_config_blob();
        let label = 1;
        let caller_pid = std::process::id() as i32;
        let child_domain = 500;
        let policy = notify_exec_if_label_config_blob("apbindlabel", label);

        let tmp = std::env::temp_dir().join(format!("actplane-bind-label-smoke-{}", caller_pid));
        std::fs::create_dir_all(&tmp).expect("create temp dir");
        let hit_path = tmp.join("apbindlabel");
        std::fs::copy("/bin/true", &hit_path).expect("copy /bin/true");

        let mut loader = Loader::load(&empty).expect("load eBPF engine");
        loader
            .seed_label(caller_pid, label)
            .expect("seed caller with inherited label");

        let mut reset_child = spawn_stopped_exec(&hit_path);

        loader
            .bind_state(
                caller_pid,
                child_domain,
                CapState {
                    scope_id: 1,
                    authority_mask: AUTH_BIND_RULE,
                    target_mask: TARGET_SELF,
                    ..CapState::default()
                },
            )
            .expect("bind caller to child domain");
        let handle = loader.reload_handle().expect("reload handle");
        handle
            .append_policy_delta(caller_pid, child_domain, &policy)
            .expect("append child domain label rule");

        loader
            .bind_state(reset_child.id() as i32, child_domain, CapState::default())
            .expect("bind child with clean process labels");

        let stop = std::sync::Arc::new(std::sync::atomic::AtomicBool::new(false));
        let (tx, rx) = std::sync::mpsc::channel();
        let run_stop = std::sync::Arc::clone(&stop);
        let run_thread = std::thread::spawn(move || {
            loader.run(&run_stop, |v| {
                let _ = tx.send(v);
            })
        });
        std::thread::sleep(std::time::Duration::from_millis(50));

        resume_and_wait(&mut reset_child);
        assert!(
            rx.recv_timeout(std::time::Duration::from_millis(300))
                .is_err(),
            "new domain saw process label inherited from old domain"
        );

        stop.store(true, std::sync::atomic::Ordering::Relaxed);
        run_thread
            .join()
            .expect("join ring thread")
            .expect("run loop");

        let mut loader = Loader::load(&empty).expect("load eBPF engine");
        loader
            .bind_state(
                caller_pid,
                child_domain,
                CapState {
                    scope_id: 1,
                    authority_mask: AUTH_BIND_RULE,
                    target_mask: TARGET_SELF,
                    ..CapState::default()
                },
            )
            .expect("bind caller to child domain");
        let handle = loader.reload_handle().expect("reload handle");
        handle
            .append_policy_delta(caller_pid, child_domain, &policy)
            .expect("append child domain label rule");

        let mut labeled_child = spawn_stopped_exec(&hit_path);
        loader
            .bind_state(
                labeled_child.id() as i32,
                child_domain,
                CapState {
                    labels: label,
                    ..CapState::default()
                },
            )
            .expect("bind child with explicit initial label");

        let stop = std::sync::Arc::new(std::sync::atomic::AtomicBool::new(false));
        let (tx, rx) = std::sync::mpsc::channel();
        let run_stop = std::sync::Arc::clone(&stop);
        let run_thread = std::thread::spawn(move || {
            loader.run(&run_stop, |v| {
                let _ = tx.send(v);
            })
        });
        std::thread::sleep(std::time::Duration::from_millis(50));

        resume_and_wait(&mut labeled_child);
        let v = rx
            .recv_timeout(std::time::Duration::from_secs(2))
            .expect("explicit initial label violation");
        assert!(v.target.ends_with("apbindlabel"), "target was {}", v.target);

        stop.store(true, std::sync::atomic::Ordering::Relaxed);
        run_thread
            .join()
            .expect("join ring thread")
            .expect("run loop");
        let _ = std::fs::remove_dir_all(&tmp);
    }

    #[test]
    #[ignore = "requires root/CAP_BPF and loads live eBPF programs"]
    fn local_domain_process_labels_do_not_satisfy_global_rules_smoke() {
        let label = 1;
        let caller_pid = std::process::id() as i32;
        let local_domain = 600;
        let global_src = format!("/tmp/apglsrc{caller_pid}");
        let local_src = format!("/tmp/aplcsrc{caller_pid}");
        let hit_name = format!("apglhit{caller_pid}");
        let hit_path = format!("/tmp/{hit_name}");

        std::fs::write(&global_src, "global\n").expect("write global source");
        std::fs::write(&local_src, "local\n").expect("write local source");
        std::fs::copy("/bin/true", &hit_path).expect("copy /bin/true");

        let global_policy = source_open_then_notify_exec_config_blob(&global_src, &hit_name, label);
        let local_source = source_open_exact_config_blob(label, &local_src);

        let mut loader = Loader::load(&global_policy).expect("load eBPF engine");
        let local_state = CapState {
            scope_id: 1,
            authority_mask: AUTH_ADD_LABEL,
            target_mask: TARGET_SELF,
            label_mask: label,
            ..CapState::default()
        };
        loader
            .bind_state(caller_pid, local_domain, local_state)
            .expect("bind caller to local domain");
        let handle = loader.reload_handle().expect("reload handle");
        handle
            .append_policy_delta(caller_pid, local_domain, &local_source)
            .expect("append local source");

        let mut local_labeled =
            spawn_stopped_shell(&format!("read _ < {local_src}; exec {hit_path}"));
        loader
            .bind_state(local_labeled.id() as i32, local_domain, local_state)
            .expect("bind local-labeled child");
        let mut global_labeled =
            spawn_stopped_shell(&format!("read _ < {global_src}; exec {hit_path}"));
        loader
            .bind_state(global_labeled.id() as i32, local_domain, local_state)
            .expect("bind global-labeled child");

        let stop = std::sync::Arc::new(std::sync::atomic::AtomicBool::new(false));
        let (tx, rx) = std::sync::mpsc::channel();
        let run_stop = std::sync::Arc::clone(&stop);
        let run_thread = std::thread::spawn(move || {
            loader.run(&run_stop, |v| {
                let _ = tx.send(v);
            })
        });
        std::thread::sleep(std::time::Duration::from_millis(50));

        resume_and_wait(&mut local_labeled);
        assert!(
            rx.recv_timeout(std::time::Duration::from_millis(300))
                .is_err(),
            "domain-local process label satisfied a global rule"
        );

        while rx.try_recv().is_ok() {}
        resume_and_wait(&mut global_labeled);
        let v = rx
            .recv_timeout(std::time::Duration::from_secs(2))
            .expect("global source violation");
        assert!(v.target.ends_with(&hit_name), "target was {}", v.target);

        stop.store(true, std::sync::atomic::Ordering::Relaxed);
        run_thread
            .join()
            .expect("join ring thread")
            .expect("run loop");
        let _ = std::fs::remove_file(&global_src);
        let _ = std::fs::remove_file(&local_src);
        let _ = std::fs::remove_file(&hit_path);
    }

    #[test]
    #[ignore = "requires root/CAP_BPF and loads live eBPF programs"]
    fn append_policy_delta_scopes_to_target_domain_and_descendants_smoke() {
        let empty = empty_config_blob();
        let policy = notify_exec_config_blob("apdomain");
        let caller_pid = std::process::id() as i32;
        let domain_a = 100;
        let domain_child = 101;
        let domain_sibling = 200;

        let tmp =
            std::env::temp_dir().join(format!("actplane-domain-smoke-{}", std::process::id()));
        std::fs::create_dir_all(&tmp).expect("create temp dir");
        let hit_path = tmp.join("apdomain");
        std::fs::copy("/bin/true", &hit_path).expect("copy /bin/true");

        let mut loader = Loader::load(&empty).expect("load eBPF engine");
        let domain_a_state = CapState {
            scope_id: 1,
            authority_mask: AUTH_BIND_RULE,
            target_mask: TARGET_SELF,
            ..CapState::default()
        };
        loader
            .bind_state(caller_pid, domain_a, domain_a_state)
            .expect("bind caller domain");

        let mut in_domain = spawn_stopped_exec(&hit_path);
        loader
            .bind_state(in_domain.id() as i32, domain_a, domain_a_state)
            .expect("bind target domain child");

        let mut in_descendant = spawn_stopped_exec(&hit_path);
        loader
            .bind_state(
                in_descendant.id() as i32,
                domain_child,
                CapState {
                    parent: domain_a,
                    scope_id: 2,
                    ..CapState::default()
                },
            )
            .expect("bind descendant domain child");

        let mut in_sibling = spawn_stopped_exec(&hit_path);
        loader
            .bind_state(
                in_sibling.id() as i32,
                domain_sibling,
                CapState {
                    scope_id: 1,
                    ..CapState::default()
                },
            )
            .expect("bind sibling domain child");

        let handle = loader.reload_handle().expect("reload handle");
        let stop = std::sync::Arc::new(std::sync::atomic::AtomicBool::new(false));
        let (tx, rx) = std::sync::mpsc::channel();
        let run_stop = std::sync::Arc::clone(&stop);
        let run_thread = std::thread::spawn(move || {
            loader.run(&run_stop, |v| {
                let _ = tx.send(v);
            })
        });
        std::thread::sleep(std::time::Duration::from_millis(50));

        handle
            .append_policy_delta(caller_pid, domain_a, &policy)
            .expect("append admitted domain-local rule");

        resume_and_wait(&mut in_domain);
        let v = rx
            .recv_timeout(std::time::Duration::from_secs(2))
            .expect("target domain violation");
        assert!(v.target.ends_with("apdomain"), "target was {}", v.target);

        resume_and_wait(&mut in_descendant);
        let v = rx
            .recv_timeout(std::time::Duration::from_secs(2))
            .expect("descendant domain violation");
        assert!(v.target.ends_with("apdomain"), "target was {}", v.target);

        while rx.try_recv().is_ok() {}
        resume_and_wait(&mut in_sibling);
        assert!(
            rx.recv_timeout(std::time::Duration::from_millis(300))
                .is_err(),
            "sibling domain matched a target-local rule"
        );

        stop.store(true, std::sync::atomic::Ordering::Relaxed);
        run_thread
            .join()
            .expect("join ring thread")
            .expect("run loop");
        let _ = std::fs::remove_dir_all(&tmp);
    }

    #[test]
    #[ignore = "requires root/CAP_BPF and loads live eBPF programs"]
    fn ancestor_domain_dynamic_labels_apply_in_child_smoke() {
        let _guard = live_bpf_test_guard();
        let empty = empty_config_blob();
        let label = 1;
        let caller_pid = std::process::id() as i32;
        let parent_domain = 700;
        let child_domain = 701;

        let tmp = std::env::temp_dir().join(format!(
            "actplane-ancestor-domain-smoke-{}",
            std::process::id()
        ));
        std::fs::create_dir_all(&tmp).expect("create temp dir");
        let src_path = tmp.join("src");
        let hit_path = tmp.join("apancestor");
        std::fs::write(&src_path, "parent-domain secret\n").expect("write source");
        std::fs::copy("/bin/true", &hit_path).expect("copy /bin/true");
        let src = src_path.to_string_lossy().to_string();
        let hit = hit_path.to_string_lossy().to_string();
        let policy = source_open_then_notify_exec_config_blob(&src, "apancestor", label);

        let mut loader = Loader::load(&empty).expect("load eBPF engine");
        let parent_state = CapState {
            scope_id: 1,
            authority_mask: AUTH_ADD_LABEL | AUTH_BIND_RULE,
            target_mask: TARGET_SELF,
            label_mask: label,
            ..CapState::default()
        };
        loader
            .bind_state(caller_pid, parent_domain, parent_state)
            .expect("bind caller to parent domain");
        let handle = loader.reload_handle().expect("reload handle");
        handle
            .append_policy_delta(caller_pid, parent_domain, &policy)
            .expect("append parent-domain source and rule");

        let mut child = spawn_stopped_shell(&format!("read _ < {src}; exec {hit}"));
        loader
            .bind_state(
                child.id() as i32,
                child_domain,
                CapState {
                    parent: parent_domain,
                    scope_id: 2,
                    ..CapState::default()
                },
            )
            .expect("bind child domain below parent");

        let stop = std::sync::Arc::new(std::sync::atomic::AtomicBool::new(false));
        let (tx, rx) = std::sync::mpsc::channel();
        let run_stop = std::sync::Arc::clone(&stop);
        let run_thread = std::thread::spawn(move || {
            loader.run(&run_stop, |v| {
                let _ = tx.send(v);
            })
        });
        std::thread::sleep(std::time::Duration::from_millis(50));

        resume_and_wait(&mut child);
        let v = rx
            .recv_timeout(std::time::Duration::from_secs(2))
            .expect("parent-domain rule matched descendant dynamic label");
        assert!(v.target.ends_with("apancestor"), "target was {}", v.target);

        stop.store(true, std::sync::atomic::Ordering::Relaxed);
        run_thread
            .join()
            .expect("join ring thread")
            .expect("run loop");
        let _ = std::fs::remove_dir_all(&tmp);
    }

    #[test]
    #[ignore = "requires root/CAP_BPF and loads live eBPF programs"]
    fn domain_handle_binds_subagent_child_domain_smoke() {
        let empty = empty_config_blob();
        let policy = notify_exec_config_blob("apchilddom");
        let caller_pid = std::process::id() as i32;
        let parent_domain = caller_pid as u32;
        let domain_child = parent_domain + 1000;
        let domain_sibling = parent_domain + 1001;

        let tmp = std::env::temp_dir().join(format!(
            "actplane-child-domain-smoke-{}",
            std::process::id()
        ));
        std::fs::create_dir_all(&tmp).expect("create temp dir");
        let hit_path = tmp.join("apchilddom");
        std::fs::copy("/bin/true", &hit_path).expect("copy /bin/true");

        let mut loader = Loader::load(&empty).expect("load eBPF engine");
        loader
            .seed_label(caller_pid, 1)
            .expect("seed parent agent domain");
        let handle = loader.reload_handle().expect("reload handle");
        let domains = loader.domain_handle().expect("domain handle");

        let mut in_child = spawn_stopped_exec(&hit_path);
        domains
            .bind_child_domain(ChildDomainSpec {
                parent_pid: caller_pid,
                parent_id: parent_domain,
                child_id: domain_child,
                pid: in_child.id() as i32,
                scope_id: 2,
                authority_mask: AUTH_BIND_RULE,
                target_mask: TARGET_SELF,
                ..ChildDomainSpec::default()
            })
            .expect("bind child domain through handle");

        let mut in_sibling = spawn_stopped_exec(&hit_path);
        domains
            .bind_child_domain(ChildDomainSpec {
                parent_pid: caller_pid,
                parent_id: parent_domain,
                child_id: domain_sibling,
                pid: in_sibling.id() as i32,
                scope_id: 2,
                authority_mask: AUTH_BIND_RULE,
                target_mask: TARGET_SELF,
                ..ChildDomainSpec::default()
            })
            .expect("bind sibling domain through handle");

        let stop = std::sync::Arc::new(std::sync::atomic::AtomicBool::new(false));
        let (tx, rx) = std::sync::mpsc::channel();
        let run_stop = std::sync::Arc::clone(&stop);
        let run_thread = std::thread::spawn(move || {
            loader.run(&run_stop, |v| {
                let _ = tx.send(v);
            })
        });
        std::thread::sleep(std::time::Duration::from_millis(50));

        handle
            .append_policy_delta(caller_pid, domain_child, &policy)
            .expect("append rule to child domain through admitted path");

        resume_and_wait(&mut in_child);
        let v = rx
            .recv_timeout(std::time::Duration::from_secs(2))
            .expect("child domain violation");
        assert!(v.target.ends_with("apchilddom"), "target was {}", v.target);

        while rx.try_recv().is_ok() {}
        resume_and_wait(&mut in_sibling);
        assert!(
            rx.recv_timeout(std::time::Duration::from_millis(300))
                .is_err(),
            "sibling child domain matched a rule appended to another child domain"
        );

        stop.store(true, std::sync::atomic::Ordering::Relaxed);
        run_thread
            .join()
            .expect("join ring thread")
            .expect("run loop");
        let _ = std::fs::remove_dir_all(&tmp);
    }

    #[test]
    #[ignore = "requires root/CAP_BPF and loads live eBPF programs"]
    fn seeded_parent_can_append_child_source_policy_smoke() {
        let empty = empty_config_blob();
        let label = 1;
        let caller_pid = std::process::id() as i32;
        let parent_domain = caller_pid as u32;
        let child_domain = parent_domain + 2000;

        let tmp = std::env::temp_dir().join(format!(
            "actplane-child-source-policy-smoke-{}",
            std::process::id()
        ));
        std::fs::create_dir_all(&tmp).expect("create temp dir");
        let src_path = tmp.join("src");
        let hit_path = tmp.join("apseedchild");
        std::fs::write(&src_path, "child secret\n").expect("write source");
        std::fs::copy("/bin/true", &hit_path).expect("copy /bin/true");
        let src = src_path.to_string_lossy().to_string();
        let hit = hit_path.to_string_lossy().to_string();
        let policy = source_open_then_notify_exec_config_blob(&src, "apseedchild", label);

        let mut loader = Loader::load_with_hook_reserve(&empty, HookReserve::runtime_file_delta())
            .expect("load eBPF engine with file-flow reserve");
        loader
            .seed_label(caller_pid, 1)
            .expect("seed parent agent domain");
        let handle = loader.reload_handle().expect("reload handle");
        let domains = loader.domain_handle().expect("domain handle");

        let mut child = spawn_stopped_shell(&format!("read _ < {src}; exec {hit}"));
        domains
            .bind_child_domain(ChildDomainSpec {
                parent_pid: caller_pid,
                parent_id: parent_domain,
                child_id: child_domain,
                pid: child.id() as i32,
                scope_id: 2,
                authority_mask: AUTH_BIND_RULE,
                target_mask: TARGET_SELF,
                ..ChildDomainSpec::default()
            })
            .expect("bind child domain through handle");

        let stop = std::sync::Arc::new(std::sync::atomic::AtomicBool::new(false));
        let (tx, rx) = std::sync::mpsc::channel();
        let run_stop = std::sync::Arc::clone(&stop);
        let run_thread = std::thread::spawn(move || {
            loader.run(&run_stop, |v| {
                let _ = tx.send(v);
            })
        });
        std::thread::sleep(std::time::Duration::from_millis(50));

        handle
            .append_policy_delta(caller_pid, child_domain, &policy)
            .expect("seeded parent appends child source+rule policy");

        resume_and_wait(&mut child);
        let v = rx
            .recv_timeout(std::time::Duration::from_secs(2))
            .expect("child source policy violation");
        assert!(v.target.ends_with("apseedchild"), "target was {}", v.target);

        stop.store(true, std::sync::atomic::Ordering::Relaxed);
        run_thread
            .join()
            .expect("join ring thread")
            .expect("run loop");
        let _ = std::fs::remove_dir_all(&tmp);
    }

    #[test]
    #[ignore = "requires root/CAP_BPF and loads live eBPF programs"]
    fn append_policy_delta_admits_self_rule_smoke() {
        let empty = empty_config_blob();
        let policy = notify_exec_config_blob("aprladd");
        let caller_pid = std::process::id() as i32;
        let target_id = 42;

        let mut loader = Loader::load(&empty).expect("load eBPF engine");
        loader
            .bind_state(
                caller_pid,
                target_id,
                CapState {
                    scope_id: 1,
                    authority_mask: AUTH_BIND_RULE,
                    target_mask: TARGET_SELF,
                    ..CapState::default()
                },
            )
            .expect("bind caller domain");
        let handle = loader.reload_handle().expect("reload handle");

        let stop = std::sync::Arc::new(std::sync::atomic::AtomicBool::new(false));
        let (tx, rx) = std::sync::mpsc::channel();
        let run_stop = std::sync::Arc::clone(&stop);
        let run_thread = std::thread::spawn(move || {
            loader.run(&run_stop, |v| {
                let _ = tx.send(v);
            })
        });
        std::thread::sleep(std::time::Duration::from_millis(50));

        handle
            .append_policy_delta(caller_pid, target_id, &policy)
            .expect("append admitted rule");

        let tmp =
            std::env::temp_dir().join(format!("actplane-append-smoke-{}", std::process::id()));
        std::fs::create_dir_all(&tmp).expect("create temp dir");
        let hit_path = tmp.join("aprladd");
        std::fs::copy("/bin/true", &hit_path).expect("copy /bin/true");
        let status = std::process::Command::new(&hit_path)
            .status()
            .expect("run matching executable");
        assert!(status.success());

        let v = rx
            .recv_timeout(std::time::Duration::from_secs(2))
            .expect("violation from appended rule");
        assert!(v.target.ends_with("aprladd"), "target was {}", v.target);
        assert_eq!(v.rule_id, 0);

        stop.store(true, std::sync::atomic::Ordering::Relaxed);
        run_thread
            .join()
            .expect("join ring thread")
            .expect("run loop");
        let _ = std::fs::remove_dir_all(&tmp);
    }

    #[test]
    #[ignore = "requires root/CAP_BPF and loads live eBPF programs"]
    fn append_policy_delta_admits_domain_local_declassify_smoke() {
        let empty = empty_config_blob();
        let label = 1u64;
        let caller_pid = std::process::id() as i32;
        let target_id = 43;
        let delegate_target_id = 44;
        let submitter_domain_id = 45;

        let tmp =
            std::env::temp_dir().join(format!("actplane-runtime-declass-{}", std::process::id()));
        std::fs::create_dir_all(&tmp).expect("create temp dir");
        let src = tmp.join("secret.txt");
        std::fs::write(&src, "classified\n").expect("write secret");
        let hit_path = tmp.join("apdeclasshit");
        std::fs::copy("/bin/true", &hit_path).expect("copy /bin/true");
        let runner = tmp.join("runner.sh");
        std::fs::write(&runner, "exec \"$1\"\n").expect("write runner");

        let source_rule = source_open_then_notify_exec_config_blob(
            src.to_str().expect("source path"),
            "apdeclasshit",
            label,
        );
        let declassify = declassify_exec_config_blob("sh", label);

        let mut loader = Loader::load_with_hook_reserve(&empty, HookReserve::runtime_file_delta())
            .expect("load eBPF engine");
        loader
            .bind_state(
                caller_pid,
                submitter_domain_id,
                CapState {
                    scope_id: 1,
                    authority_mask: AUTH_DELEGATE,
                    ..CapState::default()
                },
            )
            .expect("bind delegated submitter domain");
        let mut delegated_actor = spawn_stopped_shell("sleep 30");
        wait_until_stopped(&delegated_actor);
        let delegated_actor_pid = delegated_actor.id() as i32;
        let base_state = CapState {
            scope_id: 1,
            authority_mask: AUTH_BIND_RULE | AUTH_ADD_LABEL,
            target_mask: TARGET_SELF,
            label_mask: label,
            ..CapState::default()
        };
        loader
            .bind_state(delegated_actor_pid, delegate_target_id, base_state)
            .expect("bind delegated actor domain");
        let handle = loader.reload_handle().expect("reload handle");
        let err = handle
            .append_policy_delta(delegated_actor_pid, delegate_target_id, &declassify)
            .expect_err("declassify delta without AUTH_DECLASSIFY");
        assert!(
            err.to_string().contains("lacks runtime authority"),
            "unexpected error: {err}"
        );
        loader
            .bind_state(
                delegated_actor_pid,
                delegate_target_id,
                CapState {
                    authority_mask: AUTH_BIND_RULE | AUTH_ADD_LABEL | AUTH_DECLASSIFY,
                    ..base_state
                },
            )
            .expect("grant delegated actor declassify authority");
        handle
            .append_policy_delta(delegated_actor_pid, delegate_target_id, &declassify)
            .expect("delegated domain-local declassify delta");
        let _ = delegated_actor.kill();
        let _ = delegated_actor.wait();

        loader
            .bind_state(
                caller_pid,
                target_id,
                CapState {
                    authority_mask: AUTH_BIND_RULE | AUTH_ADD_LABEL | AUTH_DECLASSIFY,
                    ..base_state
                },
            )
            .expect("bind caller domain");

        let stop = std::sync::Arc::new(std::sync::atomic::AtomicBool::new(false));
        let (tx, rx) = std::sync::mpsc::channel();
        let run_stop = std::sync::Arc::clone(&stop);
        let run_thread = std::thread::spawn(move || {
            loader.run(&run_stop, |v| {
                let _ = tx.send(v);
            })
        });
        std::thread::sleep(std::time::Duration::from_millis(50));

        handle
            .append_policy_delta(caller_pid, target_id, &source_rule)
            .expect("append source/rule delta");

        let command = format!(
            "IFS= read -r _ < '{}'; exec /bin/sh '{}' '{}'",
            src.display(),
            runner.display(),
            hit_path.display()
        );
        let status = std::process::Command::new("/bin/sh")
            .arg("-c")
            .arg(&command)
            .status()
            .expect("run labeled flow before declassify");
        assert!(status.success());
        let v = rx
            .recv_timeout(std::time::Duration::from_secs(2))
            .expect("violation before declassify");
        assert!(
            v.target.ends_with("apdeclasshit"),
            "target was {}",
            v.target
        );
        assert_eq!(v.op, OP_EXEC as u32);
        assert_eq!(v.domain_id, target_id);
        assert_eq!(v.session_root, caller_pid);
        assert_eq!(v.matched_label, label);
        assert_eq!(v.matched_labels, label);
        while rx.try_recv().is_ok() {}

        handle
            .append_policy_delta(caller_pid, target_id, &declassify)
            .expect("append domain-local declassify delta");
        let status = std::process::Command::new("/bin/sh")
            .arg("-c")
            .arg(&command)
            .status()
            .expect("run labeled flow after declassify");
        assert!(status.success());
        assert!(
            rx.recv_timeout(std::time::Duration::from_millis(300))
                .is_err(),
            "declassified flow still matched local rule"
        );

        stop.store(true, std::sync::atomic::Ordering::Relaxed);
        run_thread
            .join()
            .expect("join ring thread")
            .expect("run loop");
        let _ = std::fs::remove_dir_all(&tmp);
    }
}
