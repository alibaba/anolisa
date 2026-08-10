# ws-ckpt K8s Sidecar 部署指南

## 前置条件

- 宿主机内核已加载 btrfs 模块（`modprobe btrfs && modprobe loop`）
- 宿主机 `/dev/loop-control` 和 `/dev/loop*` 可用
- 容器镜像包含：ws-ckpt 二进制、btrfs-progs、util-linux（losetup/mount/findmnt）、psmisc（fuser）、rsync、kmod

## 构建镜像示例

```bash
# 1. 编译 ws-ckpt（需要 Rust stable >= 1.78）
cd anolisa/src/ws-ckpt/src
cargo build --release --workspace
ls target/release/ws-ckpt

# 2. 准备构建上下文
mkdir -p ~/wsckpt-image
# 拷贝编译好的二进制可执行文件
cp target/release/ws-ckpt ~/wsckpt-image/

# 3. 如果宿主机是源码编译的 btrfs-progs（dnf 无包），需要拷贝用户态工具：
cp /usr/local/bin/btrfs /usr/local/bin/mkfs.btrfs ~/wsckpt-image/
# 若 dnf 有 btrfs-progs 则 Dockerfile 里直接 dnf install 即可，跳过此步

# 4. 写 Dockerfile
cat > ~/wsckpt-image/Dockerfile <<'EOF'
FROM registry.cn-hangzhou.aliyuncs.com/alinux/alinux3:latest

RUN dnf install -y --setopt=tsflags=nodocs \
      util-linux psmisc rsync kmod which findutils \
    && dnf clean all

# btrfs-progs：若 dnf 有包则换成 dnf install btrfs-progs
COPY btrfs mkfs.btrfs /usr/local/bin/
RUN chmod 0755 /usr/local/bin/btrfs /usr/local/bin/mkfs.btrfs \
    && ln -sf /usr/local/bin/btrfs /usr/bin/btrfs

COPY ws-ckpt /usr/local/bin/ws-ckpt
RUN chmod 0755 /usr/local/bin/ws-ckpt
EOF

# 5. 构建 + 验证
cd ~/wsckpt-image
docker build -t localhost/ws-ckpt:smoke .
docker run --rm localhost/ws-ckpt:smoke ws-ckpt --version
docker run --rm localhost/ws-ckpt:smoke btrfs --version

# 6. 导入到集群节点（以 k3s 为例；标准 k8s 推送到 registry 即可）
docker save -o ws-ckpt-smoke.tar localhost/ws-ckpt:smoke
sudo k3s ctr images import ws-ckpt-smoke.tar
# 或推送到私有 registry：
# docker tag localhost/ws-ckpt:smoke your-registry/ws-ckpt:smoke
# docker push your-registry/ws-ckpt:smoke
```

## 部署

```bash
# 确保宿主机内核模块就绪
sudo modprobe loop && sudo modprobe btrfs

# 应用 Pod 清单（yaml 示例见 k8s-sidecar-example.yaml）
kubectl apply -f k8s-sidecar-example.yaml
kubectl get pod wsckpt-smoke -w          # 等待 2/2 Running
kubectl logs wsckpt-smoke -c daemon      # 确认 bootstrap 成功
```

完整 Pod yaml 示例见 [`k8s-sidecar-example.yaml`](../k8s-sidecar-example.yaml)。

## Pod 设计

单 Pod 双容器，共用同一镜像：

| 容器 | 角色 | 权限 |
|------|------|------|
| daemon | 运行 `ws-ckpt daemon`，管理 btrfs loop | privileged: true |
| app | 业务负载，通过 UDS 调用 CLI 与 daemon 通信 | 无特权 |

## 必需 Volume

| 名称 | 类型 | 用途 |
|------|------|------|
| sock | emptyDir | UDS socket（`/run/ws-ckpt/ws-ckpt.sock`） |
| dev | hostPath `/dev` | loop 设备访问（`/dev/loop-control`、`/dev/loop*`） |
| ws-state | hostPath `/var/lib/ws-ckpt` | **必须持久化** — 存放 btrfs-data.img、state.json、daemon.lock。丢失此 volume = 数据丢失。 |
| ws-ckpt-mount | hostPath（anchor 目录） | daemon 在此 overmount btrfs；daemon 侧 `mountPropagation: Bidirectional`，app 侧 `HostToContainer` |
| data | emptyDir | workspace 父目录；workspace 使用其子目录（如 `/data/workspace`） |

