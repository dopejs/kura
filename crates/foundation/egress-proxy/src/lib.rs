//! kura-egress-proxy — the only network path out of a tenant container
//! (Agent Runtime Program 3.5, G11).
//!
//! An HTTP forward proxy: `CONNECT host:port` for TLS (a byte tunnel) and
//! absolute-form `GET http://host/…` for plain HTTP. Every request is decided
//! the same way:
//!
//! 1. the port must be on the allow list (default 80, 443);
//! 2. the host is resolved **once** by [`EgressPolicy::resolve_pinned`],
//!    which checks every address against the policy (no loopback, private,
//!    link-local / cloud-metadata, unique-local, blocked names);
//! 3. the proxy connects to **that** address — never to the name — so a
//!    resolver that answers safely here and maliciously later (DNS
//!    rebinding) has nothing left to influence.
//!
//! A refusal is `403` with an `X-Kura-Egress-Denial` reason. The proxy never
//! sees inside TLS: policy is by destination, not content.
//!
//! **Which tenant** (agent-runtime-program 3.6): with a tenant key
//! configured, every request must carry `Proxy-Authorization: Basic` with
//! the tenant id and its [`EgressKey`] credential, which the daemon puts in
//! each tenant container's proxy URL. Anything else is `407`. Each
//! connection is then logged with its tenant, and a tenant may hold at most
//! `per_tenant_connections` of the proxy's slots, so one tenant cannot use
//! up the exit for everyone. Without a key the proxy cannot tell tenants
//! apart (they share one network) and every request is `unattributed`.
//!
//! What makes this the *only* path is topology, not this crate: tenant
//! containers sit on a docker `--internal` network whose one exit is this
//! proxy (see `docs/harness/sandbox-execution-plane.md`).

use std::collections::HashMap;
use std::net::SocketAddr;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use base64::Engine as _;
use kura_egress::{EgressDenial, EgressKey, EgressPolicy};
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::{TcpListener, TcpStream};
use tokio::sync::Semaphore;

/// Largest request head accepted; anything bigger is refused, not buffered.
const MAX_HEAD_BYTES: usize = 16 * 1024;

pub use kura_telemetry::metrics::EGRESS_PROXY_REQUESTS_TOTAL;

/// Resolves a destination to the single address the proxy will dial.
pub type Resolver = Arc<dyn Fn(&str, u16) -> Result<SocketAddr, EgressDenial> + Send + Sync>;

pub struct ProxyConfig {
    pub allowed_ports: Vec<u16>,
    pub max_connections: usize,
    pub connect_timeout: Duration,
    /// Bound on reading a request head: a client that never finishes one
    /// must not hold a connection slot forever.
    pub head_timeout: Duration,
    pub resolver: Resolver,
    /// Set: requests must name their tenant (module docs). `None`: no
    /// authentication, every request is unattributed.
    pub tenant_key: Option<EgressKey>,
    /// Most connections one tenant may hold at once (only with a key).
    pub per_tenant_connections: usize,
}

impl ProxyConfig {
    /// Ports 80 and 443 through `policy`.
    #[must_use]
    pub fn new(policy: EgressPolicy) -> Self {
        let policy = Arc::new(policy);
        ProxyConfig {
            allowed_ports: vec![80, 443],
            max_connections: 256,
            connect_timeout: Duration::from_secs(10),
            head_timeout: Duration::from_secs(10),
            resolver: Arc::new(move |host, port| policy.resolve_pinned(host, port)),
            tenant_key: None,
            per_tenant_connections: 64,
        }
    }
}

/// Accept and serve connections until the listener fails.
pub async fn serve(listener: TcpListener, config: Arc<ProxyConfig>) -> std::io::Result<()> {
    let slots = Arc::new(Semaphore::new(config.max_connections.max(1)));
    let tenants = TenantSlots::default();
    loop {
        let (client, _) = listener.accept().await?;
        let config = Arc::clone(&config);
        let tenants = tenants.clone();
        match Arc::clone(&slots).try_acquire_owned() {
            Ok(permit) => {
                tokio::spawn(async move {
                    handle(client, &config, &tenants).await;
                    drop(permit);
                });
            }
            Err(_) => {
                tokio::spawn(async move {
                    let mut client = client;
                    record("any", "over_capacity");
                    let _ = respond(
                        &mut client,
                        503,
                        "Service Unavailable",
                        "",
                        "proxy at capacity",
                    )
                    .await;
                });
            }
        }
    }
}

