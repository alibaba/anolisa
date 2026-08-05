# 弯路经验沉淀改为「成功经验 / 失败教训」两路

> 2026-08-05 · 影响 `crates/agentsight-opt` cost 维度 + Dashboard 优化页

## 背景

离线 skill `pr-cost/distill-experience-from-trajectories/criteria.md` 定义了一套经验沉淀
准入判据,产出**成功经验**与**失败教训**两路。产品侧的 cost 维度已经在做同一件事
(`prompts/detour.md` 的 `fix` 字段),但结构不同:它把经验塞进一个扁平结构的
两种"形状"里。

审计发现该结构有一半是死的:

| 层 | 试错型<br>`applicability/pitfall/effective_path` | 返工型<br>`rule/good_example/bad_example/scope` |
|---|---|---|
| LLM 输出契约 `detour.md` | 有 | **无** |
| LLM 输入类型 `DetourFix` | 有 | **无** |
| Rust 映射 `cost/llm.rs:303` | 填 | **`..Default::default()`** |
| 存储类型 `types.rs:881` | 有 | 有 |
| 前端类型 `optimization.ts:194` | 有 | 有 |
| 前端渲染 `TokenFlameChart.tsx:285` | 渲染 | 渲染,但永远是空 |

返工型从存储层往上全部建好,唯独没有生产者。与其补上这个分叉,不如换成
`criteria.md` 已经验证过的两路结构——语义更清晰,且两路的存在性各自独立。

## 非目标

- **不做跨会话经验库**。`optimization_results` 仍以 `session_id` 为主键,同一条经验在多个
  会话里会重复推导。`experience_library.rs:12` 的 accuracy/cost 双报 TODO 不在本次范围。
- **不做一键注入落地**。不写 agent 配置目录。前端保持"可复制文本"的现状。
- **不新增 LLM 调用**。两路都在现有的 detour 那一次调用里产出。

## 关键设计约束:保留 `turns` 锚

`criteria.md` 的两路是**轨迹级**的(每路最多 3 条,不绑定轮次)。detour 是**弯路段级**的,
且有一条硬约束(`cost/prompts/detour.rs:11`、`cost/llm.rs:265`):

> 步号只有一种口径:账本里的 `T{n}`。节省量由 Rust 按这些轮号从账本求和,不接受模型估算。

直接改成轨迹级会脱掉 `turns` 锚,失败教训将无法从账本求和节省量,只能让模型估 token——
这是被明确禁止的。

**因此:保留 finding → `turns` 的锚,只替换 `fix` 的内部形状。** 每个弯路段产出自己的两路;
轨迹级的条数上限由现有的 `MAX_DETOUR_FINDINGS = 5` 承担,不再单设 3 条上限。

## 设计

### 1. LLM 输出契约(`prompts/detour.md`)

`fix` 由扁平 5 字段改为两路嵌套:

```json
"fix": {
  "action": "一句话可执行修复动作",
  "locus": "Skill",
  "failure_lesson": {
    "title": "一句话点出这个坑",
    "when": "什么场景/触发条件下会踩",
    "instead": "别这么做 __,改这么做 __"
  },
  "success_playbook": {
    "title": "一句话点出这条做法",
    "when": "什么场景下适用",
    "how": "可执行的「这么做」"
  }
}
```

- `action` / `locus` 保留:`WasteItem.optimization` 与 `WasteExperience.fix_locus` 依赖它们。
- `failure_lesson` **必填**。填不出就整条 finding 不报(沿用"归因不出来的段不许报")。
- `success_playbook` **可选**(允许 null)。走通的回合不一定沉淀得出可复用做法——坑是环境抖动时
  就没有。宁缺毋滥。

### 2. 准入判据落点

`criteria.md` 的判据中,产品已有等价机制的不重复实现:

| criteria.md 判据 | 落点 |
|---|---|
| 会再遇到 | `when` 必填——填不出场景即为一次性细节 |
| 能改下次行为 | `instead` / `how` 必填且必须是动作句 |
| **非显然** | **本次新增**——prompt 显式排除"模型自带常识",唯一产品尚无的一条 |
| 代价/收益太小不沉淀 | 已有 `MIN_DETOUR_TURNS = 5` 机械门槛 |
| 不要凑数 | 已有 `MAX_DETOUR_FINDINGS = 5` + "宁漏报不误报" |

prompt 净增"非显然"一条,约 8 行。

`criteria.md` 的 `rounds_saved` **不引入成功经验**:成功经验没有可观测的弯路轮次可折算
(离线跑 614 条时该字段在成功经验侧大量退化为"无法估算")。失败侧的节省继续由 Rust 从
账本求和。

### 3. Rust 侧

`types.rs`:

- 新增 `ExperienceLesson (failure_lesson)` 与 `ExperiencePlaybook (success_playbook)`。
  `when` 与 Rust 关键字冲突,用 `#[serde(rename = "when")] pub when_: String`。
- `WasteExperience` 的 7 个形状字段替换为 `lesson: Option<ExperienceLesson>` +
  `playbook: Option<ExperiencePlaybook>`;三个归因字段(`defect_type`/`root_cause`/`fix_locus`)不动。
- `DetourFix` 的 `applicability`/`pitfall`/`effective_path` 替换为 `failure_lesson`/`success_playbook`。

`cost/llm.rs`(`expand_detour_items`)保留两条现有不变量:

- **偶发故障 → 两路都剥除**。现规则是剥 `fix`;新结构下 `failure_lesson` 与 `success_playbook` 一并剥除
  (随机故障既没有可绕开的坑,也没有可复用的路)。
- **节省量仍由 `ledger_tokens_for` 从账本求和**,`turns` 锚不动。
- **新增**:`failure_lesson` 缺失的 finding 整条不报。

### 4. 兼容与迁移

- **不 bump `schema_version`**。`WasteExperience` 是 `optimization.db` 中 `cost_waste`
  **JSON 列的内容**,不是 `agentsight.json` 的配置结构。列本身不变,`opt-store` 无需 `migrate()`。
  CLAUDE.md 的 schema_version 规则针对 `agentsight.json`,不适用于此。
- **历史数据降级为空,不 panic**。所有字段 `#[serde(default)]`,旧 JSON(试错型字段)
  反序列化后两路为 `None`,前端展开区不渲染经验块。旧结论重跑一次分析即可恢复。

### 5. 前端

`optimization.ts` 类型同步;`TokenFlameChart.tsx` 的 `experienceText()`(现按 7 字段线性拼,
`:278-291`)改为按两路分组渲染。`promptText()` 复用 `experienceText()`,复制能力自动继承。

### 6. 测试

| 用例 | 断言 |
|---|---|
| `方向选错` / `可预知坑` / `隐性规范` | `failure_lesson` 必有,`success_playbook` 可选 |
| `偶发故障` | 两路均被剥除,`optimization` 回落到候选默认值 |
| `failure_lesson` 缺失 | 整条 finding 不报 |
| 旧 JSON(试错型字段)反序列化 | 不 panic,两路为 `None` |
| turns 求和 | 沿用 `sums_savings_from_ledger_not_from_model`,节省量不变 |
| 轮数门槛 | 沿用 `short_or_unpriceable_findings_are_dropped` |

## 代码表面(Footprint Ladder)

**级别 1–2**:扩展现有函数 + 模块内替换类型,不新增模块文件、不新增 FFI、不新增探针、
不新增 API 端点、不改 DB schema、不加 LLM 调用。预计 diff 约 200 行(含测试),
远低于 800 行上限。
