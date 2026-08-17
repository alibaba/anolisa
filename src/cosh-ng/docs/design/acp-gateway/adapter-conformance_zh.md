# Adapter 生命周期与验收

[English](adapter-conformance.md)

关联架构：[COSH Gateway 与 ACP 架构](README_zh.md)

## 目的

只有 Adapter source、profile、protocol behavior、failure behavior 与 release evidence
都可复现时，它才是受支持的 Agent adapter。一次 prompt 成功不构成 conformance。

## 生命周期

```text
source selection
  -> version lock
  -> staged installation
  -> provenance verification
  -> profile admission
  -> fake conformance
  -> real-agent conformance
  -> signed/offline release artifact
  -> upgrade or rollback
```

Gateway runtime 不调用 package runner 或 network installer。Package installation 是独立的
operator action。

## Installed profile

Profile 记录：

- stable profile name 与 Runtime kind；
- 准确 adapter package 与 version；
- canonical entry point 与 executable identity；
- 可信 interpreter/package closure 要求；
- fixed arguments 与 working-directory policy；
- environment allowlist；
- ACP wire version 与 required capability；
- upstream agent version 的 compatibility status。

Profile resolution 在 production admission 前完成。Task 不能覆盖 executable、argument、
environment 或 workspace。

## 原子安装

Installer 使用 private managed prefix，并在发布前 stage 完整 candidate。Verification 覆盖
package metadata、canonical binary、file ownership/mode 与 expected version。Installed marker
最后写入并原子发布。

安装失败或中断后，只能保留上一份 verified installation，或者没有 accepted installation。
不能留下使 partial tree 通过 admission 的 marker。

Release distribution 应提供 signed 或其他可验证的 offline artifact。Runtime network bootstrap
不是可接受 fallback。

## Fake conformance

Deterministic fake adapter suite 覆盖：

- initialize 与 required capability negotiation；
- Session creation 与有界 multi-chunk prompt；
- buffered observation 后恰好一个 terminal result；
- batch request 与 per-item error；
- tool-use identity 与单调 revision；
- correlated permission request 与 single-use decision；
- silent/blocked reader 下独立 cancellation；
- malformed JSON、invalid UTF-8、oversized frame、stderr flood、early exit、timeout、
  transport close；
- late update、late permission decision 与 duplicate terminal rejection；
- process-group cleanup 与 exactly one reap。

每次变更都必须运行 fake conformance，但它不能证明真实 Provider compatibility。

## Real-agent conformance

至少一个 supported adapter 在 exact candidate artifact 上完成：

- 记录 version 与 profile identity；
- initialize、Session creation、有界 text prompt、stream update 与一个 terminal outcome；
- active work 的 independent cancel；
- Provider 支持时验证真实 `allow_once` 与 `reject_once`；
- 不使用 profile 未声明的 filesystem/terminal capability；
- 脱敏 evidence，不包含 prompt、provider output、credential、private path 或 proxy URL。

Codex 与 Claude adapter 分别验收；一个通过不能代表另一个通过。

## Failure 与 race matrix

Conformance 为以下场景记录 expected behavior：

| 场景 | 预期结果 |
| --- | --- |
| Unsupported wire/capability | Session work 前失败 |
| Silent initialization/prompt | Timeout、shutdown 并 reap |
| Cancel 与 completion 竞争 | 一个 terminal winner；late loser ignore 或 reject |
| Cancellation 期间 permission | Cancellation 获胜后不发送 allow |
| Malformed stdout | Protocol failure 与 process-tree shutdown |
| Terminal 前 Runtime exit | Deterministic failure 与有界诊断 |
| Accepted callback 后响应丢失 | Durable replay，不二次 write |
| Adapter path replacement | 启动 pinned artifact 或 fail closed |

## Upgrade 与 rollback

Adapter upgrade 是显式 compatibility change。发布前重新运行 installer、fake 与 real
conformance。Rollback 恢复此前 verified profile，不改写 Task 或 audit history。

Unsupported/regressed profile 必须禁用。Gateway 不为已有 Task 静默 fallback 到其他 Provider
或 ungoverned Runtime。

## Evidence package

每个 accepted release 记录：

- candidate commit 与 artifact digest；
- adapter package、adapter version 与 upstream agent version；
- operating environment 与 required Runtime capability；
- 精确自动化 command 与结果摘要；
- 适用时的 manual step 与 expected observation；
- untested case 与 rollback result。

Evidence 必须 bounded、redacted。Secret、raw prompt 与 private provider output 不进入公开仓库。

## 社区贡献

Adapter contribution 包含 profile、provenance rule、fake fixture、failure/race matrix、文档与
real-agent evidence plan。Reviewer 无需 Provider credential 也能独立评审 deterministic suite。
