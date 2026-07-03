# 测试

## 运行测试

```bash
cd src/cosh-ng

# 全部测试
cargo test --locked

# 按 crate 运行
cargo test --locked -p cosh-types      # 纯类型，无测试
cargo test --locked -p cosh-platform   # 174 个单元测试
cargo test --locked -p cosh-cli        # 55 个集成测试
cargo test --locked -p cosh-core       # 单元 + 集成测试
cargo test --locked -p cosh-shell      # 单元 + 集成（含 PTY 测试，耗时较长）

# 运行单个测试
cargo test --locked -p cosh-platform test_detect_alinux

# 运行某个测试文件
cargo test --locked -p cosh-cli --test cli_integration
```

## 测试分层

### cosh-types

纯类型定义，通常无测试。序列化/反序列化正确性由上层 crate 测试覆盖。

### cosh-platform

单元测试覆盖：
- 发行版检测（模拟 `/etc/os-release`）
- 包管理器命令生成
- 审计策略规则匹配
- ws-ckpt IPC 协议编解码

```bash
cargo test --locked -p cosh-platform
# 约 174 个测试，< 1s
```

### cosh-cli

集成测试（`crates/cosh-cli/tests/cli_integration.rs`）：
- JSON 输出信封格式验证
- `--dry-run` 行为
- 各子命令参数解析
- 错误码映射

```bash
cargo test --locked -p cosh-cli
# 约 55 个测试，~ 4s
```

### cosh-core

| 测试文件 | 覆盖范围 |
|----------|----------|
| `tests/jsonl_protocol.rs` | JSONL 消息序列化/反序列化 |
| `tests/registry_protocol.rs` | Registry 模式请求响应 |
| `tests/tool_approval.rs` | 工具审批协议 |
| `tests/sls_integration.rs` | SLS 日志集成 |

单元测试分布在各模块中：
- `extension/` — 配置解析、变量替换、扩展加载
- `state.rs` — 状态文件读写
- `hook.rs` — 钩子协议

```bash
cargo test --locked -p cosh-core
```

### cosh-shell

测试结构最复杂，分为三层：

**集成测试**（`crates/cosh-shell/tests/`）：

| 目录 | 覆盖范围 |
|------|----------|
| `logic/` | 命令分类、失败分析、agent 事件处理 |
| `protocol/` | 适配器协议、控制消息 |
| `raw_cli/` | Raw 模式 CLI 行为 |
| `shell_host/` | PTY 会话、OSC 标记解析 |

**内联单元测试**（`src/` 内 `#[cfg(test)]` 模块）：

| 模块 | 覆盖范围 |
|------|----------|
| `approval/tests.rs` | 审批决策逻辑 |
| `hooks/` | 钩子引擎、运行时行为 |
| `runtime/` | 状态机、evidence 请求 |
| `shell_host/osc_tests.rs` | OSC 转义序列解析 |
| `tools/` | 工具风险分析、展示 |

```bash
cargo test --locked -p cosh-shell
# 耗时较长（PTY 测试需要 fork + 终端交互）
```

## 测试工具

| 工具 | 用途 |
|------|------|
| `tempfile` | 创建临时目录用于隔离测试 |
| `COSH_STATES_DIR` | 环境变量覆盖状态目录 |
| `ExtensionManager::new_isolated()` | 测试专用构造函数 |

## 编写测试指南

1. 集成测试放在 `tests/` 目录，单元测试内联在模块中
2. 测试函数命名：`test_<被测行为>_<场景>`
3. 使用 `tempfile::tempdir()` 隔离文件系统副作用
4. 不依赖网络（LLM API 测试用 mock）
5. PTY 测试标注 `#[ignore]`（如需要真实终端环境）
