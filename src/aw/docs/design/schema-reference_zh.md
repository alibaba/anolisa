# AW Schema 逐项说明

[English](schema-reference.md) · [冻结 ADR](interface-freeze_zh.md) · [全部 Schema](../../schemas/)

本文逐项解释 21 个 Schema，说明字段为什么这样设计，以及校验能覆盖到哪里。完整类型、必填项和枚举见各节链接的 JSON 文件。[contracts.json](../../tests/fixtures/contracts.json) 提供合成测试样例，用来检查合同，不作为 Provider 的运行证据。

下文的冻结规则都需要评审接受后才生效。JSON Schema 检查单份数据的结构，[validation.rs](../../src/validation.rs) 和 [orchestration.rs](../../src/orchestration.rs) 检查记录之间的关系。记录是否来自真实执行，还需要原生集成提供可信证据。

## 1. common/v1 共享值域

对应文件为 [common-v1.schema.json](../../schemas/common-v1.schema.json)。该资源用 `$defs` 提供共享定义，供其他 Schema 引用，不单独承载业务消息。

| 字段组 | 含义与思考点 |
| --- | --- |
| `name`、`rule` | 有界、可打印的 ASCII 标识；规则名更严格。它们是受信注册表中的不透明名字，不是文件名、命令或下载地址 |
| `uint`、`digest` | 安全整数与 SHA-256 小写十六进制。摘要用于绑定字节，不代表签名或身份认证 |
| `plan_ref.plan_id/revision/digest/step_id` | 固定 Core 计划及其中一步；在 Invocation 和 Receipt 中原样关联，不由 Provider 决定顺序 |
| `schema_ref.id/digest` | 同时固定协议身份和资源文件的实际字节；防止相同名字指向不同合同 |
| `scope.environment_id/execution_context_id/actor_id` | 环境、执行上下文、行为主体分别标识。不能用 session ID 代替所有身份 |
| `scope.runtime_id/runtime_generation/binding_revision` | 标识进程实例及观测绑定版本，旧实例不能复用新实例控制或证据 |
| `scope.session_id/turn_id/tool_use_id/parent_context_id` | 原生会话和工具身份按实际可得信息映射；工具边界必须提供 turn/tool ID；子 Agent 使用父上下文关联 |
| `artifact.id/digest/content/media_type/origin/tool_name` | 一个 UTF-8 文本槽位。摘要针对原始文本字节，origin 说明来源而不证明可信性，tool_name 仅作上下文 |
| `finding.rule_id/category/severity/confidence/count` | 规则、风险分类、严重性、置信度、命中数量分离。count 是该 finding 的数量，不是已扫描字节 |
| `coverage.input_digest/input_bytes/scanned_bytes/complete/ruleset_ids/languages` | 绑定被检查文本和实际声明覆盖；规则版本与语言必须明确。不能由输入长度直接宣称扫描完整 |
| `evidence.source_id/record_id/digest` | 引用独立证据源和记录；正文不嵌入 Receipt。读取端需验证来源权限与记录摘要 |
| `meter.meter_id/unit/measurement_kind/method/value` | 计量项、单位、估算/观测/计费类型、方法版本与值分开。字节减少不能直接称为实际 Token 计费减少 |

`boundary` 标明调用发生的位置，`proof_boundary` 标明确认采用的位置。工具执行后的 `post_tool` 还可能经过其他转换，只有最终返回位置才对应 `final_tool_result`。同样，写入 `local_history` 只能证明本地历史发生了变化；要确认 `model_request` 中的内容，还需在构造请求时观察。远端消费情况超出了这两个位置的证明范围。

每份 AW JSON 文档最多 4 MiB，嵌套深度最多 32 层；对象键必须为 ASCII，数字只允许绝对值不超过 2^53−1 的整数。原始文本及序列化的原生参数放在字符串里，因此可保留浮点数、非 ASCII 原生键和原始排版。此限制适用于本基线的内联文本；更大或二进制资源需要新 artifact profile，不能静默截断。

## 2. context.projection.prepare 输入 v2

