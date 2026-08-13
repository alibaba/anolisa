# ACP v1 本地 Runtime MVP 验收报告

[English](acceptance.md) | [设计](design_zh.md) |
[规划集](../../README_zh.md)

## 结果

**PARTIAL IMPLEMENTATION / NOT ACCEPTED。** 候选树已有严格 ACP v1 codec、
supervised stdio Bridge、带独立 cancellation 的有界 Session Driver、已安装 COSH
entrypoint，以及面向 `codex-acp` 和 `claude-agent-acp` 的内置 profile resolver。
Local once-only Permission Proxy 会在回复前写入脱敏 evidence，但尚未记录真实 Adapter
provider run。

本 Gate 独立于完整 G1 与 G2 验收。即使通过，也只证明设计中定义的窄范围本地互操作结果。

## 状态词表

| 状态 | 含义 |
| --- | --- |
| `PASS` | 精确 candidate evidence 满足完整 MVP criterion |
| `PARTIAL` | 已有有界 source/test slice，但用户路径或必需证明仍不完整 |
| `FAIL` | 已实现行为经过测试后违反 criterion |
| `NOT IMPLEMENTED` | 必需 production surface 不存在 |
| `NOT RUN` | Surface 已存在，但必需证据未执行 |

## 当前证据

| Area | 当前状态 | 证据与缺口 |
| --- | --- | --- |
| ACP v1 codec | `PARTIAL` | Exact wire v1 initialization、单 session、text prompt/update/stop、bound 与 malformed-input handling 有 focused fixture；未运行真实 Adapter |
| Supervised stdio | `PARTIAL` | Bridge 组合一个 Supervisor 与带 deadline/backpressure 的有界 Driver；更广的 race 与 process-tree fixture 仍缺 |
| Runtime profile | `PARTIAL` | 已安装 entrypoint 使用内置 resolver，固定 `codex-acp` 与 `claude-agent-acp`、canonical executable/workspace、fixed args 与 environment allowlist；installed-package smoke 证据仍缺 |
| Streaming | `PARTIAL` | 有界 Driver 按接收顺序交付 decoded observation，saturation 时 fail closed；local sequence 与 presentation 未完成 |
| Cancellation | `PARTIAL` | Independent control 能触达 silent Agent、结算 pending permission callback 并 reap process；更广 race coverage 仍缺 |
| Permission correlation | `PASS for local slice` | Local TTY presentation 只保留有关联的 `allow_once`/`reject_once`；non-TTY、EOF、unsupported-only option 与 explicit deny 都会取消 |
| Permission evidence | `PASS for local slice` | Private append-only JSONL 在回复前记录 bounded hash、actor UID 与 decision class；不含 raw prompt/tool/session/workspace value |
| Unsupported callback | `PARTIAL` | Fake fs request 收到有关联 method-not-found；完整 fs/terminal non-advertisement matrix 待补 |
| 真实 Adapter conformance | `NOT RUN` | 未记录 exact-version `codex-acp` 或 `claude-agent-acp` provider run |
| Rollback | `PARTIAL` | 现有 direct `cosh-shell raw cosh-core` path 保留，raw-package routing 已测试；installed-package smoke 仍缺 |

Source 存在不等于用户侧验收。使用临时 executable file 的 profile resolver test 不能证明
已安装官方 Adapter 可工作。

## 验收矩阵

| ID | Criterion | 当前结果 | 必需证明 |
| --- | --- | --- | --- |
| MVP-01 | 一个已安装 COSH entrypoint 接受内置 profile、canonical workspace 与 bounded text prompt | `PARTIAL` | Binary integration 与 raw-package routing 通过；installed-package smoke 仍缺 |
| MVP-02 | 只启动本地已安装 `codex-acp` 或 `claude-agent-acp`；不可能启动原生 Codex/Claude、`npx`、shell、package runner 或 network bootstrap | `PARTIAL` | Runtime resolver 从不 bootstrap package；distribution 与 installed-package proof 仍缺 |
| MVP-03 | Profile resolve 固定 exact basename、canonical executable/workspace、fixed args 与 allowlisted environment，且不记录 value | `PARTIAL` | Entrypoint path 的 positive 与 spoof/path/environment test |
| MVP-04 | Driver 按序执行 ACP v1 initialize、单 session/new 与单 active text prompt | `PARTIAL` | End-to-end Driver fixture 以及 wrong-order/duplicate-prompt negative |
| MVP-05 | Text update 按接收顺序交付，带有界 local sequence、queue depth 与 byte | `NOT IMPLEMENTED` | Multi-chunk 与 saturation fixture |
| MVP-06 | 每轮只报告一个 terminal result，并拒绝 late update | `PARTIAL` | Completion/cancel/error/exit/timeout race matrix |
| MVP-07 | Agent stdout 静默时 cancel 仍到达 Driver，并在配置 bound 内 settle protocol/process state | `PARTIAL` | Independent-control fake-Agent test 通过；completion/cancel race matrix 仍缺 |
| MVP-08 | Cancel 结算所有 pending permission，late decision/update 不能授权工作 | `PARTIAL` | Permission-during-cancel 与 late-response race fixture |
| MVP-09 | Permission Proxy 只提供有关联的 `allow_once` 与 `reject_once`；`allow_always`/`reject_always` 不能生成 decision 或 rule | `PASS for local slice` | 七个 focused Proxy/evidence test 与 non-interactive entrypoint cancellation |
| MVP-10 | Permission evidence 有界、脱敏，并记录 request correlation 与 decision class | `PASS for local slice` | Private-file、symlink、mode、secret exclusion、control-injection 与 entrypoint evidence test |
| MVP-11 | fs、terminal、load、resume、rich content、additional directory 与 multiple session 保持不声明并 fail closed | `PARTIAL` | 完整 capability/request negative matrix 与 zero host I/O |
| MVP-12 | Malformed/oversized/invalid UTF-8/contaminated stdout、stderr flood、child exit 与 timeout 安全终止并只 reap 一个 child | `PARTIAL` | Adversarial process fixture 与 leak assertion |
| MVP-13 | 至少一个已安装真实 Adapter 完成 initialize、prompt、多个 streamed text update、terminal、active cancel、allow once 与 reject once | `NOT RUN` | Candidate SHA 上的脱敏 exact-version transcript 与 command result |
| MVP-14 | 禁用或不选择 ACP 时保留当前 direct cosh-core path | `PARTIAL` | Installed rollback smoke test |
| MVP-15 | 中英文 MVP 与 aggregate 文档语义等价，全部 relative link 可解析 | `PASS for document slice` | 下述文档检查记录 |

