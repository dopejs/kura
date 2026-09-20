use std::io::{Read, Write};
use std::net::TcpListener;
use std::thread;

use kura_adapterprovider::{Handler, Operation};
use kura_feishulark::{
    AMBIGUOUS_CODE, Client, FaultKind, ScopedToken, adapter_failure_kind, ambiguous_fault,
    feishu_code_fault, http_status_fault, new_calendar_provider, new_mail_provider, parse_token,
};
use serde_json::value::RawValue;

fn mock_http(status: &'static str, body: &'static str) -> String {
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let addr = listener.local_addr().unwrap();
    let body = body.to_string();
    let status = status.to_string();
    thread::spawn(move || {
        if let Ok((mut stream, _)) = listener.accept() {
            let _ = stream.set_read_timeout(Some(std::time::Duration::from_millis(80)));
            let mut buf = [0u8; 2048];
            loop {
                match stream.read(&mut buf) {
                    Ok(0) => break,
                    Ok(_) => continue,
                    Err(_) => break,
                }
            }
            let resp = format!(
                "{status}\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
                body.len()
            );
            let _ = stream.write_all(resp.as_bytes());
            let _ = stream.flush();
        }
    });
    format!("http://{addr}")
}

#[derive(Debug, Default, serde::Deserialize)]
#[serde(rename_all = "snake_case")]
struct MailboxResp {
    #[serde(default)]
    mailbox_address: String,
}

#[test]
fn parse_token_fails_closed_on_empty() {
    let err = parse_token(&[]).unwrap_err();
    assert_eq!(err.kind, FaultKind::Auth);
    assert_eq!(err.code, "access_token_missing");
}