对应文件为 [context-projection-prepare-input-v2.schema.json](../../schemas/context-projection-prepare-input-v2.schema.json)。生产者为 adapter/Core，消费者为 Host/Provider。

`artifact` 固定本次要转换的文本槽位及原始摘要；`boundary` 说明触发位置。`constraints.allow_text_reencoding` 明确是否允许在受支持的文本 media type 之间转换；它不授权改动外围结构。`constraints.accepted_reversibility` 显式列举可接受的 `lossless`、`retrievable`、`unrecoverable`，并且必须是 adapter 支持集合的子集。

adapter 只提取要处理的文本槽位，错误状态、stderr、图片和其他结果块保留原样。Core 显式传入恢复策略，Provider 必须遵守。调用方未接受 `unrecoverable` 时，即使摘要更短，也不能返回不可恢复的结果。

校验器分别核对原始 UTF-8 字节的摘要和完整输入文档的 input_digest，并检查 invocation 的输入预算。adapter 还要证明，提取的文本槽位确实来自这次原生工具调用。

## 3. context.projection.prepare 输出 v2

对应文件为 [context-projection-prepare-output-v2.schema.json](../../schemas/context-projection-prepare-output-v2.schema.json)。输出只有 `candidate`，没有 `adopted` 或可直接累计的节省计数。

| 字段 | 解释 |
| --- | --- |
| `source_artifact_id/source_digest` | 固定候选针对哪一份源文本，防止跨调用替换 |
| `content/media_type` | 准备好的候选表示；media type 必须在明确许可的转换范围内 |
| `transform_chain` | 按顺序记录版本化变换名称，不是任意可执行插件路径 |
| `reversibility` | 恢复保证的类别；必须满足输入要求 |
| `recovery.mode=self_contained/decoder_id` | 候选可独立解码，使用经过协商的 decoder 版本 |
| `recovery.mode=external/resolver_id/reference/source_digest/expires_at_ms` | 通过 resolver 和不透明引用恢复，绑定原始摘要及有效期 |
| `recovery.mode=none` | 仅可与显式接受的 `unrecoverable` 对应 |

采用 lossless 或 retrievable 候选前，可信调用方要独立解码或解析引用，再核对恢复字节与原文是否完全相同。当前库检查调用方提供的回读结果，decoder/resolver 由接入层运行。外部引用在采用时必须有效；采用后的保存期限由 resolver 合同约定，本次没有永久恢复保证。

空候选可以作为待处理的 Provider 输出，但校验器禁止把空文本标记 adopted，以免意外清空工具槽位。候选不比原文短时可以保留原文；即使确实采用更长候选，派生节省也不能为负或伪装为正。

## 4. security.content.inspect 输入 v2

对应文件为 [security-content-inspect-input-v2.schema.json](../../schemas/security-content-inspect-input-v2.schema.json)。输入为 `artifact`、`boundary` 和 `constraints.include_low_confidence`。

该能力检查内容风险；它不猜测执行语言，也不自动阻断运行。`include_low_confidence` 是调用方请求的报告策略，不能被解释成跳过输入摘要或完整性校验。允许 pre/post-tool 位置，边界需同时被 adapter 和 Provider 支持。

检查过程保持原文不变。后续策略根据结果决定是否提示用户，也可以另行安排脱敏或阻断。当前校验器核对结果是否自洽，不重新运行检测算法或判断规则本应命中哪些内容。

## 5. security.content.inspect 输出 v2

对应文件为 [security-content-inspect-output-v2.schema.json](../../schemas/security-content-inspect-output-v2.schema.json)。`inspection` 包含 `verdict`、`findings`、`coverage`。

`clean` 要求无 findings 且声明完整覆盖；`suspicious`、`sensitive` 要求至少一个 finding。coverage 中 input_digest 和 input_bytes 必须匹配输入，scanned_bytes 不超过输入，complete 当且仅当扫描字节数等于输入字节数。内容检查的 languages 必须为空；ruleset_ids 非空。

