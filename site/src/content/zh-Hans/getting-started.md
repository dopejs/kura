# 快速开始

Kura 是一个**个人 agent 操作系统**：一个本地 Rust 守护进程（控制平面），
加上若干轻客户端——全屏终端界面（`kura-tui`）、React Web 操作台，以及聊天
频道连接器。守护进程负责运行时状态、模型派发、策略门、记忆和事件分发；
客户端只是它 HTTP API 的轻量消费者。

## 安装

一行命令（macOS 与 Linux，arm64 + x86_64）：

```bash
curl -fsSL https://kura.dopejs.com/install.sh | sh
```

安装脚本会识别平台，下载最新的
[GitHub Release](https://github.com/dopejs/kura/releases)，用发布包中的
`SHA256SUMS` 校验 SHA-256，然后把 `kura`（守护进程）和 `kura-tui`（终端
客户端）安装到 `~/.local/bin` 或 `/usr/local/bin`。

指定版本或安装目录：

```bash
KURA_VERSION=v0.3.0 KURA_INSTALL_DIR=~/bin sh -c "$(curl -fsSL https://kura.dopejs.com/install.sh)"
```

想手动安装？从发布页下载压缩包：

```bash
curl -LO https://github.com/dopejs/kura/releases/latest/download/kura-0.3.0-aarch64-apple-darwin.tar.gz
tar xzf kura-0.3.0-aarch64-apple-darwin.tar.gz
sudo install -m 755 kura-0.3.0-aarch64-apple-darwin/{kura,kura-tui} /usr/local/bin/
```

## 从源码构建

依赖：Rust（1.85+）、pnpm（用于 Web 客户端）。

```bash
git clone https://github.com/dopejs/kura
cd kura

# 守护进程 + TUI
make daemon-build                 # 产出 crates/target/release/kura
cd crates && cargo build --release -p kura-tui  # 产出 target/release/kura-tui

# Web 客户端 + SDK
pnpm install
pnpm build:clients
```

## 运行守护进程

一切都由 `kura` CLI 管理：

```bash
kura daemon start                 # 后台守护进程：pid 文件 + 日志文件
kura daemon status                # pid、健康状态、版本
kura daemon stop                  # 优雅停止
kura daemon run                   # 前台运行（用于 service/systemd）

kura tui                          # 终端客户端
kura web                          # 启动并打开 Web 操作台
kura config show                  # 生效的配置
kura config set llm.defaultProvider claude_code_cli
kura config edit                  # 用 $EDITOR 编辑 config.json（带校验）
```

Kura 有两个环境，由 `KURA_ENV` 选择：

| 模式 | 数据目录 | 绑定地址 | 命令 |
|------|----------|----------|------|
| prod（发布版默认） | `~/.kura` | `127.0.0.1:19191` | `kura daemon start` |
| test | `~/.kura-test` | `127.0.0.1:19192` | `KURA_ENV=test kura daemon start` |

在源码仓库里，Make 目标封装了同样的操作，并默认使用 **test** 环境（安全的
开发默认值）：

```bash
make daemon-run-test              # test 环境
make daemon-test-status           # 健康检查（GET /healthz）
make daemon-run-test-live         # test 环境并启用 Discord
```

实时连接器（Discord 等）在任何地方都**默认关闭**，直到你在配置里启用。

## 第一次对话

守护进程始终自带确定性的 `echo` 提供方，所以在配置任何模型之前就能对话：

```bash
curl -s http://127.0.0.1:19191/v1/chat/query \
  -H 'content-type: application/json' \
  -d '{"query": "hello", "provider": "echo"}'
```

然后配置真实的提供方（Claude CLI、Codex CLI，或任意 OpenAI 兼容端点）——
见**配置**。

## 终端界面

```bash
kura tui                          # 全屏、Claude-Code 风格的客户端
```

TUI 内置守护进程事件流查看器（`/events`）。
