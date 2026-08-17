# Runtime 安全边界

[English](runtime-security.md)

关联架构：[COSH Gateway 与 ACP 架构](README_zh.md)

## Threat model

Agent Runtime 及其 descendant 都不可信。它们可能有 bug、被攻击或主动对抗。Runtime
可以发送 protocol frame 和请求 capability，但它不是 operator，也不继承 Gateway authority。

即使 Runtime 与 operator software 由同一用户安装，安全边界仍必须成立。当两个进程使用
同一 kernel principal 时，只依赖 filesystem permission 不足以隔离权限。

## 必须具备的隔离

Production Runtime 不得：

- 作为 approving actor 连接 Gateway command socket；
- 读取或修改 Gateway SQLite、WAL、SHM、backup 或 audit file；
- signal 或 debug Gateway process；
- 改变 Gateway config、executable、workspace binding 或 unit state；
- 逃离 service lifecycle owner 并留下能够产生 effect 的 descendant；
- 继承 profile 不需要的 ambient credential 或 injection variable。

这要求 kernel-enforced principal、sandbox 或 service boundary。Presentation check 或进程内
Actor label 都不能替代该边界。

## Process ownership

- 每个 child process 只能有一个 lifecycle owner。
- Runtime launch 创建独立 process group 或更强 containment。
- 正常 shutdown 先传播 cancellation，等待 protocol grace，再升级 TERM/KILL，并只 reap 一次。
- Daemon hard failure 由外部 service manager 或 containment boundary 负责，在 replacement
  ready 前清理全部 descendant。
- Runtime 不能在 ownership 外创建 sibling service 或 cgroup。

Linux package 必须验证 service manager 的 effective property，不能只相信 unit template。
不支持的平台必须拒绝 production admission，或使用独立评审过的 owner。

## Executable 与 workspace identity

Production admission 在接受 Task 前固定 executable 与 workspace authority：

- absolute configured path；
- descriptor-backed device/inode identity；
- 正确 file type 与 executable/directory mode；
- 可信 installation provenance 与 profile identity；
- 与 governed execution target 共享的 workspace identity。

Launch 使用 pinned descriptor，否则 fail closed。Path rename、symlink retarget 或同名 replacement
都不能让 queued Task 执行不同 artifact 或 workspace。

Descriptor pinning 不等于整个 interpreter/package dependency tree 的 attestation。Script adapter
还需要可信 interpreter 和 immutable 或 verified package closure。

## Environment

Runtime 从清空的 environment 启动，只显式允许 locale、选定 proxy setting 和经批准的
authentication entry point。Dynamic-loader、Node injection、shell function 与任意 inherited
config variable 必须拒绝。

Credential 只在 Runtime profile 范围内使用，不进入 Task、event、audit、test transcript 或
PR evidence。

## Local endpoint admission

Gateway 从 kernel-provided peer identity 与 installation-scoped policy 认证本地 client，不相信
caller 自报的 Actor ID。

Production admission 在绑定公共 command socket 前验证 configured Runtime profile、target、
workspace、containment proof 与 service identity。Test/interoperability flag 不能静默启用 durable
production scheduler。

## Filesystem authority

Security-sensitive file 必须相对可信 directory descriptor 打开，并跨 open 保持 owner、mode、type
与 identity 校验。Validation 后重新 pathname lookup 会暴露 replacement race。

Database、WAL/SHM companion、backup destination、audit file、adapter artifact 与 governed
Unix socket 都遵守同一规则。

## Audit

Audit 必须 append-only、bounded、redacted 且 framing durable。Partial write 或 sync failure 后
writer 保持 poisoned，直到显式 recovery；不能在 corrupt tail 后追加新 record 并把它当作
durable evidence。

## 验收不变量

- 对抗 Runtime 不能批准自己的 request。
- Runtime 不能读取 Gateway durable state 或 audit evidence。
- Admission 后替换 executable/workspace path 不能改变实际 launch 对象。
- Gateway 被 SIGKILL 后不能留下 lifecycle owner 之外、可产生 effect 的 descendant。
- Environment injection 与 service-manager escape 尝试 fail closed。
- Security evidence 来自 effective runtime property 与 adversarial fixture，而非配置意图。