校验器能拒绝部分扫描却报告 clean 的矛盾记录。部分扫描可以报告已发现的风险，但要报告 clean，就必须声明完整覆盖。扫描器是否真正检查了这些字节、规则集是否正确，仍由组件测试验证。

## 6. security.code.inspect 输入 v2

对应文件为 [security-code-inspect-input-v2.schema.json](../../schemas/security-code-inspect-input-v2.schema.json)。输入为 `artifact`、`boundary` 和 `constraints.language`。

语言限定为 `bash`、`python`、`auto`。选择 `auto` 时必须同时进行 bash 和 python 检查，避免因语言判断不明而漏扫。新语言需要通过新的能力版本或 profile 加入；当前版本会拒绝未知语言。

能力检查代码文本，不声称调用工具的最终参数已经绑定。需要控制派发时应使用 command inspect 与 execution intent。

## 7. security.code.inspect 输出 v2

对应文件为 [security-code-inspect-output-v2.schema.json](../../schemas/security-code-inspect-output-v2.schema.json)。`inspection` 的 verdict、findings 和 coverage 形状与内容检查一致。

`coverage.languages` 记录实际声明的检查范围。请求 bash 或 python 时，结果只填写对应语言；请求 auto 时必须包含两者，可以交换顺序，但不能重复。scanned_bytes 按被覆盖的文本字节计算，两种扫描器检查同一份文本，也不会让这个值变成输入长度的两倍。

规则计数不等于覆盖字节计数；两个扫描器产生的 finding 可以独立报告，但不能由 findings 数量推导 complete。算法是否真正运行仍要通过 Provider 兼容性测试证明。

## 8. security.command.inspect 输入 v2

对应文件为 [security-command-inspect-input-v2.schema.json](../../schemas/security-command-inspect-input-v2.schema.json)。固定在 `pre_tool` 边界。

`command.content/digest/language/tool_name` 表达从原生参数提取的命令文本；`execution_intent_digest` 绑定完整执行意图，覆盖命令之外的上下文。前一个摘要针对扫描文本，后一个摘要覆盖完整执行意图，包括原生参数和执行目标。

命令文本检查通过后，cwd、环境、执行器或目标仍可能变化，因此还要绑定完整意图。adapter 负责从原生参数中提取正确的命令字段，并在最终派发时重新取得当前意图。这两个映射都需要单独测试。

## 9. security.command.inspect 输出 v2

对应文件为 [security-command-inspect-output-v2.schema.json](../../schemas/security-command-inspect-output-v2.schema.json)。`decision` 包含 verdict、findings、coverage、reasons 与原样返回的 execution_intent_digest。

`allow` 要求完整覆盖、无 findings、无 reasons；`warn`/`deny` 要求 findings 和 reasons 非空，本 profile 的所有决策都要求完整覆盖。无法完成扫描时返回失败 Receipt，不能合成 allow。实际语言覆盖与输入约束一致。

执行器需要结合完整计划使用这份 decision，单独转交检查结果不能授权执行。`validate_execution_gate` 仅检查单次结果；完整派发必须使用 `validate_dispatch`，同时校验全部计划步骤和独立 OS 防护。`validate_execution_gate` 只接受 allow、相同且未过期的最终意图，以及与 invocation 相同的 scope。warn 需要重新进入明确的策略/审批流程，本库不会把 warn 自动放行。最终执行器必须在同一受控派发边界完成比较与执行，避免检查后再修改。

## 10. boundary-descriptor/v1 原生能力边界

对应文件为 [boundary-descriptor-v1.schema.json](../../schemas/boundary-descriptor-v1.schema.json)。由 adapter 发布，Core 在经过可信配置确认后使用。

`adapter_id/adapter_version/boundary_id/revision` 绑定具体实现与边界版本；`boundary` 定位事件。`invocation_mode` 区分 awaitable、synchronous、observe_only。`can_replace_text`、`can_deny_dispatch`、`has_final_input_guard` 分别声明替换、阻断和最终输入守卫，不能互相推导。

