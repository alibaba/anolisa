// SPDX-License-Identifier: (LGPL-2.1 OR BSD-2-Clause)
// Copyright (c) 2025 AgentSight Project
//
// LSM audit probe — observe-only security auditing of traced Agent families via
// the BPF LSM hooks lsm/socket_connect (outbound connections) and lsm/file_open
// (file access). It records, it never denies.

use crate::config;
use anyhow::{Context, Result};
use libbpf_rs::{
    Link, MapHandle,
    skel::{OpenSkel, SkelBuilder},
};
use std::{
    mem::MaybeUninit,
    net::{IpAddr, Ipv4Addr, Ipv6Addr},
    os::fd::AsFd,
};

mod bpf {
    include!(concat!(env!("OUT_DIR"), "/lsmaudit.skel.rs"));
    include!(concat!(env!("OUT_DIR"), "/lsmaudit.rs"));
}
use bpf::*;

// Re-export the raw event type so probes.rs can size-check ring buffer records.
pub type RawLsmEvent = bpf::lsm_audit_event;

pub const LSM_EVENT_CONNECT: u8 = 1;
pub const LSM_EVENT_FILE_OPEN: u8 = 2;

// IPv6 address family as the kernel sees it (stored in lsm_audit_event.family as
// u8); any other family in a CONNECT event is decoded as IPv4.
const AF_INET6: u8 = 10;

/// An outbound connection attempt by a traced Agent.
#[derive(Debug, Clone)]
pub struct LsmConnect {
    pub pid: u32,
    pub tid: u32,
    pub uid: u32,
    pub timestamp_ns: u64,
    pub comm: String,
    pub dst_ip: IpAddr,
    pub dport: u16,
}

/// A file opened by a traced Agent.
#[derive(Debug, Clone)]
pub struct LsmFileOpen {
    pub pid: u32,
    pub tid: u32,
    pub uid: u32,
    pub timestamp_ns: u64,
    pub comm: String,
    /// File basename (full path is future work — see lsmaudit.bpf.c).
    pub path: String,
    /// Raw file->f_flags (O_RDONLY/O_WRONLY/O_RDWR in the low bits, plus O_CREAT…).
    pub open_flags: i32,
}

/// User-space LSM audit event — one variant per hook.
#[derive(Debug, Clone)]
pub enum LsmEvent {
    Connect(LsmConnect),
    FileOpen(LsmFileOpen),
}

/// Decode a fixed-size, NUL-terminated C char buffer into a String.
fn c_buf_to_string(buf: &[std::os::raw::c_char]) -> String {
    let bytes: Vec<u8> = buf.iter().take_while(|&&c| c != 0).map(|&c| c as u8).collect();
    String::from_utf8_lossy(&bytes).into_owned()
}

impl LsmEvent {
    /// Parse an event from raw ring buffer data.
    pub fn from_bytes(data: &[u8]) -> Option<Self> {
        if data.len() < std::mem::size_of::<RawLsmEvent>() {
            return None;
        }

        // SAFETY: BPF guarantees proper alignment and layout.
        let raw = unsafe { &*(data.as_ptr() as *const RawLsmEvent) };

        let pid = raw.pid;
        let tid = raw.tid;
        let uid = raw.uid;
        let timestamp_ns = config::ktime_to_unix_ns(raw.timestamp_ns);
        let comm = c_buf_to_string(&raw.comm);

        match raw.kind {
            LSM_EVENT_CONNECT => {
                let dst_ip = if raw.family == AF_INET6 {
                    IpAddr::V6(Ipv6Addr::from(raw.daddr))
                } else {
                    IpAddr::V4(Ipv4Addr::new(
                        raw.daddr[0],
                        raw.daddr[1],
                        raw.daddr[2],
                        raw.daddr[3],
                    ))
                };
                Some(LsmEvent::Connect(LsmConnect {
                    pid,
                    tid,
                    uid,
                    timestamp_ns,
                    comm,
                    dst_ip,
                    // Port is stored in network byte order.
                    dport: u16::from_be(raw.dport),
                }))
            }
            LSM_EVENT_FILE_OPEN => Some(LsmEvent::FileOpen(LsmFileOpen {
                pid,
                tid,
                uid,
                timestamp_ns,
                comm,
                path: c_buf_to_string(&raw.path),
                open_flags: raw.open_flags,
            })),
            _ => None,
        }
    }
}

/// Returns true if the kernel has BPF LSM active (`bpf` present in the active
/// LSM list). Attaching lsm/ programs requires this; without it the probe is
/// skipped with a warning rather than failing the whole run.
pub fn bpf_lsm_available() -> bool {
    std::fs::read_to_string("/sys/kernel/security/lsm")
        .map(|s| s.split(',').any(|lsm| lsm.trim() == "bpf"))
        .unwrap_or(false)
}

pub struct LsmAudit {
    _open_object: Box<MaybeUninit<libbpf_rs::OpenObject>>,
    skel: Box<LsmauditSkel<'static>>,
    _links: Vec<Link>,
}

impl LsmAudit {
    /// Re-expose the capability check at the type level for ergonomic call sites.
    pub fn bpf_lsm_available() -> bool {
        bpf_lsm_available()
    }

