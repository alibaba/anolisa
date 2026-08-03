# Extensions

[English](../../../../en/user-entrypoint/cosh-ng/core/extensions.md)

Extensions 可以打包 Skills、Hooks、MCP 服务、设置、上下文文件和 Agent 定义。通过
交互式终端管理 Extension 时，系统会展示新增或变化的能力，并在启用可执行内容前请求
确认。

## 常用命令

```text
/extensions list
/extensions info <name>
/extensions doctor [name]

/extensions install ./extension
/extensions install https://example.com/extension.git --ref main
/extensions link ./extension
/extensions update <name>
/extensions update --all
/extensions uninstall <name>

/extensions enable <name>
/extensions disable <name>
/extensions reload
```

需要由系统管理副本时使用 `install`，开发本地 Extension 时使用 `link`。HTTPS Git
地址可以指定 ref。运行 `/extensions help` 查看当前版本支持的完整语法。

## 确认变更

安装、链接、卸载或改变能力的更新可能先生成一项待确认操作。

```text
/extensions operation <operation-id>
/extensions consent <operation-id>
/extensions cancel <operation-id>
```

确认前请检查来源、版本和能力差异。能力指纹包含可执行的 Hook 命令及其环境变量，任一
内容变化都需要重新确认。

## 设置

```text
/extensions settings list <name> [--scope user|workspace]
/extensions settings get <name> <key> [--scope user|workspace]
/extensions settings set <name> <key> <value> --scope user
/extensions settings unset <name> <key> --scope workspace
```

敏感值保存在操作系统密钥存储中，显示时会替换为 `[redacted]`。工作区设置只对已经
信任的项目生效。

## 来源优先级

系统 Extensions 通常位于 `/usr/share/anolisa/extensions/`，用户 Extensions 位于
`~/.copilot-shell/extensions/`。两处出现同一个 Extension 时，系统会提示来源冲突，
此时可以手动选择。

```text
/extensions select-source <name> user
/extensions select-source <name> system
```

## 激活模型

系统会先构建并检查一份新的 Extension 注册表。检查失败时，当前版本继续工作。没有
正在运行的 Agent 任务时，新版本立即启用。有任务运行时，重载会等到安全时机，当前
任务继续使用启动时的版本。

链接的 Extension 会检查源文件是否发生变化。`/extensions doctor` 会报告无效清单、
确认过期、文件缺失、来源冲突和加载失败。

## 能力边界

- 本地 stdio MCP 工具使用完整 `<extension>/mcp/<server>/<tool>` 命名空间，并保留正常
  审批。
- Hook `env` 只作用于 Hook 子进程，不会修改宿主进程。
- Extension 上下文有大小限制，并放在项目上下文之后。
- Agent 定义会被校验和列出，在统一的子 Agent 执行器可用前报告
  `executable=false`。
- 禁用 Extension 会在下一次安全重载时移除运行时能力，不会删除已安装的软件包。

作者可使用 `/extensions new <path> --template <name>` 创建清单骨架。`<name>` 可选
`minimal`、`skill`、`hook`、`mcp`、`context` 或 `agent`，完成后运行
`/extensions doctor` 检查。