`proof_boundaries` 列出确实可观察的位置。`ledger_policy` 是 required_before_delivery 或 best_effort；前者必须有可观察交付边界，纯观察入口不能声明它。`media_types` 与 `reversibility` 列出文本处理能力。

`composition.input_finality` 区分 immutable_until_dispatch、revalidate_at_dispatch、uncontrolled；最终守卫不能绑定 uncontrolled 输入。`composition.gate` 声明 required_final_guard 或 none，必须与 has_final_input_guard 一致。`composition.result_finality` 区分 final 与 subject_to_later_change；宣称 final_tool_result 证明必须处在 final 位置。这些声明由 adapter 的原生顺序测试证明。

观察器若声明修改、阻断或交付前记账，语义校验会拒绝；没有最终守卫也不能声明阻断能力。每个 adapter 还需要运行原生测试，检查执行顺序及后续覆盖，并验证取消和异常路径。仅凭框架名称无法确认这些权限。

## 11. runtime-binding/v1 运行实例观测

对应文件为 [runtime-binding-v1.schema.json](../../schemas/runtime-binding-v1.schema.json)。由原始 launcher 或可信附着管理器发布。

`runtime_id/generation` 标识进程实例，绑定发生变化时更新 `binding_revision`。`sequence` 则记录递增的观测序号。`environment_id` 确定作用环境。`process_ref` 是原生管理器解析的不透明引用，避免把裸 PID 冻结进跨平台合同。`observation_source` 区分 owned_child 与 external_attachment，但 external_attachment 本身不授予所有权。

`owner_id` 指向控制权的原始权威。state 是 running、suspended、exited 或 detached。可选 session_id 表示这个绑定专属于某个会话；存在时必须匹配调用 scope，不存在时可由原生管理器将多个执行上下文关联到同一 runtime。可选 workspace_id 和 workspace_generation 必须成对提供。

Core 准入只接受最新的 running 绑定。原生管理器识别进程实例，防止 PID 复用造成误认，并负责持久化事件和维护递增序号。本库不读取或监听进程。已经退出的实例不能通过 resume 恢复，重新启动必须使用新代际。

## 12. control-grant/v1 控制授权引用

对应文件为 [control-grant-v1.schema.json](../../schemas/control-grant-v1.schema.json)。原始所有者向明确 holder 授予有限动作。

`grant_id/issuer_id/holder_id` 分离凭据身份、签发方和被授权者。`runtime_id/runtime_generation/binding_revision` 防止把旧授权用于新绑定。`actions` 仅包含 stop/resume，`expires_at_ms` 限制有效期。stop 对 running 有效；resume 对 suspended 有效，不能恢复 exited 或 detached 实例。

这份合同描述授权声明，凭据格式由认证层决定。调用方先认证 issuer，并确认它是当前 owner；校验器随后核对 holder 和目标实例，再检查动作与有效期。读取进程事件或复制声明都不会获得控制权。当前动作只有 stop/resume，暂停、重启和迁移需要新的版本或独立能力。

## 13. execution-intent/v1 最终执行意图

对应文件为 [execution-intent-v1.schema.json](../../schemas/execution-intent-v1.schema.json)。由原生执行器在最终参数确定后构造。

| 字段组 | 冻结理由 |
| --- | --- |
| `intent_id/scope` | 绑定本次意图与调用上下文，避免复用其他会话或主体的安全结果 |
| `tool_name/tool_definition_digest/executor_revision` | 工具名相同但实现或参数解释变化时，旧检查失效 |
| `arguments.content/media_type/digest` | 保存原生参数的精确序列化字节，不由 AW 重新排版或丢失数值精度 |
| `target_id/target_generation` | 固定实际目标实例；代际是不透明字符串，可容纳原生版本标识 |
| `working_directory_ref/environment_revision` | 固定 cwd 及实际使用的环境快照版本；引用必须由执行器解析并维持一致 |
| `expires_at_ms` | 限制结果可用于派发的时间窗口 |
| `protection_policy_digest` | 将必须生效的 OS 策略绑定到同一意图；策略变化使旧结果失效 |

