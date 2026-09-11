# AW

[English](README.md)

AW 为能力调用及其记录提供版本化 JSON Schema 和离线校验器。这个 Rust 库检查数据结构，以及调用、结果和观测记录之间的关系。它没有独立服务进程，也不执行 Provider 或控制 Agent。

当前接口仍处于实验阶段。测试使用合成记录，实际运行时接入需要另行验证。

## 运行检查

准备 Rust 工具链及 rustfmt、Clippy。跨语言摘要测试还需要 Python 3 和 Node.js，Rust 库本身不依赖这两个运行时。在仓库根目录执行以下命令。

```bash
cd src/aw
cargo test --workspace --locked
python3 tests/check_canonical.py
cargo fmt --all -- --check
cargo clippy --workspace --all-targets --locked -- -D warnings
cargo doc --workspace --no-deps --locked
```

这些检查可由普通用户运行，无需启动 Agent 或登录服务。Cargo 会下载尚未缓存的依赖，Schema 校验只读取随包资源。

已验证的环境为 Linux ARM64，使用 Rust 1.97.1、Python 3.12.3 和 Node.js 24.15.0。最低支持版本和其他操作系统尚未验证。

## 源码参考

- [已注册的 Schema](schemas/)与[合成输入输出样例](tests/fixtures/contracts.json)
- [公共 API](src/lib.rs)、[记录校验](src/validation.rs)与[计划校验](src/orchestration.rs)
- [编码测试](tests/canonical.rs)、[Schema 测试](tests/schemas.rs)、
  [记录测试](tests/contracts.rs)与[计划测试](tests/orchestration.rs)

Registry 包含 21 个 Schema 资源。`crates/aw-contracts/schemas/` 中的 8 份 v1 文件仅作参考，未注册到当前库。调用方需要匹配 Schema ID 和摘要，当前没有自动版本转换。

收到数据后，先用 `canonical::parse` 严格解析字节，再检查结构。结构检查通过不代表记录之间的关系正确，也不授予执行权限。计划级检查的用法见公共 API 文档，证据认证和实际动作仍由调用方负责。
