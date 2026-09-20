# 部署

Kura 是一个守护进程加一个 SQLite 数据库。面向团队运行它，意味着三件单用户
默认不需要的事：前置 TLS、人人都有 bearer 令牌，以及有东西在盯着它。

## 安装为服务

`./deploy/install.sh` 构建守护进程并注册为后台服务（macOS 上是 launchd，
Linux 上是 systemd）；`deploy/docker/` 构建镜像并用 `docker compose` 运行。
参数、升级和卸载见仓库中的 `deploy/README.md`。

## TLS

守护进程自身从不终结 TLS。由反向代理来做，这样证书、续期和加密套件策略都
留在专门的工具里，守护进程继续绑定私有地址：

```bash
cd deploy/docker
KURA_PUBLIC_HOST=kura.example.com docker compose \
  -f docker-compose.yml -f docker-compose.tls.yml up -d --build
```

`docker-compose.tls.yml` 移除守护进程的宿主端口，只对外发布 Caddy 的
80/443 并自动申请 Let's Encrypt 证书；`Caddyfile` 让 SSE 流不被缓冲，并把
`X-Request-Id` 转发进守护进程的访问日志。TLS 保护传输，守护进程的认证保护
数据——可从 loopback 之外访问的部署两者都需要。

## 多用户

每个请求都在 bearer 令牌认证之后；令牌解析出租户，每一次租户拥有的读写都
按它限定范围（测试套件中的路由审计门防止新路由绕开）。令牌是持久化的，
重启后所有活跃会话都保留；撤销会一直生效。

## 可观测性

`GET /metrics`（需 bearer 认证）提供 Prometheus 文本格式：按路由模板的请求
时延、按租户的 LLM 派发时延与 token 用量、按角色的存储锁等待、hook 瀑布
耗时，以及工具调用。每个响应都带 `x-request-id`，守护进程为每个请求写一行
`http.request` 访问日志。

```yaml
scrape_configs:
  - job_name: kura
    authorization: { credentials: "<bearer token>" }
    static_configs: [{ targets: ["kura.example.com"] }]
    scheme: https
```

## 存储

`store.readers`（或 `KURA_STORE_READERS`）在单一写连接旁打开只读的读连接，
让读操作在 WAL 下与写操作并发。`0` 是连接池之前的单连接行为，因此连接池
只靠配置就能启用和回滚。

## 备份与恢复

备份是在线的 SQLite 备份（`scripts/production/backup-test-state.sh`，会校验
副本的 schema 版本和完整性）：它们透过 WAL 读取，守护进程可以继续写入，
因此不需要静默窗口。直接复制 `daemon.sqlite` 文件**不是**备份——它会悄悄
丢掉仍在 WAL 里的一切。恢复就是把副本作为数据目录打开；迁移在打开时运行，
读连接池也会对它打开。`kura daemon rehearse-upgrade` 在激活前先在快照上
演练一次迁移。
