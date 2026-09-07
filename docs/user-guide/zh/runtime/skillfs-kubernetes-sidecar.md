# 在 Kubernetes 上以 Sidecar 运行 SkillFS

[English](../../en/runtime/skillfs-kubernetes-sidecar.md)

将 SkillFS 与 Kubernetes 工作负载部署在同一个 Pod 内。SkillFS 容器负责 FUSE
mount，工作负载保持非特权，只读取传播后的 SkillFS view，不直接挂载物理 Skill
source。

仓库目前没有发布可直接拉取的 SkillFS Sidecar 专用镜像。请从准备部署的源码
revision 构建镜像，完成本地验证后，再推送到集群可以访问的 registry。

## 前提条件

- Kubernetes 1.29 或更高版本。
- Linux 节点提供 `/dev/fuse`。
- 允许 SkillFS Sidecar 以特权容器运行。
- 可使用 `docker buildx` 和 `kubectl`。
- 集群能够拉取目标 registry 中的镜像。

## 选择基础镜像

SkillFS 提供两份功能相同的 Sidecar 镜像定义。它们使用相同的 entrypoint、probe、
默认路径和 Kubernetes manifest。

| Dockerfile | Runtime 基础镜像 | 适用场景 |
| --- | --- | --- |
| `src/skillfs/container/Dockerfile` | Debian Bookworm | 使用通用镜像，并由固定的 Rust 1.86 toolchain 完成构建 |
| `src/skillfs/container/Dockerfile.alinux4` | Alibaba Cloud Linux 4 | 部署环境统一使用 Alibaba Cloud Linux 4，或需要通过公开 Aliyun RPM 与 Cargo 镜像完成构建 |

Alibaba Cloud Linux 4 构建默认使用 Aliyun Cargo 镜像。构建环境需要直接访问
crates.io 时，把 `SKILLFS_CARGO_REGISTRY_INDEX` build argument 设为空值。生产构建
需要固定基础镜像 tag 或 digest 时，两份定义都可以通过 `BASE_IMAGE` build
argument 覆盖默认值。

## 构建、验证并推送镜像

下面的命令从仓库根目录运行。示例为 AMD64 构建 Debian 镜像，并用源码 commit
作为镜像 tag，方便部署后追溯构建输入。

```bash
export REVISION="$(git rev-parse HEAD)"
export VERSION="$(git describe --tags --match 'skillfs/v*' --always)"
export IMAGE="registry.example.com/anolisa/skillfs-sidecar:$(git rev-parse --short=12 HEAD)"
export PLATFORM=linux/amd64
export DOCKERFILE=src/skillfs/container/Dockerfile

docker buildx build \
  --platform "$PLATFORM" \
  --build-arg VERSION="$VERSION" \
  --build-arg REVISION="$REVISION" \
  -f "$DOCKERFILE" \
  -t "$IMAGE" \
  --load \
  src/skillfs

docker run --rm --platform "$PLATFORM" "$IMAGE" skillfs --version
docker push "$IMAGE"
```

构建 Alibaba Cloud Linux 4 镜像时，将 `DOCKERFILE` 设为
`src/skillfs/container/Dockerfile.alinux4`。`PLATFORM` 应与目标节点架构一致。
发布 multi-platform tag 前，需要逐一构建并运行 smoke check。仓库 CI 目前不会构建
或测试这两份专用 Sidecar 镜像。

上面的 `docker run` 只验证二进制和 runtime library。提供 FUSE view 还需要下文所列
的设备、权限、Volume 和 mount propagation 配置。

## 镜像启动方式

没有传入 command argument 时，镜像先运行 preflight，然后按下面的固定方式启动。

```text
skillfs mount "$SKILLFS_SOURCE" "$SKILLFS_MOUNTPOINT" \
  --foreground --allow-other
```

`SKILLFS_DISCOVER_ROOT` 和 `SKILLFS_EXTRA_ARGS` 可以追加可选 mount argument。不要
加入 `--managed`。前台 SkillFS 进程需要保持为 PID 1，让 kubelet 可以重启它，并把
`SIGTERM` 直接交给进程。给镜像传入 command argument 会完全替换 mount 命令，因此
版本 smoke check 不需要 `/dev/fuse`。

## Pod 必需结构

参考 Pod 不向工作负载暴露物理 source，只共享传播后的 FUSE view。

| Volume | Sidecar mount | 工作负载 mount | 要求 |
| --- | --- | --- | --- |
| `skill-source` | `/var/lib/skillfs/source` | 不挂载 | 必须可写。Skill 变更需要跨 Pod 保留时使用 PVC |
| `skill-shared` | `/var/lib/skillfs/shared`，使用 `Bidirectional` | 相同路径，使用 `HostToContainer` | FUSE mountpoint 必须是 `shared/mount` 这样的子目录，不能覆盖 Volume 根目录 |
| `fuse-device` | `/dev/fuse` | 不挂载 | 使用 `/dev/fuse` `hostPath`，类型设为 `CharDevice` |

只有 SkillFS Sidecar 以 root 身份运行并启用 `privileged`。镜像固定加入
`--allow-other`，工作负载可以使用另一个非 root UID。Manifest 将 SkillFS init
container 的 `restartPolicy` 设为 `Always`，从而使用原生 Kubernetes Sidecar。
Kubernetes 会等 FUSE startup probe 通过再启动工作负载，并在工作负载退出后停止
Sidecar。

## 部署

示例使用 ConfigMap 提供 Skill source。持久化部署应替换为 PVC。