/// Open connections per tenant.
#[derive(Clone, Default)]
struct TenantSlots(Arc<Mutex<HashMap<String, usize>>>);

/// One tenant connection; frees its slot when dropped.
struct TenantSlot {
    slots: TenantSlots,
    tenant: String,
}

impl TenantSlots {
    fn acquire(&self, tenant: &str, limit: usize) -> Option<TenantSlot> {
        let mut open = self.0.lock().unwrap_or_else(|p| p.into_inner());
        let count = open.entry(tenant.to_string()).or_insert(0);
        if *count >= limit.max(1) {
            return None;
        }
        *count += 1;
        Some(TenantSlot {
            slots: self.clone(),
            tenant: tenant.to_string(),
        })
    }
}

impl Drop for TenantSlot {
    fn drop(&mut self) {
        let mut open = self.slots.0.lock().unwrap_or_else(|p| p.into_inner());
        if let Some(count) = open.get_mut(&self.tenant) {
            *count = count.saturating_sub(1);
            if *count == 0 {
                open.remove(&self.tenant);
            }
        }
    }
}

/// The `Proxy-Authorization: Basic` user and password, if the head has one.
fn proxy_credentials(head: &str) -> Option<(String, String)> {
    let value = head.split("\r\n").skip(1).find_map(|line| {
        let (name, value) = line.split_once(':')?;
        name.trim()
            .eq_ignore_ascii_case("proxy-authorization")
            .then(|| value.trim())
    })?;
    let (scheme, encoded) = value.split_once(' ')?;
    if !scheme.eq_ignore_ascii_case("basic") {
        return None;
    }
    let decoded = base64::engine::general_purpose::STANDARD
        .decode(encoded.trim())
        .ok()?;
    let (user, password) = String::from_utf8(decoded)
        .ok()?
        .split_once(':')
        .map(|(u, p)| (u.to_string(), p.to_string()))?;
    Some((user, password))
}

/// Who a request is for: `Ok(None)` without a key, the tenant id with one,
/// `Err` when the key demands a credential the request lacks.
fn authenticate(head: &str, key: Option<&EgressKey>) -> Result<Option<String>, &'static str> {
    let Some(key) = key else {
        return Ok(None);
    };
    let (tenant, password) = proxy_credentials(head).ok_or("tenant credential missing")?;
    if tenant.is_empty() || !key.verify(&tenant, &password) {
        return Err("tenant credential invalid");
    }
    Ok(Some(tenant))
}

fn record(method: &str, outcome: &str) {
    kura_telemetry::metrics::registry().inc(
        EGRESS_PROXY_REQUESTS_TOTAL,
        &[("method", method), ("outcome", outcome)],
    );
}

async fn respond(
    client: &mut TcpStream,
    status: u16,
    reason: &str,
    denial: &str,
    body: &str,
) -> std::io::Result<()> {
    let mut head = format!(
        "HTTP/1.1 {status} {reason}\r\nContent-Type: text/plain\r\nContent-Length: {}\r\nConnection: close\r\n",
        body.len()
    );
    if !denial.is_empty() {
        head.push_str(&format!("X-Kura-Egress-Denial: {denial}\r\n"));
    }
    if status == 407 {
        // Clients that wait for a challenge (Chromium via Playwright) send
        // their credential only after this.
        head.push_str("Proxy-Authenticate: Basic realm=\"kura-egress\"\r\n");
    }
    head.push_str("\r\n");
    client.write_all(head.as_bytes()).await?;
    client.write_all(body.as_bytes()).await?;
    client.shutdown().await
}

