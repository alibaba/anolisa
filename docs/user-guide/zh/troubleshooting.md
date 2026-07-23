# 故障排查

使用 ANOLISA 组件时的常见问题和解决方案。

---

## 诊断工具

ANOLISA 提供内置诊断命令，帮助识别和解决问题。

### anolisa doctor

对所有已安装组件运行全面健康检查：

```bash
anolisa doctor
```

检查项包括：
- 组件二进制文件可用性
- 配置文件有效性
- 运行时依赖（FUSE、btrfs、eBPF）
- 适配器连接性
- 权限问题

### anolisa bug

生成诊断报告，用于提交 Bug：

```bash
anolisa bug
```

将收集系统信息、组件版本、配置和近期日志，合并为一份报告文件。

### anolisa logs

查看组件日志：

```bash
# 查看特定组件日志
anolisa logs <component>

# 查看 warning 和 error 记录
anolisa logs <component> --severity warn

# 显示最后 N 行
anolisa logs <component> --limit 50
```

---

## 常见问题

### 权限错误

**现象**：运行 `anolisa install` 时提示 `Permission denied`

**原因**：部分组件需要 system mode（root 权限）。

**解决**：

```bash
# system-mode 组件（agentsight、agent-sec-core）
sudo anolisa install <component>

# user-mode 组件，确保 ~/.local/bin 可写
ls -la ~/.local/bin/
```

---

**现象**：访问 `/dev/fuse` 时提示 `Permission denied`

**原因**：用户不在 `fuse` 组或设备不可用。

**解决**：

```bash
# 将用户添加到 fuse 组
sudo usermod -aG fuse $USER

# 验证设备存在
ls -la /dev/fuse
```

---

### 组件安装失败

**现象**：`anolisa install tokenless` 因网络错误失败

**解决**：

```bash
# 查看检测到的运行环境
anolisa env

# 使用 verbose 模式重试
anolisa --verbose install tokenless

# 替代方式：使用 YUM
sudo yum install tokenless
```

---

**现象**：源码编译时 `cargo build` 失败

**解决**：

```bash
# 确认 Rust 工具链已安装
rustup show

# 更新到最新 stable
rustup update stable

# 查看检测到的构建环境
anolisa env
```

---

### 适配器问题

**现象**：Tokenless hook 在 cosh 中未激活

**解决**：

```bash
# 验证 hook 安装
ls ~/.config/cosh/hooks/

# 重新安装 hook
/usr/share/tokenless/scripts/install.sh --cosh

# 检查 cosh hook 配置
cat ~/.config/cosh/config.toml | grep -A5 hooks
```

---

**现象**：ws-ckpt 插件未被 OpenClaw 检测到

**解决**：

```bash
# 重新安装插件
ws-ckpt plugin install --runtime openclaw

# 验证插件注册
anolisa status ws-ckpt

# 检查 OpenClaw 插件目录
ls ~/.config/openclaw/plugins/
```

---

### ws-ckpt 问题

**现象**：`ws-ckpt checkpoint` 失败，提示 "not a btrfs filesystem"

**解决**：

```bash
# 检查文件系统类型
df -T /path/to/workspace

# 非 btrfs 时 ws-ckpt 会回退到 rsync
# 确保工作区路径配置正确
ws-ckpt config
```

---

**现象**："workspace path must not be Agent startup directory"

**原因**：ws-ckpt 工作区设为 Agent 的 CWD 或其父目录。

**解决**：将工作区路径改为专用项目目录：

```bash
ws-ckpt config set workspace.path /home/user/projects/my-project
```

---

### SkillFS 问题

**现象**：`skillfs mount` 失败，提示 "FUSE not available"

**解决**：

```bash
# 安装 FUSE3
sudo yum install fuse3 fuse3-devel

# 加载 FUSE 内核模块
sudo modprobe fuse

# 验证
ls /dev/fuse
```

---

### AgentSight 问题

**现象**：AgentSight 无 eBPF 数据

**原因**：内核能力不足或不支持 eBPF。

**解决**：

```bash
# 检查内核版本（建议 >= 5.4）
uname -r

# 查看内核能力，再诊断已安装的组件
anolisa env
sudo anolisa --install-mode system doctor agentsight

# AgentSight 需要 system mode
sudo anolisa install agentsight
```

---

## 获取帮助

如果以上步骤未能解决问题：

1. 运行 `anolisa bug` 并附上报告
2. 查看组件日志：`anolisa logs <component>`
3. 在 ANOLISA GitHub 仓库提交 Issue
