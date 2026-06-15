# AgentSight 常见踩坑记录

> 仅记录读代码不容易发现的运行时/外部系统行为。

| # | 坑 | 原因 | 正确做法 |
|---|-----|------|---------|
| 1 | 进程启动后首个请求漏掉 | procmon 检测 → cmdline 匹配 → ELF 解析 → attach uprobe 链路约 8-15 秒 | 启动后至少 sleep 10 秒再发首个 HTTPS 请求 |
| 2 | cmdline 规则不生效 | patterns 数组按下标对齐 argv，`["*python*hermes*"]` 只匹配 `argv[0]` | 用 `["*python3*", "*hermes*"]` 按位置分别匹配 |
| 3 | truncate logtail 文件后数据漏传 | iLogtail 维护 inode+偏移 checkpoint，truncate 导致 checkpoint 失效 | 清理数据应删 SQLite DB 或在 SLS 侧按时间过滤 |
| 4 | 一次崩溃出现多条 agent_crash | 多个 pending HTTP 连接各自触发一条记录，跨窗口去重不覆盖 | 预期行为，看 `detail.call_ids` 区分对应的 pending call |
| 5 | qodercli SSE 解析失败 | SSE 行是 `{"body":"<escaped>","statusCode":200}` 包装格式，且 tool args 可能是 object 或 string | 4 处解析点都须先 unwrap 包装再解析 body |
