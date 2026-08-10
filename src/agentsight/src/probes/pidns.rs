// SPDX-License-Identifier: (LGPL-2.1 OR BSD-2-Clause)
// Copyright (c) 2025 AgentSight Project
//
// Which pid namespace does this process observe `/proc` through?
//
// Every pid that comes out of a BPF event is eventually resolved against our
// own `/proc` -- cmdline, exe, maps, cgroup, and the `/proc/<pid>/root` paths
// uprobes attach to. `/proc` numbers processes in the *reader's* pid namespace,
// so BPF has to report the pid in *our* namespace, not the target's innermost
// one. The BPF side needs one bit to decide that (see `current_observer_pid()`
// in `src/bpf/common.h`), and this module computes it.
//
// The bit is "are we in the initial pid namespace". That is the only case where
// we can be certain our namespace is an ancestor of every process on the box,
// and therefore that every task has a pid we can resolve -- its host pid. When
// we are inside a namespace instead, the only processes we can see are the ones
// sharing it, whose innermost-namespace pid is already the number our `/proc`
// shows, which is the historical behaviour.

use std::os::unix::fs::MetadataExt;
use std::sync::OnceLock;

/// Inode number of the initial pid namespace. Fixed by the kernel as
/// `PROC_PID_INIT_INO` (`include/linux/proc_ns.h`) and part of the `/proc` ABI,
/// which is what makes comparing against it a stable check.
const PROC_PID_INIT_INO: u64 = 0xEFFF_FFFC;

/// Path whose inode identifies our pid namespace.
const SELF_PIDNS: &str = "/proc/self/ns/pid";

/// Whether this process observes `/proc` through the initial pid namespace.
///
/// Cached: a process cannot change its own pid namespace (only children placed
/// via `setns`/`unshare` get a new one), so this is fixed for our lifetime.
///
/// Probes copy this into their BPF `observer_pidns_is_init` rodata flag between
/// `open()` and `load()`.
pub fn observer_in_init_pidns() -> bool {
    static CACHED: OnceLock<bool> = OnceLock::new();
    *CACHED.get_or_init(|| {
        let ino = match std::fs::metadata(SELF_PIDNS) {
            Ok(md) => md.ino(),
            Err(e) => {
                // Hidden /proc, or a kernel without nsfs. Assume we are not in
                // the initial namespace: that keeps the historical behaviour
                // rather than asserting a host-pid view we cannot back up.
                log::warn!(
                    "failed to stat {SELF_PIDNS} ({e}); assuming a non-initial pid namespace, \
                     event pids will use the target's innermost namespace"
                );
                return false;
            }
        };
        let is_init = is_init_pidns_ino(ino);
        log::debug!(
            "observer pid namespace inode {ino:#x} (init={is_init}); event pids will be {}",
            if is_init {
                "host pids"
            } else {
                "innermost-namespace pids"
            }
        );
        is_init
    })
}

/// Whether a pid-namespace inode number is the initial namespace's.
///
/// Split out from [`observer_in_init_pidns`] so the comparison is testable
/// without depending on which namespace the test runner happens to be in.
fn is_init_pidns_ino(ino: u64) -> bool {
    ino == PROC_PID_INIT_INO
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn init_pidns_inode_is_recognized() {
        assert!(is_init_pidns_ino(PROC_PID_INIT_INO));
        // The documented value, spelled out so a typo in the constant fails here.
        assert!(is_init_pidns_ino(4_026_531_836));
    }

    #[test]
    fn other_pidns_inodes_are_rejected() {
        // Namespaces created after boot get ordinary nsfs inode numbers well
        // below the initial-namespace magic value.
        assert!(!is_init_pidns_ino(4_026_532_000));
        assert!(!is_init_pidns_ino(PROC_PID_INIT_INO - 1));
        assert!(!is_init_pidns_ino(PROC_PID_INIT_INO + 1));
        assert!(!is_init_pidns_ino(0));
    }

    #[test]
    fn observer_lookup_agrees_with_a_direct_stat() {
        // Whichever namespace the test runner is in, the cached lookup must
        // report what stat()ing the namespace link says.
        let expected = std::fs::metadata(SELF_PIDNS)
            .map(|md| is_init_pidns_ino(md.ino()))
            .expect("stat /proc/self/ns/pid");
        assert_eq!(observer_in_init_pidns(), expected);
    }

    #[test]
    fn self_pidns_inode_matches_the_symlink_target() {
        // Guards the assumption that stat()ing the magic symlink yields the
        // nsfs inode, i.e. the number inside "pid:[...]" -- that identity is
        // what makes the init comparison meaningful.
        let link = std::fs::read_link(SELF_PIDNS).expect("read_link /proc/self/ns/pid");
        let text = link.to_string_lossy();
        let digits: String = text.chars().filter(|c| c.is_ascii_digit()).collect();
        let from_link: u64 = digits.parse().expect("inode digits in pid:[...]");
        let from_stat = std::fs::metadata(SELF_PIDNS).expect("stat").ino();
        assert_eq!(from_link, from_stat);
    }
}
