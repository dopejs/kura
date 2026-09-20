# 部署

Kura 是一個守護程序加一個 SQLite 資料庫。面向團隊執行它，意味著三件單使用者
預設不需要的事：前置 TLS、人人都有 bearer 權杖，以及有東西在盯著它。

## 安裝為服務

`./deploy/install.sh` 構建守護程序並註冊為後臺服務（macOS 上是 launchd，
Linux 上是 systemd）；`deploy/docker/` 構建映象並用 `docker compose` 執行。
引數、升級和解除安裝見倉庫中的 `deploy/README.md`。

## TLS

守護程序自身從不終結 TLS。由反向代理來做，這樣證書、續期和加密套件策略都
留在專門的工具裡，守護程序繼續繫結私有地址：

```bash
cd deploy/docker
KURA_PUBLIC_HOST=kura.example.com docker compose \
  -f docker-compose.yml -f docker-compose.tls.yml up -d --build
```

`docker-compose.tls.yml` 移除守護程序的宿主埠，只對外發布 Caddy 的
80/443 並自動申請 Let's Encrypt 證書；`Caddyfile` 讓 SSE 流不被緩衝，並把
`X-Request-Id` 轉發進守護程序的訪問日誌。TLS 保護傳輸，守護程序的認證保護
資料——可從 loopback 之外訪問的部署兩者都需要。

## 多使用者

每個請求都在 bearer 權杖認證之後；權杖解析出租戶，每一次租戶擁有的讀寫都
按它限定範圍（測試套件中的路由審計門防止新路由繞開）。權杖是持久化的，
重啟後所有活躍工作階段都保留；撤銷會一直生效。

## 可觀測性

`GET /metrics`（需 bearer 認證）提供 Prometheus 文字格式：按路由模板的請求
時延、按租戶的 LLM 派發時延與 token 用量、按角色的儲存鎖等待、hook 瀑布
耗時，以及工具呼叫。每個響應都帶 `x-request-id`，守護程序為每個請求寫一行
`http.request` 訪問日誌。

```yaml
scrape_configs:
  - job_name: kura
    authorization: { credentials: "<bearer token>" }
    static_configs: [{ targets: ["kura.example.com"] }]
    scheme: https
```

## 儲存

`store.readers`（或 `KURA_STORE_READERS`）在單一寫連線旁開啟只讀的讀連線，
讓讀操作在 WAL 下與寫操作併發。`0` 是連線池之前的單連線行為，因此連線池
只靠設定就能啟用和回滾。

## 備份與恢復

備份是線上的 SQLite 備份（`scripts/production/backup-test-state.sh`，會校驗
副本的 schema 版本和完整性）：它們透過 WAL 讀取，守護程序可以繼續寫入，
因此不需要靜默視窗。直接複製 `daemon.sqlite` 檔案**不是**備份——它會悄悄
丟掉仍在 WAL 裡的一切。恢復就是把副本作為資料目錄開啟；遷移在開啟時執行，
讀連線池也會對它開啟。`kura daemon rehearse-upgrade` 在啟用前先在快照上
演練一次遷移。