意图摘要覆盖完整 AW 文档，而 arguments.digest 仅覆盖参数字符串字节。最终意图有任何字段变更都需重新检查。执行器必须确保同一环境 revision 始终对应同一受控环境。当前库只检查记录的一致性，检查与执行之间的竞态，即 TOCTOU，仍需原生执行器处理。

## 14. provider-descriptor/v1 Provider 能力目录

对应文件为 [provider-descriptor-v1.schema.json](../../schemas/provider-descriptor-v1.schema.json)。由可信 manifest 加载器/Host 构造。

`provider_id/provider_version/manifest_digest` 固定具体实现与 manifest。`driver/lifecycle` 是版本化的受信名称，协议未规定只能使用某一种传输或长期进程。`guarantee` 用 declared 与 enforced 描述保障状态，真实性由可信接入层认证。

`capabilities[]` 中每项包含 capability、authority、input_schema、output_schema、boundaries。authority 分为 observe、advise、mediate、enforce；不能从 Provider 的声明自动提升 adapter 的权限。每个 capability 在该 descriptor 中唯一；当前核心校验器按完整能力版本名及两个 Schema 引用选择路由。

原生 Provider 请求响应和启动参数仍由组件拥有。这里不复制它们的 Schema，也不规定通用插件安装或发现市场。未来新增 driver 的执行器属于 Host，实现前不得声称该 driver 已可用。

## 15. capability-invocation/v1 调用准入

对应文件为 [capability-invocation-v1.schema.json](../../schemas/capability-invocation-v1.schema.json)。由 Core 为一次能力调用构造。

| 字段组 | 含义与验证 |
| --- | --- |
| `plan_ref` | 引用已固定计划的 ID、revision、完整文档摘要和 step_id；实际调用必须属于该步骤选中的 Provider |
| `invocation_id/idempotency_key` | 本次调用身份与重试语义分开。幂等键必须由接收端持久化去重，不是允许盲目重试的开关 |
| `provider_id/provider_version/manifest_digest/capability` | 与受信 descriptor 精确匹配 |
| `scope/boundary_id/boundary_revision` | 与当前 runtime、原生边界关联；工具边界需 turn/tool_use 身份 |
| `policy_revision` | 记录已解析策略的版本；Core 保存和验证策略正文，本库不解释策略 DSL |
| `deadline_at_ms` | 成功结果的最晚完成时间，使用 Unix epoch 毫秒 |
| `budget.input_bytes/output_bytes/wall_time_ms` | 输入输出规范化文档的字节上限与单次执行时长上限，均为正整数 |
| `input_schema/output_schema` | 固定输入与预期输出合同，不接受仅名字相同的未知内容 |
| `input_digest/input` | 绑定完整能力输入文档，并再次校验其 artifact 或 command 字节摘要 |

调用外壳只保留各能力共用的信息，具体输入单独放在 payload 中。当前校验器支持四个已登记的 v2 能力，遇到未知能力会明确失败。共享幂等库和调度器尚未实现，预算的强制终止及身份认证也由运行时负责。

## 16. provider-receipt/v1 Provider 结果凭据

对应文件为 [provider-receipt-v1.schema.json](../../schemas/provider-receipt-v1.schema.json)。Host 在一次调用结束或变为不确定时生成，不能由侧栏拼接。

plan_ref 与调用中的计划和步骤引用完全相同，允许把 Provider 凭据关联到 Core 执行计划，但不代表计划全部完成。身份字段重复 invocation_id、provider/version/manifest、capability、scope、input_schema、input_digest，便于单独存储后仍能精确关联。`disposition` 区分 produced、bypassed、denied、failed、uncertain、effect_applied。produced 必须有 output 引用；失败/拒绝/不确定必须有 error_code 且无 output；bypassed 两者均无。effect_applied 需要独立 evidence，但当前四个只读/候选能力的语义校验拒绝它。

