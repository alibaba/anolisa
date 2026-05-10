# ws-checkpoint

基于 btrfs 文件系统的 AI 工作区快照管理系统，支持秒级创建检查点和回滚，专为 AI Agent 场景设计。

## 特性

- **毫秒级快照创建和回滚** — 利用 btrfs COW 特性，微秒级完成快照操作
- **守护进程架构** — 特权操作封装在 daemon 中，上层调用无需 root 权限
- **Unix Socket IPC** — Bincode 二进制协议，高效通信
- **systemd 服务化** — RPM 一键部署，开机自启
- **快照列表查询** — 支持 table/json 多格式输出
- **快照差异对比** — 查看两个快照间的文件变更
- **运行时状态监控** — 守护进程和工作区健康状态一览

## 项目结构

```
ws-ckpt/
├── src/                       # Rust Cargo workspace
│   ├── Cargo.toml
│   ├── config.toml.sample     # 配置示例（安装到 /etc/ws-ckpt/；复制为 config.toml 启用）
│   ├── crates/
│   │   ├── common/            # 共享类型、IPC 协议编解码
│   │   ├── daemon/            # 守护进程核心逻辑
│   │   └── cli/               # 命令行客户端
│   ├── systemd/               # systemd service 文件
│   └── skills/                # OS Skills
├── docs/                      # 项目文档
├── ws-ckpt.spec.in            # RPM 规格模板
├── build-rpm.sh               # RPM 打包脚本
└── .gitignore
```

## 快速开始

### 环境要求

- Linux（推荐 Alinux 4）
- btrfs 文件系统
- Rust 1.70+

### 编译

```bash
cd src
cargo build --release
```

### 安装（RPM）

```bash
# 打包
./build-rpm.sh

# 安装
sudo rpm -ivh ~/rpmbuild/RPMS/x86_64/ws-ckpt-*.rpm

# 启动服务
sudo systemctl start ws-ckpt
```

### 基本用法

```bash
# 初始化工作区
ws-ckpt init --workspace ~/my-workspace

# 创建检查点
ws-ckpt checkpoint --workspace ~/my-workspace -M 1 -S 1 -m "initial version"

# 再次修改后创建检查点
ws-ckpt checkpoint --workspace ~/my-workspace -M 1 -S 2 -m "add feature"

# 回滚到指定快照
ws-ckpt rollback --workspace ~/my-workspace --to msg1-step1

# 删除快照
ws-ckpt delete --workspace ~/my-workspace --snapshot msg1-step2
```

### 快照管理

```bash
# 列出工作区所有快照
ws-ckpt list --workspace ~/my-workspace

# 以 JSON 格式输出
ws-ckpt list --workspace ~/my-workspace --format json

# 查看两个快照间的差异
ws-ckpt diff --workspace ~/my-workspace --from msg1-step1 --to msg1-step2

# 清理旧快照，保留最近 5 个
ws-ckpt cleanup --workspace ~/my-workspace --keep 5
```

### 状态与配置

```bash
# 查看系统状态
ws-ckpt status --workspace ~/my-workspace

# 查看当前配置
ws-ckpt config

# 修改配置
ws-ckpt config --set cleanup.keep=10
```

## 命令总览

| 命令 | 说明 |
|------|------|
| `checkpoint` | 创建快照检查点 |
| `rollback` | 回滚到指定快照 |
| `delete` | 删除工作区或单个快照 |
| `list` | 列出工作区所有快照 |
| `diff` | 查看两个快照间的文件变更 |
| `cleanup` | 自动清理旧快照 |
| `status` | 查看守护进程和工作区状态 |

## 组件

| 组件 | 状态 | 说明 |
|------|------|------|
| Daemon | 基本完成 | 在 root 权限在运行，实际操作文件系统 |
| CLI | 基本完成 | init / checkpoint / rollback / delete / list / diff / cleanup / status / config |
| plugin | 待实现 | OpenClaw 插件，自动 checkpoint/rollback |
| skills | 初版 | `src/skills/ws-ckpt/SKILL.md` 初版完成，目前仅适配openclaw |

## OpenClaw Skill

ws-ckpt 提供了一个配套 [OpenClaw](https://github.com/alibaba/anolisa) 的 skill 定义，位于 `src/skills/ws-ckpt/SKILL.md`。

如需从源码手动安装，可将目录复制到 OpenClaw skill 路径下：

```bash
cp -r src/skills/ws-ckpt <your-openclaw-skills-dir>/ws-ckpt
```

## 开发

```bash
# 运行测试
cd src
cargo test --workspace

# 代码检查
cargo clippy --workspace -- -D warnings
```

## 文档

- [使用文档](docs/ws-ckpt-usage.md)
- [RPM 打包](docs/RPM-PACKAGING.md)

## License

Licensed under the Apache License, Version 2.0; see [LICENSE](../../LICENSE).

ws-ckpt interacts with the Linux kernel btrfs filesystem (GPL-2.0) solely through the
public system call interface, and invokes `btrfs-progs` (GPL-2.0) exclusively as
independent executable processes. No source code, object code, or header files from any
GPL-licensed component are incorporated, statically linked, or dynamically linked into
ws-ckpt. Such interaction constitutes an independent and separate work within the meaning
of GPL-2.0 Section 2 ("mere aggregation") and imposes no copyleft obligation on ws-ckpt.