#[test]
fn parse_token_fails_closed_on_missing_token() {
    let err = parse_token(br#"{"tokenType":"user"}"#).unwrap_err();
    assert_eq!(err.code, "access_token_missing");
}

#[test]
fn parse_token_decodes_envelope() {
    let token =
        parse_token(br#"{"accessToken":"secret","grantedScopes":["calendar:write"]}"#).unwrap();
    assert_eq!(token.access_token, "secret");
    assert_eq!(token.granted_scopes, vec!["calendar:write"]);
}

#[test]
fn has_scope_empty_grants() {
    let token = ScopedToken::default();
    assert!(token.has_scope("calendar:write"));
}

#[test]
fn has_scope_matches_case_insensitive() {
    let token = ScopedToken {
        granted_scopes: vec!["Calendar:Write".to_string()],
        ..ScopedToken::default()
    };
    assert!(token.has_scope("calendar:write"));
    assert!(!token.has_scope("mail:read"));
}

#[test]
fn http_status_fault_mapping() {
    assert!(http_status_fault(200, false).is_none());
    assert_eq!(http_status_fault(401, false).unwrap().code, "token_expired");
    assert_eq!(
        http_status_fault(403, false).unwrap().kind,
        FaultKind::Scope
    );
    assert_eq!(
        http_status_fault(429, false).unwrap().kind,
        FaultKind::RateLimited
    );
    assert!(http_status_fault(500, true).unwrap().is_ambiguous());
    assert_eq!(
        http_status_fault(500, false).unwrap().code,
        "service_unavailable"
    );
    assert_eq!(
        http_status_fault(418, false).unwrap().code,
        "provider_unavailable"
    );
}

#[test]
fn feishu_code_fault_mapping() {
    assert_eq!(feishu_code_fault(99991669).code, "scope_not_granted");
    assert_eq!(feishu_code_fault(99991677).code, "token_expired");
    assert_eq!(feishu_code_fault(429).kind, FaultKind::RateLimited);
    assert_eq!(feishu_code_fault(12345).code, "provider_error_12345");
}

#[test]
fn adapter_failure_kind_mapping() {
    assert_eq!(adapter_failure_kind(FaultKind::Auth).as_str(), "auth");
    assert_eq!(adapter_failure_kind(FaultKind::Scope).as_str(), "scope");
    assert_eq!(
        adapter_failure_kind(FaultKind::Internal).as_str(),
        "internal"
    );
}

#[test]
fn ambiguous_fault_is_marked() {
    let fault = ambiguous_fault("boom");
    assert!(fault.is_ambiguous());
    assert_eq!(fault.code, AMBIGUOUS_CODE);
}

#[test]
fn client_new_trims_and_defaults() {
    assert_eq!(
        Client::new("https://open.feishu.cn/").base_url(),
        "https://open.feishu.cn"
    );
    assert_eq!(Client::new("  ").base_url(), "https://open.feishu.cn");
    assert_eq!(
        Client::new("http://127.0.0.1:9").base_url(),
        "http://127.0.0.1:9"
    );
}

#[test]
fn client_call_decodes_ok_envelope() {
    let base = mock_http(
        "HTTP/1.1 200 OK",
        r#"{"code":0,"msg":"ok","data":{"mailbox_address":"a@x.com"}}"#,
    );
    let client = Client::new(&base);
    let mut out = MailboxResp::default();
    client
        .call(None, "GET", "/x", "tok", None::<&()>, Some(&mut out), false)
        .unwrap();
    assert_eq!(out.mailbox_address, "a@x.com");
}

#[test]
fn client_call_maps_401_to_token_expired() {
    let base = mock_http("HTTP/1.1 401 Unauthorized", "");
    let client = Client::new(&base);
    let err = client
        .call(
            None,
            "GET",
            "/x",
            "tok",
            None::<&()>,
            None::<&mut MailboxResp>,
            false,
        )
        .unwrap_err();
    assert_eq!(err.kind, FaultKind::Auth);
    assert_eq!(err.code, "token_expired");
}

#[test]
fn client_call_maps_feishu_code() {
    let base = mock_http(
        "HTTP/1.1 200 OK",
        r#"{"code":99991669,"msg":"denied","data":null}"#,
    );
    let client = Client::new(&base);
    let err = client
        .call(
            None,
            "GET",
            "/x",
            "tok",
            None::<&()>,
            None::<&mut MailboxResp>,
            false,
        )
        .unwrap_err();
    assert_eq!(err.kind, FaultKind::Scope);
    assert_eq!(err.code, "scope_not_granted");
}

#[test]
fn calendar_project_account_roundtrip() {
    let base = mock_http(
        "HTTP/1.1 200 OK",
        r#"{"code":0,"msg":"ok","data":{"calendars":[{"calendar":{"calendar_id":"cal_1","summary":"Primary"},"user_id":"u_1"}]}}"#,
    );
    let provider = new_calendar_provider(Client::new(&base));
    let resource = kura_integrations::Resource {
        integration_id: "cal_1".to_string(),
        domain_kind: "calendar".to_string(),
        ..Default::default()
    };
    let op = Operation {
        domain: "calendar".to_string(),
        operation: "ProjectAccount".to_string(),
        resource: Some(RawValue::from_string(serde_json::to_string(&resource).unwrap()).unwrap()),
        credential: Some(RawValue::from_string(r#"{"accessToken":"tok"}"#.to_string()).unwrap()),
        payload: None,
    };
    let result = provider.handle(op, None).unwrap().unwrap();
    let account: kura_calendar::AccountProjection = serde_json::from_str(result.get()).unwrap();
    assert_eq!(account.primary_calendar_ref, "cal_1");
    assert_eq!(account.integration_id, "cal_1");
}

#[test]
fn mail_project_account_roundtrip() {
    let base = mock_http(
        "HTTP/1.1 200 OK",
        r#"{"code":0,"msg":"ok","data":{"mailbox_address":"a@x.com","name":"Alice"}}"#,
    );
    let provider = new_mail_provider(Client::new(&base));
    let resource = kura_integrations::Resource {
        integration_id: "m_1".to_string(),
        domain_kind: "mail".to_string(),
        ..Default::default()
    };
    let op = Operation {
        domain: "mail".to_string(),
        operation: "ProjectAccount".to_string(),
        resource: Some(RawValue::from_string(serde_json::to_string(&resource).unwrap()).unwrap()),
        credential: Some(RawValue::from_string(r#"{"accessToken":"tok"}"#.to_string()).unwrap()),
        payload: None,
    };
    let result = provider.handle(op, None).unwrap().unwrap();
    let account: kura_mail::AccountProjection = serde_json::from_str(result.get()).unwrap();
    assert_eq!(account.mailbox_address, "a@x.com");
    assert_eq!(account.integration_id, "m_1");
}