`output.schema/digest/bytes` 引用单独交付的完整输出文档，不把候选正文写进 Receipt。`meters` 保留单位、方法及计量性质；meter_id 不可重复，当前校验器不复算组件任意计量算法，消费者不能将未知指标自动累计为节省。`evidence` 引用独立记录。`started_at_ms/completed_at_ms` 必须有序；produced 必须满足 deadline 和 wall_time budget，晚到的失败仍可作为事实保留。

content-free 表示 Receipt 不包含原始输入和候选正文。Host 仍需控制 error_code 和标识的来源，避免 Provider 把敏感内容写入这些字段。Receipt 采用普通记录格式，签名认证需另行实现。采用、持久化及状态操作完成都需要独立证据，模型是否消费了内容也超出这份记录的证明范围。

## 17. context-adoption/v1 环境采用观测

对应文件为 [context-adoption-v1.schema.json](../../schemas/context-adoption-v1.schema.json)。由原生环境观测者产生，与 Provider 输出独立。

| 字段组 | 作用 |
| --- | --- |
| `invocation_id/scope/boundary_id/boundary_revision` | 保证采用来自同一次调用和同一原生边界版本 |
| `receipt_digest/source_digest/candidate_digest` | 分别绑定 Receipt 文档、原始文本字节和候选完整输出文档；没有候选时不填 candidate_digest |
| `decision/reason` | adopted、preserved、overridden、unverified 与具体原因分开；候选可被环境拒绝，不能因此伪造 adopted |
| `effective_digest/effective_bytes` | 最终观测文本的实际字节摘要与长度；只有可验证决策可填写 |
| `proof.boundary/representation_id/revision` | 观测了哪一层、哪份原生表示及版本，不能仅靠 tool ID 猜测 |
| `proof.evidence/observed_at_ms` | 独立原生证据位置和观测时间，须在 receipt 完成后且不晚于检查时间 |
| `ledger_status/ledger_evidence` | committed 必须带独立确认引用；unavailable 不带。调用方验证确认来源与持久性 |

adopted 必须等于候选文本并满足恢复条件；preserved 必须等于原文，允许 environment_rejected、recovery_unavailable 等真实原因；overridden 表示之后的变换，不归因节省；unverified 不得虚构 effective 或 proof。无节省原因需符合本基线的字节长度口径。

Ledger evidence 引用存储层独立返回的 append 确认，避免对包含确认字段的文档计算循环摘要。写入者需要先实现原子追加与确认协议，再把可信确认交给调用方。JSON 字段本身不提供事务保证。required_before_delivery 不允许在 Ledger 不可用时采用候选，best_effort 可如实记录已观测采用但不计入已记账节省。

`validate_plan_adoption` 先校验全计划，再检查本次采用引用的 Invocation、Receipt、output 确实在该计划中；计划取消或回退时不能将候选标为 adopted。底层 `validate_adoption` 按字节计算节省。已验证 adopted 且 Ledger committed 时取 `max(源字节−实际采用字节, 0)`；已确认保留或后续覆盖为 0；未验证或 Ledger 不可用为未知。聚合必须按 invocation、proof boundary、representation revision 去重，并分别展示不同观测层，不能将本地历史与模型请求的节省相加成两次节省。

## 18. operation-record/v1 持久化状态操作

对应文件为 [operation-record-v1.schema.json](../../schemas/operation-record-v1.schema.json)。面向可能影响外部状态的操作，由环境执行器及持久化协调者维护。

`operation_id/capability/scope` 固定操作身份。`resource_id/resource_generation/expected_revision` 固定目标及执行前版本，代际和版本是不透明字符串。`input_digest/idempotency_key` 固定完整操作输入及去重身份。`approval_ref` 指向与同一操作、目标、输入及有效期绑定的受信审批，不能复用无关审批；审批正文及其认证未在本合同中定义。

`state/sequence/evidence` 记录操作的生命周期，正常执行顺序为 prepared → approved → started → succeeded/no_effect/uncertain。准备或已批准阶段也可在有证据时收敛为 no_effect。uncertain 只能经查询更新证据或收敛为 succeeded/no_effect，不能直接进入 started 再做一次。终态不可修改；完全相同的记录只表示幂等读取，不能授权再次执行。

