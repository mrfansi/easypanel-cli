use anyhow::{bail, Result};
use serde_json::{json, Value};

/// EasyPanel tRPC client: POST {url}/api/rpc/{group}/{op}, body {"json": input}.
///
/// Cloning shares the same reqwest connection pool, so it's cheap: used to give
/// each TUI worker its own client.
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
            // Timeout is mandatory: without it, one hanging request freezes the
            // TUI worker forever (no other request can run).
            http: reqwest::blocking::Client::builder()
                .timeout(std::time::Duration::from_secs(30))
                .build()
                .unwrap_or_default(),
        }
    }

    /// Call the endpoint and return the `.json` payload from the response.
    /// Panel URL (without a trailing slash). Used to build the terminal WebSocket URL.
    pub fn url(&self) -> &str {
        &self.url
    }
    /// API token. Used as the `token` query param on the terminal WebSocket.
    pub fn token(&self) -> &str {
        &self.token
    }

    pub fn call(&self, group: &str, op: &str, input: Value) -> Result<Value> {
        self.call_within(group, op, input, None)
    }

    /// Like `call`, but with its own timeout.
    ///
    /// Some operations take far longer than others: `createService` responds in
    /// 0.2 seconds without a source, but 101 seconds when a GitHub source is
    /// included — measured directly against the server. With a 30-second limit,
    /// that request gets cut off WHILE the server keeps completing it, so the
    /// user sees "failed" and then finds the service exists on the next try.
    /// Raising the global timeout isn't the answer: it would force every other
    /// call to wait two minutes before it's allowed to report failure.
    pub fn call_within(
        &self,
        group: &str,
        op: &str,
        input: Value,
        timeout: Option<std::time::Duration>,
    ) -> Result<Value> {
        let endpoint = format!("{}/api/rpc/{}/{}", self.url, group, op);

        let mut r = self
            .http
            .post(&endpoint)
            .bearer_auth(&self.token)
            .json(&json!({ "json": input }));
        if let Some(t) = timeout {
            r = r.timeout(t);
        }
        let resp = r.send()?;

        let status = resp.status();
        if !status.is_success() {
            if status.as_u16() == 401 {
                bail!("Invalid or expired token (401).");
            }
            let body: Value = resp.json().unwrap_or(Value::Null);
            // EasyPanel puts the error message INSIDE "json", same as a success
            // response: {"json":{"code":"BAD_REQUEST","message":"Branch not found"}}.
            // Reading only a top-level "message" would discard every server
            // message and replace it with a generic status name ("Bad Request").
            let msg = ["/json/message", "/message", "/json/error", "/error"]
                .iter()
                .find_map(|p| body.pointer(p).and_then(Value::as_str))
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
        assert!(err.to_string().contains("Invalid or expired token"));
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

    #[test]
    fn surfaces_message_nested_under_json() {
        // The actual EasyPanel error shape (verified against the server): the
        // message is inside "json", not at the top level. The old mock used the
        // top-level shape, so the test passed while the real message was
        // discarded and the user only saw "[400] Bad Request".
        let server = MockServer::start();
        server.mock(|when, then| {
            when.method(POST);
            then.status(400).json_body(json!({
                "json": { "code": "BAD_REQUEST", "status": 400, "message": "Branch not found" }
            }));
        });

        let client = EasypanelClient::new(&server.base_url(), "tok123");
        let err = client
            .call("services/app", "updateSourceGithub", Value::Null)
            .unwrap_err();
        assert!(err.to_string().contains("Branch not found"), "{err}");
    }
}
