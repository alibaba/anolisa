# ANOLISA CLI 构建指南

> English version: [BUILD.md](BUILD.md)

## 前置条件

- Rust >= 1.88（项目使用 edition 2024）
- 工作目录：`src/anolisa/`（即 Cargo workspace 根）

```bash
cd src/anolisa
```

---

## 本地调试（开发构建）

```bash
# 仅编译，快速检查
cargo build -p anolisa-cli

# 编译并直接运行
cargo run -p anolisa-cli -- env
cargo run -p anolisa-cli -- list
cargo run -p anolisa-cli -- enable agent-observability --dry-run

# 跑测试
cargo test -p anolisa-core
cargo test --workspace
```

产物位置：`target/debug/anolisa`

---

## 生产构建

```bash
cargo build --release -p anolisa-cli
```

产物结构：

```
target/release/anolisa          # 主二进制（保留符号表，剥离 DWARF）
target/release/anolisa.dwp      # 分离的 DWARF debug info（Linux）
target/release/anolisa.dSYM/    # macOS 上的 debug info
```

发布时只交付主二进制；`.dwp` / `.dSYM` 归档保存。需要分析 coredump 时：

```bash
# 将 .dwp 放到二进制同目录，GDB 自动发现
gdb ./anolisa core.12345

# 或手动指定
gdb -s anolisa.dwp ./anolisa core.12345
```

---

## 交叉编译（为 Linux x86_64 目标构建）

```bash
# 添加 target
rustup target add x86_64-unknown-linux-gnu

# 交叉编译（需要对应 linker，如 x86_64-linux-gnu-gcc）
cargo build --release -p anolisa-cli --target x86_64-unknown-linux-gnu
```

产物：`target/x86_64-unknown-linux-gnu/release/anolisa`

---

## 快速对照

| 场景 | 命令 | 产物路径 |
|------|------|--------|
| 快速跑起来 | `cargo run -p anolisa-cli -- <subcmd>` | — |
| 本地调试 | `cargo build -p anolisa-cli` | `target/debug/anolisa` |
| Release 构建 | `cargo build --release -p anolisa-cli` | `target/release/anolisa` |
