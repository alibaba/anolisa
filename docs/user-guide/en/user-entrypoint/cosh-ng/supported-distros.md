# Supported Platforms and Linux Distributions

[中文版](../../../zh/user-entrypoint/cosh-ng/supported-distros.md)

cosh-ng builds from source on Linux and macOS. The interactive terminal uses
the installed Bash or Zsh on either platform. Package operations use Homebrew
on macOS and distribution-specific backends on Linux. Service operations
currently require Linux with systemd.

## macOS

cosh-ng detects the macOS version with `sw_vers` and routes `cosh-cli pkg`
commands to Homebrew. The documented installer and RPM path target Linux, so
macOS users build the workspace from source. `cosh-cli svc` is unavailable on
macOS because its backend uses `systemctl`.

## Linux Support Matrix

| `/etc/os-release` ID | Package manager | Service manager |
|----------------------|-----------------|-----------------|
| `alinux`, `centos`, `fedora` | dnf | systemd |
| `ubuntu`, `debian` | apt | systemd |
| `opensuse-leap`, `opensuse-tumbleweed`, `sles` | zypper | systemd |

An unlisted Linux distribution can reuse a supported package family when its
`ID_LIKE` contains one of these values:

| `ID_LIKE` family | Package manager | Example |
|---|---|---|
| `alinux`, `centos`, `fedora`, `rhel` | dnf | Rocky Linux |
| `debian`, `ubuntu` | apt | Debian-compatible derivatives |
| `opensuse`, `suse` | zypper | SUSE-compatible derivatives |

The JSON metadata keeps the distribution's real `ID`. Family routing means the
package backend is compatible; it is not a certification of every derivative
or release.

cosh-ng does not promise support for every release of those distributions.
Run `anolisa env` before installation and use `cosh-cli` read-only commands to
verify the package or service backend on the target host.

## Detection and Routing

`Distro::detect()` reads `/etc/os-release`, falling back to
`/usr/lib/os-release` only when the primary file is absent. It normalizes `ID`,
preserves `VERSION_ID`, and checks `ID_LIKE` when the ID is not built in. An ID
with no compatible family remains `Unknown`; package operations then return a
structured `UnsupportedDistro` error.

Package commands route to `dnf`, `apt-get`/`apt-cache`, or `zypper`. Service
commands use `systemctl`. Package install/remove and service mutations expose
action-specific `--dry-run` flags; check the exact action with `--help`.

To add a Linux distribution, follow the
[developer guide](../../../../developer-guide/en/cosh-ng/adding-distros.md).
