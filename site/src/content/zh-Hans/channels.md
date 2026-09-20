# 频道

Kura 原生支持 IM：Discord、Telegram、Slack 和 Matrix 连接器运行在守护进程
内，各自是一个 `channel-*` 插件。飞书/Lark 的日历和邮件走集成平面。

每个频道都获得**每线程一个上下文**：连续性、记忆捕获和窗口预算自动按线程
划分（`sourceKind: channel` 使用更紧的 16k 会话预算）。

## 启用频道

频道在任何地方都**默认关闭**。在 `<data_dir>/config.json` 中启用（令牌
最好通过环境变量提供）：

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

允许列表（`allowedGuildIds`、`allowedUserIds`、`selectedRoomIds`……）是
安全护栏：空列表表示连接器自身的默认姿态，而提及门控让群组频道在未被点名
时保持安静。

## 插件门控

即使配置里 `enabled: true`，频道也只有在其插件启用时才会运行——
`plugins.json` 的 `"disabled": ["channel-discord"]` 优先，并且不会触碰任何
网络或凭据。这让"临时拔掉一个频道"成为一行、可逆的操作。

## 回流的内容

入站消息被捕获到记忆（消息 + 线程 + dispatch 证据链接），回复走和其他每一
轮一样的可挂 hook 的对话流水线，每个连接器的投递结果都能在操作台中查看。
