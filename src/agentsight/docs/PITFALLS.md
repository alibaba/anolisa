# AgentSight 常见踩坑记录

> AI agent 和新贡献者最容易犯的错误。每条格式：问题 → 原因 → 正确做法。

## 1. curl/wget 发请求不会被 AgentSight 捕获

**问题：** 用 `curl https://api.openai.com/...` 测试，AgentSight 捕获不到流量。

**原因：** AgentSight 通过 cmdline 规则匹配 agent 进程后，对该进程的 SSL 库 attach uprobe（`SSL_read`/`SSL_write`），不是按目标域名抓包。curl 的 cmdline 通常不在 `cmdline.allow` 规则里。

**正确做法：** 必须通过被规则识别的 agent 进程发起请求，或在配置中添加对应的 cmdline 规则。

## 2. 进程启动后立即发请求 → 首个请求被漏掉

**问题：** 测试脚本启动后立即调用 API，401 在终端正常返回，但 AgentSight 完全没有记录。

**原因：** AgentSight 的处理链路是：procmon eBPF 检测到 exec → AgentScanner 匹配 cmdline → 解析 ELF 找 SSL 符号 → attach uprobe，整条链路约 8-15 秒。进程在 attach 完成前发出的请求无法被捕获。

**正确做法：** 进程启动后至少 sleep 10 秒再发首个 HTTPS 请求。

## 3. cmdline 规则按 argv 位置匹配，不是整体子串

**问题：** 写 `["*python*hermes*"]` 想匹配"包含 hermes 的 python 进程"，规则不生效。

**原因：** matcher 把 patterns 数组按下标对齐到 argv 数组，每个 pattern 单独 glob 匹配对应位置的 arg。`["*python*hermes*"]` 只匹配 `argv[0]`，要求 `argv[0]` 同时包含 python 和 hermes。

**正确做法：** `["*python3*", "*hermes*"]` 表示 `argv[0]` 含 python3 且 `argv[1]` 含 hermes。写规则时明确每个元素对应 argv 的哪一位。

## 4. 修改配置后 daemon 不会自动 reload

**问题：** 修改 `/etc/agentsight/config.json` 后新进程仍然没被识别。

**原因：** 当前版本不监听配置文件 inotify，只在启动时读一次。

**正确做法：** 必须重启 daemon：`pkill -9 -f "agentsight trace"` 然后重新启动。重启时要带回 `SLS_LOGTAIL_FILE` 等环境变量。

## 5. cbindgen 0.27 不支持 Rust 2024 `#[unsafe(no_mangle)]`

**问题：** 新增 `extern "C"` FFI 函数后，生成的 C header 中没有对应声明。

**原因：** cbindgen 0.27 无法识别 Rust 2024 的 `#[unsafe(no_mangle)]` 属性，会静默跳过。当前通过 `cbindgen.toml` 的 `after_includes` 手写声明来绕过。

**正确做法：** 新增 FFI 函数时，必须同时在 `cbindgen.toml` 的 `after_includes` 块中手写 C 函数声明，并确认 `build.rs` 的 drift guard 通过。

## 6. build.rs drift guard 只检查函数名，不检查签名

**问题：** 修改了 FFI 函数的参数类型或返回值，drift guard 没有报错，但 C 调用方出现 ABI 不匹配。

**原因：** `build.rs::check_ffi_header_drift` 只比对 `src/ffi.rs` 和 `cbindgen.toml` 中的函数名集合，不验证参数类型、数量或返回值。

**正确做法：** 修改 FFI 函数签名时，必须手动同步 `cbindgen.toml` 中的 C 声明，逐字段核对参数类型和返回值。

## 7. `include/agentsight.h` 是构建产物，不要手动编辑

**问题：** 直接编辑 `include/agentsight.h` 修改 FFI 接口，下次编译后改动被覆盖。

**原因：** 该文件由 `build.rs` 通过 cbindgen 在每次编译时自动生成，已在 `.gitignore` 中。

**正确做法：** 修改 `src/ffi.rs`（Rust 侧）和 `cbindgen.toml`（C 声明），运行 `cargo build` 重新生成 header。

## 8. `discover` 命令用的是编译时默认规则

**问题：** 改了配置文件加了新规则，`agentsight discover` 仍然只显示默认 agent。

**原因：** `discover` 命令用的是编译时嵌入的默认规则（`default_cmdline_rules()`），不读运行时配置文件。

**正确做法：** 不要用 `discover` 验证规则是否生效。直接发请求后检查 logtail/DB 是否有对应 `agent_name` 的记录。

## 9. logtail 文件不要直接 truncate

**问题：** 想清空测试数据执行 `> /var/sysom/ilog/agentsight`，之后 iLogtail 报告偏移异常，后续记录漏传 SLS。

**原因：** iLogtail 维护文件 inode + 偏移的 checkpoint，truncate 会导致 checkpoint 失效但不被发现。

**正确做法：** 不要直接清空 logtail 文件。清理测试数据应删除 SQLite DB 或在 SLS 侧按时间区间过滤。

## 10. 同一进程崩溃可能产生多条 agent_crash 记录

**问题：** 一次 OOM kill，DB 里出现 2 条 `agent_crash` 记录，PID 相同。

**原因：** 进程崩溃时如果有多个 pending HTTP 连接，drain 路径会按连接逐个写记录。1 秒窗口去重无法覆盖跨窗口边界的情况。

**正确做法：** 这是预期行为，不是 bug。看 `detail.call_ids` 数组，每条记录对应不同的 pending call。

## 11. qodercli SSE 响应使用 {body, statusCode} 包装格式

**问题：** 解析 qodercli 的 SSE 响应时 tool command 显示为 `{}`，token 提取失败。

**原因：** qodercli 的 SSE 每行不是直接的 OpenAI chunk JSON，而是 `{"body": "<escaped-json>", "statusCode": 200}` 格式。解析时需要先 unwrap 这层包装，再解析内部的 body 字符串。另外 tool_calls 的 arguments 字段可能是 object 也可能是 string，需要兼容两种形式。

**正确做法：** 代码中共 4 处解析 qodercli SSE 的位置，每处都必须先 unwrap `{body, statusCode}` 包装，再将 `body` 字符串 `serde_json::from_str` 为实际的 chunk 对象。
