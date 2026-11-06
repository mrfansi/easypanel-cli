//! Native connection/account application adapters; config owns store transactions.

use anyhow::{anyhow, Result};
use serde_json::Value;

use crate::client::EasypanelClient;
use crate::cloudflare::{CloudflareAccount, CloudflareClient};
use crate::config::{CloudflareConfig, Server, ServerConfig};
use crate::operations::valid_name;

/// Save a complete connection without prompts. Identity comes from `original`,
/// never the selected explorer target; editing cannot change the saved default.
/// Blank credentials on edit keep their stored values.
pub(crate) fn save_server(
    cfg: &ServerConfig,
    original: Option<&str>,
    mut server: Server,
) -> Result<()> {
    cfg.update(|servers| {
        if servers
            .iter()
            .any(|s| s.name == server.name && Some(s.name.as_str()) != original)
        {
            anyhow::bail!("A server called '{}' already exists", server.name);
        }
        let index = match original {
            Some(name) => Some(
                servers
                    .iter()
                    .position(|s| s.name == name)
                    .ok_or_else(|| anyhow!("No server called '{name}'"))?,
            ),
            None => None,
        };
        if let Some(index) = index {
            let old = &servers[index];
            server.default = old.default;
            if server.token.is_empty() {
                server.token.clone_from(&old.token);
            }
            if let (Some(new), Some(old)) = (&mut server.ssh, &old.ssh) {
                if new.auth == old.auth && new.secret.as_deref().is_none_or(str::is_empty) {
                    new.secret.clone_from(&old.secret);
                }
            }
        } else {
            server.default = !servers.iter().any(|s| s.default);
        }
        server.url = server.url.trim().trim_end_matches('/').to_string();
        validate_server(&server)?;
        match index {
            Some(index) => servers[index] = server,
            None => servers.push(server),
        }
        Ok(())
    })
}

fn validate_server(server: &Server) -> Result<()> {
    if !valid_name(&server.name) {
        anyhow::bail!("Server names may only contain a-z, 0-9, - and _");
    }
    let url = reqwest::Url::parse(&server.url)
        .map_err(|_| anyhow!("Panel URL must be an absolute http:// or https:// URL"))?;
    if !matches!(url.scheme(), "http" | "https") || url.host_str().is_none() {
        anyhow::bail!("Panel URL must be an absolute http:// or https:// URL");
    }
    if !url.username().is_empty()
        || url.password().is_some()
        || url.query().is_some()
        || url.fragment().is_some()
    {
        anyhow::bail!("Panel URL must not contain login credentials, a query or a fragment");
    }
    if server.token.trim().is_empty() {
        anyhow::bail!("API token is required");
    }
    if let Some(tunnel) = &server.ssh {
        crate::tunnel::validate(tunnel, &server.url)?;
        if tunnel.auth == crate::tunnel::SshAuth::Password
            && tunnel.secret.as_deref().is_none_or(str::is_empty)
        {
            anyhow::bail!("SSH password is required");
        }
    }
    Ok(())
}

pub(crate) fn remove_server(cfg: &ServerConfig, name: &str) -> Result<()> {
    cfg.update(|servers| {
        if !servers.iter().any(|s| s.name == name) {
            anyhow::bail!("No server called '{name}'");
        }
        servers.retain(|s| s.name != name);
        if !servers.is_empty() && !servers.iter().any(|s| s.default) {
            servers[0].default = true;
        }
        Ok(())
    })
}

pub(crate) fn default_server(cfg: &ServerConfig, name: &str) -> Result<()> {
    cfg.update(|servers| {
        if !servers.iter().any(|s| s.name == name) {
            anyhow::bail!("No server called '{name}'");
        }
        for server in servers {
            server.default = server.name == name;
        }
        Ok(())
    })
}

