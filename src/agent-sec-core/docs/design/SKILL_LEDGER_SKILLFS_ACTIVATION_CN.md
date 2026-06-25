# Skill Ledger 与 SkillFS Runtime Activation 接口

本文只定义 Skill Ledger 提供给 SkillFS 的接口和语义，不规定 SkillFS 的内部实现。

## 核心模型

Skill Ledger 维护版本账本、扫描结果、签名 manifest、snapshot 和运行态 activation。SkillFS 只消费 Skill Ledger 输出的运行态合同，并据此暴露文件系统视图。

- `source/current workspace`：用户或 Agent 写入的候选版本。
- `.skill-meta/versions/<id>.snapshot`：可运行的不可变版本文件树。
- `.skill-meta/versions/__pending_decision__.snapshot`：Skill Ledger 生成的安全审查占位 snapshot，不对应任何 manifest，不进入版本链。
- `.skill-meta/activation.json` 与 skill 目录 xattr：Skill Ledger 写给 SkillFS 的当前可运行 snapshot 指针。

读写路径的语义：

- 读路径：SkillFS 读取 xattr 或 `activation.json.target`，将该 snapshot 暴露为运行视图。
- 写路径：SkillFS 将写入继续落到 source/current workspace。
- 未扫描、未通过、漂移、被篡改或无用户决策时，Skill Ledger 优先继续指向最近可信 `pass` / `warn` snapshot；若没有可信 fallback，则指向 pending decision stub。用户 `block` 决策或 fail-safe 场景才写入 `target: null`。

## Runtime Activation 合同

Skill Ledger 写入：

```text
<skill_dir>/.skill-meta/activation.json
```

同时，Skill Ledger 应尽力在 `<skill_dir>` 目录上同步写入 xattr：

```text
user.agent_sec.skill_ledger.activation
```

`activation.json` 与 xattr 使用相同 UTF-8 JSON payload：

```json
{
  "schemaVersion": 1,
  "target": ".skill-meta/versions/v000002.snapshot"
}
```

无可激活版本时：

```json
{
  "schemaVersion": 1,
  "target": null
}
```

字段说明：

| 字段 | 类型 | 枚举 / 约束 | 说明 |
| --- | --- | --- | --- |
| `schemaVersion` | number | `1` | 当前固定为 `1` |
| `target` | string 或 null | `null` 或 `.skill-meta/versions/<id>.snapshot` | 相对 `skill_dir` 的 snapshot 路径；`null` 表示不应暴露该 skill。`__pending_decision__.snapshot` 是保留的安全审查占位 target |

SkillFS 预期处理：

- 可以读取 xattr，也可以读取 `activation.json`；两者是同一份 runtime decision 的并行表达。
- xattr 读取失败、缺失、格式非法或文件系统不支持 xattr 时，应回退读取 `activation.json`。
- 若 xattr 与 `activation.json` 同时存在但不一致，应 fail-safe，不暴露该 skill，并记录诊断事件。
- 只接受相对 `skill_dir` 的 target。
- target 必须指向 `.skill-meta/versions/<id>.snapshot`；`__pending_decision__.snapshot` 作为普通 snapshot 目录处理，但 SkillFS 不赋予它 ledger 版本语义。
- target 缺失、为 `null`、越界、非 snapshot、或路径不存在时，应 fail-safe，不暴露该 skill。
- SkillFS 不解析 `latest.json`、`scanStatus`、`policy`、`findings`。

## 内部 Resolver

Skill Ledger 不提供面向用户或 SkillFS 的 `resolve` CLI。activation refresh 是 Skill Ledger daemon 的内部职责；daemon 内部调用 resolver 后，同步写入 `activation.json` 与 xattr。SkillFS 只依赖 Runtime Activation 合同中的 `schemaVersion` 和 `target`，不得依赖 resolver 的内部返回值。

## SkillFS 变更通知接口

SkillFS 发现 source/current workspace 写变化后，通过现有 `agent-sec-daemon` 协议通知 Skill Ledger daemon。本接口已经由 Skill Ledger 侧实现；SkillFS 侧只需要按该协议发送事件。
启动或重启后，SkillFS 也可以对已加载的普通 skill 发送 `eventKind="reconcile"`、`paths=[]`，请求 Ledger 以当前磁盘状态重新对齐扫描与 activation。

传输形式固定为当前 `agent-sec-daemon` 机制：

- Unix domain socket。
- 单连接发送一个 NDJSON request frame。
- socket 路径由 `AGENT_SEC_DAEMON_SOCKET` 指定；未指定时使用 `$XDG_RUNTIME_DIR/agent-sec-core/daemon.sock`。
- request/response 外层遵循现有 daemon protocol：`id`、`method`、`params`、`trace_context`、`timeout_ms`。
- method 必须注册到 daemon allowlist；未注册 method 会返回 structured error。

