# v0.2 AgentSight C FFI API 文档

本文档描述 AgentSight 提供的 C 语言接口。采用 **eventfd + read 模式**：AgentSight 内部通过 `eventfd` 通知调用方有新事件就绪，调用方可将该 fd 注册到自己的 epoll/select 事件循环中，被唤醒后调用 `agentsight_read()` 通过回调消费数据。

## 1. C 数据结构

```c
/* HTTP 层数据 — 非 LLM 的 HTTPS 流量会产生此结构 */
typedef struct {
    int32_t     pid;
    char        process_name[16];
    uint64_t    timestamp_ns;
    uint64_t    duration_ns;
    const char* method;               /* "GET", "POST", ...; NUL-terminated */
    const char* path;                 /* "/v1/chat/completions"; NUL-terminated */
    uint16_t    status_code;
    uint8_t     is_sse;
    const char* request_headers;      /* JSON string */
    uint32_t    request_headers_len;
    const char* request_body;         /* JSON or raw text, may be NULL */
    uint32_t    request_body_len;     /* 0 when request_body is NULL */
    const char* response_headers;     /* JSON string */
    uint32_t    response_headers_len;
    const char* response_body;        /* JSON or raw text, may be NULL */
    uint32_t    response_body_len;    /* 0 when response_body is NULL */
} AgentsightHttpsData;

/* LLM 语义层数据 — 仅当 HTTP 流量被识别为 LLM API 调用时产生 */
typedef struct {
    /* 追踪标识 */
    const char* response_id;          /* LLM API 响应 ID（如 chatcmpl-xxx）; may be NULL */
    const char* conversation_id;      /* 同一 user query 触发的调用链标识; may be NULL */
    const char* session_id;           /* may be NULL */

    /* 进程 */
    int32_t     pid;
    char        process_name[16];
    const char* agent_name;           /* may be NULL */

    /* 时间与延迟 */
    uint64_t    timestamp_ns;
    uint64_t    duration_ns;

    /* 请求信息 */
    const char* request_url;          /* 完整 API 地址，如 "https://api.openai.com/v1/chat/completions" */
    const char* provider;             /* "openai", "anthropic", ... */
    const char* model;
    uint16_t    status_code;
    uint8_t     is_sse;

    /* LLM 特有信息 */
    const char* finish_reason;        /* "stop", "length", "tool_calls", ...; may be NULL */

    /* Token 用量（无信息时全为 0）
     * llm_usage: true = 数据来自 LLM API 响应中的 usage 字段（精确值）
     *            false = 由 AgentSight 本地 tokenizer 计算 */
    bool        llm_usage;
    uint32_t    input_tokens;
    uint32_t    output_tokens;
    uint32_t    total_tokens;
    uint32_t    cache_creation_input_tokens;
    uint32_t    cache_read_input_tokens;

    /* 请求/响应语义内容（JSON 字符串） */
    const char* request_messages;     /* LLMRequest.messages 序列化 JSON */
    uint32_t    request_messages_len;
    const char* response_messages;    /* LLMResponse.messages 序列化 JSON */
    uint32_t    response_messages_len;
} AgentsightLLMData;

```

## 2. C API 接口

```c
/* ---- 错误处理 ---- */
/* 返回最近一次 API 调用的错误描述，未出错时返回 NULL。
 * 返回的指针在下次 API 调用前有效，调用者应立即拷贝。 */
const char* agentsight_last_error(void);

/* ---- 配置 ---- */
AgentsightConfigHandle* agentsight_config_new(void);
void agentsight_config_set_verbose(AgentsightConfigHandle* cfg, int verbose);
void agentsight_config_set_log_path(AgentsightConfigHandle* cfg, const char* path);
/* 其他配置项待与调用方商定后补充 */
void agentsight_config_free(AgentsightConfigHandle* cfg);

/* ---- 回调类型 ---- */
typedef void (*agentsight_https_callback_fn)(const AgentsightHttpsData* data, void* user_data);
typedef void (*agentsight_llm_callback_fn)(const AgentsightLLMData* data, void* user_data);

/* ---- 生命周期 ---- */
AgentsightHandle* agentsight_new(AgentsightConfigHandle* cfg);
int agentsight_start(AgentsightHandle* h);
int agentsight_stop(AgentsightHandle* h);
void agentsight_free(AgentsightHandle* h);
const char* agentsight_version(void);

/* ---- 事件通知 ---- */
/* 获取 eventfd 文件描述符，可注册到调用方的 epoll/select 事件循环。
 * 当有新事件就绪时，该 fd 变为可读（EPOLLIN）。
 * 返回 >= 0 的 fd 表示成功，< 0 表示不支持（可降级为轮询模式）。
 * 注意：该 fd 由 AgentSight 内部管理，调用方不得 close()。 */
int agentsight_get_eventfd(AgentsightHandle* h);

/* ---- 数据读取 ---- */
/* 处理当前可用事件，通过回调返回数据。返回处理事件数，0=无事件，<0=出错。
 * 两个回调独立，传 NULL 表示不关心该类型。
 * flags: 0 = 非阻塞（处理当前队列后立即返回）
 *        AGENTSIGHT_READ_BLOCK = 阻塞直到有至少一个事件 */
#define AGENTSIGHT_READ_BLOCK 1
int agentsight_read(AgentsightHandle* h,
                    agentsight_https_callback_fn http_cb, void* http_ud,
                    agentsight_llm_callback_fn  llm_cb,  void* llm_ud,
                    int flags);

```

### 2.1 返回值

