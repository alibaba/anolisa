# Supported Distributions

cosh-ng automatically detects the current operating system by reading `/etc/os-release` (Linux) or calling `sw_vers` (macOS), and routes to the corresponding package manager and service manager backend.

## Support Matrix

| Distribution | Package Manager | Service Manager | Notes |
|-------------|----------------|-----------------|-------|
| Alinux (2/3/4) | dnf | systemd | Alibaba Cloud native Linux |
| CentOS 7/8/9 | dnf | systemd | |
| Fedora | dnf | systemd | |
| Ubuntu | apt-get | systemd | |
| Debian | apt-get | systemd | |
| openSUSE Leap/Tumbleweed | zypper | systemd | |
| macOS | brew | — | Only pkg subsystem available |

## Detection Logic

Distribution detection is implemented via `Distro::detect()`:

1. When the compile target is macOS, calls `sw_vers -productVersion` to get the version
2. On Linux, reads `/etc/os-release` and parses the `ID` and `VERSION_ID` fields
3. Maps `ID` to a known distribution variant
4. Unrecognized `ID` values fall into `Unknown`, and most operations return `UnsupportedDistro` error

## Package Manager Mapping

| Distribution ID | Package Manager | Install Command | Search Command |
|----------------|----------------|-----------------|----------------|
| alinux / centos / fedora | Dnf | `dnf install -y` | `dnf search -q` |
| ubuntu / debian | Apt | `apt-get install -y` | `apt-cache search` |
| opensuse-leap / opensuse-tumbleweed / sles | Zypper | `zypper install -y` | `zypper search` |
| macOS | Brew | `brew install` | `brew search` |

## Service Manager

All Linux distributions use systemd (`systemctl`) uniformly. macOS does not support the svc subsystem.

## Adding New Distribution Support

To add support for a new distribution, see the developer documentation
[Adding Distribution Support Guide](../../../../developer-guide/en/cosh-ng/adding-distros.md).