/// Test an unsaved draft and return evidence rather than writing to stdout.
pub(crate) fn test_server(server: &Server) -> Result<String> {
    let result = (|| -> Result<String> {
        validate_server(server)?;
        let tunnel = server
            .ssh
            .as_ref()
            .map(|tunnel| crate::tunnel::test(tunnel, &server.url))
            .transpose()?;
        EasypanelClient::for_server(server)
            .call("projects", "listProjects", Value::Null)
            .map_err(|e| anyhow!("API check failed: {e}"))?;
        let api = format!("API token OK — {} answered", server.url);
        Ok(match tunnel {
            Some(tunnel) => format!("{tunnel}\n{api}"),
            None => api,
        })
    })();
    let redact = |text: &str| {
        std::iter::once(server.token.as_str())
            .chain(server.ssh.as_ref().and_then(|ssh| ssh.secret.as_deref()))
            .filter(|secret| !secret.is_empty())
            .fold(text.to_owned(), |text, secret| {
                text.replace(secret, "[redacted]")
            })
    };
    result
        .map(|text| redact(&text))
        .map_err(|error| anyhow!(redact(&format!("{error:#}"))))
}

/// Create or atomically edit/rename an account, preserving its default and order.
/// A blank API token on edit retains the saved token.
pub(crate) fn save_cf_account(
    cfg: &CloudflareConfig,
    original: Option<&str>,
    mut account: CloudflareAccount,
) -> Result<()> {
    cfg.update(|accounts| {
        if accounts
            .iter()
            .any(|a| a.name == account.name && Some(a.name.as_str()) != original)
        {
            anyhow::bail!(
                "A Cloudflare account called '{}' already exists",
                account.name
            );
        }
        let index = match original {
            Some(name) => Some(
                accounts
                    .iter()
                    .position(|a| a.name == name)
                    .ok_or_else(|| anyhow!("No Cloudflare account called '{name}'"))?,
            ),
            None => None,
        };
        if let Some(index) = index {
            account.default = accounts[index].default;
            if account.api_token.is_empty() {
                account.api_token.clone_from(&accounts[index].api_token);
            }
        } else {
            account.default = !accounts.iter().any(|a| a.default);
        }
        account.account_id = account
            .account_id
            .map(|id| id.trim().to_string())
            .filter(|id| !id.is_empty());
        validate_cf_account(&account)?;
        match index {
            Some(index) => accounts[index] = account,
            None => accounts.push(account),
        }
        Ok(())
    })
}

fn validate_cf_account(account: &CloudflareAccount) -> Result<()> {
    if !valid_name(&account.name) {
        anyhow::bail!("Cloudflare account names may only contain a-z, 0-9, - and _");
    }
    if account.api_token.trim().is_empty() {
        anyhow::bail!("Cloudflare API token is required");
    }
    Ok(())
}

pub(crate) fn remove_cf_account(cfg: &CloudflareConfig, name: &str) -> Result<()> {
    cfg.update(|accounts| {
        if !accounts.iter().any(|a| a.name == name) {
            anyhow::bail!("No Cloudflare account called '{name}'");
        }
        accounts.retain(|a| a.name != name);
        if !accounts.is_empty() && !accounts.iter().any(|a| a.default) {
            accounts[0].default = true;
        }
        Ok(())
    })
}

pub(crate) fn default_cf_account(cfg: &CloudflareConfig, name: &str) -> Result<()> {
    cfg.update(|accounts| {
        if !accounts.iter().any(|a| a.name == name) {
            anyhow::bail!("No Cloudflare account called '{name}'");
        }
        for account in accounts {
            account.default = account.name == name;
        }
        Ok(())
    })
}

/// Verify token validity independently of the token's product permissions.
pub(crate) fn test_cf_account(
    account: &CloudflareAccount,
    client: &CloudflareClient,
) -> Result<String> {
    test_cf_account_at(account, client.base_url())
}