method 固定为：

```text
skill_ledger.skillfs_notify_change
```

请求示例：

```json
{
  "id": "skillfs-01HX...",
  "method": "skill_ledger.skillfs_notify_change",
  "params": {
    "schemaVersion": 1,
    "skillDir": "/path/to/source/tianqi-weather",
    "skillName": "tianqi-weather",
    "eventKind": "write",
    "paths": ["SKILL.md"]
  },
  "trace_context": {},
  "timeout_ms": 5000
}
```

启动对齐请求示例：

```json
{
  "id": "skillfs-01HY...",
  "method": "skill_ledger.skillfs_notify_change",
  "params": {
    "schemaVersion": 1,
    "skillDir": "/path/to/source/tianqi-weather",
    "skillName": "tianqi-weather",
    "eventKind": "reconcile",
    "paths": []
  },
  "trace_context": {},
  "timeout_ms": 5000
}
```

外层字段说明：

| 字段 | 类型 | 枚举 / 约束 | 说明 |
| --- | --- | --- | --- |
| `id` | string | 非空字符串，可省略 | 请求 id；省略时由 daemon 生成 |
| `method` | string | `skill_ledger.skillfs_notify_change` | SkillFS 变更通知方法名 |
| `params` | object | 见下表 | 变更通知 payload |
| `trace_context` | object | 可为空对象 | 复用现有 daemon trace 透传字段 |
| `timeout_ms` | number 或 null | `1..300000`，可省略 | 复用现有 daemon timeout 规则 |

`params` 字段说明：

| 字段 | 类型 | 枚举 / 约束 | 说明 |
| --- | --- | --- | --- |
| `schemaVersion` | number | `1` | 当前固定为 `1` |
| `skillDir` | string | 绝对路径；不支持 `~` 展开 | source/current workspace 中的 skill 根目录 |
| `skillName` | string | 非空字符串 | skill 名称；应与 `skillDir` basename 一致 |
| `eventKind` | string | `mkdir` / `create` / `write` / `rename` / `unlink` / `rmdir` / `setattr` / `truncate` / `reconcile` | SkillFS 观察到的文件系统变化类型；`reconcile` 表示启动后状态对齐，不代表具体文件操作 |
| `paths` | string[] | 相对 `skillDir` 的路径数组，可为空；不得是绝对路径，不得包含 `..` | 触发变化的相对路径 |

响应示例：

```json
{
  "id": "skillfs-01HX...",
  "ok": true,
  "data": {
    "schemaVersion": 1,
    "accepted": true,
    "ignored": false,
    "queued": true,
    "coalesced": false
  },
  "stdout": "",
  "stderr": "",
  "exit_code": 0
}
```

响应字段说明：

| 字段 | 类型 | 枚举 / 约束 | 说明 |
| --- | --- | --- | --- |
| `ok` | boolean | `true` / `false` | daemon 是否成功处理该请求 |
| `data.schemaVersion` | number | `1` | 当前固定为 `1` |
| `data.accepted` | boolean | `true` / `false` | 事件是否被接收或入队 |
| `data.ignored` | boolean | `true` / `false` | 是否因仅包含 `.skill-meta/**` 路径而忽略 |
| `data.queued` | boolean | `true` / `false` | 是否进入后台 activation job 队列；`ignored=true` 时不存在或为 `false` |
| `data.coalesced` | boolean | `true` / `false` | 是否与同一 skill 的待处理事件合并 |
| `exit_code` | number | `0` 表示请求成功 | 复用现有 daemon response 语义 |
| `error.code` | string | 现有 daemon error code | `ok=false` 时返回 |

通知语义：

- 通知表示“某个 skill 的 source workspace 可能已变化”，不是安全结论。
- `reconcile` 表示“请将 Ledger 侧状态与当前 skill 目录状态对齐”，不是具体文件操作；它应使用 `paths=[]`，且进入同一 notify queue。
- SkillFS 是受信任事件来源；daemon 对 `skillDir` 做格式、存在性和 `SKILL.md` 检查，不要求该目录预先存在于 `managedSkillDirs`。
- 对未被当前配置覆盖的新 skill，包括 SkillFS 启动时发来的 `reconcile` skill，daemon 执行 scan 时会沿用 Skill Ledger 现有自动记忆逻辑，将该 skill 目录或父目录 glob 写入 `managedSkillDirs`，供后续 reconcile 使用。
- 通知成功只表示 daemon 已接收事件，不表示 scan 已完成，也不表示 activation 已刷新。
- `.skill-meta/**` only 事件会返回 `accepted=true, ignored=true`，不触发扫描，避免 Ledger 写 metadata 时形成循环。SkillFS 也可以选择不发送这类事件。
- 事件可以重复、乱序或合并；daemon 必须按 skill 维度 debounce，并以当前磁盘状态重新计算。

