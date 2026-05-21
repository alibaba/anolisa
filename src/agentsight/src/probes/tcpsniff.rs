// SPDX-License-Identifier: (LGPL-2.1 OR BSD-2-Clause)
// Copyright (c) 2025 AgentSight Project
//
// TCP plain-text traffic probe - captures HTTP traffic on configurable ports
// by hooking tcp_sendmsg (fentry) and tcp_recvmsg (fentry+fexit).
//
// Emits probe_SSL_data_t events (same format as sslsniff) so the entire
// downstream pipeline (parser, aggregator, analyzer, storage) works unchanged.
//
// Multi-kernel support:
//   - Kernel 5.18+: tcp_recvmsg(sk, msg, size, flags, addr_len)
//   - Kernel 5.8–5.17: tcp_recvmsg(sk, msg, size, nonblock, flags, addr_len)
//   Userspace tries the new signature first and falls back to old on attach failure.

use crate::config;
use anyhow::{Context, Result};
use libbpf_rs::{
    Link, MapHandle, MapFlags,
    skel::{OpenSkel, SkelBuilder},
};
use std::{
    mem::MaybeUninit,
    os::fd::AsFd,
};

// --- Generated skeleton ---
mod bpf {
    include!(concat!(env!("OUT_DIR"), "/tcpsniff.skel.rs"));
}
use bpf::*;

/// TCP plain-text traffic probe
pub struct TcpSniff {
    _open_object: Box<MaybeUninit<libbpf_rs::OpenObject>>,
    skel: Box<TcpsniffSkel<'static>>,
    _links: Vec<Link>,
    use_old_sig: bool,
}

