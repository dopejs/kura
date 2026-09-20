# Agent 外掛化

決策日期：2026-08-17（操作員）。參考架構：
[deepseek-ai/deepseek-harness](https://github.com/deepseek-ai/deepseek-harness)
（"一切皆外掛"）。本文是該計劃的規劃記錄——新功能直接在這類設計文件中
規劃並實現；編號規格流程（`docs/specs/NNN-*`）對新工作已停用，僅作歷史
保留。

## 為什麼

在繼續構建 agent 能力（工作階段/上下文管理、檢索、技能）之前，先把守護程序
重構為：每個非核心子系統都是一個**外掛**——具名單元，宣告依賴，可解析
啟用狀態，裝配結果可檢查。收益：

- **工作階段管理成為外掛決策。** 純個人 agent 執行長工作階段外掛（何時壓縮、什麼
  常駐、什麼寫入記憶）；支援執行緒的 IM 頻道執行每執行緒上下文外掛。守護程序
  核心不再硬編碼單一上下文策略。
- **既有功能成為預設外掛**——可替換、可禁用，最終可由程序外提供方替代。
- **裝配是設定產物**，而不是編譯進去的口口相傳：什麼在執行、什麼被禁用、
  為什麼，都可以從 `/v1/plugins` 匯出。

## 我們從 deepseek-harness 借鑑的

1. **Seam 模型**——一個能力 = 服務定義（介面）+ 提供方（實現）+ 消費者，
   三者一起設計。換掉提供方就同時換掉了所有消費者的行為。
2. **profile 分層裝配**——執行中的守護程序是一棵已解析的外掛條目樹；任何
   條目都可按 id 禁用/設定；解析結果可匯出。
3. **三類事件域**——持久事實（我們的事件匯流排 + 儲存）、瀑布式攔截 hook
   （新增：`HookBus`，處理器可修改 payload 或中止動作）、能力事件。
4. **"模型可見 = 已記錄"**——到達模型的一切都必須能從工作階段日誌重建
   （隨第二階段的可掛 hook 的 agent 迴圈落地）。

## 有意的偏離

deepseek-harness 沒有特權核心；我們保留一個。信任邊界——**儲存、事件
匯流排、身份、認證、策略、金鑰、審計**——直接在 `App::with_profile` 中構建，
不能被外掛禁用或替換。安全姿態優先於可組合性；這是固定決策。

## 兩層模型（Rust 現實）

- **第一層——內建外掛**（已交付，第一階段）：編譯進去的外掛
  （`crates/surface/app/src/plugins.rs`），向核心 crate `kura-plugin`
  （`crates/foundation/plugin`）註冊。沒有動態載入（首個版本不值得承擔
  dylib ABI 風險）。
- **第二層——程序外提供方**（第三階段）：任何 seam 都可以透過已驗證的平面
  ——adapter RPC、受監管的能力程序、MCP——遠端提供，並可透過目錄
  （`kind=plugin`）在執行時安裝。語言無關、沙箱化、受監管。消費者分不出
  兩層的區別。

## 機制（第一階段，2026-08-17 交付）

- **`kura-plugin` 核心 crate**：`PluginDescriptor`（id、摘要、provides、
  requires）、`resolve()`（顯式 + 傳遞性禁用，未知 id 產生警告——不會
  靜默丟棄）、`PluginProfile`（`<data_dir>/plugins.json`；檔案缺失 = 預設
  profile，格式錯誤讓啟動明確失敗）、`SeamMap`（裝配期的型別化登錄檔）、
  `HookBus`（瀑布式攔截：有序處理器，修改或中止）。
- **內建外掛集**：llm、skills、sandbox、mcp、integrations、calendar、mail、
  providers、connectors、capabilities、chat、billing、activation、
  computer-use、delivery、scheduler、reminders、routines、memory、triage、
  webhooks、catalog、exec-profiles、evidence、evaluation、live-validation、
  setup-wizard、channel-{discord,telegram,slack,matrix}。宣告的 `requires`
  邊讓失敗即關閉的門保持誠實（例如禁用 `billing` 會傳遞性地禁用
  `webhooks`——配額門絕不會半裝配執行）。
- **禁用語義**：被禁用外掛的 `AppState` 欄位保持 `None`；API 層已經會回答
  "未裝配"。頻道外掛還額外門控它們在 serve 時的執行時構建（profile 優先於
  聯結器設定開關；不觸碰網路或憑據）。
- **自省**：`GET /v1/plugins` 返回啟動時的 `AssemblyReport`（構建順序、
  啟用狀態、原因、警告）。契約：`schemas/plugin/plugin-profile.schema.json`、
  `schemas/api/plugins-report.schema.json`；SDK `listPlugins()`。
- **profile 管理**（隨設定介面加入）：`GET/PUT /v1/plugins/profile` 讀取並
  原子替換 `plugins.json`（啟動時生效；響應攜帶 `restartRequired: true`）；
  SDK `getPluginProfile()`/`updatePluginProfile()`；操作檯有外掛區
  （`/plugins`）——裝配列表及啟用/禁用開關、hook 註冊、profile 警告和重啟
  提示。

## 第二階段——可掛 hook 的 agent 迴圈（2026-08-17 交付）

對話流水線（query 與 stream）現在執行三個核心 hook 點：

- `chat/turn-start`——提示片語裝之前；hook 可以改寫查詢或中止（否決）。
  payload：`{tenantId, threadId, query, sourceKind}`。
- `chat/pre-dispatch`——完整上下文組裝（技能、profile、連續性）之後、
  dispatch 準備與持久化之前；hook 可以改寫 provider/model/messages 或
  中止。**這個順序就是"模型可見 = 已記錄"不變數**：dispatch 記錄在 hook
  執行之後才建立，所以持久化的內容與提供方收到的位元組完全一致（服務層測試
  與真實裝配的端到端測試覆蓋）。
- `chat/turn-end`——dispatch 結束且連續性持久化之後；觀察性（中止只停止
  後續處理器）。
- `chat/tool-call`（第 9.0 階段，2026-09-19）——在模型請求的工具被執行
  之前。payload：`{tenantId, threadId, agentProfileId, callId, name,
  arguments}`；hook 可以改寫 `arguments` 或中止。否決會作為工具錯誤報告
  給模型（並記錄為 `chat.hook.vetoed`），絕不靜默丟棄，因此模型可以不用
  該工具繼續作答。

工具呼叫（第 9.0 階段，2026-09-20 合併）：一輪對話就是 `kura-core` 已有的
agent 迴圈，驅動在 dispatcher 之上——`chat/src/round.rs` 把一次 dispatcher
輪次做成一個 `ModelProvider`，因此每一輪都像過去的單次 dispatch 一樣被
準備、掛 hook、持久化（`tools_json`、`tool_calls_json`）和發出事件。裝配層
的 `ToolSource`（`crates/surface/app/src/tool_host.rs`）按輪次、按租戶解析
登錄檔：已接入 MCP 伺服器的工具、`memory.lookup`，以及每個 Ready 工具
profile 對應的一個工具；每個工具都被包裝，使呼叫先經過 `chat/tool-call`、
記錄為 `chat.tool.called` 事件並計數。迴圈受 `toolMaxRounds`
（`plugins.json` → `entries.chat.config`，預設 16）限制；觸頂的一輪會被判定
失敗，而不是用半個結果作答。

否決表現為 `ChatError::HookVetoed` → HTTP 403，並記錄為 `chat.hook.vetoed`
事件。`GET /v1/plugins` 報告每一條 hook 註冊（`hooks: [{point, pluginId}]`）。
上下文已經來自持久化記錄（連續性輪次在每次 dispatch 時渲染為訊息——
`derive_messages` 形態）；把這份日誌推廣為工作階段事件模型的工作併入
session-strategy 外掛切片，它掛在 `chat/pre-dispatch`。

### session-strategy 外掛（第一片 2026-08-17 交付）

`session-strategy` 內建外掛（策略引擎：`crates/domains/session`）掛在
`chat/pre-dispatch`，確定性地整形已組裝的視窗——在 dispatch 持久化之前，
所以整形後的視窗就是被記錄的、也是模型看到的。機制：保留框架的省略——
系統訊息（人設、技能、安全）是框架，絕不省略；最近的 `keepRecent` 條非
系統訊息始終保留；超出預算的最早歷史被替換為一行標記，指向執行緒連續性和
記憶平面。逐出在構造上是安全的：每一輪都在結束時獨立於視窗捕獲到 L0。

兩種策略共享機制、只在預算上不同，按本輪的 `sourceKind` 區分：**personal**
（長工作階段預設，48k 字元）和 **thread**（`sourceKind=channel`，緊湊的 16k
字元——每執行緒一個上下文；執行緒範圍本身來自連續性平面）。操作員透過 profile
條目設定
（`entries.session-strategy.config.{personalBudgetChars,threadBudgetChars,keepRecent}`）；
設定格式錯誤會讓啟動明確失敗。禁用外掛即恢復未整形的視窗。

壓縮到記憶（第二片，2026-08-17）：逐出絕不直接丟棄內容。當本輪有執行緒時，
被省略的片段透過受治理的記憶流水線捕獲為一條 L0 引用（執行緒來源連結、有界
摘錄）——非同步整合器在回覆路徑之外把它蒸餾進 L1/L2，寫策略保持不變——省略
標記引用被捕獲的資產（`…; elided span captured as Memory[l0_ref …]`），模型
可以回溯。無執行緒的輪次只能透過 dispatch 記錄觸達其片段。

後續切片：工作階段框架物件（顯式的目標/約束記錄）和頻道原生的執行緒分段策略——
作為 [`agent-deepening-program.md`](agent-deepening-program.md) 的第 5 階段
規劃。

### context 外掛（第一片 2026-08-17 交付）

`context` 內建外掛（策略引擎：`crates/domains/context`）是預設的上下文
管理器，而其他外掛按設計可以修改它的結果——它在 `chat/pre-dispatch` 瀑布中
最先執行，然後 `session-strategy` 整形視窗，之後任何內建/外部 hook 都可以
改寫或否決。

它做的事是確定性的：把租戶的記憶引導——先是 Ready 的 L3 人設，再是 Ready
的 L2 場景，最新優先（private/team 可見性；restricted/agent 等待繫結感知
裝載）——作為系統框架訊息注入，受 `entries.context.config.memoryBudgetChars`
（預設 4000，校驗失敗即明確報錯）限制。每條注入的訊息內聯引用
（`Memory[l3 mem_xxx] title: content`）：召回的記憶是證據，而不是裸文字。
L1 原子絕不批次注入（僅下鑽/檢索，遵循設計根基）。每次裝配都發出
`context.assembled` 事件，其 `AssemblyRecord` 列出納入項（資產、層、字元數）
和帶原因的排除項（`over_budget`/`empty_content`/`visibility`）——沒有任何
內容悄悄進入或錯過上下文，工程師可以重建任意 dispatch 中模型看到的記憶。
由於注入發生在 dispatch 準備之前，持久化的 dispatch 記錄原樣攜帶引導內容
（模型可見 = 已記錄成立）。

按需檢索（第二片，2026-08-17）：本輪最後一條使用者訊息透過 BM25 + 時近性
排序器召回 Ready 的 L1 原子，以倒數排名融合（k=60）合併。沒有詞法重疊的
原子絕不被召回（僅憑時近性不能把無關記憶拉進來）；融合後的前 8 個候選在
`retrievalBudgetChars`（預設 2000）內以 `Memory[l1 …] (recalled): …` 系統
訊息注入，併合並進同一個 AssemblyRecord，`source: retrieval`。向量排序器
（第三片，2026-08-17）透過 `Embedder` seam 加入融合。預設提供方：確定性的
雜湊字元三元組嵌入器（256 維 FNV 特徵雜湊，L2 歸一化餘弦）——一個字元級
向量空間，能召回基於詞的 BM25 分詞器錯過的內容，尤其是 CJK
（`請用中文回覆` 與 `中文回覆偏好` 在三元組上匹配，卻沒有任何共享詞
token）。候選範圍擴大為 BM25>0 或 餘弦 ≥ 0.25；低於閾值時雜湊噪聲絕不會把
無關記憶洩漏進上下文。神經網路嵌入提供方透過 seam 替換預設實現，無需改動
融合邏輯。

符號壓縮（第四片，2026-08-17，`92191d5`）：超過 `refThresholdChars`（預設
8000）的非框架訊息在 `chat/pre-dispatch` 外接——全文作為 L0 引用持久化，
視窗保留 200 字元預覽加 `Memory[l0_ref …]` 引用。繫結感知裝載（第五片，
2026-08-17，`81a0529`）：`Visibility::Agent` 資產只有在其繫結包含本輪當前
agent profile id 時才被准入，失敗即關閉並記錄在 AssemblyRecord 中。

仍待完成（規劃於 [`agent-deepening-program.md`](agent-deepening-program.md)）：
seam 的神經網路嵌入提供方、供模型在對話中追隨 `Memory[…]` 引用的查詢工具
（第 1.5 階段）、在 L2/L3 上檢索而不只是 L1 原子（第 1.4 階段），以及專門
的裝配記錄讀取 API（第 1.6 階段）。

## 行為外掛化（2026-08-17 交付）

裝配層外掛化（第一階段）讓子系統可禁用；這一片把它們的**行為**搬到外掛
機制上：

- **生命週期 seam**：外掛在構建時註冊 `on_start`/`on_close` 回撥；
  `App::serve`/`App::close` 執行這些登錄檔，而不是硬編碼管理器名。排程器和
  提醒擁有自己的啟動與關閉，沙箱擁有關閉，記憶擁有它的 60 秒整合/保留節拍。
- **記憶捕獲是一個 hook**：memory 外掛註冊 `chat/turn-end` 觀察者，把結束的
  輪次捕獲到 L0（執行緒 + dispatch + 來源訊息連結），並在回覆路徑之外排程到期
  的整合——硬編碼的 API 層呼叫已移除。覆蓋說明：流式輪次會被捕獲（之前只有
  非流式查詢會）；頻道來源的輪次也在這裡捕獲——閘道器驅動的 IM 流量不經過
  HTTP 入站流水線就到達對話，所以這個 hook 是它唯一的捕獲點。同時派發對話
  的 HTTP 流水線訊息會產生 `inbound_message` 和 `chat_turn` 兩條 L0；可以
  接受——L0 是摘錄證據，整合透過引用提取。每條捕獲路徑（對話 hook、入站、
  工作流、工作階段逐出）現在都在請求路徑之外執行整合，以尊重到期的輪次觸發。
- **聯結器執行時暫留在核心中**（記錄的決策）：四個頻道執行時構建器留在
  `App::serve` 中，因為 telegram/matrix 傳輸是 `!Send` 的，跑在裸指標執行緒
  上——把這套體操搬到外掛擁有的生命週期回撥後面只增加風險而不改變行為。
  頻道外掛擁有身份、啟用與門控；完整的執行時所有權隨 seam-RPC 切片落地，
  屆時一個頻道可以完全在程序外提供。

## 第三階段——程序外外掛提供方（第一片 2026-08-17 交付）

已交付：外部外掛作為受監管的 stdio 程序接入 hook 平面。

- **Manifest**（`schemas/plugin/plugin-manifest.schema.json`）：id/version/
  summary/provides/requires、`hooks: [{point, onError}]`、`entry: {kind:
  "process", command, args, timeoutMs}`。
- **發現**：啟動時讀取 `<data_dir>/plugins/<dir>/manifest.json`。第三方內容
  絕不會讓啟動失敗——格式錯誤的 manifest 被跳過並在報告中警告（不同於
  操作員擁有的 profile，後者會明確失敗）。外部外掛透過與內建外掛相同的
  profile/`requires` 機制解析，並在 `/v1/plugins` 中顯示為
  `source: "external"`；id 重複時內建優先。
- **程序宿主**：首次 hook 呼叫時延遲啟動，按行 JSON 協議（請求
  `{point, payload}` → 響應 `{outcome, reason?, payload?}`，響應 payload
  替換 hook payload），每次呼叫超時，子程序已死時每次呼叫重啟一次，守護
  程序關閉時終止。失敗遵循該 hook 的 `onError` 策略：`continue`（可用性
  優先，預設）或 `veto`（失敗即關閉——策略外掛）。因此一個外部程序可以像
  內建 hook 一樣改寫上下文或否決輪次（端到端驗證：磁碟上的 manifest → 裝配
  → 對話輪次被子程序改寫）。
- **目錄**：安裝目錄接受 `kind=plugin`。

信任提示：manifest 會從資料目錄執行命令——安裝外部外掛等於以守護程序許可權
執行程式碼，與安裝能力屬於同一信任等級。分發/驗證流程走目錄的信任層級。

Seam 分發（第二片，2026-08-17）：manifest 宣告 `seams`；呼叫走與 hook 相同
的按行 JSON 通道（點位 `seam:<name>:<op>`）。第一個被提供的 seam 是
`context.embedder`：安裝一個外部嵌入程序（神經網路或其他），context 外掛和
`/v1/retrieval/queries` 中的向量排序器就切換到它——不改守護程序，不改融合。
失敗回退到確定性的程序內嵌入器（可用性優先，記錄日誌）。先宣告者勝出；
重複的提供方產生警告並被忽略（確定性裝配）。

留給後續切片：透過程序協議提供更多 seam（記憶整合器、整個頻道執行時），
以及由目錄驅動、把外掛放入 `<data_dir>/plugins/` 的安裝/更新生命週期。

## 排序

外掛化先於其餘的能力計劃（操作員決策）：工作階段/上下文管理在第二階段之後
**作為外掛**交付；檢索、agent 管理的技能、經審計的自我改進隨後。操作員
主導的釋出工作（Roadmap 76 soak、Roadmap 77 釋出門）並行進行且不受影響——
在預設 profile 下，裝配與外掛化之前的守護程序行為完全一致。

## 驗證（第一階段）

- `kura-plugin` 單元測試：解析（顯式/傳遞/未知 id）、profile 載入、seam 表、
  hook 瀑布（修改 + 中止順序）。
- `kura-app` 測試：預設 profile ⇒ 所有內建外掛啟用且每個 seam 介面卡已裝配
  （既有裝配測試不變）；葉子禁用 ⇒ 管理器為 `None` 且記錄原因；禁用
  `billing` ⇒ 依賴者傳遞性禁用；`App::new` 讀取 `plugins.json`；頻道外掛
  禁用 ⇒ 儘管設定開關開啟也跳過聯結器執行時。
- 契約測試：profile 與報告 schema，包括把真實的 `resolve()` 輸出往返透過
  報告 schema。

回滾：回退到外掛化之前的裝配提交；`plugins.json` 是增量的，舊守護程序會
忽略它。
