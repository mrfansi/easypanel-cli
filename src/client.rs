use anyhow::{bail, Result};
use serde_json::{json, Value};

/// Klien tRPC EasyPanel: POST {url}/api/rpc/{group}/{op}, body {"json": input}.
///
/// Clone berbagi connection pool reqwest yang sama, jadi murah: dipakai untuk
/// memberi tiap worker TUI klien sendiri.
#[derive(Clone)]
pub struct EasypanelClient {
    url: String,
    token: String,
    http: reqwest::blocking::Client,
}

impl EasypanelClient {
    pub fn new(url: &str, token: &str) -> Self {
        Self {
            url: url.trim_end_matches('/').to_string(),
            token: token.to_string(),
            // Timeout wajib: tanpa ini satu request menggantung membekukan
            // worker TUI selamanya (tak ada request lain yang bisa jalan).
            http: reqwest::blocking::Client::builder()
                .timeout(std::time::Duration::from_secs(30))
                .build()
                .unwrap_or_default(),
        }
    }

    /// Panggil endpoint dan kembalikan payload `.json` dari respons.
    pub fn call(&self, group: &str, op: &str, input: Value) -> Result<Value> {
        let endpoint = format!("{}/api/rpc/{}/{}", self.url, group, op);

        let resp = self
            .http
            .post(&endpoint)
            .bearer_auth(&self.token)
            .json(&json!({ "json": input }))
            .send()?;

        let status = resp.status();
        if !status.is_success() {
            if status.as_u16() == 401 {
                bail!("Token tidak valid atau kadaluarsa (401).");
            }
            let body: Value = resp.json().unwrap_or(Value::Null);
            let msg = body
                .get("message")
                .and_then(Value::as_str)
                .or_else(|| body.get("error").and_then(Value::as_str))
                .unwrap_or_else(|| status.canonical_reason().unwrap_or("error"));
            bail!("[{}] {}", status.as_u16(), msg);
        }

        let body: Value = resp.json()?;
        Ok(body.get("json").cloned().unwrap_or(Value::Null))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use httpmock::prelude::*;

    #[test]
    fn posts_json_envelope_with_bearer_and_unwraps_json() {
        let server = MockServer::start();
        let mock = server.mock(|when, then| {
            when.method(POST)
                .path("/api/rpc/projects/listProjects")
                .header("authorization", "Bearer tok123")
                .json_body(json!({ "json": null }));
            then.status(200)
                .json_body(json!({ "json": [{ "name": "proj-a" }], "meta": [] }));
        });

        let client = EasypanelClient::new(&server.base_url(), "tok123");
        let result = client
            .call("projects", "listProjects", Value::Null)
            .unwrap();

        mock.assert();
        assert_eq!(result, json!([{ "name": "proj-a" }]));
    }

    #[test]
    fn sends_given_input_wrapped_in_json() {
        let server = MockServer::start();
        let mock = server.mock(|when, then| {
            when.method(POST)
                .path("/api/rpc/projects/createProject")
                .json_body(json!({ "json": { "name": "proj-a" } }));
            then.status(200)
                .json_body(json!({ "json": { "ok": true } }));
        });

        let client = EasypanelClient::new(&server.base_url(), "tok123");
        client
            .call("projects", "createProject", json!({ "name": "proj-a" }))
            .unwrap();

        mock.assert();
    }

    #[test]
    fn maps_401_to_friendly_message() {
        let server = MockServer::start();
        server.mock(|when, then| {
            when.method(POST);
            then.status(401)
                .json_body(json!({ "message": "Unauthorized" }));
        });

        let client = EasypanelClient::new(&server.base_url(), "tok123");
        let err = client
            .call("projects", "listProjects", Value::Null)
            .unwrap_err();
        assert!(err.to_string().contains("Token tidak valid"));
    }

    #[test]
    fn surfaces_api_message_on_other_errors() {
        let server = MockServer::start();
        server.mock(|when, then| {
            when.method(POST);
            then.status(500).json_body(json!({ "message": "Boom" }));
        });

        let client = EasypanelClient::new(&server.base_url(), "tok123");
        let err = client
            .call("projects", "listProjects", Value::Null)
            .unwrap_err();
        assert!(err.to_string().contains("Boom"));
    }
}
