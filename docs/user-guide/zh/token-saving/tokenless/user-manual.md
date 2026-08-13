# Tokenless 用户手册

[English](../../../en/token-saving/tokenless/user-manual.md)

Tokenless 面向工具调用密集的 AI Agent。它的 CLI 可以精简 Schema 和 JSON 响应，Adapter 还可以改写 Shell 命令、检查工具依赖，并把压缩结果交给 Agent。最终效果取决于宿主框架：有的 Adapter 会替换原始结果，有的只会追加压缩上下文而保留原文。

第一次使用请从[快速开始](QUICKSTART.md)进入。

## 从源码构建独立 CLI

源码构建适合开发和调试。当前项目只在 Linux 上验证和支持源码构建：

```bash
git clone https://github.com/alibaba/anolisa.git
cd anolisa/src/tokenless
cargo build --release --locked -p tokenless-cli
./target/release/tokenless --version
```

这条路径只生成独立的 `tokenless` CLI，不会安装 `rtk`、`toon` 或 Agent 接入资源。需要在 Agent 中使用完整能力时，请按照[快速开始](QUICKSTART.md)通过 anolisa CLI 安装。

### 二进制发布包（无需构建工具链）

预构建的发布 tarball 内置 `install.sh`，可在目标机器上直接部署，无需安装 Rust、Cargo 或 Node.js。仅支持 Linux（x86_64 / aarch64）。

```bash
# 构建 tarball（在 Linux 构建机上）
cd anolisa/src/tokenless
make release
# => dist/tokenless-<VERSION>-<ARCH>.tar.gz

# 在目标机器上部署
tar xzf tokenless-<VERSION>-<ARCH>.tar.gz
cd tokenless-<VERSION>
sudo ./install.sh              # 安装到 /usr/local（默认）
sudo PREFIX=/usr ./install.sh  # 安装到 /usr
```

安装器部署内容：
- `bin/tokenless` 和 `libexec/anolisa/tokenless/{rtk,toon}`，并在 `bin/` 创建符号链接
- 适配器资源：`share/anolisa/adapters/tokenless/`
- 组件契约：`share/anolisa/components/tokenless/component.toml`
- Cosh extension：`share/anolisa/extensions/tokenless/`（hooks、commands、cosh-extension.json）

**注意：** `PREFIX` 必须为绝对路径。安装器采用覆盖式部署——每次运行会替换整个适配器资源目录和 cosh extension。如需非破坏性或分阶段安装，请使用源码构建的 `make install`。

如果 cosh 默认不扫描 `$PREFIX/share/anolisa/extensions`（系统扫描路径为 `/usr/share/anolisa/extensions`），使用非标准 PREFIX 时可能需要手动配置 cosh 的 extension 扫描路径。

## 能力与边界

| 能力 | 当前代码实际执行的行为 | 重要边界 |
|------|------------------------|----------|
| Schema 压缩 | 移除 `title` 和 `examples`，删除描述中的围栏代码和行内代码，合并空白并截断描述 | 只有 cosh 和 Qwen Code 注册了 Schema Hook；其他场景可直接调用 CLI |
| 响应压缩 | 移除名称完全匹配且区分大小写的调试字段、`null`、空字符串/数组/对象，并按配置阈值截断 | 输入必须是 JSON；Adapter 会主动跳过内容读取类工具 |
| TOON 编码 | 编码 JSON；估算 Token 没有下降时保留 JSON 输入 | TOON 是替换原文还是与原文并存，取决于 Adapter |
| 命令重写 | 有匹配规则时调用 `rtk rewrite`，再向框架提交改写后的 Shell 输入 | 真正提交给 Shell 的命令会变化；无规则或被拒绝时透传 |
| Tool Ready | 检查声明的二进制、版本、配置、权限和可选依赖 | `--fix` 只安装缺失的必需依赖，并可能改变环境 |
| Stash | 保存因字符串、数组、深度或 Schema 描述截断而移除的内容 | 默认 TTL 一小时、最多 10,000 个有效条目；其他被移除字段不会进入 Stash |

代码没有提供固定节省率保证。结果取决于 Payload、Adapter 交付语义，以及工具数据在模型上下文中的占比。请按[效果度量](measuring-savings.md)使用自己的工作负载测量。

## Tokenless 如何参与一次工具调用

启用对应 Adapter 后，一次工具调用可能经过以下阶段：

```text
工具调用前：Tool Ready 检查 → 命令重写
工具调用后：响应压缩 → 可选 Stash → TOON 编码 → 写入统计
模型调用前：Schema 压缩
```

这是能力示意，不是所有框架都会完整执行的固定流水线。例如 OpenClaw 默认关闭 TOON，Codex 追加压缩上下文而不替换原始工具结果，只有 cosh 和 Qwen Code 注册 Schema 压缩。具体见[框架集成](framework-integration.md)。

## 需要特别理解的行为

