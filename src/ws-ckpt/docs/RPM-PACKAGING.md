# ws-ckpt RPM 打包说明

目标系统：Alinux 4（基于 RHEL/CentOS 系列）

## 快速打包

```bash
cd /root/code/ws-checkpoint
./build-rpm.sh
```

脚本会自动完成：编译 release 二进制 → 准备 RPM 构建目录 → 调用 rpmbuild 生成 RPM 包。

构建完成后 RPM 包会被复制到 `src/ws-ckpt/target/` 目录下。

> **前置依赖**：需要安装 `rpm-build` 包：`yum install -y rpm-build`

## 安装到系统

```bash
rpm -ivh ws-ckpt-0.1.0-1.x86_64.rpm
```

安装过程会自动：
- 将 `ws-ckpt` 二进制部署到 `/usr/bin/`
- 安装 systemd 服务文件到 `/etc/systemd/system/`
- 创建运行时目录（`/run/ws-ckpt`、`/data/ws-ckpt`、`/mnt/btrfs-workspace`）
- 执行 `systemctl daemon-reload` 并 `enable` 服务

## 打入 Alinux 4 镜像

### 方法 A：Packer 方式（推荐）

在 Packer 模板的 provisioner 中添加：

```json
{
  "type": "shell",
  "inline": [
    "rpm -ivh /tmp/ws-ckpt-0.1.0-1.x86_64.rpm",
    "systemctl enable ws-ckpt"
  ]
}
```

配合 `file` provisioner 先将 RPM 上传到实例：

```json
{
  "type": "file",
  "source": "src/ws-ckpt/target/ws-ckpt-0.1.0-1.x86_64.rpm",
  "destination": "/tmp/ws-ckpt-0.1.0-1.x86_64.rpm"
}
```

### 方法 B：Kickstart 方式

在 Kickstart 文件的 `%post` 段落中安装：

```
%post
# 假设 RPM 已放置在可访问的位置（HTTP 服务器或本地目录）
rpm -ivh http://your-repo-server/ws-ckpt-0.1.0-1.x86_64.rpm
systemctl enable ws-ckpt
%end
```

### 方法 C：直接修改镜像

```bash
# 1. 挂载镜像
mkdir -p /mnt/image
mount -o loop image.raw /mnt/image

# 2. chroot 安装
cp ws-ckpt-0.1.0-1.x86_64.rpm /mnt/image/tmp/
chroot /mnt/image rpm -ivh /tmp/ws-ckpt-0.1.0-1.x86_64.rpm
chroot /mnt/image systemctl enable ws-ckpt

# 3. 清理并卸载
rm /mnt/image/tmp/ws-ckpt-0.1.0-1.x86_64.rpm
umount /mnt/image
```

## 验证安装

```bash
# 检查服务状态
systemctl status ws-ckpt

# 查看帮助
ws-ckpt --help

# 检查已安装的 RPM 信息
rpm -qi ws-ckpt
```

## 卸载

```bash
rpm -e ws-ckpt
```

卸载时会自动停止并禁用 systemd 服务。

## 相关文件

```
ws-ckpt.spec               # RPM spec 文件（项目根目录）
build-rpm.sh                # 一键打包脚本（项目根目录）
src/ws-ckpt/systemd/       # systemd 服务文件
docs/RPM-PACKAGING.md       # 本说明文档
```
