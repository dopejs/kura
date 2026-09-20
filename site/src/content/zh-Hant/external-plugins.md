# 外部外掛

外部（二級）外掛是**用任意語言編寫的獨立程序**，接入與內建外掛相同的 hook
瀑布和 seam。把一個目錄放到 `<data_dir>/plugins/` 下即可安裝：

```
~/.kura-test/plugins/
  my-plugin/
    manifest.json
    run.py            # 任何可執行檔案
```

## manifest.json

```json
{
  "id": "my-plugin",
  "version": "0.1.0",
  "summary": "vetoes risky turns and serves embeddings",
  "requires": ["chat"],
  "hooks": [
    { "point": "chat/pre-dispatch", "onError": "veto" },
    { "point": "chat/turn-end",     "onError": "continue" }
  ],
  "seams": ["context.embedder"],
  "entry": {
    "kind": "process",
    "command": "python3",
    "args": ["run.py"],
    "timeoutMs": 2000
  }
}
```

- 第三方 manifest 格式錯誤**絕不會讓守護程序無法啟動**——它會變成裝配報告
  中的警告，該外掛被跳過。
- 外部外掛走和內建外掛一樣的 profile/`requires` 機制，並在 `/v1/plugins`
  中顯示為 `source: "external"`。id 重複時內建外掛優先。
- `onError` 按 hook 設定：`continue`（可用性優先——失敗只記錄日誌，本輪
  繼續）或 `veto`（失敗即關閉——用於策略類外掛）。

## 程序協議

stdio 上的按行 JSON。守護程序每行寫一個請求；你的程序回答一行：

```jsonc
// 請求
{"point": "chat/pre-dispatch", "payload": {"messages": [...], "query": "..."}}

// 響應——payload（可選）會替換 hook 的 payload
{"outcome": "continue", "payload": {"messages": [...]}}

// 或否決
{"outcome": "halt", "reason": "tenant policy forbids this"}
```

子程序在第一次呼叫時延遲啟動，若已退出則每次呼叫最多重啟一次，每次呼叫受
`timeoutMs` 限制，守護程序關閉時被終止。

## 提供 seam

宣告 `"seams": ["context.embedder"]`，並在同一通道上回答 seam 呼叫——點位為
`seam:<name>:<op>`：

```jsonc
// 請求
{"point": "seam:context.embedder:embed", "payload": {"text": "..."}}
// 響應
{"outcome": "continue", "payload": {"vector": [0.12, -0.4, ...]}}
```

這樣你的程序就**接管了向量排序器**，同時作用於 context 外掛和
`/v1/retrieval/queries`——神經網路嵌入模型就是這樣在不改守護程序的情況下
接入的。失敗時回退到內建的確定性嵌入器。

> 信任提示：安裝外部外掛等於以守護程序許可權執行程式碼——與安裝一個能力屬於
> 同一信任等級。目錄（`kind=plugin`）為分發提供信任層級。
