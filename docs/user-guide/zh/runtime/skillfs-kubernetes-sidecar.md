# 在 Kubernetes 上以 Sidecar 运行 SkillFS

[English](../../en/runtime/skillfs-kubernetes-sidecar.md)

将 SkillFS 与 Kubernetes 工作负载部署在同一个 Pod 内。SkillFS 容器负责 FUSE
mount，工作负载保持非特权，只读取传播后的 SkillFS view，不直接挂载物理 Skill
source。

## 前提条件

- Kubernetes 1.29 或更高版本。
- Linux 节点提供 `/dev/fuse`。
- 允许 SkillFS Sidecar 以特权容器运行。
- 可使用 `docker buildx` 和 `kubectl`。
- 集群能够拉取目标 registry 中的镜像。

## 构建并推送镜像

请为目标节点架构构建镜像：

```bash
export IMAGE=registry.example.com/anolisa/skillfs-sidecar:0.4.0
export PLATFORM=linux/amd64

docker buildx build \
  --platform "$PLATFORM" \
  -f src/skillfs/container/Dockerfile \
  -t "$IMAGE" \
  --push \
  src/skillfs
```

ARM64 节点使用 `linux/arm64`。

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

从非特权工作负载容器读取 SkillFS view：

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

修改 `src/skillfs/deploy/kubernetes/20-pod.yaml`：

1. 将 `skill-source` 替换为自己的 PVC；
2. 删除示例 ConfigMap 和 `seed-example` init container；
3. 将 `SKILLFS_PROBE_FILE` 设置为挂载视图中的稳定文件；
4. 替换 `agent` 镜像和命令；
5. 保留 SkillFS mount 的 `Bidirectional` 和工作负载 mount 的
   `HostToContainer`。

工作负载 readiness probe 应读取有意义的 SkillFS 内容，不应只检查目录存在或运行
`skillfs --version`。

## 故障排查

```bash
kubectl -n "$NS" describe pod "$POD"
kubectl -n "$NS" logs "$POD" -c skillfs
kubectl -n "$NS" logs "$POD" -c skillfs --previous
kubectl -n "$NS" get events --sort-by=.lastTimestamp
```

常见原因包括特权容器被策略阻止、`/dev/fuse` 缺失、mount propagation 配置错误、
probe 文件不可读或 source volume 为只读。

## 清理

```bash
kubectl delete namespace "$NS" --wait=true
```

`emptyDir` 无法跨 Pod 保留内容。Skill 变更需要持久化时请使用 PVC。
