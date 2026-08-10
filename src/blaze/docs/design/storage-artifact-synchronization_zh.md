# 存储制品同步

[English](storage-artifact-synchronization.md)

Blaze 可以定期要求已配置的 `StorageProvider` 持久化 running sandbox 中已经
写入的宿主机制品文件和目录元数据。这样，provider 不仅具备同步单个 slot
制品的能力，daemon 也能安全调度所有符合条件的 sandbox。

周期同步默认关闭。将 `storage.sync_interval` 设置为正数 duration 后启用。
`storage.sync_timeout` 是 scheduler 等待单次 provider attempt 的最长时间，
默认值为 30 秒。

## 哪些 sandbox 会被同步

每轮开始时，manager 先选择 lifecycle 状态为 `Running` 的记录。调用某个
sandbox 的 provider 前，会尝试非阻塞地取得 lifecycle 变更和 guest exec、read、
write 请求共用的 operation lock。lock 已被占用时，该 sandbox 会推迟到后续
sweep，避免一个正在进行的 sandbox 操作阻止 worker 继续处理其他符合条件的
记录。取得 lock 后，manager 会再次检查记录。

只有同时满足以下条件时才会调用 provider：

- lifecycle 状态仍为 `Running`；
- 没有未结束的 lifecycle operation；
- metadata 记录 backend 正在运行，而且 daemon 仍持有该 backend；
- provider 可以根据 sandbox ID 重建完整 slot。

已经不再处于 Running 状态的 sandbox 会被跳过。状态为 Running 但 ownership
不完整的记录会计为失败，而不是被静默遗漏；本轮的其他 sandbox 仍会继续处理。

第一次 sweep 在一个完整 interval 后开始。错过的 tick 会被跳过，不会排队，
因此耗时较长的一轮不会形成无界积压。

## 失败与重试

每次 provider attempt 都有一个覆盖 slot 重建和同步的 scheduler deadline。
已经返回的失败不会改变 lifecycle 状态，slot 仍归 sandbox 持有；后续 sweep
或 destroy 可以再次尝试。

某些文件系统操作开始后无法取消。这类操作超过 deadline 时，scheduler 会报告
超时，但原 attempt 仍持有 sandbox operation lock。它还会继续占用唯一的同步
许可，因此后续 attempt 会被推迟，而不会继续创建 blocking 文件系统任务。
原 attempt 完成后会释放 lock 和许可，正常重试随之恢复。在此期间到达的 guest
和 lifecycle 操作会等待 provider 工作完成；配置的 timeout 只限制 scheduler
的等待时间，不限制这些操作的等待时间。

`StorageProvider::sync_artifacts` 是 provider 特定的持久化边界。file provider
会依次对规范的 `rootfs.ext4`、`mem.bin`、`mem.diff` 和 `rootfs.diff` 文件执行
`sync_all`，然后同步 slot 目录；其他 provider 可以采用不同机制，但必须保持
相同的 ownership-until-completion 合同。

## 能力边界

每次 provider 同步调用会持久化本次调用可见、且已经写入的制品字节与目录元数据。
与一次同步并发发生的更新可能在本次或后续同步中到达持久化边界。

## Daemon 关闭

daemon 提供请求服务时会同时监控周期 worker。如果 worker 意外退出，daemon
会退出 accept loop，并报告 worker 错误。收到终止信号后，daemon 先退出
accept loop，再取消并等待周期 scheduler 退出。无法取消的 provider 工作会
继续由对应 sandbox lock 持有，直至完成。

这个改动只负责同步 worker 的生命周期。排空已经接收的连接以及释放 daemon
持有的全部 runtime 资源属于其他关闭职责；在这些职责实现之前，不能把
service loop 返回理解为所有正在处理的请求和 runtime owner 都已经结束。