Skill Ledger daemon 侧执行的逻辑：

- 接收事件并按 `skillDir` debounce，默认 debounce 窗口为 500ms。
- 对 source/current workspace 执行 `scan`。如果扫描为 `noop`，仍继续刷新 activation。
- `reconcile` 不走独立扫描链路；它与安装、修改、rename 等事件一样进入同一 debounced worker，执行同一套 scan + activation refresh。
- 如果 scan 失败，仍尝试刷新 activation，以便 `drifted`、`tampered` 等状态可以回退到历史 pass snapshot。
- 调用内部 resolver，写入新的 `.skill-meta/activation.json` 与 xattr。
- 启动或重启时 reconcile `managedSkillDirs`，补处理 daemon 下线期间错过的变化。

`check` 保持只读状态检查，不作为版本或 snapshot 创建入口。

daemon 返回 `ok=true` 只表示事件已被接收或入队；若传输失败、daemon 不可达或返回 `ok=false`，SkillFS 应写入事件日志并继续按当前 activation 暴露已有可信视图，等待 daemon reconcile。

## SkillFS 事件日志需求

SkillFS 应维护 append-only JSONL 事件日志，供 daemon reconcile、观测和排障使用。事件日志是补偿线索，不是唯一可信状态源。

最小字段：

```json
{
  "schemaVersion": 1,
  "time": "2026-06-11T10:00:00.000Z",
  "skillDir": "/path/to/source/tianqi-weather",
  "skillName": "tianqi-weather",
  "eventKind": "write",
  "paths": ["SKILL.md"]
}
```

字段说明：

| 字段 | 类型 | 枚举 / 约束 | 说明 |
| --- | --- | --- | --- |
| `schemaVersion` | number | `1` | 当前固定为 `1` |
| `time` | string | RFC 3339 UTC timestamp | SkillFS 记录事件的时间 |
| `skillDir` | string | 绝对路径 | source/current workspace 中的 skill 根目录 |
| `skillName` | string | 非空字符串 | skill 名称；应与 `skillDir` basename 一致 |
| `eventKind` | string | `mkdir` / `create` / `write` / `rename` / `unlink` / `rmdir` / `setattr` / `truncate` / `reconcile` | SkillFS 观察到的文件系统变化类型；`reconcile` 表示启动后状态对齐 |
| `paths` | string[] | 相对 `skillDir` 的路径数组，可为空；不得是绝对路径，不得包含 `..` | 触发变化的相对路径 |

日志要求：

- 日志写入失败不应放行错误 runtime 版本；最多影响实时性，最终由 daemon reconcile 修复。
- daemon 重启后必须以 skill 目录当前状态和 `.skill-meta` 为准做 reconcile，不能只依赖事件日志完整性。
- 事件日志允许重复、乱序或合并；daemon 读取后仍需以当前磁盘状态重新计算。
- `.skill-meta/**` 变化不应写入 SkillFS 事件日志，避免 metadata 写入循环。

职责边界：

- SkillFS 负责捕获写事件、通知 daemon、维护事件日志、读取 xattr 或 `activation.json.target` 暴露 snapshot；写入始终落到 source/current workspace。
- Skill Ledger daemon 负责接收事件、debounce、扫描、写入 version/snapshot、执行 activation policy，并刷新 `activation.json` 与 xattr。
- SkillFS 不解析 activation policy，也不解析 `latest.json`、`scanStatus`、`findings`。

## 策略

activation policy 是 Skill Ledger 配置项，SkillFS 不感知策略，只消费最终
`activation.json.target` 或同语义 xattr。当前唯一运行态策略为：

```json
{
  "activationPolicy": "pass_warn_only"
}
```

行为规则：

| 场景 | activation target |
|------|-------------------|
| 用户决策 `allow` / `always_allow` / `rollback` | 指向该决策允许的真实 snapshot |
| 用户决策 `block` | `null` |
| 无用户决策，latest 为可信 `pass` / `warn` | 指向 latest snapshot |
| 无用户决策，latest 为 `deny` / `none` / `drifted` / `tampered`，存在历史可信 `pass` / `warn` | 指向最近的历史可信 snapshot |
| 无用户决策，latest 风险状态且没有历史可信 fallback | 指向 `.skill-meta/versions/__pending_decision__.snapshot` |

历史配置值 `pass_only` / `latest_scanned` 会被 Skill Ledger 兼容读取并归一化为 `pass_warn_only`，不再产生独立行为分支。策略永远不会激活 source/current 工作区；SkillFS 也不根据 `scanStatus` 自行选择版本。
