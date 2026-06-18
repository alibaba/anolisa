---
name: skillfs-mount
description: >
  Set up, mount, unmount, and recover the SkillFS FUSE virtual filesystem
  on a local machine. Covers preflight checks, generating
  skillfs-views.toml, mounting (normal or in-place), post-mount health
  checks, and graceful unmount.
  TRIGGER when the user asks to mount or unmount SkillFS, generate or
  regenerate skillfs-views.toml, recover from a broken SkillFS mount, or
  verify whether SkillFS is currently mounted.
---

# skillfs 配置与挂载指南

## 主流程（5 步，按顺序执行；失败即停）

### STEP 1：预检 + 模式确认

```bash
SKILLS_DIR="<skills-dir>"                          # source 路径
MOUNT_POINT="$SKILLS_DIR"                          # 默认 in-place；normal 模式改成独立目录
MOUNT_ID=$(basename "$SKILLS_DIR")
PID_FILE="/tmp/skillfs-${MOUNT_ID}.pid"
LOG_FILE="/tmp/skillfs-${MOUNT_ID}-{pid}.log"      # mount 会把 {pid} 替换为实际 pid
```

硬门槛——任一失败立即停：

```bash
[ -d "$SKILLS_DIR" ]                                                            || exit
[ -n "$(find "$SKILLS_DIR" -maxdepth 2 -name SKILL.md -print -quit)" ]           || exit
! grep -Fq " $MOUNT_POINT " /proc/mounts                                         || exit

command -v skillfs       && skillfs --version
command -v fusermount3
```

**in-place mount 必须先与用户确认**：

- 用户未明确模式时：询问 normal（独立挂载点）还是 in-place（覆盖 `SKILLS_DIR`）
- 选 in-place 时复述：挂载后该目录仅显示主视图技能 + `skill-discover`；要恢复完整 source 必须先 unmount
- 未获明确同意 → 走 normal

---

### STEP 2：验证 skills

```bash
skillfs validate "$SKILLS_DIR"
```

返回非 0 → 按报错修 SKILL.md frontmatter 后**重跑这一步**，不要继续。

---

### STEP 3：生成 / 调整 skillfs-views.toml

先 dry-run：

```bash
skillfs classify "$SKILLS_DIR" --primary-count 8 --dry-run
```

确认无误后落地：

```bash
skillfs classify "$SKILLS_DIR" --primary-count 8
```

手动编辑 `$SKILLS_DIR/skillfs-views.toml` 时：每条 `skills = [...]` 字符串
必须与对应 SKILL.md frontmatter 的 `name:` 一字不差；改后重跑
`skillfs validate`；挂载期间改 views.toml 不生效，需 STEP 5 → STEP 4 重挂。

---

### STEP 4：挂载 + 健康检查

```bash
skillfs mount "$SKILLS_DIR" "$MOUNT_POINT" \
  --pid-file "$PID_FILE" \
  --log-file "$LOG_FILE" &
sleep 1
```

按顺序跑三项检查，全部通过才算成功：

```bash
kill -0 "$(cat "$PID_FILE" 2>/dev/null)" 2>/dev/null && echo "ok: process alive"
grep -Fq " $MOUNT_POINT " /proc/mounts                && echo "ok: mount registered"
ls "$MOUNT_POINT" | head                                # 应能看到主视图技能
```

任一失败 → 解析实际日志路径再看：

```bash
ACTUAL_LOG=$(printf '%s\n' "$LOG_FILE" | sed "s/{pid}/$(cat "$PID_FILE" 2>/dev/null)/")
tail -n 50 "$ACTUAL_LOG" 2>/dev/null \
  || echo "no log file — 重跑 mount 去掉 '&' 加 --foreground 直接看 stderr"
```

---

### STEP 5：卸载（**禁止 `kill -9`**）

```bash
kill -TERM "$(cat "$PID_FILE" 2>/dev/null)" 2>/dev/null
sleep 2

if grep -Fq " $MOUNT_POINT " /proc/mounts; then
  fusermount3 -u "$MOUNT_POINT"
fi

if grep -Fq " $MOUNT_POINT " /proc/mounts; then
  echo "still mounted; try: fusermount3 -uz $MOUNT_POINT"
else
  echo "unmounted ok"
  rm -f "$PID_FILE"
fi
```

---

## 可选：按使用频率定义主视图

仅在用户明确要求"按历史使用频次定主视图"时执行：

```bash
# openclaw 格式：默认 ~/.openclaw
python3 <skill_dir>/scripts/skill_usage_from_session_logs.py [--logs-dir <path>]

# copilot-shell 格式：默认 ~/.copilot-shell
python3 <skill_dir>/scripts/skill_usage_from_chat_logs.py [--logs-dir <path>]
```

按结果调整 STEP 3 的 `--primary-count`，或手工编辑 views.toml。

---

## 故障排查

| 错误 / 现象 | 解决方案 |
|------|----------|
| `Package fuse3 was not found` | `yum install -y fuse3 fuse3-devel` 或 `apt install -y fuse3 libfuse3-dev` |
| 挂载前 `/proc/mounts` 已含目标路径 | `fusermount3 -u "$MOUNT_POINT"`；仍失败再 `fusermount3 -uz "$MOUNT_POINT"` |
| `Transport endpoint is not connected` | `fusermount3 -u "$MOUNT_POINT"` 清残留；进程已死时直接 `umount` |
| skill 消失于列表 | 统一 SKILL.md `name:` 与 views.toml 字符串；改后重跑 `skillfs validate` |
| skill-discover 不完整 | 重跑 `skillfs classify --dry-run` 或手编 views.toml 后按 STEP 5 → STEP 4 重挂 |
| 修改 views.toml 后未生效 | views 在挂载时一次性加载，必须重挂 |
| `tail -f` 日志路径找不到 | `{pid}` 替换成实际 pid（见 STEP 4 的 sed 命令）；或加 `--foreground` 看 stderr |