fn test_cf_account_at(account: &CloudflareAccount, base: &str) -> Result<String> {
    validate_cf_account(account)?;
    // Cloudflare has separate verification routes for user-owned and
    // account-owned tokens; the saved account intentionally supports either.
    let http = reqwest::blocking::Client::builder()
        .timeout(std::time::Duration::from_secs(30))
        .build()?;
    let mut url = reqwest::Url::parse(&format!("{base}/user/tokens/verify"))?;
    let mut errors = Vec::new();
    for account_id in [None, account.account_id.as_deref()] {
        if let Some(id) = account_id {
            url = reqwest::Url::parse(base)?;
            url.path_segments_mut()
                .map_err(|_| anyhow!("Invalid Cloudflare API base URL"))?
                .pop_if_empty()
                .push("accounts")
                .push(id)
                .push("tokens")
                .push("verify");
        } else if !errors.is_empty() {
            break;
        }
        let response = http
            .get(url.clone())
            .bearer_auth(&account.api_token)
            .send()?;
        let body = response.text()?;
        match crate::cloudflare::parse_envelope::<Value>(&body) {
            Ok(result) => {
                let status = result
                    .get("status")
                    .and_then(Value::as_str)
                    .unwrap_or("unknown");
                if status != "active" {
                    anyhow::bail!("Cloudflare token is {status}, not active");
                }
                return Ok("Cloudflare API token is active. Product permissions are checked when each operation runs.".into());
            }
            Err(error) => errors.push(error.to_string()),
        }
    }
    anyhow::bail!(
        "Cloudflare token verification failed: {}",
        errors.join("; ")
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use httpmock::prelude::*;
    use serde_json::json;

    fn server(name: &str) -> Server {
        Server {
            name: name.into(),
            url: "https://panel.example.test".into(),
            token: "fixture-token".into(),
            default: false,
            ssh: None,
        }
    }

    fn account(name: &str) -> CloudflareAccount {
        CloudflareAccount {
            name: name.into(),
            api_token: "fixture-token".into(),
            account_id: Some("fixture-account".into()),
            default: false,
        }
    }

    #[test]
    fn gui_connection_saves_serialize_independent_records() {
        let dir = tempfile::tempdir().unwrap();
        let start = std::sync::Barrier::new(12);
        std::thread::scope(|scope| {
            for i in 0..12 {
                let root = dir.path();
                let start = &start;
                scope.spawn(move || {
                    start.wait();
                    let name = format!("host-{i}");
                    save_server(
                        &ServerConfig::new(root.join("servers.json")),
                        None,
                        server(&name),
                    )
                    .unwrap();
                    save_cf_account(
                        &CloudflareConfig::new(root.join("cloudflare.json")),
                        None,
                        account(&name),
                    )
                    .unwrap();
                });
            }
        });
        assert_eq!(
            ServerConfig::new(dir.path().join("servers.json"))
                .try_all()
                .unwrap()
                .len(),
            12
        );
        assert_eq!(
            CloudflareConfig::new(dir.path().join("cloudflare.json"))
                .try_all()
                .unwrap()
                .len(),
            12
        );
    }

    #[test]
    fn account_test_accepts_a_product_scoped_user_token_without_listing_zones() {
        let host = MockServer::start();
        let verify = host.mock(|when, then| {
            when.method(GET)
                .path("/user/tokens/verify")
                .header("authorization", "Bearer fixture-token");
            then.status(200)
                .json_body(json!({"success": true, "result": {"status": "active"}}));
        });
        assert!(test_cf_account_at(&account("user"), &host.base_url())
            .unwrap()
            .contains("active"));
        verify.assert();
    }

    #[test]
    fn account_test_uses_account_owned_token_route_and_refuses_inactive_tokens() {
        let host = MockServer::start();
        host.mock(|when, then| {
            when.method(GET).path("/user/tokens/verify");
            then.status(403).json_body(json!({
                "success": false, "errors": [{"code": 1000, "message": "Not a user token"}],
                "result": null
            }));
        });
        let active = host.mock(|when, then| {
            when.method(GET)
                .path("/accounts/fixture-account/tokens/verify");
            then.status(200)
                .json_body(json!({"success": true, "result": {"status": "active"}}));
        });
        assert!(test_cf_account_at(&account("account"), &host.base_url()).is_ok());
        active.assert();

        let expired_host = MockServer::start();
        expired_host.mock(|when, then| {
            when.method(GET).path("/user/tokens/verify");
            then.status(200)
                .json_body(json!({"success": true, "result": {"status": "expired"}}));
        });
        let error = test_cf_account_at(&account("expired"), &expired_host.base_url()).unwrap_err();
        assert!(error.to_string().contains("expired"));
    }

    #[test]
    fn connection_edit_renames_in_place_without_changing_default_or_ssh_credentials() {
        let dir = tempfile::tempdir().unwrap();
        let cfg = ServerConfig::new(dir.path().join("servers.json"));
        let mut original = server("prod");
        original.url = "http://localhost:3000".into();
        original.ssh = Some(crate::tunnel::SshTunnel {
            host: "ssh.example.test".into(),
            port: Some(2222),
            user: Some("operator".into()),
            auth: crate::tunnel::SshAuth::Password,
            secret: Some("fixture-password".into()),
            timeout: Some(23),
            key_path: None,
        });
        save_server(&cfg, None, original.clone()).unwrap();
        let mut second = server("staging");
        second.default = true; // Explorer selection is not a default-setting intent.
        save_server(&cfg, None, second).unwrap();
        assert_eq!(cfg.default().unwrap().name, "prod");

        let mut edited = original.clone();
        edited.name = "production".into();
        edited.token.clear();
        edited.ssh.as_mut().unwrap().secret = None;
        save_server(&cfg, Some("prod"), edited).unwrap();
        let saved = cfg.try_all().unwrap();
        assert_eq!(
            saved.iter().map(|s| s.name.as_str()).collect::<Vec<_>>(),
            ["production", "staging"]
        );
        assert!(saved[0].default);
        assert!(!saved[1].default);
        assert_eq!(saved[0].token, original.token);
        assert_eq!(saved[0].ssh, original.ssh);
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            assert_eq!(
                std::fs::metadata(dir.path().join("servers.json"))
                    .unwrap()
                    .permissions()
                    .mode()
                    & 0o777,
                0o600
            );
        }
    }

    #[test]
    fn connection_save_carries_every_key_login_field() {
        let dir = tempfile::tempdir().unwrap();
        let key = dir.path().join("fixture-key");
        std::fs::write(&key, "fixture-only").unwrap();
        let cfg = ServerConfig::new(dir.path().join("servers.json"));
        let mut draft = server("key-host");
        draft.url = "http://localhost:3000".into();
        let hop = crate::tunnel::SshTunnel {
            host: "key.example.test".into(),
            port: Some(2200),
            user: Some("operator".into()),
            auth: crate::tunnel::SshAuth::Key,
            key_path: Some(key.to_str().unwrap().into()),
            secret: Some("fixture-passphrase".into()),
            timeout: Some(27),
        };
        draft.ssh = Some(hop.clone());
        save_server(&cfg, None, draft).unwrap();
        assert_eq!(cfg.get("key-host").unwrap().ssh, Some(hop));
    }

    #[test]
    fn duplicate_or_missing_connection_identity_never_overwrites_a_credential() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("servers.json");
        let cfg = ServerConfig::new(path.clone());
        save_server(&cfg, None, server("prod")).unwrap();
        save_server(&cfg, None, server("stage")).unwrap();
        let before = std::fs::read(&path).unwrap();
        for (original, draft) in [
            (None, server("prod")),
            (Some("stage"), server("prod")),
            (Some("missing"), server("new")),
        ] {
            assert!(save_server(&cfg, original, draft).is_err());
            assert_eq!(std::fs::read(&path).unwrap(), before);
        }
        assert!(default_server(&cfg, "missing").is_err());
        assert!(remove_server(&cfg, "missing").is_err());
        assert_eq!(std::fs::read(&path).unwrap(), before);
        default_server(&cfg, "stage").unwrap();
        assert_eq!(cfg.default().unwrap().name, "stage");
        remove_server(&cfg, "stage").unwrap();
        assert_eq!(cfg.default().unwrap().name, "prod");
    }

    #[test]
    fn connection_save_rejects_unsafe_urls_without_creating_a_store() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("servers.json");
        let cfg = ServerConfig::new(path.clone());
        for url in [
            "panel.test",
            "file:///etc/passwd",
            "https://",
            "https://user:secret@panel.test",
            "https://panel.test?token=secret",
            "https://panel.test#fragment",
        ] {
            let mut draft = server("prod");
            draft.url = url.into();
            assert!(save_server(&cfg, None, draft).is_err(), "{url}");
            assert!(!path.exists());
        }
        let mut draft = server("prod");
        draft.url = "  https://panel.test/  ".into();
        save_server(&cfg, None, draft).unwrap();
        assert_eq!(cfg.get("prod").unwrap().url, "https://panel.test");
    }

    #[test]
    fn account_edit_preserves_default_order_and_blank_token() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("cloudflare.json");
        let cfg = CloudflareConfig::new(path.clone());
        save_cf_account(&cfg, None, account("primary")).unwrap();
        let mut second = account("other");
        second.default = true;
        save_cf_account(&cfg, None, second).unwrap();
        let mut edited = account("renamed");
        edited.api_token.clear();
        edited.account_id = Some(" updated-account ".into());
        save_cf_account(&cfg, Some("primary"), edited).unwrap();
        let saved = cfg.try_all().unwrap();
        assert_eq!(saved[0].name, "renamed");
        assert_eq!(saved[1].name, "other");
        assert!(saved[0].default);
        assert!(!saved[1].default);
        assert_eq!(saved[0].api_token, "fixture-token");
        assert_eq!(saved[0].account_id.as_deref(), Some("updated-account"));
        let before = std::fs::read(&path).unwrap();
        assert!(save_cf_account(&cfg, None, account("other")).is_err());
        assert!(save_cf_account(&cfg, Some("renamed"), account("other")).is_err());
        assert!(save_cf_account(&cfg, Some("missing"), account("new")).is_err());
        assert!(default_cf_account(&cfg, "missing").is_err());
        assert!(remove_cf_account(&cfg, "missing").is_err());
        assert_eq!(std::fs::read(&path).unwrap(), before);
        default_cf_account(&cfg, "other").unwrap();
        assert_eq!(cfg.default().unwrap().name, "other");
        remove_cf_account(&cfg, "other").unwrap();
        assert_eq!(cfg.default().unwrap().name, "renamed");
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            assert_eq!(
                std::fs::metadata(&path).unwrap().permissions().mode() & 0o777,
                0o600
            );
        }
    }

    #[test]
    fn corrupt_connection_stores_block_all_gui_writes() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("servers.json");
        std::fs::write(&path, "{broken credential store").unwrap();
        let cfg = ServerConfig::new(path.clone());
        assert!(save_server(&cfg, None, server("prod")).is_err());
        assert!(save_server(&cfg, Some("prod"), server("prod")).is_err());
        assert!(remove_server(&cfg, "prod").is_err());
        assert!(default_server(&cfg, "prod").is_err());
        assert_eq!(
            std::fs::read_to_string(&path).unwrap(),
            "{broken credential store"
        );

        let path = dir.path().join("cloudflare.json");
        std::fs::write(&path, "{broken token store").unwrap();
        let cfg = CloudflareConfig::new(path.clone());
        assert!(save_cf_account(&cfg, None, account("primary")).is_err());
        assert!(save_cf_account(&cfg, Some("primary"), account("primary")).is_err());
        assert!(remove_cf_account(&cfg, "primary").is_err());
        assert!(default_cf_account(&cfg, "primary").is_err());
        assert_eq!(
            std::fs::read_to_string(&path).unwrap(),
            "{broken token store"
        );
    }

    #[test]
    fn connection_test_returns_actual_api_result_without_saving() {
        let host = MockServer::start();
        let check = host.mock(|when, then| {
            when.method(POST).path("/api/rpc/projects/listProjects");
            then.status(200).json_body(json!({"json": []}));
        });
        let mut draft = server("draft");
        draft.url = host.base_url();
        assert!(test_server(&draft).unwrap().contains("API token OK"));
        check.assert();
    }

    #[test]
    fn connection_test_redacts_draft_credentials_even_before_a_target_is_bound() {
        let host = MockServer::start();
        host.mock(|when, then| {
            when.method(POST).path("/api/rpc/projects/listProjects");
            then.status(403).json_body(json!({"json": {
                "message": "fixture-token was rejected"
            }}));
        });
        let mut draft = server("draft");
        draft.url = host.base_url();
        let error = test_server(&draft).unwrap_err().to_string();
        assert!(!error.contains("fixture-token"));
        assert!(error.contains("[redacted]"));

        let dir = tempfile::tempdir().unwrap();
        draft.url = "http://localhost:3000".into();
        draft.ssh = Some(crate::tunnel::SshTunnel {
            host: "unused.test".into(),
            auth: crate::tunnel::SshAuth::Key,
            key_path: Some(
                dir.path()
                    .join("fixture-token-fixture-passphrase")
                    .to_str()
                    .unwrap()
                    .into(),
            ),
            secret: Some("fixture-passphrase".into()),
            ..Default::default()
        });
        let error = test_server(&draft).unwrap_err().to_string();
        assert!(!error.contains("fixture-token"));
        assert!(!error.contains("fixture-passphrase"));
        assert!(error.contains("SSH key not found"));
    }
}