/// The request head and whatever arrived after it (a TLS hello or a body
/// the client sent without waiting).
async fn read_head(client: &mut TcpStream) -> Result<(String, Vec<u8>), &'static str> {
    let mut buffer = Vec::with_capacity(1024);
    let mut chunk = [0u8; 2048];
    loop {
        if let Some(end) = buffer.windows(4).position(|w| w == b"\r\n\r\n") {
            let rest = buffer.split_off(end + 4);
            let head = String::from_utf8(buffer).map_err(|_| "request head is not UTF-8")?;
            return Ok((head, rest));
        }
        if buffer.len() > MAX_HEAD_BYTES {
            return Err("request head too large");
        }
        let read = client.read(&mut chunk).await.map_err(|_| "read failed")?;
        if read == 0 {
            return Err("connection closed before a request");
        }
        buffer.extend_from_slice(&chunk[..read]);
    }
}

fn denial_code(denial: &EgressDenial) -> &'static str {
    match denial {
        EgressDenial::Unparseable(_) => "unparseable",
        EgressDenial::SchemeNotAllowed(_) => "scheme_not_allowed",
        EgressDenial::HostMissing => "host_missing",
        EgressDenial::HostBlocked { .. } => "host_blocked",
        EgressDenial::AddressBlocked { .. } => "address_blocked",
        EgressDenial::Unresolvable { .. } => "unresolvable",
    }
}

/// `host:port` with an optional bracketed IPv6 host.
fn split_authority(authority: &str) -> Option<(String, u16)> {
    let (host, port) = authority.rsplit_once(':')?;
    let port = port.parse::<u16>().ok()?;
    let host = host.trim_matches(['[', ']']);
    (!host.is_empty()).then(|| (host.to_string(), port))
}

/// What a request asks for, once parsed.
struct Target {
    method: &'static str,
    host: String,
    port: u16,
    /// For plain HTTP: the head rewritten to origin form for the upstream.
    upstream_head: Option<String>,
}

fn parse_target(head: &str) -> Result<Target, String> {
    let mut lines = head.split("\r\n");
    let request_line = lines.next().unwrap_or_default();
    let mut parts = request_line.split(' ');
    let (Some(method), Some(target), Some(version), None) =
        (parts.next(), parts.next(), parts.next(), parts.next())
    else {
        return Err("malformed request line".to_string());
    };
    if !version.starts_with("HTTP/1.") {
        return Err("only HTTP/1.x is proxied".to_string());
    }
    if method.eq_ignore_ascii_case("CONNECT") {
        let (host, port) =
            split_authority(target).ok_or_else(|| "CONNECT needs host:port".to_string())?;
        return Ok(Target {
            method: "connect",
            host,
            port,
            upstream_head: None,
        });
    }
    let url = url::Url::parse(target)
        .map_err(|_| "a proxied request needs an absolute http:// URL".to_string())?;
    if url.scheme() != "http" {
        // https goes through CONNECT; anything else is not proxied.
        return Err(format!("scheme {} is not proxied this way", url.scheme()));
    }
    let host = url
        .host_str()
        .map(|h| h.trim_matches(['[', ']']).to_string())
        .filter(|h| !h.is_empty())
        .ok_or_else(|| "the URL has no host".to_string())?;
    let port = url.port_or_known_default().unwrap_or(80);
    let mut origin = url.path().to_string();
    if let Some(query) = url.query() {
        origin.push('?');
        origin.push_str(query);
    }
    let mut upstream = format!("{method} {origin} {version}\r\n");
    for line in lines {
        let name = line.split(':').next().unwrap_or_default().trim();
        // Hop-by-hop proxy headers stop here; credentials meant for this
        // proxy must not leak to the destination.
        if name.eq_ignore_ascii_case("proxy-connection")
            || name.eq_ignore_ascii_case("proxy-authorization")
        {
            continue;
        }
        upstream.push_str(line);
        upstream.push_str("\r\n");
    }
    Ok(Target {
        method: "http",
        host,
        port,
        upstream_head: Some(upstream),
    })
}

