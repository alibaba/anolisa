# 会话恢复

[English](../../../../en/user-entrypoint/cosh-ng/shell/session-recovery.md)

cosh 可以恢复当前工作空间中以前的 Agent 对话。下一次请求会继续使用模型
之前看到的消息。

会话恢复需要 `cosh-core` 适配器。它不会恢复历史终端输出、审批提示、用户
问题或其他临时 UI 状态。`/session status` 会明确说明这一边界，避免把旧终端
证据误呈现为当前证据。

## 从历史会话启动

Shell 就绪后打开选择器。

```bash
cosh --resume
```

也可以直接选择已知的规范会话 UUID。

```bash
cosh --resume 2d711642-b726-4b04-8d2a-8a0470f4ed24
```

两种用法都会先确认对话属于当前工作空间，然后再选中它。

## 在 Shell 中管理会话

Shell 提示符中可以使用以下命令。

| 命令 | 行为 |
|------|------|
| `/session` | 打开按更新时间倒序排列的会话选择器 |
| `/session list` | 不打开选择器，打印包含完整、可复制会话 UUID 的首个有界摘要分页 |
| `/session list --all` | 打印同一存储根下所有工作空间的会话，按工作空间路径分组 |
| `/session new` | 与当前 provider 对话分离，使下一次 Agent 请求开启全新对话 |
| `/new` | `/session new` 的别名 |
| `/session status` | 显示 Shell、已选择、恢复中和已激活的 provider 身份 |
| `/session resume <id>` | 验证并选择一个 provider 会话 |
| `/resume [id]` | `/session` 或 `/session resume <id>` 的别名 |
| `/session clear <id>...` | 清理指定 ID 前请求确认 |
| `/session clear --all` | 准备精确 ID，并在清理全部持久化会话前请求确认 |

选中对话时不会调用模型。恢复从下一次 Agent 请求开始。恢复失败后 Shell 仍然可用，
可以重试，也可以开始新对话。

`/session list --all` 会列出同一存储根下所有工作空间的持久化会话，输出按规范
工作空间路径分组，每组内按更新时间倒序排列。当前工作空间的分组标题会附加
`(current)` 标记，方便快速定位可恢复的会话。属于其他工作空间的会话显示为
`scope_mismatch`，便于识别，但 `/session resume <id>` 仍会拒绝恢复，也不会自动
切换工作目录。`/session` 打开的交互式选择器仍保持当前工作空间作用域，不支持
`--all` 模式。

`/session new` 会与当前 Agent 对话分离。它不会删除旧记录、重启 Shell，也不会改变
工作目录和 Shell history。

## 选择器按键

| 按键 | 操作 |
|------|------|
| `Up` / `Down`、`j` / `k` | 移动光标 |
| `Enter` | 恢复高亮的健康会话 |
| `Space` | 标记或取消标记待清理条目 |
| `d` | 对已标记条目或当前高亮条目打开清理确认 |
| `y` | 确认精确的清理集合 |
| `n`、`Esc`、`Ctrl-C` | 取消确认或关闭选择器 |

选择器每一行会显示短 ID、提示摘要、更新时间、消息数、模型、健康状态和保护状态。
直接恢复和清理命令需要 `/session list` 输出的完整 UUID。

## 工作空间作用域与存储

会话归属于规范化后的当前工作空间。即使把其他工作空间的文件复制进当前目录，
也无法误恢复该会话。

默认持久化根目录如下。

```text
~/.copilot-shell/cosh-core/sessions/
```

默认根目录下会为每个工作空间建立独立子目录。其他工作空间的对话无法在当前目录
直接恢复。需要先进入它原来的工作空间，再启动 cosh。用 `session.persist_dir` 可以
修改保存根目录。设置 `session.auto_persist = false` 后，对话只保留到当前 cosh 进程结束。

存储权限、旧版迁移、并发锁和协议细节见[会话管理协议](../../../../../developer-guide/zh/cosh-ng/ipc-protocol.md#cosh-core-会话管理-json-协议)。

## 健康状态与恢复错误

选择器会保留异常条目，便于识别和清理。

| 健康状态或错误 | 含义 | 后续操作 |
|----------------|------|----------|
| `ready` | 信封有效且属于当前工作空间 | 正常恢复 |
| `corrupt` | JSON 或必需信封字段损坏 | 核对 ID 后确认清理 |
| `incompatible` | 当前版本不支持该 schema | 升级 cosh-core 或清理 |
| `scope_mismatch` | 记录的工作空间不一致 | 返回原工作空间 |
| `not_found` | 列出后文件被删除 | 刷新选择器 |
| `conflict` | 其他写入者持有或推进了会话 | 等待完成后重试 |

损坏、缺失、不兼容、作用域不匹配和并发冲突都不会终止交互式 Shell。只有
`ready` 条目可恢复；异常条目仍可在确认后清理。

## 清理保护

清理始终需要显式请求和确认，确认界面会标明要删除的准确 ID 或数量。已选择
会话和当前活跃 provider 会话在 cosh-shell 与 cosh-core 两层都受到保护，因此
即使出现在清理全部请求中也会被跳过。取消确认不会修改任何记录。
如果所有持久化会话均受保护，命令会显示受保护数量，而不会错误提示工作空间
为空。