校验器拒绝变更身份、目标、输入或既有 approval_ref，序号必须单步增加；存储层负责 compare-and-swap、审批真实性、先持久化 started 再执行副作用以及崩溃恢复。no_effect 需要证据，不能把网络断开当成“确定没有变化”。

快照范围和排除规则需要由后续状态能力 profile 定义，恢复预检、文件摘要及 create/restore 输入输出也留在该 profile 中。本 Schema 只提供通用操作记录，当前分支尚未实现状态 Provider。

## 19. capability-plan/v1 Core 拥有的有序能力计划

对应文件为 [capability-plan-v1.schema.json](../../schemas/capability-plan-v1.schema.json)。生产者为可信 Core，消费者为调度实现、最终执行器和审计者。Core 在这里明确每一步的能力、Provider 选择方式和失败策略，Agent Loop 与原生事件机制继续由框架维护。

| 字段组 | 含义与取舍 |
| --- | --- |
| `plan_id/revision/event_id` | 固定一次边界事件的计划身份；event_id 由 adapter 映射，不由插件分别生成互不相干的事件 |
| `scope/boundary_id/boundary_revision/boundary` | 绑定原生环境和位置，调用前与受信 descriptor 校验 |
| `policy_revision/source_digest` | 固定已解析策略及原始文本字节；完整计划摘要包括路由、顺序及所有策略选择 |
| `steps[].step_id/capability/input_schema/output_schema` | 有序步骤与精确能力合同；step_id 在计划内唯一 |
| `steps[].selection/providers` | exactly_one 或 all_distinct_providers；固定 provider_id/version/manifest_digest，拒绝重复身份和未选择的实现 |
| `steps[].required/on_failure` | 区分必需检查和可选事实；失败可 reject_plan、record_gap_and_continue 或 deny_dispatch，不能由 Provider 自行降级 |
| `steps[].input_source` | 初版固定 boundary_source，所有检查绑定原文，不隐式复用其他步骤输出 |
| `os_requirement.policy_digest/required_controls` | pre_tool 必填、其他边界不填；固定 OS 策略及必须实际覆盖的版本化控制项 |

命令检查必须位于 pre_tool、required=true 且 on_failure=deny_dispatch；计划至少包含一个命令检查，原生边界必须有可阻断的最终守卫。投影不能位于 pre_tool，必须 exactly_one、reject_plan，并且是最后一步，因此源检查先于候选生成，一个计划最多一次投影。未知能力显式拒绝。[完整执行前计划样例](../../tests/fixtures/pre-tool-plan.json) 展示了两项有序必需检查及独立 OS 要求；对应的派发反例见 [orchestration.rs](../../tests/orchestration.rs)。样例为合成合同数据。

all_distinct_providers 汇总本步全部已选择结果；任一 command 不是 allow 都拒绝派发，结果不能按“最后返回值”覆盖。步骤间串行；同一步的只读检查不依赖 Provider 返回顺序。路由为空时通过 execution 记录 gap，必需步骤据失败策略终止。content/code 是观察事实，required 不等于自动把所有 finding 升级为阻断。任意多阶段变换或新的风险解释规则需要显式 profile 评审。

## 20. plan-execution/v1 完整计划执行结果

对应文件为 [plan-execution-v1.schema.json](../../schemas/plan-execution-v1.schema.json)。由 Core 的可信日志写入者产生，不能由某一个 Provider 宣称全计划成功。

`plan_id/revision/plan_digest/event_id/scope/boundary_id/boundary_revision` 关联完整计划。steps 数量和顺序必须与计划完全一致。每项包含 step_id、outcome、invocations，以及已开始步骤的 started_sequence/settled_sequence；非 completed 还需 reason。completed 表示获得所选 Provider 的可用结果，gap 表示缺少可用结果，cancelled 表示取消，skipped 表示此前计划已经终止而本步未启动。

