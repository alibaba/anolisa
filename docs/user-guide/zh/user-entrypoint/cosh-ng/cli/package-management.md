# 包管理

[English](../../../../en/user-entrypoint/cosh-ng/cli/package-management.md)

`cosh-cli pkg` 子系统在 Linux 和 macOS 上提供结构化软件包操作。macOS 使用
Homebrew，Linux 使用 dnf、apt 或 zypper，也支持通过 `ID_LIKE` 声明的兼容衍生版。

## 命令列表

| 命令 | 说明 |
|------|------|
| `cosh-cli pkg install <name>` | 安装软件包 |
| `cosh-cli pkg remove <name>` | 卸载软件包 |
| `cosh-cli pkg search <query>` | 搜索软件包 |
| `cosh-cli pkg list --installed` | 列出已安装包 |

## install

安装指定的软件包。

```bash
cosh-cli pkg install nginx
cosh-cli pkg install nginx --dry-run
```

成功输出示例

```json
{
  "ok": true,
  "data": {
    "package": "nginx",
    "version": "1.24.0",
    "already_installed": false,
    "dependencies_installed": []
  },
  "meta": { "subsystem": "pkg", "duration_ms": 5200, "distro": "alinux", "dry_run": false }
}
```

若包已安装，`already_installed` 为 `true`，命令仍返回成功。

## remove

卸载指定的软件包。

```bash
cosh-cli pkg remove nginx
cosh-cli pkg remove nginx --dry-run
```

成功输出示例

```json
{
  "ok": true,
  "data": {
    "package": "nginx",
    "version_removed": "1.24.0",
    "dependencies_removed": []
  },
  "meta": { "subsystem": "pkg", "duration_ms": 2100, "distro": "ubuntu", "dry_run": false }
}
```

## search

使用便携 glob 子集 `*`、`?` 和方括号字符类搜索软件包名。返回结果包含安装状态。

```bash
cosh-cli pkg search 'libssl*'
```

输出示例

```json
{
  "ok": true,
  "data": {
    "packages": [
      { "name": "libssl3", "version": "3.0.2", "summary": "...", "installed": true },
      { "name": "libssl-dev", "summary": "...", "installed": false }
    ],
    "total": 2
  },
  "meta": { "subsystem": "pkg", "duration_ms": 800, "distro": "ubuntu", "dry_run": false }
}
```

Query 会作为单个参数传递，不经过 Shell 展开。系统会拒绝 backend-specific regex，
并转换便携 pattern，让各个受支持的后端使用一致的完整软件包名语义。

## list

列出已安装的软件包。

```bash
cosh-cli pkg list --installed
```

输出示例

```json
{
  "ok": true,
  "data": {
    "packages": [
      { "name": "nginx", "version": "1.24.0" },
      { "name": "curl", "version": "8.5.0" }
    ],
    "total": 2
  },
  "meta": { "subsystem": "pkg", "duration_ms": 300, "distro": "centos", "dry_run": false }
}
```

## 后端映射

| 操作 | dnf | apt | zypper | Homebrew |
|------|-----|-----|--------|----------|
| install | `dnf install -y` | `apt-get install -y` | `zypper install -y` | `brew install` |
| remove | `dnf remove -y` | `apt-get remove -y` | `zypper remove -y` | `brew uninstall` |
| search | `dnf search -q` | `apt-cache search --names-only` | `zypper search` | `brew search` |
| list | `dnf list installed -q` | `dpkg-query -W` | `zypper se --installed-only` | `brew list --versions` |

## 错误处理

- 软件包不存在时返回 `PkgNotFound`，`hint` 建议搜索。
- 包管理器执行失败返回 `PkgBackendError`，并设置 `recoverable: true`。
- 没有直接 ID 或兼容 `ID_LIKE` 家族的发行版返回 `UnsupportedDistro`。

路由和支持边界见[支持的平台](../supported-distros.md)。