impl TcpSniff {
    /// Build and load the BPF skeleton, selecting the correct tcp_recvmsg
    /// program variant for the running kernel.
    ///
    /// `use_old_sig`: true → load old (5.8-5.17) programs, false → new (5.18+)
    fn load_skel(
        traced_processes: &MapHandle,
        rb: &MapHandle,
        use_old_sig: bool,
    ) -> Result<(
        Box<MaybeUninit<libbpf_rs::OpenObject>>,
        Box<TcpsniffSkel<'static>>,
    )> {
        let mut builder = TcpsniffSkelBuilder::default();
        builder.obj_builder.debug(config::verbose());

        let open_object = Box::new(MaybeUninit::<libbpf_rs::OpenObject>::uninit());
        let mut open_skel = builder.open().context("failed to open tcpsniff BPF object")?;

        // Reuse external maps
        open_skel
            .maps_mut()
            .traced_processes()
            .reuse_fd(traced_processes.as_fd())
            .context("failed to reuse traced_processes map for tcpsniff")?;
        open_skel
            .maps_mut()
            .rb()
            .reuse_fd(rb.as_fd())
            .context("failed to reuse rb map for tcpsniff")?;

        // Selectively enable programs:
        // tcp_sendmsg fentry: always enabled (signature unchanged across kernels)
        // tcp_recvmsg fentry + fexit: enable either new or old variant
        if use_old_sig {
            // Disable new-signature programs
            open_skel
                .progs_mut()
                .trace_tcp_recvmsg_entry()
                .set_autoload(false)
                .context("failed to disable new recvmsg fentry")?;
            open_skel
                .progs_mut()
                .trace_tcp_recvmsg_exit()
                .set_autoload(false)
                .context("failed to disable new recvmsg fexit")?;
        } else {
            // Disable old-signature programs
            open_skel
                .progs_mut()
                .trace_tcp_recvmsg_entry_old()
                .set_autoload(false)
                .context("failed to disable old recvmsg fentry")?;
            open_skel
                .progs_mut()
                .trace_tcp_recvmsg_exit_old()
                .set_autoload(false)
                .context("failed to disable old recvmsg fexit")?;
        }

        let skel = open_skel.load().context("failed to load tcpsniff BPF object")?;

        // SAFETY: skel borrows open_object which lives in a Box<MaybeUninit>
        let skel =
            unsafe { Box::from_raw(Box::into_raw(Box::new(skel)) as *mut TcpsniffSkel<'static>) };

        Ok((open_object, skel))
    }

    /// Create a new TcpSniff that reuses existing traced_processes and ring buffer maps.
    /// Automatically detects the tcp_recvmsg signature for the running kernel.
    pub fn new_with_maps(traced_processes: &MapHandle, rb: &MapHandle) -> Result<Self> {
        // Try new signature first (5.18+), fall back to old (5.8-5.17) on load failure
        let (open_object, skel, use_old_sig) = match Self::load_skel(traced_processes, rb, false) {
            Ok((obj, skel)) => {
                log::info!("TcpSniff: loaded with new tcp_recvmsg signature (5.18+)");
                (obj, skel, false)
            }
            Err(e) => {
                log::info!(
                    "TcpSniff: new tcp_recvmsg signature failed ({}), trying old (5.8-5.17)",
                    e
                );
                let (obj, skel) = Self::load_skel(traced_processes, rb, true)
                    .context("failed to load tcpsniff with old tcp_recvmsg signature")?;
                log::info!("TcpSniff: loaded with old tcp_recvmsg signature (5.8-5.17)");
                (obj, skel, true)
            }
        };

        Ok(Self {
            _open_object: open_object,
            skel,
            _links: Vec::new(),
            use_old_sig,
        })
    }

    /// Populate the BPF tcp_target_ports map with the given ports.
    /// Must be called after new_with_maps() and before attach().
    pub fn set_target_ports(&mut self, ports: &[u16]) -> Result<()> {
        let binding = self.skel.maps();
        let map = binding.tcp_target_ports();
        let dummy: u8 = 1;
        for &port in ports {
            let net_port = port.to_be(); // convert to network byte order
            map.update(
                &net_port.to_ne_bytes(),
                &[dummy],
                MapFlags::ANY,
            )
            .with_context(|| format!("failed to add port {} to tcp_target_ports map", port))?;
        }
        log::info!("TcpSniff: configured {} target port(s): {:?}", ports.len(), ports);
        Ok(())
    }

    /// Attach fentry/fexit hooks for tcp_sendmsg and tcp_recvmsg.
    /// Attaches whichever tcp_recvmsg variant was loaded.
    pub fn attach(&mut self) -> Result<()> {
        let mut links = Vec::new();

        // tcp_sendmsg fentry — always present
        let link = self
            .skel
            .progs_mut()
            .trace_tcp_sendmsg()
            .attach()
            .context("failed to attach tcp_sendmsg fentry")?;
        links.push(link);

        // tcp_recvmsg — attach the variant that was loaded
        if self.use_old_sig {
            let entry_link = self
                .skel
                .progs_mut()
                .trace_tcp_recvmsg_entry_old()
                .attach()
                .context("failed to attach tcp_recvmsg fentry (old signature)")?;
            links.push(entry_link);

            let exit_link = self
                .skel
                .progs_mut()
                .trace_tcp_recvmsg_exit_old()
                .attach()
                .context("failed to attach tcp_recvmsg fexit (old signature)")?;
            links.push(exit_link);
        } else {
            let entry_link = self
                .skel
                .progs_mut()
                .trace_tcp_recvmsg_entry()
                .attach()
                .context("failed to attach tcp_recvmsg fentry")?;
            links.push(entry_link);

            let exit_link = self
                .skel
                .progs_mut()
                .trace_tcp_recvmsg_exit()
                .attach()
                .context("failed to attach tcp_recvmsg fexit")?;
            links.push(exit_link);
        }

        let n = links.len();
        self._links = links;
        log::info!(
            "TcpSniff: attached {} BPF programs (tcp_sendmsg fentry, tcp_recvmsg fentry+fexit)",
            n
        );
        Ok(())
    }
}
