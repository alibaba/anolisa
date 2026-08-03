# 接入 MCP server

[English](../../../en/user-entrypoint/cosh-ng/mcp.md)

MCP server 可以把本地进程或远程服务提供的工具交给 Agent。完成下面的步骤后，
`/mcp list` 会列出这个 server，`/mcp inspect` 可以看到它提供的工具，Agent 也能在任务中
请求调用这些工具。

## 开始前的准备

先安装并启动一次 cosh-ng。你还需要 MCP server 提供方给出的启动命令或 Streamable
HTTP URL。下面的本地示例使用 `npx`，运行这个示例前需要准备 Node.js。

把 MCP 定义写入 `~/.copilot-shell/config.toml`。管理员也可以写入
`/etc/copilot-shell/config.toml`。项目配置不能添加 MCP server，打开一个仓库时不会因此
启动外部程序或向它提供凭据。

## 接入本地 stdio server

在配置中写入 `command` 和参数。把工作区路径换成允许 server 读取的目录。

```toml
[mcp.servers.filesystem]
command = "npx"
args = ["-y", "@modelcontextprotocol/server-filesystem", "/absolute/path/to/workspace"]
startup_timeout_ms = 30000
timeout_ms = 10000
allowed_tools = ["read_file", "list_directory"]
```

cosh 会直接启动这个命令，不经过 Shell。server 需要环境变量时，在配置中明确写出。

```toml
[mcp.servers.filesystem.env]
SERVICE_TOKEN = "${FILESYSTEM_MCP_TOKEN}"
```

## 接入远程 HTTP server

Streamable HTTP endpoint 使用 `url`。同一个 server 只能配置 `command` 或 `url`。

```toml
[mcp.servers.search]
url = "https://mcp.example.com/mcp"
allowed_tools = ["query"]

[mcp.servers.search.oauth]
scopes = ["search"]
```

服务提供静态 token 时，删除 OAuth 配置，并加入
`bearer_token = "${SEARCH_MCP_TOKEN}"`。使用 OAuth 登录时不能同时配置
`bearer_token`。远程 endpoint 必须使用 HTTPS，本地开发使用的 loopback 地址可以使用
HTTP。

## 连接并检查工具

进入 server 应当访问的工作区，随后启动 `cosh`。在 cosh 提示符中运行下面的命令。

```text
/mcp list
/mcp connect filesystem
/mcp inspect filesystem
```

`list` 用来确认 cosh 已读取配置。`connect` 会启动或连接 server，并发现它提供的工具。
`inspect` 会显示发现的工具名和 Agent 可见的名称，不会打印凭据。

HTTP server 使用 OAuth 时，先运行下面的 slash 命令。

```text
/mcp login search
```

cosh 会显示一条需要在 Shell 提示符中运行的命令。运行这条命令并在浏览器完成授权，
随后连接 server。

```text
/mcp connect search
/mcp inspect search
```

## 在任务中使用 MCP 工具

描述任务时可以写明要使用哪个已连接的服务，方便 Agent 选择工具。

```text
$ use the filesystem MCP tools to list Markdown files in this workspace; do not modify them
```

当前模式需要确认时，审批卡片会在执行前显示 MCP 工具及其输入。`allowed_tools` 决定
哪些已发现工具对 Agent 可见，它不会取消原有的审批要求。

## 刷新或断开 server

server 调整了工具列表后运行 `refresh`。不再使用某个 server 时，可以断开连接。

```text
/mcp refresh filesystem
/mcp disconnect filesystem
```

`disconnect` 会禁用 server，并删除保存的 OAuth 凭据。再次运行
`/mcp connect <server>` 可以启用它。`/mcp logout <server>` 只删除 OAuth 凭据，不修改
server 定义。

Agent 任务正在执行时，cosh 会等到安全边界再应用连接变化。下一项任务会使用刷新后的
工具列表。

## 排查连接问题

| 现象 | 检查内容 |
|---|---|
| `/mcp list` 没有列出 server | 确认配置位于系统或用户配置文件中，项目配置不会生效 |
| 本地 server 无法启动 | 检查程序路径、参数、明确传入的环境变量和 `startup_timeout_ms` |
| 已连接但没有可用工具 | 检查 `allowed_tools`，空列表不会暴露任何工具 |
| 远程 server 被拒绝 | 检查 HTTPS、token 和 OAuth 设置，只有 loopback 地址可以使用 HTTP |
| OAuth 登录无法开始 | 删除 `bearer_token`，运行 `/mcp login <server>`，再按终端中的提示操作 |

进入 Agent 上下文的 MCP 工具输出最多保留 64 KiB。外部工具提供的说明不会降低当前
审批要求。