### 安装不等于启用

`anolisa install tokenless` 安装组件和 Adapter 资源。要让某个 Agent 自动使用 Tokenless，还需要：

```bash
anolisa adapter enable tokenless <framework>
```

CLI-only 用法不需要 Adapter。

### “关闭压缩”只影响三个压缩操作

设置 `compression_enabled=false` 或 `TOKENLESS_COMPRESSION_ENABLED=0` 后，无论是直接调用还是通过 Adapter 调用，`compress-schema`、`compress-response` 和 `compress-toon` 都仍会计算预测节省并可能写入统计，但会返回原始输入。该模式不会写入 Stash 条目。

这个设置不会关闭 RTK 命令重写、Tool Ready 检查、Adapter 执行或内容取回。如需停止 Agent 中的所有 Tokenless 行为，应禁用 Adapter：

```bash
anolisa adapter disable tokenless <framework>
```

### 可逆压缩是有条件的

启用压缩时，响应和 Schema 截断默认会把被移除的 Payload 写入 `~/.tokenless/stash.db`，并在输出中加入：

```text
<<tokenless:0123456789abcdef01234567>>
```

可以通过 `tokenless retrieve` 或 MCP `tokenless_retrieve` 取回。以下情况会失去可逆性：

- 使用了 `--no-stash`。
- 压缩处于 dry-run 模式。
- Stash 数据库不可用或写入失败。
- 条目已经超过 TTL。
- 有效条目超过 10,000 个后，较早条目被容量策略淘汰。
- 调用方使用了不同的 Stash 数据库路径。

Stash 并不能让所有压缩都可逆。被移除的 `debug`/`trace` 字段、`null` 和空值、Schema `title`/`examples` 以及 Markdown 格式不会保存供取回。启用实际压缩前，应使用有代表性的数据验证关键 Payload。

### 普通处理错误通常 fail-open

缺少 `tokenless` 或 `rtk`、压缩无收益或发生普通处理错误时，压缩和重写 Hook 通常不返回修改。Tool Ready 不同：部分 Adapter 会在自动修复后仍为 `NOT_READY` 时主动阻止工具执行。Stash 写入失败时，仍可能继续执行有损压缩。

命令重写也会改变宿主提交的 Shell 命令。大多数 Adapter 会直接替换命令输入；Hermes 会先阻止第一次调用，再提示 Agent 使用改写命令重试。因此，除了压缩结果，还应验证重要命令工作流。

## 支持的 Agent 框架

| 框架 | 集成方式 | 当前代码路径 |
|------|----------|--------------|
| cosh | Extension | Tool Ready、命令重写、响应压缩 + TOON、Schema；Cosh-NG 有替换路径，旧版 Copilot Shell 则追加额外上下文 |
| OpenClaw | Plugin | Tool Ready、`exec` 命令重写、替换持久化结果、可选 TOON；无 Schema |
| Hermes | Plugin | Tool Ready、阻止后重试的命令重写、用响应压缩 + TOON 替换结果；无 Schema |
| Qoder | Plugin | Tool Ready、命令重写、通过 `additionalContext` 交付响应压缩 + TOON；无 Schema |
| Claude Code | Marketplace Plugin | Tool Ready、Bash 命令重写；Claude Code 2.1.121 及以上可替换响应；条件式 TOON；无 Schema |
| Codex | Plugin | Tool Ready、命令重写；把响应/TOON 分析追加为上下文，保留原始结果；无 Schema |
| Qwen Code | Extension | Tool Ready、命令重写、通过 `additionalContext` 交付响应压缩 + TOON、Schema |

## 按任务查找文档

| 我想做什么 | 文档 |
|------------|------|
| 第一次安装并验证 | [快速开始](QUICKSTART.md) |
| 从源码构建独立 CLI | [本页 · 从源码构建独立 CLI](#从源码构建独立-cli) |
| 接入或切换 Agent 框架 | [框架集成](framework-integration.md) |
| 手动压缩、取回或运行 MCP | [CLI 参考](cli-reference.md) |
| 查看节省或内容变化、做双跑对比 | [效果度量](measuring-savings.md) |
| 修改配置或了解本地数据 | [配置与数据隐私](configuration-and-privacy.md) |
| 解决无统计、Adapter 或 Stash 问题 | [故障排查](troubleshooting.md) |
| 升级或卸载 | [故障排查 · 升级与卸载](troubleshooting.md#升级与卸载) |

## 推荐的上线顺序

1. 在非敏感测试任务中完成[快速开始](QUICKSTART.md)。
2. 使用 dry-run 记录同一任务的基线。
3. 开启真实压缩并比较结果质量与节省。
4. 确认本地数据和 SLS 策略符合要求。
5. 再为生产使用的 Agent 启用 Adapter。

Tokenless 的配置和 CLI 以当前安装版本的 `tokenless --help` 为最终依据。