## 关键约束

1. **`privileged: true`** — losetup/mount/mkfs.btrfs 需要。

2. **hostPath `/dev` 透传** — k3s containerd 不会自动向 privileged 容器暴露宿主机 loop 设备节点，必须显式挂载。

3. **`/var/lib/ws-ckpt` 必须持久化**（hostPath 或 PVC）。btrfs 镜像路径不可配置。若此目录落在容器可写层，pod 重启后 img 消失，但 orphan loop/mount 泄漏到宿主机。

4. **`--mount-path` 必须指向一个两容器同路径挂载的共享 volume**（mount anchor），daemon 侧配 `mountPropagation: Bidirectional`，app 侧配 `HostToContainer`。不能使用容器 rootfs 里的普通目录，那样 overmount 只存在于 daemon 容器私有的 mount namespace 里，无法传播到 app 容器，`init` 之后 app 会拿到悬空 symlink，而 daemon 侧无任何报错。

5. **workspace 必须是共享 volume 的子目录**，不能直接使用 volume 挂载点本身。`ws-ckpt init` 会对 workspace 路径执行 `rename()`，对挂载点 rename 返回 EBUSY。

6. **`shareProcessNamespace: true`** — 使 `fuser -m`（rollback/recover/img-shrink 判断挂载是否空闲时调用）能看到 app 容器的进程。

7. **后端自动检测**：若宿主机 `/var/lib` 在原生 btrfs 上，`auto_detect` 选择 BtrfsBase（直接操作 subvolume，无需 loop 设备）。BtrfsLoop overmount 路径仅在 ext4/xfs 宿主机上激活。两种后端均可正常工作，无需 ConfigMap 强制指定。

## SIGTERM 与 Loop 设备清理

**daemon 收到 SIGTERM 只退出，不执行 `umount` 或 `losetup -d`。**

不做清理的后果：
- 宿主机级 btrfs 挂载在容器死亡后持续存在
- loop 设备保持 attached，backing path 指向已消失的容器 rootfs 内路径
- 反复重建 pod 会累积 stacked mount 并耗尽 `/dev/loop*` 设备

**缓解方案：** daemon 容器配置 `preStop` lifecycle hook：

```yaml
lifecycle:
  preStop:
    exec:
      command:
        - /bin/sh
        - -c
        - |
          umount /opt/ws-ckpt-mount 2>/dev/null || true
          losetup -ln -O NAME,BACK-FILE | while read dev file; do
            [ -e "$file" ] || losetup -d "$dev" 2>/dev/null
          done
```

对于 preStop 无法执行的异常场景（OOMKill、节点故障）：daemon 下次启动时 `try_exists` 检测到 orphan loop 设备（backing file 已不存在），跳过陈尸挂载走冷路径重新 bootstrap。此场景下旧挂载中的数据不可恢复 — 持久化 `ws-state` volume 是防止此情况发生的根本保障。

## 冒烟验证

```bash
# Pod 达到 2/2 Running 后：
kubectl exec wsckpt-smoke -c app -- mkdir -p /data/workspace
kubectl exec wsckpt-smoke -c app -- sh -c 'echo hello > /data/workspace/file.txt'
kubectl exec wsckpt-smoke -c app -- ws-ckpt init --workspace /data/workspace
kubectl exec wsckpt-smoke -c app -- ws-ckpt checkpoint --workspace /data/workspace --message smoke-v1
kubectl exec wsckpt-smoke -c app -- ws-ckpt list --workspace /data/workspace

# init 后 workspace 仍可正常读写（验证 symlink + btrfs 传播链完整）：
kubectl exec wsckpt-smoke -c app -- sh -c 'echo bug6-verify-$(date +%s) > /data/workspace/verify.txt'
kubectl exec wsckpt-smoke -c app -- cat /data/workspace/verify.txt
kubectl exec wsckpt-smoke -c app -- ls -la /data/workspace/
```
