//! Plan item 2.3, checked offline: Anthropic's prompt cache only pays off
//! if what Kura sends ahead of each breakpoint is byte-for-byte the same
//! request after request. That is Kura's half of the contract, and it can be
//! checked without a key: run the real assembly against a local
//! `/v1/messages` that records every body, and compare.
//!
//! The cached prefix is tools, then `system`, then messages. Breakpoints sit
//! on the last `system` block (the frame, read across turns) and, in a tool
//! loop, on the last message block (read by the next round). So:
//! - every request carries the same `tools` and `system`;
//! - within a turn, each round's messages are an exact prefix of the next
//!   round's;
//! - the breakpoints are where the provider puts them.
//!
//! Found by it: every tool name went out as Kura names it (`memory.lookup`),
//! which the OpenAI and Anthropic APIs refuse; the fakes never checked.

use std::sync::Arc;

use axum::body::{Body, to_bytes};
use axum::http::{Request, StatusCode};
use chrono::Utc;
use kura_app::App;
use kura_config::{Config, Environment};
use parking_lot::Mutex;
use serde_json::{Value, json};
use tower::ServiceExt;

type Recorded = Arc<Mutex<Vec<Value>>>;

fn sse(events: &[Value]) -> String {
    events
        .iter()
        .map(|event| {
            format!(
                "event: {}\ndata: {event}\n\n",
                event["type"].as_str().unwrap_or("")
            )
        })
        .collect()
}

/// A round with a tool result in it is answered with text; any other round
/// asks for one memory lookup.
async fn messages(
    axum::extract::State(recorded): axum::extract::State<Recorded>,
    axum::Json(request): axum::Json<Value>,
) -> impl axum::response::IntoResponse {
    let has_result = request["messages"].as_array().is_some_and(|messages| {
        messages.iter().any(|m| {
            m["content"]
                .as_array()
                .is_some_and(|blocks| blocks.iter().any(|b| b["type"] == "tool_result"))
        })
    });
    let tool = request["tools"][0]["name"].clone();
    recorded.lock().push(request);
    let usage = json!({"input_tokens": 10, "output_tokens": 2});
    let body = if has_result {
        sse(&[
            json!({"type":"message_start","message":{"usage":usage}}),
            json!({"type":"content_block_start","index":0,"content_block":{"type":"text","text":""}}),
            json!({"type":"content_block_delta","index":0,"delta":{"type":"text_delta","text":"done"}}),
            json!({"type":"content_block_stop","index":0}),
            json!({"type":"message_delta","delta":{"stop_reason":"end_turn"},"usage":{"output_tokens":2}}),
            json!({"type":"message_stop"}),
        ])
    } else {
        sse(&[
            json!({"type":"message_start","message":{"usage":usage}}),
            json!({"type":"content_block_start","index":0,"content_block":{"type":"tool_use","id":"toolu_1","name":tool}}),
            json!({"type":"content_block_delta","index":0,"delta":{"type":"input_json_delta","partial_json":"{\"query\":\"fruit\"}"}}),
            json!({"type":"content_block_stop","index":0}),
            json!({"type":"message_delta","delta":{"stop_reason":"tool_use"},"usage":{"output_tokens":2}}),
            json!({"type":"message_stop"}),
        ])
    };
    ([("content-type", "text/event-stream")], body)
}

async fn start_model(recorded: Recorded) -> String {
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
        .await
        .expect("bind fake model");
    let addr = listener.local_addr().expect("addr");
    let router = axum::Router::new()
        .route("/v1/messages", axum::routing::post(messages))
        .with_state(recorded);
    tokio::spawn(async move { axum::serve(listener, router).await.expect("serve") });
    format!("http://{addr}")
}