MVP-01 到 MVP-15 全部强制。MVP-13 可以使用任一官方 Adapter，但验收报告必须写明
哪个 profile 通过；另一个 profile 保持 `NOT RUN` 或记录自己的结果。

## 必需自动化证据

实现报告必须记录下列等价 coverage 的 exact command 与 count：

```text
profile resolver unit tests
ACP codec and supervised bridge tests
session driver protocol tests
installed local entrypoint integration tests
silent-Agent cancellation race tests
permission allow/reject/cancel tests
malformed-output and process-leak tests
rollback smoke test
```

Fake-Agent corpus 必须包含：

- 正常 initialization 与至少两个 text chunk；
- wrong version、malformed JSON、invalid UTF-8、stdout log contamination、oversized
  frame、stderr flood 与 early exit；
- 通过 independent control handle 取消的 silent prompt；
- allow-once、reject-once、unsupported-only option、duplicate ID、late decision，
  以及 permission pending 时 cancellation；
- 未声明 filesystem、terminal、load 与 resume request，并证明没有执行 host callback；
- output saturation 与 cancellation/completion race。

## 必需真实 Adapter 证据

验收要求一个本地已安装的 `codex-acp` 或 `claude-agent-acp`。Evidence package 记录：

1. 完整 candidate commit SHA 与 operating-system environment；
2. Selected profile 与 canonical Adapter path，但不含 credential；
3. Adapter executable version 与 installation source；
4. Normal prompt 与 cancellation 的 exact COSH entrypoint command；
5. 脱敏 transcript，证明 initialization、至少两个有序 text update、唯一 terminal、
   allow once、reject once 与 active cancellation；
6. 确认 COSH 未使用 `npx`、download、network bootstrap、filesystem callback 或
   terminal callback；
7. 另一个内置 profile 的 unsupported 或 untested behavior。

Evidence 必须移除 provider output、prompt、credential、environment value、host identifier
与 private workspace content。

## Stage 2 Permission 证据

`cosh agent run` 默认在本地 `/dev/tty` 上呈现，只提供 Agent 实际给出的
`allow_once` 与 `reject_once` choice。`--permission deny`、没有 TTY、EOF、无效输入与
unsupported-only option 都会取消且不授权。只有 private append-only JSONL record 完成
同步后才回复 callback。Record 只包含 correlation hash、actor UID、profile、time 与
decision class，不含 raw prompt、tool argument、option label、provider session identifier
或 workspace path。

Stage 2 targeted check：

```bash
cargo +1.88.0 test --locked --package cosh-gateway permission:: --lib
# 7 passed
cargo +1.88.0 test --locked --package cosh-gateway \
  --test cli_entrypoint --bin cosh-gateway
# 7 passed
```

## Exit Criteria

ACP MVP 只在以下条件全部成立时接受：

1. MVP-01 到 MVP-15 在同一个 exact candidate commit 上全部为 `PASS`。
2. Installed entrypoint 与 fake-Agent failure/race suite 通过并记录 exact count。
3. 至少一个真实官方 Adapter 通过完整 prompt、stream、cancel、allow-once 与
   reject-once scenario。
4. 验收报告写明 passing revision 使用的全部 timeout、frame、queue、stderr 与 shutdown bound。
5. 报告明确说明该结果不是 G1/G2、durable governance、filesystem/terminal、Web、
   Shell Attachment 或 daemon acceptance。

## 文档验证

双语文档必须通过仓库 docs lint、relative-link check、pairing/parity review 与
`git diff --check`。Stage 2 不运行 provider、ECS 或真实 Adapter，因此不能把 MVP-13
从 `NOT RUN` 改为通过。
