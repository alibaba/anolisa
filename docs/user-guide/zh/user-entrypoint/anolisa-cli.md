# anolisa CLI

`anolisa` CLI 是 ANOLISA 组件的统一生命周期入口。它负责解析组件来源、
维护按 scope 隔离的安装记录、将 RPM transaction 委托给 native package
manager，并诊断或修复状态漂移。

---

## 安装

### 方式 A：安装脚本（推荐）

```bash
curl -fsSL https://agentic-os.sh | sh
```

### 方式 B：YUM（Alinux）

```bash
sudo yum install anolisa
```

验证安装：

```bash
anolisa --version
```

---

## Scope 与可见性

`--install-mode user` 写入当前用户目录，`--install-mode system` 写入
system state，通常需要 root。未指定时，root 默认使用 system mode，普通
用户默认使用 user mode。

只读命令使用 user-plus-system 视图。因此普通用户可以看到和诊断 system
安装，adapter discovery 也可以使用其发布的 contract。修改命令仍然只写入
显式选择的 scope；同一组件的 user 安装和 system 安装可以共存。

---

## 命令

### install

通过配置的 raw 或 RPM backend 安装单个组件，或规划 index 中的全部组件：

```bash
anolisa --install-mode user install <component>
sudo anolisa --install-mode system install <component>
anolisa install --all
```

另一个 scope 中已有安装，不会让当前 scope 变成“already installed”。已有
记录的重装或变更由 lifecycle planner 处理，不会被静默覆盖。

### uninstall

从所选 scope 移除一个安装：

```bash
anolisa uninstall <component>
anolisa uninstall <component> --purge
sudo anolisa --install-mode system uninstall <component> --remove-system-package
```

ANOLISA-owned 文件和 managed RPM package 由各自的 owner backend 移除。
adopted 或 observed system RPM 默认保留；只有确实要移除 native package 时
才使用 `--remove-system-package`。

### update

更新一个组件、全部已记录组件、CLI 本身，或运行只读 RPM 更新检查：

```bash
anolisa update <component>
anolisa update all
anolisa update self
anolisa update --check
```

`update all` 不更新 CLI 本身。delegated 成员会在可行时合并成一次 native
transaction，但每个组件仍保留独立的 recovery journal 和 record。

### list 与 status

查看有效的 user-plus-system 视图：

```bash
anolisa list
anolisa list --installed
anolisa status
anolisa status <component>
```

user view 中两个 scope 都有同名组件时，user record 为 active，system record
仍作为 shadowed state 可见。system-mode view 只读取 system root，不枚举其他
用户的 state。

### doctor

运行只读的 health、dependency、service、state 与 recovery journal 检查：

```bash
anolisa doctor
anolisa doctor <component>
anolisa --dry-run doctor <component>
```

`doctor` 扫描当前 visibility view 中的全部 root：user mode 包含 user root 和
可读的 system root，system mode 只包含 system root。当前调用不能修改某个
system root 时，它会在修复建议中补全
`sudo anolisa --install-mode system`。`--fix` 在当前版本中仍为保留参数；请
显式执行输出的 `fix_plan`。

### restart

重启所选 scope 安装记录中的 service：

```bash
anolisa --install-mode user restart <component>
sudo anolisa --install-mode system restart <component>
```

### upgrade

预览或应用 system/RPM image 升级。raw-managed 组件会报告为 skipped，不会被
迁移到其他 backend：

```bash
anolisa --install-mode system --dry-run upgrade
sudo anolisa --install-mode system upgrade
sudo anolisa --install-mode system upgrade --target <profile>
```

### adopt、repair 与 forget

在不混淆 package ownership 的前提下管理状态：

```bash
sudo anolisa --install-mode system adopt <component>
sudo anolisa --install-mode system repair <component>
anolisa --install-mode user forget <component>
sudo anolisa --install-mode system forget <component>
```

`adopt` 将已有 system RPM 记录为 delegated-adopted，不取得 native removal
authority。`repair` 协调指定 scope 的 record 与 rpmdb 或中断 journal。
`forget` 只删除所选 scope 的记录，绝不执行 package 或 owned-file 删除；
user scope 的 forget 不能删除只是在视图中可见的 system 记录。

### adapter

管理组件 adapter：

```bash
anolisa adapter scan
anolisa adapter enable <component> [framework]
anolisa adapter disable <component> [framework]
anolisa adapter status [component]
```

### logs 与 bug report

查看组件日志或生成诊断包：

```bash
anolisa logs <component>
anolisa logs <component> --limit 50
anolisa logs <component> --severity warn
anolisa bug
```

---

## 恢复行为

install、uninstall、update、adopt 和 repair 会先在所选 state root 写入
recovery intent，再执行 lifecycle 副作用。native package 操作采用
forward-only 策略：如果 dnf 可能已经提交而 ANOLISA record 尚未提交，journal
会保持 pending，`anolisa repair <component>` 会重新观察 rpmdb。owned-file
操作保留已校验的 backup，并在失败时按逆序补偿。`forget` 是原子的 record-only
状态更新，不执行 package/file 副作用，也不创建 recovery journal。

`upgrade` 仍是兼容性 orchestrator，不是 planner/journal consumer。它会拒绝
已有的 pending recovery，并在 transaction 失败后重新观察 rpmdb，但不会创建
per-component recovery journal。`upgrade` 若被进程中断，应先运行
`anolisa doctor`，处理报告的 component drift 后再执行其他 lifecycle 修改。

不要为了解除阻塞而直接删除 pending journal。先运行 `doctor` 确认其 scope
和 subject，再执行带完整 scope 的 `repair` 命令。malformed 或 ambiguous
journal 会有意保持 pending，等待人工检查。

---

## 全局选项

| 选项 | 说明 |
|------|------|
| `--install-mode user\|system` | 选择修改 scope |
| `--prefix <PATH>` | 覆盖所选 scope 的安装前缀 |
| `--dry-run` | 输出计划但不执行 |
| `--json` | 输出机器可读的 JSON |
| `-v, --verbose` | 增加详细程度 |
| `-q, --quiet` | 隐藏非错误输出 |
| `--no-color` | 禁用彩色输出 |
| `--version` | 显示 CLI 版本 |
| `--help` | 显示命令帮助 |

---

## 示例流程

```bash
curl -fsSL https://agentic-os.sh | sh
anolisa env
anolisa install cosh
anolisa install tokenless
anolisa adapter enable tokenless cosh
anolisa doctor
anolisa status
```

---

## 配置

system mode 从 `/etc/anolisa/config.toml` 读取 registry 设置，user mode 从
`~/.config/anolisa/config.toml` 读取。registry resolution 只使用
`[registry]` 表：

```toml
[registry]
url = "https://registry.example.com/index.toml"
cache_ttl_secs = 3600
offline_fallback = true
```

backend 选择和 endpoint 位于对应的 `repo.toml`（`/etc/anolisa/repo.toml`
或 `~/.config/anolisa/repo.toml`）。CLI 参数覆盖当前执行的操作；不存在
`[install] mode` 配置。

---

## 参见

- [安装指南](../installation.md)
- [故障排查](../troubleshooting.md)