fn test_config(model_url: &str) -> (tempfile::TempDir, Config) {
    let dir = tempfile::Builder::new()
        .prefix("kura-app-cache-")
        .tempdir()
        .expect("temp dir");
    let mut llm = kura_config::LlmConfig {
        default_provider: "claude-acct".to_string(),
        default_model: "claude-test".to_string(),
        default_timeout_ms: 30_000,
        ..Default::default()
    };
    llm.accounts.push(kura_config::AccountProviderConfig {
        id: "claude-acct".to_string(),
        title: "claude".to_string(),
        protocol: kura_config::AccountProtocol::AnthropicMessages,
        base_url: model_url.to_string(),
        model: "claude-test".to_string(),
        access_token: String::new(),
        headers: Default::default(),
    });
    let config = Config {
        project_root: Default::default(),
        environment: Environment::Test,
        bind_addr: "127.0.0.1:0".to_string(),
        data_dir: dir.path().to_string_lossy().into_owned(),
        log_level: "info".to_string(),
        version: "dev".to_string(),
        llm,
        connectors: Default::default(),
        egress: Default::default(),
        store: Default::default(),
    };
    (dir, config)
}

/// One active tenant with an Owner principal and a granted token, as a
/// provisioned shared deployment has (same shape as `multi_tenant_e2e`).
fn seed_tenant(app: &App, label: &str) -> String {
    let now = Utc::now();
    let tenant_id = format!("ten_{label}");
    let principal_id = format!("prn_{label}");
    let (token, secret) = app
        .state
        .auth
        .as_ref()
        .expect("auth manager")
        .issue_token(kura_identity::auth::IssueTokenInput {
            principal_id: principal_id.clone(),
            label: label.to_string(),
            default_tenant_id: tenant_id.clone(),
            expires_at: None,
        })
        .expect("issue token");
    let store = app.state.store.lock();
    store
        .upsert_tenant(
            &serde_json::from_value(serde_json::json!({
                "tenantId": tenant_id, "tenantKind": "organization", "displayName": label,
                "status": "active", "createdAt": now, "updatedAt": now,
            }))
            .expect("tenant"),
        )
        .expect("upsert tenant");
    store
        .upsert_principal(
            &serde_json::from_value(serde_json::json!({
                "principalId": principal_id, "principalKind": "user", "displayName": label,
                "status": "active", "defaultTenantId": tenant_id,
                "createdAt": now, "updatedAt": now,
            }))
            .expect("principal"),
        )
        .expect("upsert principal");
    store
        .upsert_membership(
            &serde_json::from_value(serde_json::json!({
                "membershipId": format!("mem_{label}"), "tenantId": tenant_id,
                "principalId": principal_id, "role": "owner", "status": "active",
                "createdAt": now, "updatedAt": now,
            }))
            .expect("membership"),
        )
        .expect("upsert membership");
    store
        .upsert_token_tenant_grant(
            &serde_json::from_value(serde_json::json!({
                "grantId": format!("grant_{label}"), "tokenId": token.token_id,
                "tenantId": tenant_id, "isDefault": true, "status": "active",
                "createdAt": now, "updatedAt": now,
            }))
            .expect("grant"),
        )
        .expect("upsert grant");
    secret
}

/// A thread as a connector creates it: continuity (earlier turns in the
/// prompt) applies only to a thread the store knows, with a live segment.
fn seed_thread(app: &App, tenant_id: &str, thread_id: &str) {
    let now = Utc::now();
    let segment_id = format!("seg_{thread_id}");
    let store = app.state.store.lock();
    store
        .upsert_thread(&kura_threads::Thread {
            thread_id: thread_id.to_string(),
            tenant_id: tenant_id.to_string(),
            lifecycle_state: kura_threads::LifecycleState::Active,
            current_session_segment_id: segment_id.clone(),
            source_kind: kura_threads::SourceKind::Shell,
            source_summary: "cache test".to_string(),
            last_activity_at: now,
            created_at: now,
            updated_at: now,
            retention_expires_at: Some(now + chrono::TimeDelta::days(30)),
            redaction_status: kura_threads::RedactionStatus::Redacted,
        })
        .expect("thread");
    store
        .upsert_thread_session_segment(&kura_threads::SessionSegment {
            session_segment_id: segment_id,
            thread_id: thread_id.to_string(),
            tenant_id: tenant_id.to_string(),
            session_id: String::new(),
            generation: 1,
            state: "active".to_string(),
            started_at: now,
            ended_at: None,
            last_active_at: now,
            reset_from_session_segment_id: String::new(),
            partial_evidence: false,
        })
        .expect("segment");
}

