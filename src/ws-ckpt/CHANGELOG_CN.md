# 更新日志

## 0.4.1

### 新功能

- 新增 rollback 后跳过自动 checkpoint (#1263)

### 缺陷修复

- 修复 config 更新后的工作区同步 (#1263)
- 修复 crontab 条目中 ws-ckpt 的绝对路径处理 (#1263)
- 变更 rollback -n 偏移量，直接传递 numAncestors (#1263)

## 0.4.0

### 不兼容变更

- **不兼容** checkpoint `-i`/`--id` 参数更名为 `-s`/`--snapshot` 作为主参数；`-i` 保留为隐藏别名，未来版本可能移除 (#1064)

### 新功能

- 新增插件安装/卸载子命令 (#1005)
- 新增 component.toml 用于 anolisa-cli 适配器发现 (#1005)
- 新增 rollback 预览功能，支持 --preview 参数 (#1103)
- 新增每次 CLI 操作后的耗时显示 (#1075)
- 新增省略 --snapshot 时自动生成快照 ID (#1064)
- 新增 SLS 运维日志输出用于仪表盘指标 (#1059)
- 新增 diff 的可选 -t 参数，用于将快照与当前工作区对比 (#848)
- 新增按祖先数量 rollback 和快照 DAG 追踪 (#877)
- 新增基于 cron 的定时 checkpoint 快照 (#819)

### 缺陷修复

- 修复 --snapshot/-s 作为主参数的处理及插件参数对齐 (#1103, #1064)
- 修复 SKILL.md 与实际 CLI/插件实现的同步 (#847)
- 修复 init 和 recover 对被替换的 workspace 符号链接的防护 (#860)
- 修复 init rsync 去除 --copy-unsafe-links (#873)

## 0.3.3

### 新功能

- 新增每工作区策略覆盖，支持 hermes/openclaw 插件 (#721)
- 新增 `/proc` cwd 占用者检测，用于 init 和 rollback (#684)
- 新增 Hermes 适配器运行脚本 (#617)

### 缺陷修复

- 修复 rollback 中的写锁竞争和 cwd 检测死锁 (#721, #684)
- 修复非 UTF-8 路径和路径穿越快照 ID 的输入验证 (#695, #678)
- 修复 seccomp 架构选择、工作区注册并发和 RPM 打包问题 (#695, #684)

## 0.3.2

- 修复 openclaw 卸载时未从配置中移除工具白名单
- 修复父路径拒绝规则作为工作区级别规则应用于 skill 和 openclaw 插件

## 0.3.1

- 修复插件工作区配置注册和自动加载
- 拒绝将 hermes cwd 本身或其父路径作为工作区路径
- 修复插件工具优先使用显式 workspace 参数而非配置
- 修复 skill 删除需要 --force 参数
- 修复 daemon 工作区路径验证和 fswatch 文件描述符泄漏
- 移除未使用的 btrfs_ops.rs 模块

## 0.3.0

- 新增 openclaw 插件脚手架
- 新增 hermes 插件脚手架
- 将 ws-ckpt skill 改为 agent 无关，在调用时提示输入工作区
- 遵循 `make install` 契约用于 build-all 集成
- 修复 list 和 diff 子命令的缺陷
- 将 daemon 改为有状态

## 0.2.0

- 新增 auto_cleanup 功能及开关
- 统一通过 TOML 文件修改配置
- 新增全局 CLI 警告：当任意工作区快照数 >1000 或文件系统使用率 >90%
- 修复后端检测和 daemon 状态恢复逻辑
- 修复 daemon 重启后镜像大小配置不生效
- 移除过时的 fs_warn_threshold_percent 参数
- 修复 config.toml 作为示例文件分发

## 0.1.0

- 带 Unix Socket IPC 和 Bincode 二进制协议的 Daemon
- `init` / `checkpoint` / `rollback` / `delete` / `list` / `diff` / `cleanup` / `status` / `config` 命令
- 后台调度器：自动清理、健康检查、孤立恢复
- 多后端：btrfs-base / btrfs-loop / overlayfs 自动检测
- TOML 配置持久化及运行时热重载
- systemd 服务及 Alinux 4 RPM 打包
