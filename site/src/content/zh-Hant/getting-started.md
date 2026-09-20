# 快速開始

Kura 是一個**個人 agent 作業系統**：一個本地 Rust 守護程序（控制平面），
加上若干輕客戶端——全屏終端介面（`kura-tui`）、React Web 操作檯，以及聊天
頻道聯結器。守護程序負責執行時狀態、模型派發、策略門、記憶和事件分發；
客戶端只是它 HTTP API 的輕量消費者。

## 安裝

一行命令（macOS 與 Linux，arm64 + x86_64）：

```bash
curl -fsSL https://kura.dopejs.com/install.sh | sh
```

安裝指令碼會識別平臺，下載最新的
[GitHub Release](https://github.com/dopejs/kura/releases)，用釋出包中的
`SHA256SUMS` 校驗 SHA-256，然後把 `kura`（守護程序）和 `kura-tui`（終端
客戶端）安裝到 `~/.local/bin` 或 `/usr/local/bin`。

指定版本或安裝目錄：

```bash
KURA_VERSION=v0.3.0 KURA_INSTALL_DIR=~/bin sh -c "$(curl -fsSL https://kura.dopejs.com/install.sh)"
```

想手動安裝？從釋出頁下載壓縮包：

```bash
curl -LO https://github.com/dopejs/kura/releases/latest/download/kura-0.3.0-aarch64-apple-darwin.tar.gz
tar xzf kura-0.3.0-aarch64-apple-darwin.tar.gz
sudo install -m 755 kura-0.3.0-aarch64-apple-darwin/{kura,kura-tui} /usr/local/bin/
```

## 從原始碼構建

依賴：Rust（1.85+）、pnpm（用於 Web 客戶端）。

```bash
git clone https://github.com/dopejs/kura
cd kura

# 守護程序 + TUI
make daemon-build                 # 產出 crates/target/release/kura
cd crates && cargo build --release -p kura-tui  # 產出 target/release/kura-tui

# Web 客戶端 + SDK
pnpm install
pnpm build:clients
```

## 執行守護程序

一切都由 `kura` CLI 管理：

```bash
kura daemon start                 # 後臺守護程序：pid 檔案 + 日誌檔案
kura daemon status                # pid、健康狀態、版本
kura daemon stop                  # 優雅停止
kura daemon run                   # 前臺執行（用於 service/systemd）

kura tui                          # 終端客戶端
kura web                          # 啟動並開啟 Web 操作檯
kura config show                  # 生效的設定
kura config set llm.defaultProvider claude_code_cli
kura config edit                  # 用 $EDITOR 編輯 config.json（帶校驗）
```

Kura 有兩個環境，由 `KURA_ENV` 選擇：

| 模式 | 資料目錄 | 繫結地址 | 命令 |
|------|----------|----------|------|
| prod（釋出版預設） | `~/.kura` | `127.0.0.1:19191` | `kura daemon start` |
| test | `~/.kura-test` | `127.0.0.1:19192` | `KURA_ENV=test kura daemon start` |

在原始碼倉庫裡，Make 目標封裝了同樣的操作，並預設使用 **test** 環境（安全的
開發預設值）：

```bash
make daemon-run-test              # test 環境
make daemon-test-status           # 健康檢查（GET /healthz）
make daemon-run-test-live         # test 環境並啟用 Discord
```

實時聯結器（Discord 等）在任何地方都**預設關閉**，直到你在設定裡啟用。

## 第一次對話

守護程序始終自帶確定性的 `echo` 提供方，所以在設定任何模型之前就能對話：

```bash
curl -s http://127.0.0.1:19191/v1/chat/query \
  -H 'content-type: application/json' \
  -d '{"query": "hello", "provider": "echo"}'
```

然後設定真實的提供方（Claude CLI、Codex CLI，或任意 OpenAI 相容端點）——
見**設定**。

## 終端介面

```bash
kura tui                          # 全屏、Claude-Code 風格的客戶端
```

TUI 內建守護程序事件流檢視器（`/events`）。