async fn ask(app: &App, secret: &str, thread: &str, query: &str) {
    let request = Request::builder()
        .method("POST")
        .uri("/v1/chat/query")
        .header("authorization", format!("Bearer {secret}"))
        .header("content-type", "application/json")
        .body(Body::from(
            json!({ "query": query, "threadId": thread }).to_string(),
        ))
        .expect("request");
    let response = app.router().oneshot(request).await.expect("oneshot");
    let status = response.status();
    let body = to_bytes(response.into_body(), usize::MAX)
        .await
        .expect("body");
    assert_eq!(status, StatusCode::OK, "{}", String::from_utf8_lossy(&body));
    let body: Value = serde_json::from_slice(&body).expect("json");
    assert_eq!(body["reply"], "done", "{body}");
}

/// The request without its cache markers: the provider reads a marker as
/// "cache up to here", not as content, so it is not part of the prefix.
fn unmarked(value: &Value) -> Value {
    match value {
        Value::Object(map) => Value::Object(
            map.iter()
                .filter(|(key, _)| *key != "cache_control")
                .map(|(key, value)| (key.clone(), unmarked(value)))
                .collect(),
        ),
        Value::Array(items) => Value::Array(items.iter().map(unmarked).collect()),
        other => other.clone(),
    }
}

/// Where two sequences first differ, if one is not a prefix of the other.
fn first_difference(earlier: &[Value], later: &[Value]) -> Option<usize> {
    if earlier.len() > later.len() {
        return Some(later.len());
    }
    earlier.iter().zip(later).position(|(a, b)| a != b)
}

