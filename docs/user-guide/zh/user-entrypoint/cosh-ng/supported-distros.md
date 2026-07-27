# 支持的发行版

cosh-ng 通过读取 `/etc/os-release`（Linux）或调用 `sw_vers`（macOS）自动检测
当前操作系统，并路由到对应的包管理器和服务管理器后端。

## 支持矩阵

| 发行版 | 包管理器 | 服务管理器 | 备注 |
|--------|----------|------------|------|
| Alinux (2/3/4) | dnf | systemd | 阿里云原生 Linux |
| CentOS 7/8/9 | dnf | systemd | |
| Fedora | dnf | systemd | |
| Ubuntu | apt-get | systemd | |
| Debian | apt-get | systemd | |
| openSUSE Leap/Tumbleweed | zypper | systemd | |
| macOS | brew | — | 仅 pkg 子系统可用 |

## 检测逻辑

发行版检测通过 `Distro::detect()` 实现：

1. 编译目标为 macOS 时，调用 `sw_vers -productVersion` 获取版本
2. Linux 系统读取 `/etc/os-release`，解析 `ID` 和 `VERSION_ID` 字段
3. 根据 `ID` 映射到已知发行版变体
4. 未识别的 `ID` 归入 `Unknown`，大多数操作返回 `UnsupportedDistro` 错误

## 包管理器映射

| 发行版 ID | 包管理器 | 安装命令 | 搜索命令 |
|-----------|----------|----------|----------|
| alinux / centos / fedora | Dnf | `dnf install -y` | `dnf search -q` |
| ubuntu / debian | Apt | `apt-get install -y` | `apt-cache search` |
| opensuse-leap / opensuse-tumbleweed / sles | Zypper | `zypper install -y` | `zypper search` |
| macOS | Brew | `brew install` | `brew search` |

## 服务管理器

所有 Linux 发行版统一使用 systemd（`systemctl`）。macOS 不支持 svc 子系统。

## 添加新发行版支持

如需支持新的发行版，参见开发者文档
[新增发行版适配指南](../../../../developer-guide/zh/cosh-ng/adding-distros.md)。