async fn handle(mut client: TcpStream, config: &ProxyConfig, tenants: &TenantSlots) {
    let head = tokio::time::timeout(config.head_timeout, read_head(&mut client)).await;
    let (head, rest) = match head {
        Ok(Ok(parsed)) => parsed,
        Ok(Err(reason)) => {
            record("any", "bad_request");
            let _ = respond(&mut client, 400, "Bad Request", "", reason).await;
            return;
        }
        Err(_) => {
            record("any", "head_timeout");
            let _ = respond(
                &mut client,
                408,
                "Request Timeout",
                "",
                "request head timed out",
            )
            .await;
            return;
        }
    };
    let tenant = match authenticate(&head, config.tenant_key.as_ref()) {
        Ok(tenant) => tenant,
        Err(reason) => {
            record("any", "unauthenticated");
            eprintln!("[kura] egress-proxy: refused a request: {reason}");
            let _ = respond(
                &mut client,
                407,
                "Proxy Authentication Required",
                "tenant_credential",
                reason,
            )
            .await;
            return;
        }
    };
    let who = tenant.as_deref().unwrap_or("unattributed");
    let _tenant_slot = match &tenant {
        Some(tenant) => match tenants.acquire(tenant, config.per_tenant_connections) {
            Some(slot) => Some(slot),
            None => {
                record("any", "tenant_over_capacity");
                eprintln!(
                    "[kura] egress-proxy: tenant={tenant} refused: {} connections already open",
                    config.per_tenant_connections
                );
                let _ = respond(
                    &mut client,
                    429,
                    "Too Many Requests",
                    "tenant_over_capacity",
                    "this tenant has too many connections open",
                )
                .await;
                return;
            }
        },
        None => None,
    };
    let target = match parse_target(&head) {
        Ok(target) => target,
        Err(reason) => {
            record("any", "bad_request");
            let _ = respond(&mut client, 400, "Bad Request", "", &reason).await;
            return;
        }
    };
    if !config.allowed_ports.contains(&target.port) {
        record(target.method, "denied");
        let body = format!("port {} is not an allowed egress port", target.port);
        eprintln!(
            "[kura] egress-proxy: tenant={who} refused {}:{} ({body})",
            target.host, target.port
        );
        let _ = respond(&mut client, 403, "Forbidden", "port_not_allowed", &body).await;
        return;
    }

    // Resolve once, off the async workers (the system resolver blocks).
    let resolver = Arc::clone(&config.resolver);
    let (host, port) = (target.host.clone(), target.port);
    let pinned = tokio::task::spawn_blocking(move || resolver(&host, port)).await;
    let address = match pinned {
        Ok(Ok(address)) => address,
        Ok(Err(denial)) => {
            record(target.method, "denied");
            eprintln!(
                "[kura] egress-proxy: tenant={who} refused {}:{} ({denial})",
                target.host, target.port
            );
            let _ = respond(
                &mut client,
                403,
                "Forbidden",
                denial_code(&denial),
                &denial.to_string(),
            )
            .await;
            return;
        }
        Err(_) => {
            record(target.method, "upstream_failed");
            let _ = respond(&mut client, 502, "Bad Gateway", "", "resolver failed").await;
            return;
        }
    };

    let upstream = tokio::time::timeout(config.connect_timeout, TcpStream::connect(address)).await;
    let mut upstream = match upstream {
        Ok(Ok(stream)) => stream,
        Ok(Err(err)) => {
            record(target.method, "upstream_failed");
            let body = format!("connecting to {address} failed: {err}");
            let _ = respond(&mut client, 502, "Bad Gateway", "", &body).await;
            return;
        }
        Err(_) => {
            record(target.method, "upstream_failed");
            let body = format!("connecting to {address} timed out");
            let _ = respond(&mut client, 504, "Gateway Timeout", "", &body).await;
            return;
        }
    };

    let started = match &target.upstream_head {
        None => {
            client
                .write_all(b"HTTP/1.1 200 Connection Established\r\n\r\n")
                .await
        }
        Some(upstream_head) => upstream.write_all(upstream_head.as_bytes()).await,
    };
    if started.is_err() {
        record(target.method, "upstream_failed");
        return;
    }
    if !rest.is_empty() && upstream.write_all(&rest).await.is_err() {
        record(target.method, "upstream_failed");
        return;
    }
    record(target.method, "allowed");
    // The attribution record: who went where (on open, so a tunnel that
    // stays up for hours is visible while it is up), then how much.
    eprintln!(
        "[kura] egress-proxy: tenant={who} {} {}:{} -> {address} opened",
        target.method, target.host, target.port
    );
    let started = Instant::now();
    let (up, down) = tokio::io::copy_bidirectional(&mut client, &mut upstream)
        .await
        .unwrap_or((0, 0));
    eprintln!(
        "[kura] egress-proxy: tenant={who} {} {}:{} -> {address} closed up={up} down={down} ms={}",
        target.method,
        target.host,
        target.port,
        started.elapsed().as_millis()
    );
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::{AtomicUsize, Ordering};

    async fn start(config: ProxyConfig) -> SocketAddr {
        let listener = TcpListener::bind("127.0.0.1:0").await.expect("bind");
        let address = listener.local_addr().expect("addr");
        tokio::spawn(serve(listener, Arc::new(config)));
        address
    }

    /// A loopback-permitting policy, so tests can reach local upstreams.
    fn loopback_config(ports: Vec<u16>) -> ProxyConfig {
        ProxyConfig {
            allowed_ports: ports,
            ..ProxyConfig::new(EgressPolicy {
                allow_loopback: true,
                ..EgressPolicy::default()
            })
        }
    }

    async fn echo_upstream() -> SocketAddr {
        let listener = TcpListener::bind("127.0.0.1:0").await.expect("bind");
        let address = listener.local_addr().expect("addr");
        tokio::spawn(async move {
            while let Ok((mut stream, _)) = listener.accept().await {
                tokio::spawn(async move {
                    let (mut read, mut write) = stream.split();
                    let _ = tokio::io::copy(&mut read, &mut write).await;
                });
            }
        });
        address
    }

    async fn exchange(proxy: SocketAddr, request: &str) -> (TcpStream, String) {
        let mut stream = TcpStream::connect(proxy).await.expect("connect");
        stream.write_all(request.as_bytes()).await.expect("write");
        let mut buffer = vec![0u8; 4096];
        let read = stream.read(&mut buffer).await.expect("read");
        (stream, String::from_utf8_lossy(&buffer[..read]).to_string())
    }

    #[tokio::test]
    async fn connect_tunnels_bytes_to_a_permitted_destination() {
        let upstream = echo_upstream().await;
        let proxy = start(loopback_config(vec![upstream.port()])).await;
        let (mut stream, reply) = exchange(
            proxy,
            &format!("CONNECT {upstream} HTTP/1.1\r\nHost: {upstream}\r\n\r\n"),
        )
        .await;
        assert!(reply.starts_with("HTTP/1.1 200"), "{reply}");
        stream.write_all(b"tls-bytes").await.expect("write");
        let mut buffer = [0u8; 9];
        stream.read_exact(&mut buffer).await.expect("echo");
        assert_eq!(&buffer, b"tls-bytes");
    }

    #[tokio::test]
    async fn plain_http_is_rewritten_to_origin_form_without_proxy_credentials() {
        let listener = TcpListener::bind("127.0.0.1:0").await.expect("bind");
        let upstream = listener.local_addr().expect("addr");
        let seen = tokio::spawn(async move {
            let (mut stream, _) = listener.accept().await.expect("accept");
            let mut buffer = vec![0u8; 4096];
            let read = stream.read(&mut buffer).await.expect("read");
            stream
                .write_all(b"HTTP/1.1 204 No Content\r\nContent-Length: 0\r\n\r\n")
                .await
                .expect("reply");
            String::from_utf8_lossy(&buffer[..read]).to_string()
        });
        let proxy = start(loopback_config(vec![upstream.port()])).await;
        let (_stream, reply) = exchange(
            proxy,
            &format!(
                "GET http://{upstream}/path?q=1 HTTP/1.1\r\nHost: {upstream}\r\nProxy-Authorization: Basic c2VjcmV0\r\nAccept: */*\r\n\r\n"
            ),
        )
        .await;
        assert!(reply.starts_with("HTTP/1.1 204"), "{reply}");
        let request = seen.await.expect("upstream");
        assert!(
            request.starts_with("GET /path?q=1 HTTP/1.1\r\n"),
            "{request}"
        );
        assert!(request.contains("Accept: */*"));
        assert!(
            !request.to_lowercase().contains("proxy-authorization"),
            "{request}"
        );
    }

    #[tokio::test]
    async fn the_default_policy_refuses_loopback_metadata_and_other_ports() {
        let upstream = echo_upstream().await;
        let mut config = ProxyConfig::new(EgressPolicy::default());
        config.allowed_ports.push(upstream.port());
        let proxy = start(config).await;

        let (_, reply) = exchange(proxy, &format!("CONNECT {upstream} HTTP/1.1\r\n\r\n")).await;
        assert!(reply.starts_with("HTTP/1.1 403"), "{reply}");
        assert!(
            reply.contains("X-Kura-Egress-Denial: address_blocked"),
            "{reply}"
        );

        let (_, reply) = exchange(proxy, "CONNECT 169.254.169.254:80 HTTP/1.1\r\n\r\n").await;
        assert!(
            reply.contains("X-Kura-Egress-Denial: address_blocked"),
            "{reply}"
        );

        let (_, reply) = exchange(proxy, "GET http://10.0.0.5/ HTTP/1.1\r\n\r\n").await;
        assert!(
            reply.contains("X-Kura-Egress-Denial: address_blocked"),
            "{reply}"
        );

        let (_, reply) = exchange(proxy, "CONNECT 93.184.216.34:22 HTTP/1.1\r\n\r\n").await;
        assert!(
            reply.contains("X-Kura-Egress-Denial: port_not_allowed"),
            "{reply}"
        );
    }

    #[tokio::test]
    async fn malformed_requests_are_refused() {
        let proxy = start(ProxyConfig::new(EgressPolicy::default())).await;
        for request in [
            "GET /relative HTTP/1.1\r\n\r\n",
            "CONNECT nohostport HTTP/1.1\r\n\r\n",
            "GET ftp://example.com/ HTTP/1.1\r\n\r\n",
            "nonsense\r\n\r\n",
        ] {
            let (_, reply) = exchange(proxy, request).await;
            assert!(reply.starts_with("HTTP/1.1 400"), "{request:?} -> {reply}");
        }
    }

    /// The rebinding defence: the destination is resolved exactly once, and
    /// the connection goes to that answer. A second lookup — the one a
    /// rebinding resolver would poison — never happens.
    #[tokio::test]
    async fn the_destination_is_resolved_once_and_that_address_is_dialled() {
        let upstream = echo_upstream().await;
        let calls = Arc::new(AtomicUsize::new(0));
        let counted = Arc::clone(&calls);
        let config = ProxyConfig {
            allowed_ports: vec![443],
            resolver: Arc::new(move |host, port| {
                assert_eq!((host, port), ("rebind.test", 443));
                // First answer is the checked one; any later answer would be
                // the attacker's.
                if counted.fetch_add(1, Ordering::SeqCst) == 0 {
                    Ok(upstream)
                } else {
                    Ok("169.254.169.254:443".parse().unwrap())
                }
            }),
            ..ProxyConfig::new(EgressPolicy::default())
        };
        let proxy = start(config).await;
        let (mut stream, reply) = exchange(proxy, "CONNECT rebind.test:443 HTTP/1.1\r\n\r\n").await;
        assert!(reply.starts_with("HTTP/1.1 200"), "{reply}");
        stream.write_all(b"ping").await.expect("write");
        let mut buffer = [0u8; 4];
        stream
            .read_exact(&mut buffer)
            .await
            .expect("echo from the pinned address");
        assert_eq!(&buffer, b"ping");
        assert_eq!(calls.load(Ordering::SeqCst), 1);
    }

    fn tenant_key() -> EgressKey {
        EgressKey::from_bytes(&[3u8; 32]).unwrap()
    }

    fn basic(user: &str, password: &str) -> String {
        base64::engine::general_purpose::STANDARD.encode(format!("{user}:{password}"))
    }

    #[tokio::test]
    async fn with_a_key_only_a_valid_tenant_credential_gets_through() {
        let upstream = echo_upstream().await;
        let proxy = start(ProxyConfig {
            tenant_key: Some(tenant_key()),
            ..loopback_config(vec![upstream.port()])
        })
        .await;
        let key = tenant_key();
        let good = basic("ten_a", &key.credential("ten_a"));
        let borrowed = basic("ten_b", &key.credential("ten_a"));
        let forged = basic("ten_a", &"0".repeat(64));

        let (_, reply) = exchange(proxy, &format!("CONNECT {upstream} HTTP/1.1\r\n\r\n")).await;
        assert!(reply.starts_with("HTTP/1.1 407"), "{reply}");
        assert!(
            reply.contains("Proxy-Authenticate: Basic realm=\"kura-egress\""),
            "{reply}"
        );
        for credential in [&borrowed, &forged] {
            let (_, reply) = exchange(
                proxy,
                &format!(
                    "CONNECT {upstream} HTTP/1.1\r\nProxy-Authorization: Basic {credential}\r\n\r\n"
                ),
            )
            .await;
            assert!(reply.starts_with("HTTP/1.1 407"), "{reply}");
        }
        let (_, reply) = exchange(
            proxy,
            &format!("CONNECT {upstream} HTTP/1.1\r\nProxy-Authorization: Basic {good}\r\n\r\n"),
        )
        .await;
        assert!(reply.starts_with("HTTP/1.1 200"), "{reply}");
    }

    #[tokio::test]
    async fn one_tenant_cannot_take_every_slot() {
        let upstream = echo_upstream().await;
        let proxy = start(ProxyConfig {
            tenant_key: Some(tenant_key()),
            per_tenant_connections: 2,
            ..loopback_config(vec![upstream.port()])
        })
        .await;
        let key = tenant_key();
        let connect = |tenant: &str| {
            format!(
                "CONNECT {upstream} HTTP/1.1\r\nProxy-Authorization: Basic {}\r\n\r\n",
                basic(tenant, &key.credential(tenant))
            )
        };
        let (first, reply) = exchange(proxy, &connect("ten_a")).await;
        assert!(reply.starts_with("HTTP/1.1 200"), "{reply}");
        let (_second, reply) = exchange(proxy, &connect("ten_a")).await;
        assert!(reply.starts_with("HTTP/1.1 200"), "{reply}");
        let (_, reply) = exchange(proxy, &connect("ten_a")).await;
        assert!(reply.starts_with("HTTP/1.1 429"), "{reply}");
        assert!(
            reply.contains("X-Kura-Egress-Denial: tenant_over_capacity"),
            "{reply}"
        );
        // Another tenant is unaffected.
        let (_, reply) = exchange(proxy, &connect("ten_b")).await;
        assert!(reply.starts_with("HTTP/1.1 200"), "{reply}");
        // Closing a connection frees its slot.
        drop(first);
        let mut freed = false;
        for _ in 0..50 {
            let (_, reply) = exchange(proxy, &connect("ten_a")).await;
            if reply.starts_with("HTTP/1.1 200") {
                freed = true;
                break;
            }
            tokio::time::sleep(Duration::from_millis(20)).await;
        }
        assert!(freed, "the slot was not released");
    }

    #[test]
    fn proxy_credentials_parse_only_basic() {
        let head = format!(
            "CONNECT a:443 HTTP/1.1\r\nproxy-authorization: basic {}\r\n\r\n",
            basic("ten_a", "pw")
        );
        assert_eq!(
            proxy_credentials(&head),
            Some(("ten_a".to_string(), "pw".to_string()))
        );
        assert_eq!(
            proxy_credentials("CONNECT a:443 HTTP/1.1\r\nProxy-Authorization: Bearer x\r\n\r\n"),
            None
        );
        assert_eq!(
            authenticate("CONNECT a:443 HTTP/1.1\r\n\r\n", None),
            Ok(None)
        );
    }

    #[tokio::test]
    async fn a_request_head_that_never_ends_is_cut_off() {
        let config = ProxyConfig {
            head_timeout: Duration::from_millis(200),
            ..ProxyConfig::new(EgressPolicy::default())
        };
        let proxy = start(config).await;
        let (_, reply) = exchange(proxy, "CONNECT example.com:443 HTTP/1.1\r\n").await;
        assert!(reply.starts_with("HTTP/1.1 408"), "{reply}");
    }
}