```bash
export NS=skillfs-container-example

kubectl apply -f src/skillfs/deploy/kubernetes/00-namespace.yaml
kubectl apply -f src/skillfs/deploy/kubernetes/10-example-configmap.yaml
sed "s|skillfs-sidecar:dev|$IMAGE|g" \
  src/skillfs/deploy/kubernetes/20-pod.yaml | kubectl apply -f -

kubectl -n "$NS" wait \
  --for=condition=Ready pod/skillfs-sidecar-example \
  --timeout=300s
```

## 验证挂载视图

在非特权工作负载容器中读取 SkillFS view。

```bash
export POD=skillfs-sidecar-example
export VIEW=/var/lib/skillfs/shared/mount/skills

kubectl -n "$NS" exec "$POD" -c agent -- ls -1 "$VIEW"
kubectl -n "$NS" exec "$POD" -c agent -- \
  cat "$VIEW/skillfs-container-example/SKILL.md"
kubectl -n "$NS" exec "$POD" -c agent -- \
  cat "$VIEW/skill-discover/SKILL.md"
kubectl -n "$NS" exec "$POD" -c agent -- \
  cat "$VIEW/skillfs-container-reserve/SKILL.md"
```

目录应包含 `skillfs-container-example` 和 `skill-discover`，但不应直接包含
`skillfs-container-reserve`。`skill-discover` 输出应包含 `reserve` view 和
最后一条命令使用的绝对路径。Secondary skill 不出现在目录列表中，但可以通过
其中提供的路径读取。

## 验证 Sidecar 重启

```bash
kubectl -n "$NS" exec "$POD" -c skillfs -- \
  /bin/bash -c 'kill -TERM 1'
kubectl -n "$NS" wait \
  --for=condition=Ready pod/skillfs-sidecar-example \
  --timeout=300s
```

Pod 恢复 Ready 后，重新执行挂载视图验证命令。

## 使用自己的工作负载

按下面的步骤修改 `src/skillfs/deploy/kubernetes/20-pod.yaml`。

1. 将 `skill-source` 替换为自己的 PVC；
2. 删除示例 ConfigMap 和 `seed-example` init container；
3. 将 `SKILLFS_PROBE_FILE` 设置为挂载视图中在 mount 生命周期内始终可见的稳定非空
   文件；
4. 替换 `agent` 镜像和命令；
5. 保留 SkillFS mount 的 `Bidirectional` 和工作负载 mount 的
   `HostToContainer`。

工作负载 readiness probe 应读取有意义的 SkillFS 内容，不应只检查目录存在或运行
`skillfs --version`。

参考 manifest 在第一次 FUSE 读取失败后将 Pod 标记为 NotReady，并在连续两次
liveness 失败后只重启 SkillFS Sidecar。单次失败阈值意味着偶发 probe 超时也会立即
将 Pod 标记为 NotReady，高并发时可能触发流量切走。工作负载刻意不配置 liveness
probe。消费方遇到 `EIO` 或 `ENOTCONN` 后必须关闭失败的文件描述符，并在 Pod 恢复
Ready 后重新打开文件。

## 镜像配置

镜像默认值与参考 manifest 一致。Volume 路径或 probe Skill 不同时，在 Pod 中覆盖
对应配置。

| 变量 | 默认值 | 用途 |
| --- | --- | --- |
| `SKILLFS_SOURCE` | `/var/lib/skillfs/source` | 可写的物理 Skill source 根目录 |
| `SKILLFS_MOUNTPOINT` | `/var/lib/skillfs/shared/mount` | 共享 Volume 内的 FUSE mountpoint |
| `SKILLFS_DISCOVER_ROOT` | `/var/lib/skillfs/shared/mount/skills` | `skill-discover` 提供给读取方的可见根目录 |
| `SKILLFS_EXTRA_ARGS` | 空 | 追加给 `skillfs mount` 的参数，以空白字符分隔 |
| `SKILLFS_PROBE_FILE` | `skills/skillfs-container-example/SKILL.md` | Health probe 通过 FUSE 读取的稳定非空文件 |
| `SKILLFS_PROBE_TIMEOUT` | `5` | 每次 probe 读取的超时时间，单位为秒 |
| `RUST_LOG` | `info` | SkillFS 日志过滤规则 |

Preflight 要求 source 与 mountpoint 是两个不同的绝对路径，source 可写，
`/dev/fuse` 可以打开，同时能找到 `fusermount3`，并且 `/etc/fuse.conf` 含有
`user_allow_other`。`SKILLFS_SKIP_PREFLIGHT=1` 只用于调试，正常部署不要启用。

退出时，PID 1 形式的 SkillFS 直接收到 `SIGTERM` 并卸载 FUSE view。上一个进程没能
完成清理时，下一次 preflight 会删除配置 mountpoint 上残留的 FUSE mount。发现其他
文件系统时，preflight 会拒绝卸载，避免错误的 Volume 路径影响已有 mount。

## 故障排查

```bash
kubectl -n "$NS" describe pod "$POD"
kubectl -n "$NS" logs "$POD" -c skillfs
kubectl -n "$NS" logs "$POD" -c skillfs --previous
kubectl -n "$NS" get events --sort-by=.lastTimestamp
```

常见原因包括特权容器被策略阻止、`/dev/fuse` 缺失、mount propagation 配置错误、
probe 文件不可读或 source volume 为只读。

Preflight 失败时，容器日志会包含稳定的数字代码。代码 10 到 16 依次表示配置错误、
FUSE device、`fusermount3`、source 根目录、mountpoint、`fuse.conf` 和残留 mount
清理失败。

## 清理

```bash
kubectl delete namespace "$NS" --wait=true
```

`emptyDir` 无法跨 Pod 保留内容。Skill 变更需要持久化时请使用 PVC。
