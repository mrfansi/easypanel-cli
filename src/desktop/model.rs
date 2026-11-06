use super::backend::{Backend, Lane, TrustContext};
use crate::{cloudflare::CloudflareAccount, config::Server};
use serde_json::Value;

pub(super) type DocId = u64;
#[derive(Clone, Debug, PartialEq, Eq)]
pub(super) enum PageKind {
    Fleet,
    Connections,
    Host,
    Project,
    Service,
    Cloudflare,
    Data,
    Terminal,
}
#[derive(Clone, Default)]
pub(super) struct Target {
    pub server: Option<Server>,
    pub account: Option<CloudflareAccount>,
    pub project: String,
    pub service: String,
    pub service_type: String,
    pub section: String,
    pub resource: Value,
}
impl Target {
    pub fn label(&self) -> String {
        let root = self
            .account
            .as_ref()
            .map(|a| a.name.as_str())
            .or_else(|| self.server.as_ref().map(|s| s.name.as_str()))
            .unwrap_or("Workspace");
        let mut parts = vec![root];
        if !self.project.is_empty() {
            parts.push(&self.project);
        }
        if !self.service.is_empty() {
            parts.push(&self.service);
        }
        if self.account.is_some() {
            if let Some(name) = self
                .resource
                .get("name")
                .or_else(|| self.resource.get("id"))
                .and_then(Value::as_str)
            {
                parts.push(name);
            }
        }
        if !self.section.is_empty() {
            parts.push(&self.section);
        }
        parts.join(" / ")
    }
    pub fn same_resource(&self, other: &Self) -> bool {
        // Include endpoint and token identity: editing a connection must not reuse a document bound to old credentials.
        self.server
            .as_ref()
            .map(|s| (&s.name, &s.url, &s.token, &s.ssh))
            == other
                .server
                .as_ref()
                .map(|s| (&s.name, &s.url, &s.token, &s.ssh))
            && self
                .account
                .as_ref()
                .map(|a| (&a.name, &a.api_token, &a.account_id))
                == other
                    .account
                    .as_ref()
                    .map(|a| (&a.name, &a.api_token, &a.account_id))
            && self.project == other.project
            && self.service == other.service
            && self.service_type == other.service_type
            && self.section == other.section
            && (self.account.is_none()
                || self
                    .resource
                    .get("id")
                    .or_else(|| self.resource.get("name"))
                    == other
                        .resource
                        .get("id")
                        .or_else(|| other.resource.get("name")))
    }
    fn secrets(&self) -> Vec<String> {
        let mut secrets = Vec::new();
        if let Some(server) = &self.server {
            secrets.push(server.token.clone());
            if let Some(secret) = server.ssh.as_ref().and_then(|ssh| ssh.secret.as_ref()) {
                secrets.push(secret.clone());
            }
        }
        if let Some(account) = &self.account {
            secrets.push(account.api_token.clone());
        }
        secrets
    }
}
pub(super) struct OpenPage {
    pub kind: PageKind,
    pub target: Target,
    pub title: String,
}
pub(super) enum Payload {
    Json(Value),
    Worker(crate::worker::Resp),
    Text(String),
}
pub(super) struct PageContext<'a> {
    pub target: &'a Target,
    pub servers: &'a [Server],
    pub accounts: &'a [CloudflareAccount],
    pub(super) server_config: &'a std::path::Path,
    pub(super) cloudflare_config: &'a std::path::Path,
    pub(super) watchlist: &'a std::path::Path,
    pub(super) doc: DocId,
    pub(super) backend: &'a mut Backend,
    pub(super) opened: &'a mut Vec<OpenPage>,
    pub(super) reload: &'a mut bool,
}
impl PageContext<'_> {
    pub fn server_config_path(&self) -> &std::path::Path {
        self.server_config
    }
    pub fn cloudflare_config_path(&self) -> &std::path::Path {
        self.cloudflare_config
    }
    pub fn watchlist_path(&self) -> &std::path::Path {
        self.watchlist
    }
    pub fn cloudflare_client(
        &self,
        account: &CloudflareAccount,
    ) -> crate::cloudflare::CloudflareClient {
        (self.backend.runtime.make_cf_client)(&account.api_token)
    }
    pub fn rpc(&mut self, key: &str, group: &str, op: &str, input: Value) {
        self.rpc_on(key, group, op, input, Lane::Interactive);
    }
    pub fn poll_rpc(&mut self, key: &str, group: &str, op: &str, input: Value) {
        self.rpc_on(key, group, op, input, Lane::Poll);
    }
    fn rpc_on(&mut self, key: &str, group: &str, op: &str, input: Value, lane: Lane) {
        let Some(server) = self.target.server.clone() else {
            self.missing_server(key);
            return;
        };
        let group = group.to_owned();
        let op = op.to_owned();
        let label = format!("{group} / {op}");
        self.backend.submit(
            self.doc,
            key,
            &label,
            TrustContext {
                target: self.target.label(),
                secrets: self.target.secrets(),
            },
            lane,
            Box::new(move |_| {
                Ok(Payload::Json(
                    crate::client::EasypanelClient::for_server(&server).call(&group, &op, input)?,
                ))
            }),
        );
    }
    pub fn request(&mut self, key: &str, req: crate::worker::Req) {
        let req = match req {
            crate::worker::Req::Cf(req) => {
                self.cloudflare(key, req);
                return;
            }
            req => req,
        };
        let lane = match &req {
            crate::worker::Req::DumpR2 { .. }
            | crate::worker::Req::DownloadR2 { .. }
            | crate::worker::Req::RestoreR2 { .. }
            | crate::worker::Req::CopyDb { .. }
            | crate::worker::Req::RestoreBackup { .. }
            | crate::worker::Req::CloneService { .. }
            | crate::worker::Req::Migrate { .. } => Lane::Transfer,
            _ => Lane::Interactive,
        };
        self.request_on(key, req, lane);
    }
    pub fn poll_request(&mut self, key: &str, req: crate::worker::Req) {
        self.request_on(key, req, Lane::Poll);
    }
    fn request_on(&mut self, key: &str, req: crate::worker::Req, lane: Lane) {
        let Some(server) = self.target.server.clone() else {
            self.missing_server(key);
            return;
        };
        self.backend.submit(
            self.doc,
            key,
            key,
            TrustContext {
                target: self.target.label(),
                secrets: self.target.secrets(),
            },
            lane,
            Box::new(move |emit| {
                let client = crate::client::EasypanelClient::for_server(&server);
                Ok(Payload::Worker(crate::worker::handle_req(
                    &client, req, &emit,
                )))
            }),
        );
    }
    pub fn cloudflare(&mut self, key: &str, req: crate::worker::CfReq) {
        let factory = self.backend.runtime.make_cf_client.clone();
        self.backend.submit(
            self.doc,
            key,
            key,
            TrustContext {
                target: self.target.label(),
                secrets: self.target.secrets(),
            },
            Lane::Interactive,
            Box::new(move |_| {
                Ok(Payload::Worker(crate::worker::handle_cf(
                    req,
                    factory.as_ref(),
                )))
            }),
        );
    }
    pub fn background(
        &mut self,
        key: &str,
        label: &str,
        run: impl FnOnce() -> anyhow::Result<Payload> + Send + 'static,
    ) {
        self.backend.submit(
            self.doc,
            key,
            label,
            TrustContext {
                target: self.target.label(),
                secrets: self.target.secrets(),
            },
            Lane::Interactive,
            Box::new(move |_| run()),
        );
    }
    pub fn background_poll(
        &mut self,
        key: &str,
        label: &str,
        run: impl FnOnce() -> anyhow::Result<Payload> + Send + 'static,
    ) {
        self.backend.submit(
            self.doc,
            key,
            label,
            TrustContext {
                target: self.target.label(),
                secrets: self.target.secrets(),
            },
            Lane::Poll,
            Box::new(move |_| run()),
        );
    }
    pub fn background_transfer(
        &mut self,
        key: &str,
        label: &str,
        run: impl FnOnce() -> anyhow::Result<Payload> + Send + 'static,
    ) {
        self.backend.submit(
            self.doc,
            key,
            label,
            TrustContext {
                target: self.target.label(),
                secrets: self.target.secrets(),
            },
            Lane::Transfer,
            Box::new(move |_| run()),
        );
    }
    pub fn background_progress(
        &mut self,
        key: &str,
        label: &str,
        run: impl FnOnce(std::sync::Arc<dyn Fn(String) + Send + Sync>) -> anyhow::Result<Payload>
            + Send
            + 'static,
    ) {
        self.backend.submit(
            self.doc,
            key,
            label,
            TrustContext {
                target: self.target.label(),
                secrets: self.target.secrets(),
            },
            Lane::Transfer,
            Box::new(move |emit| {
                run(std::sync::Arc::new(move |message| {
                    emit(crate::worker::Resp::Progress(message))
                }))
            }),
        );
    }
    pub fn take(&mut self, key: &str) -> Option<Result<Payload, String>> {
        self.backend.take(self.doc, key)
    }
    pub fn progress(&self, key: &str) -> Option<&str> {
        self.backend.progress(self.doc, key)
    }
    pub fn busy(&self, key: &str) -> bool {
        self.backend.busy(self.doc, key)
    }
    pub fn open(&mut self, page: OpenPage) {
        self.opened.push(page);
    }
    pub fn reload_connections(&mut self) {
        *self.reload = true;
    }
    fn missing_server(&mut self, key: &str) {
        self.backend.submit(
            self.doc,
            key,
            key,
            TrustContext {
                target: self.target.label(),
                secrets: Vec::new(),
            },
            Lane::Interactive,
            Box::new(|_| anyhow::bail!("Choose an EasyPanel server for this document.")),
        );
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn independent_sections_and_changed_credentials_do_not_retarget_documents() {
        let a = Target {
            server: Some(Server {
                name: "host".into(),
                url: "https://panel.invalid".into(),
                token: "one".into(),
                default: false,
                ssh: None,
            }),
            section: "env".into(),
            ..Target::default()
        };
        let mut b = a.clone();
        assert!(a.same_resource(&b));
        b.section = "source".into();
        assert!(!a.same_resource(&b));
        b = a.clone();
        b.server.as_mut().unwrap().token = "two".into();
        assert!(!a.same_resource(&b));
        assert_eq!(a.server.as_ref().unwrap().token, "one");
    }
    #[test]
    fn refreshed_metadata_reuses_service_and_cloudflare_documents_but_not_other_resources() {
        let a = Target {
            project: "shop".into(),
            service: "api".into(),
            resource: serde_json::json!({"enabled":true}),
            ..Target::default()
        };
        let mut b = a.clone();
        b.resource = Value::Null;
        assert!(a.same_resource(&b));
        let a = Target {
            account: Some(CloudflareAccount {
                name: "account".into(),
                api_token: "token".into(),
                account_id: None,
                default: false,
            }),
            resource: serde_json::json!({"id":"zone-a","status":"pending"}),
            ..Target::default()
        };
        let mut b = a.clone();
        b.resource = serde_json::json!({"id":"zone-a","status":"active"});
        assert!(a.same_resource(&b));
        b.resource = serde_json::json!({"id":"zone-b"});
        assert!(!a.same_resource(&b));
    }
}