| 函数 | 返回值 | 说明 |
| --- | --- | --- |
| `agentsight_config_new` | `AgentsightConfigHandle*` | 成功返回句柄，失败返回 NULL |
| `agentsight_new` | `AgentsightHandle*` | 成功返回句柄，失败返回 NULL（可用 `agentsight_last_error` 查看原因） |
| `agentsight_start` | `int` | 0=成功，<0=失败 |
| `agentsight_stop` | `int` | 0=成功，<0=失败 |
| `agentsight_get_eventfd` | `int` | >= 0 为有效 fd，< 0 表示不支持 eventfd |
| `agentsight_read` | `int` | \>0=处理的事件数，0=无事件，<0=出错 |
| `agentsight_last_error` | `const char*` | 错误描述字符串，无错误时返回 NULL |
| `agentsight_version` | `const char*` | 版本号字符串（如 `"0.1.0"`），静态存储，无需释放 |

### 2.2 配置默认值

| 配置项 | 默认值 | 说明 |
| --- | --- | --- |
| `verbose` | 0 | 设为 1 开启调试日志输出 |
| `log_path` | NULL | 日志文件保存路径，NULL 时输出到 stderr |

> 其他配置项待与调用方商定后补充。

### 2.3 线程安全

* 同一 `AgentsightHandle` 不可多线程并发调用，所有 API（start/read/stop）须在同一线程执行

* 回调函数在调用 `agentsight_read()` 的线程上同步执行，无需额外同步

* 不同 `AgentsightHandle` 实例之间完全独立，可跨线程使用

* `agentsight_get_eventfd()` 返回的 fd 可安全地在其他线程中用于 epoll/select 等待

## 3. 使用示例

### 3.1 eventfd + epoll 模式（推荐）

```c
/* --- 初始化阶段 --- */
AgentsightConfigHandle* cfg = agentsight_config_new();
agentsight_config_set_verbose(cfg, 1);  // 可选：开启调试日志

AgentsightHandle* h = agentsight_new(cfg);
agentsight_config_free(cfg);

if (!h) {
    fprintf(stderr, "agentsight_new failed: %s\n", agentsight_last_error());
    return -1;
}

agentsight_start(h);

/* 获取 eventfd，注册到统一 epoll */
int as_efd = agentsight_get_eventfd(h);
if (as_efd < 0) {
    fprintf(stderr, "eventfd not supported, fallback to polling\n");
    /* 降级到轮询模式，见 3.2 */
}

int epoll_fd = epoll_create1(0);
struct epoll_event ev = {
    .events   = EPOLLIN,
    .data.ptr = h,
};
epoll_ctl(epoll_fd, EPOLL_CTL_ADD, as_efd, &ev);

/* --- 事件循环（可与其他 fd 共用同一 epoll_wait）--- */
while (running) {
    struct epoll_event events[64];
    int n = epoll_wait(epoll_fd, events, 64, 200 /* ms */);

    for (int i = 0; i < n; i++) {
        if (events[i].data.ptr == h) {
            /* AgentSight 有数据就绪，非阻塞消费 */
            agentsight_read(h, my_http_cb, http_ctx,
                               my_llm_cb,  llm_ctx,
                               0 /* non-blocking */);
        } else {
            handle_other_event(&events[i]);
        }
    }
}

/* --- 清理阶段 --- */
epoll_ctl(epoll_fd, EPOLL_CTL_DEL, as_efd, NULL);
agentsight_stop(h);
agentsight_free(h);  /* 内部 close(as_efd)，调用方不得重复 close */
close(epoll_fd);
```

### 3.2 轮询模式（降级 / 简单场景）

```c
AgentsightConfigHandle* cfg = agentsight_config_new();
AgentsightHandle* h = agentsight_new(cfg);
agentsight_config_free(cfg);

if (!h) {
    fprintf(stderr, "agentsight_new failed: %s\n", agentsight_last_error());
    return -1;
}

agentsight_start(h);

while (running) {
    agentsight_read(h, my_http_cb, http_ctx,
                       my_llm_cb,  llm_ctx,
                       0 /* non-blocking */);
    usleep(100000);  // 100ms 轮询间隔
}

agentsight_stop(h);
agentsight_free(h);
```

## 4. 内存规则

* 回调中的指针仅在回调执行期间有效，调用方需自行拷贝

* `agentsight_new()` 内部拷贝配置，不消费 config handle，调用者须自行 `agentsight_config_free(cfg)`

* 同一 config handle 可复用于创建多个 `AgentsightHandle` 实例

* `agentsight_free()` 须在 `agentsight_stop()` 之后调用

* `agentsight_get_eventfd()` 返回的 fd 由 `agentsight_free()` 内部关闭，调用方**不得**自行 `close()`

## 5. HttpsData 与 LLMData 的关系

一条被捕获的 HTTPS 流量只会产生一种数据：若被识别为 LLM API 调用，则产生 `AgentsightLLMData`；否则产生 `AgentsightHttpsData`。两者互斥，不会同时产生，无需关联。

## 6. 编译与链接

* 库文件：`libcoolbpf.so`（Linux）

* 头文件：`include/agentsight.h`

* 编译：`gcc -I/include/agentsight -lcoolbpf -o myapp myapp.c`

## 7. 变更记录

| 版本 | 变更 |
| --- | --- |
| v0.1 | 初始版本，轮询 read 模式 |
| v0.2 | 升级为 eventfd + read 模式；新增 `agentsight_get_eventfd()`；`agentsight_read()` 增加 `flags` 参数；新增 `agentsight_config_set_log_path()`；大 buffer 指针增加 `_len` 字段；新增 `llm_usage` 字段区分 token 数据来源 |
