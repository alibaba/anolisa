# 新增发行版适配

## 概述

cosh-ng 通过 `Distro` 枚举抽象操作系统差异，新增发行版支持需要修改两层：
检测层（`cosh-platform/src/detect.rs`）和后端路由层（`cosh-platform/src/pkg.rs` 等）。

## 步骤

### 1. 添加 Distro 枚举变体

在 `crates/cosh-platform/src/detect.rs` 中添加变体：

```rust
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Distro {
    // ...existing...
    MyDistro { version: String },   // 新增
}
```

### 2. 实现检测逻辑

在 `detect_from_content()` 的 match 分支中添加 ID 映射：

```rust
match id.as_deref() {
    // ...existing...
    Some("mydistro") => Distro::MyDistro { version },
    // ...
}
```

Linux 系统通过解析 `/etc/os-release` 的 `ID` 字段识别发行版，值需小写匹配。

### 3. 实现辅助方法

```rust
impl Distro {
    pub fn id_str(&self) -> &str {
        match self {
            // ...existing...
            Distro::MyDistro { .. } => "mydistro",
        }
    }

    pub fn display_name(&self) -> String {
        match self {
            // ...existing...
            Distro::MyDistro { version } => format!("MyDistro {}", version),
        }
    }

    pub fn pkg_manager(&self) -> PkgManager {
        match self {
            // ...existing...
            Distro::MyDistro { .. } => PkgManager::Dnf, // 根据实际情况选择
        }
    }
}
```

如果新发行版使用的包管理器不在现有 `PkgManager` 枚举中，需先扩展该枚举。

### 4. 添加包管理器后端（如需新增）

如果需要新的 `PkgManager` 变体，在 `crates/cosh-platform/src/pkg.rs` 中添加对应的命令构建函数：

```rust
// 新增 PkgManager 变体
pub enum PkgManager {
    // ...existing...
    Pacman,
}

// 在 pkg_install / pkg_remove / pkg_search / pkg_list 中添加路由分支
PkgManager::Pacman => ("pacman", vec!["-S", "--noconfirm", package]),
```

### 5. 添加单元测试

在 `detect.rs` 的 `#[cfg(test)]` 模块中添加：

```rust
#[test]
fn test_detect_mydistro() {
    let content = "NAME=\"My Distro\"\nVERSION_ID=\"1.0\"\nID=mydistro\n";
    let distro = Distro::detect_from_content(content);
    assert_eq!(distro, Distro::MyDistro { version: "1.0".into() });
    assert_eq!(distro.pkg_manager(), PkgManager::Dnf);
}
```

### 6. 运行测试验证

```bash
cd src/cosh-ng

# 运行检测相关测试
cargo test --locked -p cosh-platform test_detect

# 运行完整测试套件
cargo test --locked -p cosh-platform

# 运行 CLI 集成测试（确保新路由不破坏 JSON 信封）
cargo test --locked -p cosh-cli
```

## 当前支持矩阵

| 发行版 ID | Distro 变体 | PkgManager | 备注 |
|-----------|-------------|------------|------|
| `alinux` | `Alinux` | Dnf | 阿里云原生 Linux |
| `centos` | `CentOS` | Dnf | |
| `fedora` | `Fedora` | Dnf | |
| `ubuntu` | `Ubuntu` | Apt | |
| `debian` | `Debian` | Apt | |
| `opensuse-leap` / `opensuse-tumbleweed` / `sles` | `OpenSUSE` | Zypper | 三个 ID 映射到同一变体 |
| macOS (编译目标) | `MacOS` | Brew | 通过 `sw_vers` 检测 |

## 设计约束

| 规则 | 说明 |
|------|------|
| ID 小写 | `detect_from_content()` 对 ID 做 `to_lowercase()` |
| Unknown 兜底 | 未识别的 ID 归入 `Unknown(String)`，后续操作返回 `UnsupportedDistro` |
| 多 ID 合并 | 多个 ID 可映射同一 Distro 变体（如 opensuse 系列） |
| 包管理器解耦 | `PkgManager` 与 `Distro` 是独立枚举，通过 `pkg_manager()` 映射 |
| 不合并配置 | 检测逻辑只取第一个匹配的 `ID` 和 `VERSION_ID` |

## 完整检查清单

- [ ] `Distro` 枚举添加新变体
- [ ] `detect_from_content()` 添加 match 分支
- [ ] `id_str()` 返回正确的字符串标识
- [ ] `display_name()` 返回可读名称
- [ ] `pkg_manager()` 映射到正确的包管理器
- [ ] `Display` trait（通过 `display_name()`）正确格式化
- [ ] 单元测试覆盖正常解析和边界情况
- [ ] 如需新 PkgManager，在 `pkg.rs` 各函数中添加路由
- [ ] 更新用户文档 `users/supported-distros.md`
