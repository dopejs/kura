# 頻道

Kura 原生支援 IM：Discord、Telegram、Slack 和 Matrix 聯結器執行在守護程序
內，各自是一個 `channel-*` 外掛。飛書/Lark 的日曆和郵件走整合平面。

每個頻道都獲得**每執行緒一個上下文**：連續性、記憶捕獲和視窗預算自動按執行緒
劃分（`sourceKind: channel` 使用更緊的 16k 工作階段預算）。

## 啟用頻道

頻道在任何地方都**預設關閉**。在 `<data_dir>/config.json` 中啟用（權杖
最好透過環境變數提供）：

```json
{
  "connectors": {
    "discord": {
      "enabled": true,
      "botTokenEnv": "DISCORD_BOT_TOKEN",
      "requireMention": true,
      "respondInDM": true,
      "allowedGuildIds": [],
      "allowedChannelIds": []
    },
    "telegram": {
      "enabled": true,
      "botTokenEnv": "TELEGRAM_BOT_TOKEN",
      "botUsername": "my_kura_bot",
      "allowedUserIds": []
    },
    "slack": {
      "enabled": true,
      "botTokenSecretRef": "secret://slack-bot-token",
      "workspaceId": "T…",
      "botUserId": "U…",
      "allowedChannelIds": []
    },
    "matrix": {
      "enabled": true,
      "homeserverUrl": "https://matrix.example.org",
      "botUserId": "@kura:example.org",
      "botAccessTokenEnv": "MATRIX_TOKEN",
      "selectedRoomIds": []
    }
  }
}
```

允許列表（`allowedGuildIds`、`allowedUserIds`、`selectedRoomIds`……）是
安全護欄：空列表表示聯結器自身的預設姿態，而提及門控讓群組頻道在未被點名
時保持安靜。

## 外掛門控

即使設定裡 `enabled: true`，頻道也只有在其外掛啟用時才會執行——
`plugins.json` 的 `"disabled": ["channel-discord"]` 優先，並且不會觸碰任何
網路或憑據。這讓"臨時拔掉一個頻道"成為一行、可逆的操作。

## 迴流的內容

入站訊息被捕獲到記憶（訊息 + 執行緒 + dispatch 證據連結），回覆走和其他每一
輪一樣的可掛 hook 的對話流水線，每個聯結器的投遞結果都能在操作檯中檢視。
