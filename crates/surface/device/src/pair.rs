//! Trading a pairing code for a device token (`POST /v1/devices/pair`,
//! docs/harness/remote-computer-use.md §4).

use serde::Deserialize;
use serde_json::json;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Paired {
    pub device_id: String,
    pub token: String,
}

#[derive(Deserialize)]
struct Response {
    device: Device,
    token: String,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct Device {
    device_id: String,
}

/// The code is spent by this call whether or not the answer arrives, so a
/// failure after sending it means asking the user for a new code.
pub async fn pair(server: &str, code: &str, name: &str) -> Result<Paired, String> {
    let code = code.trim();
    if !code.starts_with("kpair.") {
        return Err("a pairing code starts with `kpair.`".into());
    }
    let url = format!("{}/v1/devices/pair", server.trim_end_matches('/'));
    let client = reqwest::Client::builder()
        .timeout(std::time::Duration::from_secs(30))
        .build()
        .map_err(|err| err.to_string())?;
    let response = client
        .post(&url)
        .json(&json!({"code": code, "name": name, "platform": std::env::consts::OS}))
        .send()
        .await
        .map_err(|err| format!("{url}: {err}"))?;
    let status = response.status();
    if status.as_u16() == 401 {
        return Err("the code is unknown, already used, or expired; ask for a new one".into());
    }
    if !status.is_success() {
        let body = response.text().await.unwrap_or_default();
        return Err(format!(
            "{url}: {status}: {}",
            body.chars().take(300).collect::<String>()
        ));
    }
    let body: Response = response
        .json()
        .await
        .map_err(|err| format!("{url}: unexpected answer: {err}"))?;
    if !body.token.starts_with("kdev.") {
        return Err(format!("{url}: the answer carries no device token"));
    }
    Ok(Paired {
        device_id: body.device.device_id,
        token: body.token,
    })
}
