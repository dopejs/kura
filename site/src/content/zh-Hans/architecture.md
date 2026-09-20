# Agent 插件化

决策日期：2026-08-17（操作员）。参考架构：
[deepseek-ai/deepseek-harness](https://github.com/deepseek-ai/deepseek-harness)
（"一切皆插件"）。本文是该计划的规划记录——新功能直接在这类设计文档中
规划并实现；编号规格流程（`docs/specs/NNN-*`）对新工作已停用，仅作历史
保留。

## 为什么

在继续构建 agent 能力（会话/上下文管理、检索、技能）之前，先把守护进程
重构为：每个非内核子系统都是一个**插件**——具名单元，声明依赖，可解析
启用状态，装配结果可检查。收益：

- **会话管理成为插件决策。** 纯个人 agent 运行长会话插件（何时压缩、什么
  常驻、什么写入记忆）；支持线程的 IM 频道运行每线程上下文插件。守护进程
  内核不再硬编码单一上下文策略。
- **既有功能成为默认插件**——可替换、可禁用，最终可由进程外提供方替代。
- **装配是配置产物**，而不是编译进去的口口相传：什么在运行、什么被禁用、
  为什么，都可以从 `/v1/plugins` 导出。

## 我们从 deepseek-harness 借鉴的

1. **Seam 模型**——一个能力 = 服务定义（接口）+ 提供方（实现）+ 消费者，
   三者一起设计。换掉提供方就同时换掉了所有消费者的行为。
2. **profile 分层装配**——运行中的守护进程是一棵已解析的插件条目树；任何
   条目都可按 id 禁用/配置；解析结果可导出。
3. **三类事件域**——持久事实（我们的事件总线 + 存储）、瀑布式拦截 hook
   （新增：`HookBus`，处理器可修改 payload 或中止动作）、能力事件。
4. **"模型可见 = 已记录"**——到达模型的一切都必须能从会话日志重建
   （随第二阶段的可挂 hook 的 agent 循环落地）。

## 有意的偏离

deepseek-harness 没有特权内核；我们保留一个。信任边界——**存储、事件
总线、身份、认证、策略、密钥、审计**——直接在 `App::with_profile` 中构建，
不能被插件禁用或替换。安全姿态优先于可组合性；这是固定决策。

## 两层模型（Rust 现实）

- **第一层——内置插件**（已交付，第一阶段）：编译进去的插件
  （`crates/surface/app/src/plugins.rs`），向内核 crate `kura-plugin`
  （`crates/foundation/plugin`）注册。没有动态加载（首个版本不值得承担
  dylib ABI 风险）。
- **第二层——进程外提供方**（第三阶段）：任何 seam 都可以通过已验证的平面
  ——adapter RPC、受监管的能力进程、MCP——远程提供，并可通过目录
  （`kind=plugin`）在运行时安装。语言无关、沙箱化、受监管。消费者分不出
  两层的区别。

## 机制（第一阶段，2026-08-17 交付）

- **`kura-plugin` 内核 crate**：`PluginDescriptor`（id、摘要、provides、
  requires）、`resolve()`（显式 + 传递性禁用，未知 id 产生警告——不会
  静默丢弃）、`PluginProfile`（`<data_dir>/plugins.json`；文件缺失 = 默认
  profile，格式错误让启动明确失败）、`SeamMap`（装配期的类型化注册表）、
  `HookBus`（瀑布式拦截：有序处理器，修改或中止）。
- **内置插件集**：llm、skills、sandbox、mcp、integrations、calendar、mail、
  providers、connectors、capabilities、chat、billing、activation、
  computer-use、delivery、scheduler、reminders、routines、memory、triage、
  webhooks、catalog、exec-profiles、evidence、evaluation、live-validation、
  setup-wizard、channel-{discord,telegram,slack,matrix}。声明的 `requires`
  边让失败即关闭的门保持诚实（例如禁用 `billing` 会传递性地禁用
  `webhooks`——配额门绝不会半装配运行）。
- **禁用语义**：被禁用插件的 `AppState` 字段保持 `None`；API 层已经会回答
  "未装配"。频道插件还额外门控它们在 serve 时的运行时构建（profile 优先于
  连接器配置开关；不触碰网络或凭据）。
- **自省**：`GET /v1/plugins` 返回启动时的 `AssemblyReport`（构建顺序、
  启用状态、原因、警告）。契约：`schemas/plugin/plugin-profile.schema.json`、
  `schemas/api/plugins-report.schema.json`；SDK `listPlugins()`。
- **profile 管理**（随配置界面加入）：`GET/PUT /v1/plugins/profile` 读取并
  原子替换 `plugins.json`（启动时生效；响应携带 `restartRequired: true`）；
  SDK `getPluginProfile()`/`updatePluginProfile()`；操作台有插件区
  （`/plugins`）——装配列表及启用/禁用开关、hook 注册、profile 警告和重启
  提示。

## 第二阶段——可挂 hook 的 agent 循环（2026-08-17 交付）

对话流水线（query 与 stream）现在运行三个内核 hook 点：

- `chat/turn-start`——提示词组装之前；hook 可以改写查询或中止（否决）。
  payload：`{tenantId, threadId, query, sourceKind}`。
- `chat/pre-dispatch`——完整上下文组装（技能、profile、连续性）之后、
  dispatch 准备与持久化之前；hook 可以改写 provider/model/messages 或
  中止。**这个顺序就是"模型可见 = 已记录"不变量**：dispatch 记录在 hook
  运行之后才创建，所以持久化的内容与提供方收到的字节完全一致（服务层测试
  与真实装配的端到端测试覆盖）。
- `chat/turn-end`——dispatch 结束且连续性持久化之后；观察性（中止只停止
  后续处理器）。
- `chat/tool-call`（第 9.0 阶段，2026-09-19）——在模型请求的工具被执行
  之前。payload：`{tenantId, threadId, agentProfileId, callId, name,
  arguments}`；hook 可以改写 `arguments` 或中止。否决会作为工具错误报告
  给模型（并记录为 `chat.hook.vetoed`），绝不静默丢弃，因此模型可以不用
  该工具继续作答。

工具调用（第 9.0 阶段，2026-09-20 合并）：一轮对话就是 `kura-core` 已有的
agent 循环，驱动在 dispatcher 之上——`chat/src/round.rs` 把一次 dispatcher
轮次做成一个 `ModelProvider`，因此每一轮都像过去的单次 dispatch 一样被
准备、挂 hook、持久化（`tools_json`、`tool_calls_json`）和发出事件。装配层
的 `ToolSource`（`crates/surface/app/src/tool_host.rs`）按轮次、按租户解析
注册表：已接入 MCP 服务器的工具、`memory.lookup`，以及每个 Ready 工具
profile 对应的一个工具；每个工具都被包装，使调用先经过 `chat/tool-call`、
记录为 `chat.tool.called` 事件并计数。循环受 `toolMaxRounds`
（`plugins.json` → `entries.chat.config`，默认 16）限制；触顶的一轮会被判定
失败，而不是用半个结果作答。

否决表现为 `ChatError::HookVetoed` → HTTP 403，并记录为 `chat.hook.vetoed`
事件。`GET /v1/plugins` 报告每一条 hook 注册（`hooks: [{point, pluginId}]`）。
上下文已经来自持久化记录（连续性轮次在每次 dispatch 时渲染为消息——
`derive_messages` 形态）；把这份日志推广为会话事件模型的工作并入
session-strategy 插件切片，它挂在 `chat/pre-dispatch`。

### session-strategy 插件（第一片 2026-08-17 交付）

`session-strategy` 内置插件（策略引擎：`crates/domains/session`）挂在
`chat/pre-dispatch`，确定性地整形已组装的窗口——在 dispatch 持久化之前，
所以整形后的窗口就是被记录的、也是模型看到的。机制：保留框架的省略——
系统消息（人设、技能、安全）是框架，绝不省略；最近的 `keepRecent` 条非
系统消息始终保留；超出预算的最早历史被替换为一行标记，指向线程连续性和
记忆平面。逐出在构造上是安全的：每一轮都在结束时独立于窗口捕获到 L0。

两种策略共享机制、只在预算上不同，按本轮的 `sourceKind` 区分：**personal**
（长会话默认，48k 字符）和 **thread**（`sourceKind=channel`，紧凑的 16k
字符——每线程一个上下文；线程范围本身来自连续性平面）。操作员通过 profile
条目配置
（`entries.session-strategy.config.{personalBudgetChars,threadBudgetChars,keepRecent}`）；
配置格式错误会让启动明确失败。禁用插件即恢复未整形的窗口。

压缩到记忆（第二片，2026-08-17）：逐出绝不直接丢弃内容。当本轮有线程时，
被省略的片段通过受治理的记忆流水线捕获为一条 L0 引用（线程来源链接、有界
摘录）——异步整合器在回复路径之外把它蒸馏进 L1/L2，写策略保持不变——省略
标记引用被捕获的资产（`…; elided span captured as Memory[l0_ref …]`），模型
可以回溯。无线程的轮次只能通过 dispatch 记录触达其片段。

后续切片：会话框架对象（显式的目标/约束记录）和频道原生的线程分段策略——
作为 [`agent-deepening-program.md`](agent-deepening-program.md) 的第 5 阶段
规划。

### context 插件（第一片 2026-08-17 交付）

`context` 内置插件（策略引擎：`crates/domains/context`）是默认的上下文
管理器，而其他插件按设计可以修改它的结果——它在 `chat/pre-dispatch` 瀑布中
最先运行，然后 `session-strategy` 整形窗口，之后任何内置/外部 hook 都可以
改写或否决。

它做的事是确定性的：把租户的记忆引导——先是 Ready 的 L3 人设，再是 Ready
的 L2 场景，最新优先（private/team 可见性；restricted/agent 等待绑定感知
装载）——作为系统框架消息注入，受 `entries.context.config.memoryBudgetChars`
（默认 4000，校验失败即明确报错）限制。每条注入的消息内联引用
（`Memory[l3 mem_xxx] title: content`）：召回的记忆是证据，而不是裸文本。
L1 原子绝不批量注入（仅下钻/检索，遵循设计根基）。每次装配都发出
`context.assembled` 事件，其 `AssemblyRecord` 列出纳入项（资产、层、字符数）
和带原因的排除项（`over_budget`/`empty_content`/`visibility`）——没有任何
内容悄悄进入或错过上下文，工程师可以重建任意 dispatch 中模型看到的记忆。
由于注入发生在 dispatch 准备之前，持久化的 dispatch 记录原样携带引导内容
（模型可见 = 已记录成立）。

按需检索（第二片，2026-08-17）：本轮最后一条用户消息通过 BM25 + 时近性
排序器召回 Ready 的 L1 原子，以倒数排名融合（k=60）合并。没有词法重叠的
原子绝不被召回（仅凭时近性不能把无关记忆拉进来）；融合后的前 8 个候选在
`retrievalBudgetChars`（默认 2000）内以 `Memory[l1 …] (recalled): …` 系统
消息注入，并合并进同一个 AssemblyRecord，`source: retrieval`。向量排序器
（第三片，2026-08-17）通过 `Embedder` seam 加入融合。默认提供方：确定性的
哈希字符三元组嵌入器（256 维 FNV 特征哈希，L2 归一化余弦）——一个字符级
向量空间，能召回基于词的 BM25 分词器错过的内容，尤其是 CJK
（`请用中文回复` 与 `中文回复偏好` 在三元组上匹配，却没有任何共享词
token）。候选范围扩大为 BM25>0 或 余弦 ≥ 0.25；低于阈值时哈希噪声绝不会把
无关记忆泄漏进上下文。神经网络嵌入提供方通过 seam 替换默认实现，无需改动
融合逻辑。

符号压缩（第四片，2026-08-17，`92191d5`）：超过 `refThresholdChars`（默认
8000）的非框架消息在 `chat/pre-dispatch` 外置——全文作为 L0 引用持久化，
窗口保留 200 字符预览加 `Memory[l0_ref …]` 引用。绑定感知装载（第五片，
2026-08-17，`81a0529`）：`Visibility::Agent` 资产只有在其绑定包含本轮当前
agent profile id 时才被准入，失败即关闭并记录在 AssemblyRecord 中。

仍待完成（规划于 [`agent-deepening-program.md`](agent-deepening-program.md)）：
seam 的神经网络嵌入提供方、供模型在对话中追随 `Memory[…]` 引用的查找工具
（第 1.5 阶段）、在 L2/L3 上检索而不只是 L1 原子（第 1.4 阶段），以及专门
的装配记录读取 API（第 1.6 阶段）。

## 行为插件化（2026-08-17 交付）

装配层插件化（第一阶段）让子系统可禁用；这一片把它们的**行为**搬到插件
机制上：

- **生命周期 seam**：插件在构建时注册 `on_start`/`on_close` 回调；
  `App::serve`/`App::close` 运行这些注册表，而不是硬编码管理器名。调度器和
  提醒拥有自己的启动与关闭，沙箱拥有关闭，记忆拥有它的 60 秒整合/保留节拍。
- **记忆捕获是一个 hook**：memory 插件注册 `chat/turn-end` 观察者，把结束的
  轮次捕获到 L0（线程 + dispatch + 来源消息链接），并在回复路径之外调度到期
  的整合——硬编码的 API 层调用已移除。覆盖说明：流式轮次会被捕获（之前只有
  非流式查询会）；频道来源的轮次也在这里捕获——网关驱动的 IM 流量不经过
  HTTP 入站流水线就到达对话，所以这个 hook 是它唯一的捕获点。同时派发对话
  的 HTTP 流水线消息会产生 `inbound_message` 和 `chat_turn` 两条 L0；可以
  接受——L0 是摘录证据，整合通过引用提取。每条捕获路径（对话 hook、入站、
  工作流、会话逐出）现在都在请求路径之外运行整合，以尊重到期的轮次触发。
- **连接器运行时暂留在内核中**（记录的决策）：四个频道运行时构建器留在
  `App::serve` 中，因为 telegram/matrix 传输是 `!Send` 的，跑在裸指针线程
  上——把这套体操搬到插件拥有的生命周期回调后面只增加风险而不改变行为。
  频道插件拥有身份、启用与门控；完整的运行时所有权随 seam-RPC 切片落地，
  届时一个频道可以完全在进程外提供。

## 第三阶段——进程外插件提供方（第一片 2026-08-17 交付）

已交付：外部插件作为受监管的 stdio 进程接入 hook 平面。

- **Manifest**（`schemas/plugin/plugin-manifest.schema.json`）：id/version/
  summary/provides/requires、`hooks: [{point, onError}]`、`entry: {kind:
  "process", command, args, timeoutMs}`。
- **发现**：启动时读取 `<data_dir>/plugins/<dir>/manifest.json`。第三方内容
  绝不会让启动失败——格式错误的 manifest 被跳过并在报告中警告（不同于
  操作员拥有的 profile，后者会明确失败）。外部插件通过与内置插件相同的
  profile/`requires` 机制解析，并在 `/v1/plugins` 中显示为
  `source: "external"`；id 重复时内置优先。
- **进程宿主**：首次 hook 调用时延迟启动，按行 JSON 协议（请求
  `{point, payload}` → 响应 `{outcome, reason?, payload?}`，响应 payload
  替换 hook payload），每次调用超时，子进程已死时每次调用重启一次，守护
  进程关闭时终止。失败遵循该 hook 的 `onError` 策略：`continue`（可用性
  优先，默认）或 `veto`（失败即关闭——策略插件）。因此一个外部进程可以像
  内置 hook 一样改写上下文或否决轮次（端到端验证：磁盘上的 manifest → 装配
  → 对话轮次被子进程改写）。
- **目录**：安装目录接受 `kind=plugin`。

信任提示：manifest 会从数据目录执行命令——安装外部插件等于以守护进程权限
执行代码，与安装能力属于同一信任等级。分发/验证流程走目录的信任层级。

Seam 分发（第二片，2026-08-17）：manifest 声明 `seams`；调用走与 hook 相同
的按行 JSON 通道（点位 `seam:<name>:<op>`）。第一个被提供的 seam 是
`context.embedder`：安装一个外部嵌入进程（神经网络或其他），context 插件和
`/v1/retrieval/queries` 中的向量排序器就切换到它——不改守护进程，不改融合。
失败回退到确定性的进程内嵌入器（可用性优先，记录日志）。先声明者胜出；
重复的提供方产生警告并被忽略（确定性装配）。

留给后续切片：通过进程协议提供更多 seam（记忆整合器、整个频道运行时），
以及由目录驱动、把插件放入 `<data_dir>/plugins/` 的安装/更新生命周期。

## 排序

插件化先于其余的能力计划（操作员决策）：会话/上下文管理在第二阶段之后
**作为插件**交付；检索、agent 管理的技能、经审计的自我改进随后。操作员
主导的发布工作（Roadmap 76 soak、Roadmap 77 发布门）并行进行且不受影响——
在默认 profile 下，装配与插件化之前的守护进程行为完全一致。

## 验证（第一阶段）

- `kura-plugin` 单元测试：解析（显式/传递/未知 id）、profile 加载、seam 表、
  hook 瀑布（修改 + 中止顺序）。
- `kura-app` 测试：默认 profile ⇒ 所有内置插件启用且每个 seam 适配器已装配
  （既有装配测试不变）；叶子禁用 ⇒ 管理器为 `None` 且记录原因；禁用
  `billing` ⇒ 依赖者传递性禁用；`App::new` 读取 `plugins.json`；频道插件
  禁用 ⇒ 尽管配置开关打开也跳过连接器运行时。
- 契约测试：profile 与报告 schema，包括把真实的 `resolve()` 输出往返通过
  报告 schema。

回滚：回退到插件化之前的装配提交；`plugins.json` 是增量的，旧守护进程会
忽略它。