#[tokio::test(flavor = "multi_thread")]
async fn every_request_keeps_the_cached_prefix_byte_stable() {
    let recorded: Recorded = Arc::default();
    let model_url = start_model(recorded.clone()).await;
    let (_dir, config) = test_config(&model_url);
    let app = App::new(config).expect("build app");
    let secret = seed_tenant(&app, "cache");
    seed_thread(&app, "ten_cache", "thr_cache_a");
    seed_thread(&app, "ten_cache", "thr_cache_b");

    // Two turns on one thread, a third on another; minutes apart in real
    // use, which is what a timestamp in the frame would break.
    ask(&app, &secret, "thr_cache_a", "What did I say about apples?").await;
    ask(&app, &secret, "thr_cache_a", "And about pears?").await;
    ask(&app, &secret, "thr_cache_a", "Which did I like more?").await;
    ask(&app, &secret, "thr_cache_b", "Anything about plums?").await;

    let bodies = recorded.lock().clone();
    // Each turn: a round that asks for the lookup, a round that answers.
    // Summaries or memory consolidation may add dispatches without tools.
    let rounds: Vec<&Value> = bodies.iter().filter(|b| b.get("tools").is_some()).collect();
    assert_eq!(rounds.len(), 8, "two rounds per turn: {}", bodies.len());
    eprintln!(
        "tools offered: {:?}",
        rounds[0]["tools"]
            .as_array()
            .unwrap()
            .iter()
            .map(|t| t["name"].as_str().unwrap_or(""))
            .collect::<Vec<_>>()
    );
    eprintln!("system: {}", unmarked(&rounds[0]["system"]));

    // 0. Tool names the API accepts (`^[a-zA-Z0-9_-]{1,64}$`): Kura's
    //    namespaced names (`memory.lookup`) are refused as they are. The
    //    model calls the wire name; the call must reach the real tool, so
    //    round two carries its result, not an unknown-tool error.
    for tool in rounds[0]["tools"].as_array().unwrap() {
        let name = tool["name"].as_str().unwrap();
        assert!(
            (1..=64).contains(&name.len())
                && name
                    .chars()
                    .all(|c| c.is_ascii_alphanumeric() || c == '_' || c == '-'),
            "{name:?} is not a valid tool name on the wire"
        );
    }
    let result = rounds[1]["messages"]
        .as_array()
        .unwrap()
        .iter()
        .flat_map(|m| m["content"].as_array().cloned().unwrap_or_default())
        .find(|block| block["type"] == "tool_result")
        .expect("a tool result");
    assert!(
        !result.to_string().to_lowercase().contains("unknown tool"),
        "the call did not reach the tool: {result}"
    );
    let replayed = rounds[1]["messages"]
        .as_array()
        .unwrap()
        .iter()
        .flat_map(|m| m["content"].as_array().cloned().unwrap_or_default())
        .find(|block| block["type"] == "tool_use")
        .expect("the call, replayed");
    assert_eq!(
        replayed["name"], rounds[0]["tools"][0]["name"],
        "history uses the wire name too"
    );

    // 1. The frame: tools and system, the same in every request.
    for (index, body) in rounds.iter().enumerate() {
        assert_eq!(
            unmarked(&body["tools"]),
            unmarked(&rounds[0]["tools"]),
            "tools changed at round {index}"
        );
        assert_eq!(
            unmarked(&body["system"]),
            unmarked(&rounds[0]["system"]),
            "system changed at round {index}:\n{}\nvs\n{}",
            unmarked(&body["system"]),
            unmarked(&rounds[0]["system"])
        );
    }

    // 2. Within a turn, the first round's messages are a prefix of the
    //    second's: the second round reads what the first wrote.
    for turn in rounds.chunks(2) {
        let first = unmarked(&turn[0]["messages"]);
        let second = unmarked(&turn[1]["messages"]);
        let (first, second) = (first.as_array().unwrap(), second.as_array().unwrap());
        assert_eq!(
            first_difference(first, second),
            None,
            "round 2 rewrote round 1's messages:\n{}\nvs\n{}",
            Value::Array(first.clone()),
            Value::Array(second.clone())
        );
        assert!(second.len() > first.len());
    }

    // 3. Breakpoints: the last system block, and the last message block of
    //    every round (tools are offered, so another round may follow).
    for body in &rounds {
        let system = body["system"].as_array().expect("system blocks");
        assert_eq!(system.last().unwrap()["cache_control"]["type"], "ephemeral");
        let last = body["messages"].as_array().unwrap().last().unwrap();
        let block = last["content"].as_array().unwrap().last().unwrap();
        assert_eq!(block["cache_control"]["type"], "ephemeral", "{last}");
    }

    // 4. Earlier turns come back (continuity) in the same bytes every time:
    //    turn three's history begins with turn two's.
    let history = |body: &Value| -> Vec<Value> {
        let messages = unmarked(&body["messages"]);
        let messages = messages.as_array().unwrap();
        // Everything before the profile messages, which follow the history.
        let end = messages
            .iter()
            .position(|m| m.to_string().contains("Agent profile"))
            .unwrap_or(0);
        messages[..end].to_vec()
    };
    let (second, third) = (history(rounds[2]), history(rounds[4]));
    assert_eq!(second.len(), 2, "turn two sees turn one: {second:?}");
    assert_eq!(
        third.len(),
        4,
        "turn three sees turns one and two: {third:?}"
    );
    assert_eq!(
        first_difference(&second, &third),
        None,
        "history rewritten between turns"
    );

    let frame = unmarked(&rounds[0]["tools"]).to_string().len()
        + unmarked(&rounds[0]["system"]).to_string().len();
    eprintln!(
        "frame (tools + system): {frame} bytes, about {} tokens",
        frame / 4
    );

    // What the next turn on the same thread reuses beyond the frame: its
    // first round against the previous turn's last. Printed, not asserted:
    // recalled memory sits ahead of the history (2.3, open).
    let previous = unmarked(&rounds[1]["messages"]);
    let next = unmarked(&rounds[2]["messages"]);
    let shared = previous
        .as_array()
        .unwrap()
        .iter()
        .zip(next.as_array().unwrap())
        .take_while(|(a, b)| a == b)
        .count();
    if std::env::var("KURA_CACHE_DUMP").is_ok() {
        for (label, messages) in [
            ("previous turn, last round", &previous),
            ("next turn, first round", &next),
        ] {
            eprintln!("--- {label}");
            for message in messages.as_array().unwrap() {
                eprintln!(
                    "{} {}",
                    message["role"],
                    message["content"]
                        .to_string()
                        .chars()
                        .take(160)
                        .collect::<String>()
                );
            }
        }
    }
    eprintln!(
        "next turn reuses {shared} of the previous turn's {} messages",
        previous.as_array().unwrap().len()
    );
    app.close();
}
