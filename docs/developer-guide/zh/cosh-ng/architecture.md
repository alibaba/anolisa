# 架构

cosh-ng 采用 5-crate Rust 工作空间架构，版本号 0.11.0，Rust 版本要求 1.74+。

## Crate 依赖图

```
cosh-types          cosh-platform          cosh-cli / cosh-core
  (纯类型)       ← (发行版检测 +        ← (CLI 入口 / Agent 核心)
  零副作用          后端路由)

cosh-shell
  (独立 crate，无内部依赖)

依赖方向: cosh-cli / cosh-core → cosh-platform → cosh-types
         cosh-shell 独立（通过 stdin/stdout 与 cosh-core 进程通信）
```

## Crate 职责

| Crate | 二进制 | 职责 |
|-------|--------|------|
| `cosh-types` | — | 纯数据类型，零副作用。定义 CoshResponse 信封、CoshError、ws-ckpt IPC 类型 |
| `cosh-platform` | — | 平台抽象层。发行版检测、包管理器路由、systemd 适配、ws-ckpt IPC 客户端、审计系统 |
| `cosh-cli` | `cosh-cli` | CLI 入口。4 个命令域（pkg/svc/checkpoint/audit），JSON 输出 |
| `cosh-core` | `cosh-core` | Agent 核心。Headless JSONL 后端、LLM 集成、钩子、工具、技能、扩展、会话 |
| `cosh-shell` | `cosh-shell` | 交互终端。PTY 主机、OSC 标记、AI 适配器、审批控制、TUI 渲染 |

## 目录布局

```
cosh-ng/
├── crates/
│   ├── cosh-types/       # 纯类型定义
│   │   └── src/          # audit.rs, checkpoint.rs, config.rs, error.rs, output.rs, pkg.rs, svc.rs
│   ├── cosh-platform/    # 平台抽象
│   │   └── src/          # audit/, checkpoint.rs, detect.rs, pkg.rs, svc.rs, validate.rs
│   ├── cosh-cli/         # CLI 二进制
│   │   ├── src/          # main.rs, cmd/{pkg,svc,checkpoint,audit}.rs
│   │   └── tests/        # 集成测试
│   ├── cosh-core/        # Agent 核心二进制
│   │   └── src/          # main.rs, core.rs, headless.rs, hook.rs, provider/, tool/, skill/, extension/
│   └── cosh-shell/       # 交互终端二进制
│       ├── src/          # main.rs, adapter/, agent/, approval/, hooks/, shell_host/, tools/, ui/
│       └── tests/        # 分层测试
├── Cargo.toml            # 工作空间配置
└── rust-toolchain.toml
```

## 数据流

### cosh-cli 执行流程

```
用户命令 → clap 解析 → cmd 模块路由 → cosh-platform 后端执行 → CoshResponse<T> JSON 输出
```

### cosh-core headless 流程

```
stdin JSONL → 消息解析 → UserPromptSubmit 钩子 → LLM 生成 → 工具调用 → 审批协议 → stdout JSONL
```

### cosh-shell 交互流程

```
用户键入 → PTY 主机 → OSC 边界检测 → AI 适配器（启动 cosh-core 子进程）
         → 流式响应 → 审批卡片渲染 → 工具执行结果回显
```

## 关键设计约束

- **ws-ckpt IPC 线格式** — bincode + 4 字节小端长度前缀。枚举变体顺序即二进制契约，不可重排
- **统一 JSON 信封** — cosh-cli 所有命令返回 `CoshResponse<T>`（ok + data/error + meta）
- **跨发行版路由** — `Distro::detect()` 读取 `/etc/os-release` 路由到正确后端
- **工具分类** — ReadOnly / FileEdit / ShellExec / ShellEvidence，审批模式据此决策
- **钩子别名** — cosh-ng 内部工具名与 copilot-shell 标准名双向映射

## 依赖管理

所有第三方依赖在 `[workspace.dependencies]` 声明版本，子 crate 通过
`dep = { workspace = true }` 引用。主要依赖：

| 依赖 | 用途 |
|------|------|
| `serde` / `serde_json` | 序列化 |
| `clap` | CLI 参数解析 |
| `tokio` | 异步运行时（cosh-core） |
| `reqwest` | HTTP 客户端（LLM API） |
| `tracing` | 结构化日志 |
| `ratatui` | TUI 渲染（cosh-shell） |
| `nix` | Unix 系统调用 |
| `bincode` | ws-ckpt IPC 序列化 |
