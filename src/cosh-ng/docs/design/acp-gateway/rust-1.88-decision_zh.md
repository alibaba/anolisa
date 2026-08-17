# Rust 1.88 工具链决策

[English](rust-1.88-decision.md)

关联架构：[COSH Gateway 与 ACP 架构](README_zh.md)

## 背景

官方 `agent-client-protocol` 2.0.0 声明 MSRV 1.88.0。为了让本地开发、CI、RPM
构建和 ACP Runtime 使用同一可复现环境，cosh-ng 统一工具链基线。

## 决策

cosh-ng 的最低 Rust 版本和固定 toolchain 统一为 **1.88.0**。

该决策只提升编译器基线，不要求把现有 crate 的 Rust edition 从 2021 改为 2024。
工具链升级与 ACP SDK、协议实现和 Gateway 功能保持独立，以便单独判断构建影响并安全
回滚。

## 原因

- 满足官方 ACP Rust SDK 2.0.0 的明确 MSRV。
- 固定开发机、CI、RPM 构建和发布环境，避免 stable 漂移导致不可复现。
- 让所有贡献者从一致基线开始，避免协议代码与工具链故障混在同一变更中。
- 1.88.0 是依赖要求的最小版本，不主动抬高到更新 stable。

## Workspace 要求

工具链基线必须在以下位置保持一致：

| 位置 | 要求 |
|------|------|
| `src/cosh-ng/Cargo.toml` | `workspace.package.rust-version = "1.88"` |
| `src/cosh-ng/rust-toolchain.toml` | 固定 `channel = "1.88.0"`，保留 rustfmt 和 clippy |
| cosh-ng CI job | 安装并使用 1.88.0，不依赖 runner 默认 stable |
| RPM/build image | 构建前检查 `rustc` 满足 1.88，缺失时明确失败 |
| 开发文档 | 更新 MSRV、安装和故障提示 |

不得只修改 Cargo manifest 而让 CI 或 RPM 使用隐式工具链。未来升级 Rust 时也必须原子
更新这些位置。

## 验证门禁

工具链或依赖变更需要在 Linux 环境完成：

```bash
cd src/cosh-ng
rustc --version
cargo fmt --all -- --check
cargo clippy --workspace --all-targets --locked -- -D warnings
cargo test --workspace --locked
cargo build --workspace --release --locked
```

`rustc --version` 必须报告 1.88.x。涉及 public API 或 rustdoc 的同期调整还需执行：

```bash
cargo doc --workspace --no-deps --locked
```

还要在项目实际使用的 RPM 构建环境中验证一次，确认系统仓库或构建镜像能够稳定提供
1.88，而不是只在 GitHub Actions 中成功。

## 兼容性影响

- 使用旧版 Rust 的源码构建者需要先升级工具链。
- 已发布二进制和运行时协议不因编译器升级自动发生变化。
- edition 继续使用 2021，降低一次迁移中无关语义变化的范围。
- 其他依赖仍需独立评审，Rust 1.88 不构成接受任意新版依赖的授权。

## 升级与回滚

Rust 基线升级独立于协议和功能变更，并同时验证 CI 与 RPM 环境。升级后的构建环境无法
稳定提供目标版本时，整体回滚该次工具链升级。ACP 实现不得通过 fork SDK、复制生成
类型或绕过 MSRV 检查进入主干；替代方案需要新的架构决策。

贡献者使用仓库中的 `rust-toolchain.toml` 获取一致工具链，不应依赖系统默认 stable。

## 参考

- [agent-client-protocol 2.0.0 manifest](https://docs.rs/crate/agent-client-protocol/2.0.0/source/Cargo.toml)
