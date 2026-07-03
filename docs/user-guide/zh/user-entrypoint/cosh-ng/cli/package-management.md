# 包管理

`cosh-cli pkg` 子系统提供跨发行版的包管理操作。自动检测当前发行版并路由到
对应的包管理器后端（dnf / apt-get / zypper / brew）。

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

成功输出：

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

成功输出：

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

搜索可用包。返回结果包含安装状态。

```bash
cosh-cli pkg search "web server"
```

输出：

```json
{
  "ok": true,
  "data": {
    "packages": [
      { "name": "nginx", "version": "1.24.0", "description": "...", "installed": true },
      { "name": "apache2", "version": "2.4.58", "description": "...", "installed": false }
    ],
    "total": 2
  },
  "meta": { "subsystem": "pkg", "duration_ms": 800, "distro": "ubuntu", "dry_run": false }
}
```

## list

列出已安装的软件包。

```bash
cosh-cli pkg list --installed
```

输出：

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

| 操作 | dnf | apt-get | zypper | brew |
|------|-----|---------|--------|------|
| install | `dnf install -y` | `apt-get install -y` | `zypper install -y` | `brew install` |
| remove | `dnf remove -y` | `apt-get remove -y` | `zypper remove -y` | `brew uninstall` |
| search | `dnf search -q` | `apt-cache search` | `zypper search` | `brew search` |
| list | `dnf list installed -q` | `dpkg-query -W` | `zypper se --installed-only` | `brew list --versions` |

## 错误处理

- 包不存在时返回 `PkgNotFound`，`hint` 建议搜索
- 包管理器执行失败返回 `PkgBackendError`，`recoverable: true`
- 未支持的发行版返回 `UnsupportedDistro`
