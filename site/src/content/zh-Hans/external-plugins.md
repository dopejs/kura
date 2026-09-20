# 外部插件

外部（二级）插件是**用任意语言编写的独立进程**，接入与内置插件相同的 hook
瀑布和 seam。把一个目录放到 `<data_dir>/plugins/` 下即可安装：

```
~/.kura-test/plugins/
  my-plugin/
    manifest.json
    run.py            # 任何可执行文件
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

- 第三方 manifest 格式错误**绝不会让守护进程无法启动**——它会变成装配报告
  中的警告，该插件被跳过。
- 外部插件走和内置插件一样的 profile/`requires` 机制，并在 `/v1/plugins`
  中显示为 `source: "external"`。id 重复时内置插件优先。
- `onError` 按 hook 设置：`continue`（可用性优先——失败只记录日志，本轮
  继续）或 `veto`（失败即关闭——用于策略类插件）。

## 进程协议

stdio 上的按行 JSON。守护进程每行写一个请求；你的进程回答一行：

```jsonc
// 请求
{"point": "chat/pre-dispatch", "payload": {"messages": [...], "query": "..."}}

// 响应——payload（可选）会替换 hook 的 payload
{"outcome": "continue", "payload": {"messages": [...]}}

// 或否决
{"outcome": "halt", "reason": "tenant policy forbids this"}
```

子进程在第一次调用时延迟启动，若已退出则每次调用最多重启一次，每次调用受
`timeoutMs` 限制，守护进程关闭时被终止。

## 提供 seam

声明 `"seams": ["context.embedder"]`，并在同一通道上回答 seam 调用——点位为
`seam:<name>:<op>`：

```jsonc
// 请求
{"point": "seam:context.embedder:embed", "payload": {"text": "..."}}
// 响应
{"outcome": "continue", "payload": {"vector": [0.12, -0.4, ...]}}
```

这样你的进程就**接管了向量排序器**，同时作用于 context 插件和
`/v1/retrieval/queries`——神经网络嵌入模型就是这样在不改守护进程的情况下
接入的。失败时回退到内置的确定性嵌入器。

> 信任提示：安装外部插件等于以守护进程权限执行代码——与安装一个能力属于
> 同一信任等级。目录（`kind=plugin`）为分发提供信任层级。