invocations 只引用真实 invocation_id 与 receipt_digest。没有调用的步骤不虚构 Receipt。重复 invocation_id、同一 Provider 重复使用 idempotency_key、漏掉已选择 Provider、跨步骤复用 Receipt、伪造 completed 均被拒绝。调用前仍必须通过 validate_invocation 校验 descriptor 的对应关系并检查 runtime/deadline；身份认证由调用方负责，结果校验不替代准入。

序号来自同一 Core 的单调日志，started < settled 且前一步 settled < 后一步 started。skipped 没有开始/结束序号，原因固定 previous_step_stopped，不能假装已经执行。计划终态后不得继续运行后续步骤；正常结束必须用 proceed、deny、preserve 或 cancelled 与实际结果匹配。deny 和 warn 不会被后续 allow 覆盖；投影失败导致 preserve，并不宣称工具执行失败或回滚。

`evidence` 引用持久化日志。记录完整性、日志认证、计划终态不可覆盖、最终派发按 event_id 仅领取一次，均由 Core/执行器的存储和原子边界保证。当前校验器检查提供的记录是否一致，不实现调度器、持久化或 next() 包装器。

## 21. os-protection-binding/v1 独立 OS 防护绑定

对应文件为 [os-protection-binding-v1.schema.json](../../schemas/os-protection-binding-v1.schema.json)。由经过认证的系统保护权威发布。它记录已经建立的防护。安装动作由系统管理器执行，Provider 无权通过这份记录为自己授予信任。

| 字段组 | 解释 |
| --- | --- |
| `binding_id/scope/target_id/target_generation` | 关联真实运行实例、执行上下文和目标代际；避免旧进程的保护被用于新实例 |
| `policy_digest/authority_id` | 固定实际受控策略及权威身份；必须与 plan、intent 和调用方预配置的权威一致 |
| `state` | active、unavailable、failed；后两者不能用于满足必需防护 |
| `controls[].control_id/mechanism/coverage` | 控制项、实现机制与 enforced/declared/unsupported 分开；只有匹配的 enforced 可以满足必需项 |
| `observed_at_ms/expires_at_ms` | 使用可信时钟检查有效期；调用方须向权威取得当前状态，不能依赖旧缓存 |
| `evidence` | active 必须提供独立生效证据，读取端验证来源和摘要；声明本身不是内核证明 |

必需控制项来自计划，例如 filesystem.access/v1 与 network.egress/v1；确切访问规则由 policy_digest 对应的受信策略定义。重复控制 ID、覆盖部分必需项、未生效、错误目标或代际、过期绑定均拒绝。OS 控制直接约束资源访问，不依赖普通 hook 是否继续调用 next；AW 的 allow 无权解除限制。保护权威必须与不可信插件隔离。

validate_os_protection 是单项绑定校验，最终应调用 validate_dispatch 校验整个计划、意图及防护。内核规则语法、权限安装协议和 OS 拦截日志格式需要后续设计，实机防护也尚未验收。现有 Provider 继续使用原生协议，接入系统防护需要另行实现。

## 编码、版本与审核顺序

AW JSON v1 采用本项目定义的受限编码规则，未实现 RFC 8785 的完整规范。对象按 ASCII 键排序，数组保序，使用无额外空白的 UTF-8 JSON，字符串不做 Unicode 规范化。拒绝重复键、浮点表示、指数表示、负零、非 ASCII 元数据键、非法 Unicode 和超限文档。网络入口必须先用严格解析器；普通解析器一旦丢失重复键，之后的 Schema 校验无法找回歧义。

原始 content 摘要使用原始 UTF-8 字节；input/output/receipt/intent 的文档摘要使用上述规范化字节；schema_ref 摘要使用仓库 Schema 文件的精确字节。这三类摘要不能混用。资源 URI 是身份，不代表已部署可下载端点；registry 只使用随包资源。

建议评审先确认权威和采用边界，再确认能力输入输出，最后确认编码及兼容策略。需要评审确认的参数包括 4 MiB/32 层上限、ASCII 元数据域、auto 的双语言语义、恢复引用有效期与更长期保留的分工、时间预算口径和 operation 状态机。当前实现已明确这些参数，评审者可以逐项确认或修改。
