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
| config | emptyDir | `/etc/ws-ckpt` —— 全局配置面。CLI 写 `config.toml`，daemon 重载同一份文件。**不共享则所有 `config --global` 设置对 daemon 无效。** |

## 关键约束

1. **`privileged: true`** — losetup/mount/mkfs.btrfs 需要。

2. **hostPath `/dev` 透传** — k3s containerd 不会自动向 privileged 容器暴露宿主机 loop 设备节点，必须显式挂载。

3. **`/var/lib/ws-ckpt` 必须持久化**（hostPath 或 PVC）。btrfs 镜像路径不可配置。若此目录落在容器可写层，pod 重启后 img 消失，但 orphan loop/mount 泄漏到宿主机。

4. **`--mount-path` 必须指向一个两容器同路径挂载的共享 volume**（mount anchor），daemon 侧配 `mountPropagation: Bidirectional`，app 侧配 `HostToContainer`。不能使用容器 rootfs 里的普通目录，那样 overmount 只存在于 daemon 容器私有的 mount namespace 里，无法传播到 app 容器，`init` 之后 app 会拿到悬空 symlink，而 daemon 侧无任何报错。

5. **workspace 必须是共享 volume 的子目录**，不能直接使用 volume 挂载点本身。`ws-ckpt init` 会对 workspace 路径执行 `rename()`，对挂载点 rename 返回 EBUSY。

6. **`shareProcessNamespace: true`** — 使 `fuser -m`（rollback/recover/img-shrink 判断挂载是否空闲时调用）能看到 app 容器的进程。

7. **后端自动检测**：若宿主机 `/var/lib` 在原生 btrfs 上，`auto_detect` 选择 BtrfsBase（直接操作 subvolume，无需 loop 设备）。BtrfsLoop overmount 路径仅在 ext4/xfs 宿主机上激活。两种后端均可正常工作，无需 ConfigMap 强制指定。

8. **`/etc/ws-ckpt` 必须两容器共享**。全局配置是文件面而非 socket 面：`ws-ckpt config --global` 由 CLI 直接写 `/etc/ws-ckpt/config.toml`，daemon 重载时读**它自己容器内**的同一路径。两容器各用镜像层时，daemon 读不到文件即回落内置默认值，`auto-cleanup` 等策略全部失效。CLI 写入后会比对 daemon 的 effective 配置并报错，但根治方式是把 `/etc/ws-ckpt` 挂成共享 volume（`emptyDir` 即可）。

   per-workspace 配置（`config -w`）走 daemon 状态，不受此约束。

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
          own_loops=$(losetup -j /var/lib/ws-ckpt/btrfs-data.img 2>/dev/null | cut -d: -f1)
          umount /opt/ws-ckpt-mount 2>/dev/null || true
          for dev in $own_loops; do
            losetup -d "$dev" 2>/dev/null || true
          done
          losetup -ln -O NAME,BACK-FILE | while read dev file; do
            case "$file" in
              "/var/lib/ws-ckpt/btrfs-data.img (deleted)"|"/data/ws-ckpt/btrfs-data.img (deleted)")
                losetup -d "$dev" 2>/dev/null || true ;;
            esac
          done
```

两条规则对应两类遗留，且都**不能用路径存在性（`-e`）作为判断依据**：

- 本 Pod 自己的 loop 在 umount **之前**用 `losetup -j` 捕获——它按 backing 文件的**（设备, inode）**匹配。路径字符串相同不代表是同一个文件：旧 Pod 的 loop 可能仍挂着已删除的旧 inode，而新 Pod 已在同一路径创建了另一个文件，用 `-e` 检查会把两者混淆；
- 历史孤儿按内核显式追加的 ` (deleted)` 后缀识别：只有 backing 文件已被 unlink 的 loop 才带该后缀，运行中容器的设备不会命中；`/data/ws-ckpt` 为旧版本遗留镜像路径，一并覆盖。

若 preStop 运行时 daemon 仍持有挂载，umount/detach 会以 EBUSY 失败——这是预期内的最好努力，下次启动的孤儿恢复（同样按 inode 匹配）会兜底。

对于 preStop 无法执行的异常场景（OOMKill、节点故障）：daemon 下次启动时 `try_exists` 检测到 orphan loop 设备（backing file 已不存在），跳过陈尸挂载走冷路径重新 bootstrap。此场景下旧挂载中的数据不可恢复 — 持久化 `ws-state` volume 是防止此情况发生的根本保障。

裸 `docker run`（没有 preStop）每次容器退出都会遗留一个 attached loop 设备，反复运行会持续累积，建议：

- 给 `/var/lib/ws-ckpt` 挂持久卷。镜像文件跨运行保留后，下次启动的孤儿恢复能按 inode 匹配到上次遗留并自动 detach，不再累积，状态也得以保留；
- 清理存量遗留前先确认没有 ws-ckpt 容器在运行。只处理 ws-ckpt 两个镜像路径（默认 `/var/lib/ws-ckpt`、旧版本遗留 `/data/ws-ckpt`，daemon 与卸载流程对两者均保留支持）的悬空设备，**不要对全部 loop 设备做扫描**——宿主机上其他服务的 loop 设备同样可能存在悬空 backing 路径，全量扫描会误杀：

```bash
sudo sh <<'CLEANUP'
losetup -ln -O NAME,BACK-FILE | while read dev file; do
    clean=${file%" (deleted)"}
    case "$clean" in
        /var/lib/ws-ckpt/btrfs-data.img|/data/ws-ckpt/btrfs-data.img) ;;
        *) continue ;;
    esac
    # "(deleted)" = backing inode 已随容器删除；文件不存在 = 陈旧 attach。
    # 宿主机级（systemd）部署的镜像文件真实存在，自动跳过。
    if [ "$file" != "$clean" ] || [ ! -e "$clean" ]; then
        losetup -d "$dev"
    fi
done
CLEANUP
```

  `losetup -d` 需要 root，故整段以 `sudo` 运行；detach 失败原样报错，不做重定向静默。

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
