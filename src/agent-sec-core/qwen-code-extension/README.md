# Qwen Code extension

全本地、零 Token 成本地把 Qwen Code 的生命周期事件写入 agent-sec-core
Observability。扩展通过 Qwen Code 原生 command hook 挂载，不自行实现 HookRegistry
或事件聚合器。

协议依据是 Qwen Code 官方的
[Hooks 文档](https://qwenlm.github.io/qwen-code-docs/en/users/features/hooks/)；
extension manifest、变量替换和安装行为同时依据
[Extensions 文档](https://qwenlm.github.io/qwen-code-docs/en/developers/extensions/extension/)
及当前 Qwen Code 源码校验。

## 前置条件

- `qwen`、`python3` 和 `agent-sec-cli` 均在 `PATH` 中。
- 当前目录已被 Qwen Code 信任；Qwen Code 会拒绝从不受信任的工作区安装扩展。

当前实现与真实安装测试以 Qwen Code `0.19.9` 源码为基线；该版本声明需要
Node.js `>=22`。与仓库内其他插件部署脚本一致，本脚本仅通过 `command -v` 检查
`qwen`、`python3` 和 `agent-sec-cli` 是否存在，不绑定或推断其版本和接口实现。

存在性检查不能证明二进制来源。部署前应通过系统包管理、制品签名或校验和确认
`qwen`、`python3`、`node` 和 `agent-sec-cli` 来自受信源；运行中的 hook 也会从
Qwen Code 进程的 `PATH` 查找 `python3` 和 `agent-sec-cli`，因此该运行时 PATH
必须只包含受信目录。

## 部署

```bash
./qwen-code-extension/scripts/deploy.sh
```

脚本调用 Qwen Code 的 `extensions install`、`extensions update` 和
`extensions enable` 命令，安装到 user scope。可通过 `QWEN_BIN` 和 `QWEN_HOME`
覆盖 Qwen Code 可执行文件及配置目录。也可以把扩展目录作为第一个参数传入：

```bash
./qwen-code-extension/scripts/deploy.sh /path/to/qwen-code-extension
```

Qwen Code 对本地扩展按 manifest 版本判断是否更新。因此修改扩展内容后必须同步提升
`qwen-extension.json` 中的 `version`，部署脚本才会执行更新。

## 可观测事件映射

| Qwen Code event | agent-sec-core hook |
| --- | --- |
| `UserPromptSubmit` | `before_agent_run` |
| `PreToolUse` | `before_tool_call` |
| `PostToolUse` | `after_tool_call`（success） |
| `PostToolUseFailure` | `after_tool_call`（error/interrupted） |
| `Stop` | `after_agent_run`（success） |
| `StopFailure` | `after_agent_run`（failure） |

Qwen Code 当前没有与 `before_llm_call` / `after_llm_call` 对应的模型调用 hook，
所以本扩展不会伪造这两类记录。当前直接 HookInput 也没有贯穿 prompt、tool 和 stop
事件的稳定 run 标识，因此现阶段所有记录的 `runId` 统一使用全零 GUID；
`tool_call_id` 优先作为 `toolCallId`，缺失时回退到 `tool_use_id`。

Qwen Code 的工具结果回填会再次触发 `UserPromptSubmit`，其中仅包含 function response
的回填会被序列化为空 `prompt`。扩展直接检查 HookInput：缺失或空白的 `prompt` 不记录，
非空 `prompt` 记录为 `before_agent_run`。该判断不创建本地 active-run 状态，因此不会因
并发或缺失 `Stop` / `StopFailure` 而影响后续用户 prompt。

`SessionStart`、`SessionEnd`、compact、notification、subagent 和 todo 等官方事件目前
没有与 agent-sec-core Observability schema 含义一致的记录类型，因此不会被错误映射到
agent/tool run；后续应先扩展 schema，再单独挂载。

`agent-sec-prompt-scanner` 是同步安全 hook：默认 `PROMPT_SCANNER_MODE=observe`，只记录
扫描事件且不阻断；设置为 `deny` 后，`agent-sec-cli scan-prompt` 返回 `warn` 或 `deny`
时会向 Qwen Code 返回拒绝决策并阻断该 prompt。`PROMPT_SCANNER_TIMEOUT` 控制内部
`agent-sec-cli` 调用超时，默认 10 秒；外层 manifest 为 prompt scanner 预留 15 秒
command-hook 超时。

`agent-sec-observability` 仍异步执行并保持 fail-open：任何脚本、PII 扫描或记录写入异常
都不会改变 Qwen Code 的执行决策。敏感指标在写入前由本地 `scan-pii` 脱敏；脱敏失败时
直接丢弃对应敏感字段。

## 测试

```bash
QWEN_CODE_EXTENSION_E2E_DIR="$PWD/qwen-code-extension" \
  uv run --project agent-sec-cli pytest \
  tests/unit-test/qwen-code-extension \
  tests/e2e/qwen-code-extension -v
```

E2E 测试从 `qwen-extension.json` 读取并直接执行 command hook，使用独立进程和隔离的
`AGENT_SEC_DATA_DIR` 验证 JSON 输出、Observability JSONL、全零 `runId`、tool call
关联以及空 prompt 工具结果回填的过滤。它不安装或启动 Qwen Code。