    /// Create a new LsmAudit that reuses the shared traced_processes and ring
    /// buffer maps.
    pub fn new_with_maps(traced_processes: &MapHandle, rb: &MapHandle) -> Result<Self> {
        let mut builder = LsmauditSkelBuilder::default();
        builder.obj_builder.debug(config::verbose());

        let open_object = Box::new(MaybeUninit::<libbpf_rs::OpenObject>::uninit());
        let mut open_skel = builder.open().context("failed to open lsmaudit BPF object")?;

        open_skel
            .maps_mut()
            .traced_processes()
            .reuse_fd(traced_processes.as_fd())
            .context("failed to reuse traced_processes map for lsmaudit")?;

        open_skel
            .maps_mut()
            .rb()
            .reuse_fd(rb.as_fd())
            .context("failed to reuse rb map for lsmaudit")?;

        let skel = open_skel.load().context("failed to load lsmaudit BPF object")?;

        // SAFETY: skel borrows open_object which lives in a Box<MaybeUninit>.
        let skel =
            unsafe { Box::from_raw(Box::into_raw(Box::new(skel)) as *mut LsmauditSkel<'static>) };

        Ok(Self {
            _open_object: open_object,
            skel,
            _links: Vec::new(),
        })
    }

    /// Attach both LSM programs (socket_connect + file_open).
    pub fn attach(&mut self) -> Result<()> {
        let mut links = Vec::new();

        let link = self
            .skel
            .progs_mut()
            .audit_socket_connect()
            .attach()
            .context("failed to attach lsm/socket_connect")?;
        links.push(link);

        let link = self
            .skel
            .progs_mut()
            .audit_file_open()
            .attach()
            .context("failed to attach lsm/file_open")?;
        links.push(link);

        self._links = links;
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_lsm_event_from_bytes_too_short() {
        let data = [0u8; 8];
        assert!(LsmEvent::from_bytes(&data).is_none());
    }

    // Build a raw lsm_event byte buffer for tests.
    fn raw_bytes(mut fill: impl FnMut(&mut RawLsmEvent)) -> Vec<u8> {
        // SAFETY: RawLsmEvent is a plain-old-data C struct.
        let mut raw: RawLsmEvent = unsafe { std::mem::zeroed() };
        fill(&mut raw);
        let size = std::mem::size_of::<RawLsmEvent>();
        let ptr = &raw as *const RawLsmEvent as *const u8;
        unsafe { std::slice::from_raw_parts(ptr, size) }.to_vec()
    }

    fn set_comm(raw: &mut RawLsmEvent, s: &str) {
        for (i, b) in s.bytes().enumerate() {
            if i < raw.comm.len() {
                raw.comm[i] = b as std::os::raw::c_char;
            }
        }
    }

    fn set_path(raw: &mut RawLsmEvent, s: &str) {
        for (i, b) in s.bytes().enumerate() {
            if i < raw.path.len() {
                raw.path[i] = b as std::os::raw::c_char;
            }
        }
    }

    #[test]
    fn test_parse_connect_ipv4() {
        let data = raw_bytes(|raw| {
            raw.kind = LSM_EVENT_CONNECT;
            raw.pid = 4321;
            raw.tid = 4322;
            raw.family = 2; // AF_INET
            // 1.2.3.4
            raw.daddr[0] = 1;
            raw.daddr[1] = 2;
            raw.daddr[2] = 3;
            raw.daddr[3] = 4;
            // port 443 in network byte order
            raw.dport = 443u16.to_be();
            set_comm(raw, "curl");
        });

        match LsmEvent::from_bytes(&data).unwrap() {
            LsmEvent::Connect(c) => {
                assert_eq!(c.pid, 4321);
                assert_eq!(c.tid, 4322);
                assert_eq!(c.comm, "curl");
                assert_eq!(c.dst_ip, IpAddr::V4(Ipv4Addr::new(1, 2, 3, 4)));
                assert_eq!(c.dport, 443);
            }
            other => panic!("expected Connect, got {other:?}"),
        }
    }

    #[test]
    fn test_parse_connect_ipv6() {
        let data = raw_bytes(|raw| {
            raw.kind = LSM_EVENT_CONNECT;
            raw.family = AF_INET6;
            // ::1 (loopback) → last byte = 1
            raw.daddr[15] = 1;
            raw.dport = 8080u16.to_be();
        });

        match LsmEvent::from_bytes(&data).unwrap() {
            LsmEvent::Connect(c) => {
                assert_eq!(c.dst_ip, IpAddr::V6(Ipv6Addr::LOCALHOST));
                assert_eq!(c.dport, 8080);
            }
            other => panic!("expected Connect, got {other:?}"),
        }
    }

    #[test]
    fn test_parse_file_open() {
        let data = raw_bytes(|raw| {
            raw.kind = LSM_EVENT_FILE_OPEN;
            raw.pid = 99;
            raw.open_flags = 2; // O_RDWR
            set_path(raw, "shadow");
            set_comm(raw, "agent");
        });

        match LsmEvent::from_bytes(&data).unwrap() {
            LsmEvent::FileOpen(f) => {
                assert_eq!(f.pid, 99);
                assert_eq!(f.path, "shadow");
                assert_eq!(f.comm, "agent");
                assert_eq!(f.open_flags, 2);
            }
            other => panic!("expected FileOpen, got {other:?}"),
        }
    }

    #[test]
    fn test_parse_unknown_kind_is_none() {
        let data = raw_bytes(|raw| {
            raw.kind = 99;
        });
        assert!(LsmEvent::from_bytes(&data).is_none());
    }
}
