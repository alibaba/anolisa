# 支持的平台与 Linux 发行版

[English](../../../en/user-entrypoint/cosh-ng/supported-distros.md)

cosh-ng 源码可在 Linux 和 macOS 上构建。交互式终端在两个平台上都会使用系统已有的
Bash 或 Zsh。macOS 软件包操作使用 Homebrew，Linux 软件包操作使用发行版对应的
后端。服务操作目前需要 Linux 和 systemd。

## macOS

cosh-ng 通过 `sw_vers` 检测 macOS 版本，并把 `cosh-cli pkg` 命令交给 Homebrew。
当前安装脚本和 RPM 面向 Linux，macOS 用户需要从源码构建。`cosh-cli svc` 使用
`systemctl`，因此不能在 macOS 上使用。

## Linux 支持矩阵

| `/etc/os-release` ID | 包管理器 | 服务管理器 |
|----------------------|----------|------------|
| `alinux`、`centos`、`fedora` | dnf | systemd |
| `ubuntu`、`debian` | apt | systemd |
| `opensuse-leap`、`opensuse-tumbleweed`、`sles` | zypper | systemd |

未单独列出的 Linux 发行版可以通过 `ID_LIKE` 复用已支持的软件包家族。

| `ID_LIKE` 家族 | 包管理器 | 示例 |
|---|---|---|
| `alinux`、`centos`、`fedora`、`rhel` | dnf | Rocky Linux |
| `debian`、`ubuntu` | apt | Debian 兼容衍生版 |
| `opensuse`、`suse` | zypper | SUSE 兼容衍生版 |

JSON 元数据仍保留发行版的真实 `ID`。家族路由只表示包管理后端兼容，
不代表对每个衍生版或版本做完整认证。

这不代表 cosh-ng 承诺支持上述发行版的每一个版本。安装前先运行 `anolisa env`，
并在目标主机上用 `cosh-cli` 的只读命令确认软件包或服务后端。

## 检测与路由

`Distro::detect()` 读取 `/etc/os-release`，仅在主文件不存在时回退到
`/usr/lib/os-release`。它会规范化 `ID`、保留 `VERSION_ID`，并在 ID 未内置时
检查 `ID_LIKE`。没有兼容家族的 ID 保持为 `Unknown`；软件包操作随后返回
结构化的 `UnsupportedDistro` 错误。

软件包命令会调用 `dnf`、`apt-get`、`apt-cache` 或 `zypper`，服务命令使用
`systemctl`。软件包安装、卸载和服务变更支持 `--dry-run`，请用对应操作的 `--help`
确认准确参数。

新增 Linux 发行版支持请参见
[开发者指南](../../../../developer-guide/zh/cosh-ng/adding-distros.md)。
