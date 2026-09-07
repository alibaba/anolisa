# AgentSec daemon 进程与部署契约

| 属性 | 值 |
| --- | --- |
| 状态 | V1 Python 交付基线、兼容语料及仓库内 V2 部署目标 |
| 实现核对日期 | 2026-09-04 |
| 当前行为基线 | fe58ed4b23b8；与 main 中已有 systemd/RPM 行为交叉核对 |
| 适用实现 | V1 Python daemon oracle；V2 Rust asc-daemon；安装器、migrator 和进程管理器 |

## 1. 文档地位与范围

本文冻结 V1 agent-sec-daemon 的进程入口、安装布局、systemd user service、启动/停止、
重启、runtime/data path 和诊断日志事实，并定义这些事实如何进入 V2 compatibility 和迁移
验收。V2 产品形态以仓库内
[`AGENT_SEC_RUST_MIGRATION_zh.md`](AGENT_SEC_RUST_MIGRATION_zh.md#1-文档状态与仓库内权威关系)
为准：

- one daemon per host；
- Linux system-scope systemd；
- Kubernetes 每 Node 一个 DaemonSet；
- system-owned runtime/state；
- Rust asc-cli 仅作为 daemon client；
- 不保留 Python CLI、PyO3 local fallback 或 per-user daemon。

socket 生命周期见
[DAEMON_CURRENT_BEHAVIOR_zh.md](DAEMON_CURRENT_BEHAVIOR_zh.md)，wire protocol 见
[DAEMON_PROTOCOL_V1_zh.md](DAEMON_PROTOCOL_V1_zh.md)，后台任务见
[DAEMON_JOB_CONTRACT_zh.md](DAEMON_JOB_CONTRACT_zh.md)。

标签含义：

- **[CURRENT]**：V1 当前事实；
- **[PRESERVE V1]**：supported V1 接口在兼容期保持的语义；
- **[TARGET V2]**：与仓库内迁移总计划一致的 V2 目标；
- **[SUPERSEDED]**：已被当前仓库 V2 架构取代的旧目标。

未标记行为只属于 **[CURRENT]**，不能自动升级为 V2 要求。外部兼容可以由 Rust binary、
命令 alias、protocol adapter 或 state migrator 提供，不要求保留 Python runtime。

deploy/sidecar/healthcheck.py 不在 main 的受支持交付基线中，不得作为 CURRENT/PRESERVE V1
证据。sidecar、本地 probe 或实验 chart 不能反向定义 readiness。

## 2. **[CURRENT]** V1 进程入口与信号

受支持 V1 安装当前提供 agent-sec-daemon：

- agent-sec-daemon serve 启动前台 daemon；
- 无子命令当前等价于 serve；
- --help 成功并输出 usage/help；
- daemon 不 fork、double-fork、写 pidfile 或自行转入后台；
- SIGTERM、SIGINT 进入 drain/cleanup；
- SIGHUP 记录 no-op；
- 启动配置、runtime path、lock、Job start 或 bind 失败非零退出；
- SIGKILL 和不可恢复 crash 由进程管理器与下次启动恢复；
- Python traceback 文本不是稳定接口，稳定面是 exit status、结构化日志和 health RPC。

当前 wheel 使用 Python console script；RPM/raw wrapper 启动私有 Python runtime。这些是
V1 packaging 事实，不是 V2 实现约束。

agent-sec-daemon 命令名、serve、参数、默认值和退出语义必须进入 compatibility inventory。
如果继续标记为 supported，V2 应由 Rust asc-daemon binary、Rust 命令 alias 或明确版本化
入口承接；不能静默删除，也不要求保留 Python wrapper。

## 3. **[CURRENT]** V1 per-user 部署

### 3.1 安装产物

V1 RPM 当前提供：

- /usr/bin/agent-sec-daemon，mode 0755；
- /usr/lib/systemd/user/agent-sec-core.service，mode 0644；
- ExecStart 指向 agent-sec-daemon serve。

raw package 保存可重定位 wrapper 和带 bindir/datadir 占位符的 user unit template；
source/venv 使用 console-script symlink。安装器渲染后不得残留占位符。

### 3.2 systemd user unit

V1 agent-sec-core.service 当前是 per-user service：

| 项目 | 当前值/语义 |
| --- | --- |
| service type | Type=simple；被 exec 的 daemon 是主进程 |
| runtime env | XDG_RUNTIME_DIR=/run/user/%U |
| runtime directory | RuntimeDirectory=agent-sec-core、mode 0700 |
| restart | Restart=on-failure、RestartSec=2 |
| crash-loop gate | 300 秒内最多 5 次启动失败 |
| install target | default.target |
| privilege | 不要求 root，不允许 privilege uplift |

当前 hardening 包括 NoNewPrivileges、PrivateTmp、ProtectSystem、受控 ReadWritePaths、
kernel/control-group protection、RestrictSUIDSGID 和 LockPersonality。V2 应保留等价或更强
hardening；需要放宽时提供 syscall/filesystem 证据、最小例外和测试。

上述 user unit、XDG path 和用户级 singleton 只属于 V1。V2 交付物不得继续安装 user-scope
unit，也不得让 asc-cli 自动创建用户 daemon。

### 3.3 active 不等于 ready

V1 Type=simple 的 active 不证明 socket 已 bind、Job 已启动或 daemon.health 可返回。此
可观察区别必须保留到 V2 readiness 设计：

- process active 与 application READY 分开；
- readiness 必须通过受支持 health RPC/probe 验证；
- prompt compatibility stub 不代表 capability readiness；
- 单个 Job error 不自动等于顶层 daemon 不可用；
- 引入 sd_notify、socket activation 或 container probe 时必须冻结 timeout、failure 和
  restart 语义。

## 4. **[TARGET V2]** system-level 部署

### 4.1 Host

- 安装 system-scope systemd unit，不安装或启用 user-scope unit；
- 默认一个 Host 一个 asc-daemon；第二实例必须因 Host 级 singleton 失败；
- unit 使用专用 service account 和最小 capability，system-level 不等于 UID 0；
- systemd 负责 start/stop/restart、资源限制、目录准备和故障拉起；
- daemon 不自行 daemonize，不在启动路径隐式执行不可逆 migration；
- packaging 通过 sysusers/tmpfiles 或等价机制创建 system-owned runtime/state/log path；
- 两个不同 UID/Agent 通过同一 system socket 访问并保持 owner-scope 隔离。

### 4.2 Kubernetes

- 默认 DaemonSet，每个目标 Node 恰有一个 Ready 实例；
- Helm/manifest 明确 service account、security context、volume、resource 和 probe；
- init job 或安装流程显式调用 asc-state-migrator；daemon 启动不偷偷升级状态；
- rollout、rollback、drain 和 Node replacement 分别留存证据。

### 4.3 CLI 与进程所有权

asc-cli 是 Rust daemon client：

- 不执行 systemctl start；
- 不创建用户 socket、lock 或 daemon；
- daemon unavailable 时返回稳定错误；
- 不使用 PyO3、Python backend 或通用 local fallback；
- 少数纯函数的 Rust local mode 必须有独立批准合同，不能继承旧 fallback 语义。

## 5. Runtime、singleton 和 lock

### 5.1 **[CURRENT]** V1 lock

V1 daemon.lock 正常 stop 后保留。下次启动当前会：

1. read/write 打开或创建；
2. 尝试 non-blocking exclusive flock；
3. 持锁时报告 already running；
4. 无持锁者时复用 inode、truncate、写 PID；
5. 继续 stale-socket probe。

V1 未对已有 lock path 完整执行 no-follow、regular-file、owner 和 mode 验证。这是安全缺口，
不是 PRESERVE V1 行为。

### 5.2 **[TARGET V2]** Host 级 singleton hardening

- lock、socket、runtime 和 state 是 Host 级 system-owned 资源；
- 最终 path component 不跟随 symlink；
- 在同一已打开 fd 上验证 regular file、owner、mode、lock、truncate 和 PID，避免 reopen
  TOCTOU；
- 无持锁者时可复用安全遗留 lock；持锁实例阻止第二实例；
- cleanup 只删除本实例绑定的同一 socket inode；
- held lock、unsafe path、permission 和普通 I/O failure 使用稳定分类；
- 多 UID client 不能通过替换 runtime path 或客户端自报身份影响 singleton。

## 6. 数据目录与状态迁移

### 6.1 **[CURRENT]** AGENT_SEC_DATA_DIR

V1 AGENT_SEC_DATA_DIR 是 Python CLI writer 与 daemon query/log 共用的数据根，不只是日志
目录。设置后承载：

- security-events.db/jsonl；
- observability.db/jsonl；
- daemon.jsonl；
- 其它复用 security-event path resolver 的本地流。

未设置时，V1 resolver 依次尝试 /var/log/agent-sec、~/.agent-sec-core 和 per-user 临时目录，
并创建 mode 0700 目录。这些 fallback 是 V1 discovery 输入，不是 V2 默认布局。

### 6.2 **[TARGET V2]** system-owned persistence

- asc-daemon composition root 装配 persistence adapter；
- CLI/TUI 不直读 SQLite，所有查询经过 daemon-core authorization 和 server QueryScope；
- state 以 owner principal 隔离，不以客户端传入 UID/role/scope 决定访问；
- AGENT_SEC_DATA_DIR 是否继续作为 operator override 必须在 config contract 中版本化定义；
- V2 不让每个用户和 daemon 各自推导不同数据库路径。

asc-state-migrator 必须验证 V1 path discovery、显式 source、owner mapping、schema migration、
重复运行、事务、失败恢复、回滚、mixed-read、权限、symlink/hardlink 和多用户数据冲突。
Credential、token、passphrase 和 key material 不进入通用 persistence。

## 7. 诊断日志

V1 当前 daemon 日志语义进入兼容语料：

- 主流为 data-dir/daemon.jsonl；
- 默认 INFO，AGENT_SEC_DAEMON_LOG_LEVEL=off 禁用；
- debug/info/warning/error/critical 大小写和空白不敏感；
- 单文件 10 MiB，保留 5 个备份；
- 写入失败 best-effort，不改变业务 response。

V2 logging/OTel 可以更换 sink 和 delivery，但结构化字段、脱敏、failure isolation 和
operator-visible semantics 必须进入 compatibility/change record。journald 文本不能替代稳定
机器可读日志或 SecurityEvent。

## 8. **[TARGET V2]** Rust 交付要求

1. asc-daemon、asc-cli 和 asc-state-migrator 都是 Rust binary；
2. V2 runtime 不依赖 Python interpreter、site-packages、PyO3 extension 或 wheel；
3. raw/RPM/container/systemd/Helm 安装相互一致；
4. Linux 只交付 system-scope unit；Kubernetes 交付每 Node 一个 DaemonSet；
5. supported V1 命令/RPC/config/state 由兼容 adapter 或版本化迁移承接；
6. restart、signal、readiness、runtime/state path、权限、日志和 exit semantics 使用黑盒
   fixture；
7. 安装、升级和不可逆 migration 由 packaging/deploy/state-migrator 所有；
8. 不把未进入 main 的 helper、probe 或本地 chart 当作 V1 事实。

### 8.1 **[TARGET V2][PARTIAL]** 当前 Rust transport bring-up

`v2/apps/asc-daemon` 当前提供可执行的前台 Rust binary 和 composition bootstrap。它接受
无子命令或显式 `serve` 两种形式，要求通过 `--socket` 提供绝对路径，安装 SIGTERM/SIGINT
cooperative shutdown，并消费 SIGHUP 而不 reload。bootstrap 使用
`asc-daemon-service` 完成真实 UDS bind、bounded admission、单请求 frame 读取、drain 和同
inode socket cleanup。

transport 对 frame read、application dispatch、transport rejection encode、response
write 和 drain 分别设置显式 deadline。dispatch deadline 到期会释放 connection admission
并向 handler 发出 cooperative cancellation，但 Rust 不能强制终止已经运行且忽略取消信号的
blocking call。`asc-daemon` 因此显式拥有 Tokio runtime，并在 service drain 后使用额外的
runtime shutdown timeout，避免残留 `spawn_blocking` 让前台进程永久不能退出。

该 slice 已由唯一的 concrete `DaemonDispatcher` 注册 first-version PAP daemon protocol，
但尚未注册 `daemon.health`。dispatcher 完成 envelope decode、request ID、kernel peer
credentials 到 trusted Principal 的绑定、method allowlist、authorization 和 response
encode；PAP 是其中一组显式注册的方法，不增加第二个 service dispatch 层。当前 composition
root 使用 `RootManagedPrincipalPolicy`：UID 0 默认具有 PAP 管理权限，其它 UID 在未加载
root-owned delegation 前返回 `permission_denied`，caller-supplied identity 不能覆盖该判断。

当前 PAP 由 `PolicyTemplateCompiler` 和过渡性的 process-local Repository 组成。Policy、Scope
和 Binding CRUD 可在同一 daemon 生命周期内经真实 UDS 执行，但所有状态在进程重启后丢失，
进程启动时会显式输出该限制。这些结果只证明 protocol、identity、authorization 和应用装配的
integration slice，不表示 durable persistence、target enforcement 或 application READY。
Busy、timeout、shutdown 等 transport failure 由独立且有短 deadline 的
`RejectionEncoder` 投影，正常依赖图不包含 PAP、Repository 或 Compiler。
framework 不能证明具体 PAP/Repository 内部没有全局 mutex、长 transaction 或其它共享阻塞
点；该项必须由 PAP direct-consumer concurrency fixture 在集成时验收。

当前还未实现 packaging-owned system socket 默认值、runtime directory hardening、Host
singleton/stale-socket 判定、日志/OTel 和 health readiness。因此这一 slice 提供
DPROC-002/DPROC-003 的 focused process evidence，以及 DPROC-013 中 binary + UDS protocol
注册、server-side permission 和 signal cleanup 的部分证据；它不能宣称 DPROC-012、完整
DPROC-013、DPROC-014 或 production process gate 已完成。

## 9. 验收矩阵

### 9.1 **[CURRENT]** V1 oracle

| ID | 必须固定的 V1 事实 |
| --- | --- |
| DPROC-001 | wheel/source、RPM、raw 当前命令和 --help 行为 |
| DPROC-002 | 无子命令/serve、前台主进程和不 daemonize |
| DPROC-003 | SIGTERM/SIGINT、启动失败、SIGKILL 与 cleanup |
| DPROC-004 | user unit、RuntimeDirectory 0700、restart 和 crash-loop 当前值 |
| DPROC-005 | 当前 hardening 与 privilege 行为 |
| DPROC-006 | systemd active 与 UDS health 分离 |
| DPROC-007 | AGENT_SEC_DATA_DIR 的 V1 CLI/daemon path 解析 |
| DPROC-008 | log level、rotation 和 best-effort failure |
| DPROC-009 | V1 lock reuse、held lock 和 stale socket |

以上 ID 都必须有 fixture，但 DPROC-004、DPROC-007 的 per-user 形态不自动成为 V2 PRESERVE。

### 9.2 **[TARGET V2]**

| ID | 必须验证的 V2 行为 |
| --- | --- |
| DPROC-010 | Rust binaries 不装载 Python/PyO3，supported V1 命令具有兼容或版本化路径 |
| DPROC-011 | daemon unavailable 时 asc-cli 返回稳定错误，不启动 user daemon、不 local fallback |
| DPROC-012 | Host lock/socket 拒绝 symlink、非 regular、错误 owner/mode 和 reopen TOCTOU |
| DPROC-013 | system-scope restart、signal、readiness、permission 和 log 黑盒测试通过 |
| DPROC-014 | 不安装 user unit；Host 第二实例被拒绝 |
| DPROC-015 | 两个 UID/Agent 经同一 socket 访问，owner scope 隔离且自报身份不能越权 |
| DPROC-016 | 每个目标 Kubernetes Node 恰有一个 Ready DaemonSet 实例 |
| DPROC-017 | state migrator 完成 V1 per-user 到 system-owned state 的 owner-safe 迁移和回滚 |
| DPROC-018 | CLI/TUI 不直读 SQLite；query 必须经过 daemon authorization |
| DPROC-019 | raw/RPM/container/systemd/Helm 生成 checksum、SBOM 和 build metadata |

每个 DPROC ID 必须映射到机器可执行 fixture 或真实部署证据。Rust unit test 不能代替安装后
service/package、server-side admission 或真实 Kubernetes rollout 验证。

## 10. 当前实现证据

- daemon entry/process/signal：agent-sec-cli/src/agent_sec_cli/daemon/server.py；
- wheel console script：agent-sec-cli/pyproject.toml；
- RPM wrapper：scripts/agent-sec-daemon-wrapper.sh；
- raw wrapper：packaging/raw/assets/bin/agent-sec-daemon；
- V1 systemd template：packaging/systemd/agent-sec-core.service.in；
- install layout：Makefile、agent-sec-core.spec.in、packaging/raw/package.sh；
- data/log path：security_events/config.py、daemon/logging.py；
- service tests：tests/e2e/daemon/test_daemon_systemd_e2e.py；
- process/signal tests：tests/e2e/daemon/test_daemon_e2e.py；
- package layout tests：tests/packaging/test-package-raw.sh；
- Rust DPROC-002/DPROC-003 与部分 DPROC-013 process fixture：
  v2/apps/asc-daemon/tests/bootstrap.rs；
- Rust PAP 完整 serialized UDS scenario：
  v2/crates/daemon/asc-daemon-protocol/tests/fixtures/pap-crud-e2e.json。
