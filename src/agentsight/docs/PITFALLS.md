# AgentSight 常见踩坑记录

> AI agent 和新贡献者最容易踩的坑。

| # | 坑 | 原因 | 正确做法 |
|---|-----|------|---------|
| 1 | curl/wget 请求不被捕获 | AgentSight 按 cmdline 规则匹配进程后才 attach uprobe，curl 不在规则里 | 通过被规则识别的 agent 进程发请求，或添加对应 cmdline 规则 |
| 2 | 进程启动后首个请求漏掉 | procmon 检测 → cmdline 匹配 → ELF 解析 → attach uprobe 链路约 8-15 秒 | 启动后至少 sleep 10 秒再发首个 HTTPS 请求 |
| 3 | cmdline 规则不生效 | patterns 数组按下标对齐 argv，`["*python*hermes*"]` 只匹配 `argv[0]` | 用 `["*python3*", "*hermes*"]` 按位置分别匹配 |
| 4 | 改配置后不生效 | 当前版本只在启动时读一次配置，不支持 reload | 必须重启 daemon，注意带回环境变量 |
| 5 | 新 FFI 函数在 C header 中缺失 | cbindgen 0.27 不识别 Rust 2024 `#[unsafe(no_mangle)]` | 在 `cbindgen.toml` 的 `after_includes` 手写 C 声明（详见 [ADR-005](adr/005-cbindgen-handwritten-declarations.md)） |
| 6 | 改 FFI 签名 drift guard 不报错 | `build.rs` drift guard 只检查函数名，不检查参数/返回值 | 改签名时必须手动同步 `cbindgen.toml` 中的 C 声明 |
| 7 | 手动编辑 `include/agentsight.h` 被覆盖 | 该文件由 `build.rs` + cbindgen 每次编译自动生成 | 修改 `src/ffi.rs` 和 `cbindgen.toml`，`cargo build` 重新生成 |
| 8 | `discover` 命令看不到新规则 | `discover` 用编译时默认规则，不读运行时配置 | 直接发请求后检查 DB/logtail 中的记录 |
| 9 | truncate logtail 文件后数据漏传 | iLogtail 维护 inode+偏移 checkpoint，truncate 导致 checkpoint 失效 | 清理数据应删 SQLite DB 或在 SLS 侧按时间过滤 |
| 10 | 一次崩溃出现多条 agent_crash | 多个 pending HTTP 连接各自触发一条记录，跨窗口去重不覆盖 | 预期行为，看 `detail.call_ids` 区分对应的 pending call |
| 11 | qodercli SSE 解析失败 | SSE 行是 `{"body":"<escaped>","statusCode":200}` 包装格式，且 tool args 可能是 object 或 string | 4 处解析点都须先 unwrap 包装再解析 body（详见代码注释） |
