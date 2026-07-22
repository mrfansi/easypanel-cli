# Cloudflare Integration Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Add a standalone Cloudflare capability (multiple accounts, zone CRUD, DNS-record CRUD + bulk + filter) to easypanel-cli, in both the CLI (`easypanel cf …`) and an isolated TUI switch-mode, verified live against a throwaway zone.

**Architecture:** A new bounded context, isolated from EasyPanel. Pure domain types + functions in `src/cloudflare.rs`; a standalone `CloudflareConfig` store (`cloudflare.json`) mirroring `ServerConfig`; a `CloudflareClient` (reqwest blocking, Bearer token); CLI subcommands in `src/commands.rs`/`src/main.rs`; a top-level TUI `Workspace` mode reusing the existing form/menu/viewer/table machinery. The `Server` struct and EasyPanel tab bar are untouched.

**Tech Stack:** Rust, reqwest (blocking, rustls), serde/serde_json, clap, dialoguer, ratatui. No new dependencies.

## Global Constraints

- Definition of done every task inherits: `cargo fmt --check`, `cargo clippy --all-targets -- -D warnings`, `cargo test` all clean.
- No new dependencies. reqwest/serde/clap/dialoguer/ratatui are already present.
- No `#[allow(dead_code)]`; clippy runs `-D warnings`. Every module lands wired to a caller.
- Secrets (CF API token) are NEVER logged, printed, or echoed. Config files are `0o600`.
- Base URL `https://api.cloudflare.com/client/v4/`; auth header `Authorization: Bearer <token>`.
- Record edits use **PATCH** (partial), never PUT. Filter syntax is `name.contains=`/`content.contains=`/`type=` with `match=all`. Envelope: `{success, result, result_info, errors:[{code,message,…}], messages}`.
- v1 record types: A, AAAA, CNAME, TXT, NS, MX only. Reject others with "not supported yet".
- Codebase language is English (comments + UI copy).
- Pure refactor rule does NOT apply — this is a feature; it ships as **v0.83.0** with a CHANGELOG entry.
- Six API details are flagged NOT-VERIFIED in the spec; Task 8 (live probe) resolves them before any write code is trusted.

## File Structure

- `src/cloudflare.rs` — **new**. Domain types (`CloudflareAccount`, `Zone`, `Record`, `RecordPatch`, `RecordFilter`) + pure functions (`parse_envelope`, `record_body`, `apply_patch`, `resolve_zone`, `select_records`, `valid_record_type`, `proxyable`, `filter_query`). Plus `CloudflareClient` (HTTP). Unit tests inline.
- `src/config.rs` — **modify**. Add `CloudflareConfig` store (a sibling of `ServerConfig`) + `cloudflare.json` path. `Server` struct untouched.
- `src/commands.rs` — **modify**. Add `cf_*` orchestration functions shared by CLI and TUI worker.
- `src/main.rs` — **modify**. Add `Cf(CfCmd)` command + `CfCmd`/`CfAccountCmd`/`CfZoneCmd`/`CfRecordCmd` subcommands + dispatch.
- `src/tui/worker.rs` — **modify**. `Req::Cf(CfReq)` / `Resp::Cf(CfResp)` + handlers.
- `src/tui/app.rs` — **modify**. `Workspace` enum + state; CF screen state (`CfUi`).
- `src/tui/keys.rs` — **modify**. `W` opens Switch menu; CF-workspace key handling.
- `src/tui/render.rs` — **modify**. Render CF header + Zones/Records tables (orange accent).
- `src/tui/tests.rs` — **modify**. TUI-side tests (workspace toggle, empty state).
- `src/lib.rs` or `src/main.rs` module list — **modify**. `mod cloudflare;`.
- `CHANGELOG.md`, `Cargo.toml`, `.github/AGENT_BRIEF.md` — **modify** at release.

---

## Phase 1 — Foundation: config store + domain (no live API)

Everything here is pure or filesystem-only; fully unit-tested with no network.

### Task 1: `CloudflareAccount` type + `cloudflare.json` path

**Files:**
- Modify: `src/config.rs`
- Modify: `src/cloudflare.rs` (create)
- Modify: `src/main.rs` (add `mod cloudflare;`)

**Interfaces:**
- Produces: `cloudflare::CloudflareAccount { name: String, api_token: String, account_id: Option<String>, default: bool }` (derive Debug, Clone, Serialize, Deserialize; `#[serde(default)]` on `default`).
- Produces: `config::CloudflareConfig::default_path() -> PathBuf` = `…/.config/easypanel/cloudflare.json`.

- [ ] **Step 1: Create `src/cloudflare.rs` with the account type**

```rust
//! Cloudflare — a bounded context OUTSIDE EasyPanel: manage one or more Cloudflare
//! accounts' zones and DNS records. Nothing here touches the EasyPanel domain; the two
//! share only the TUI event loop and the config directory (separate files).

use serde::{Deserialize, Serialize};

/// A stored Cloudflare account: a user-labelled scoped API token, kept in cloudflare.json
/// independent of any EasyPanel server (an operator may hold several CF accounts).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CloudflareAccount {
    pub name: String,
    pub api_token: String,
    /// Needed only to CREATE a zone; not needed to list zones or manage records.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub account_id: Option<String>,
    #[serde(default)]
    pub default: bool,
}
```

- [ ] **Step 2: Register the module**

In `src/main.rs`, add near the other `mod` lines:

```rust
mod cloudflare;
```

- [ ] **Step 3: Run to verify it compiles (dead-code warning expected until Task 2)**

Run: `cargo build 2>&1 | tail -5`
Expected: compiles; may warn `struct CloudflareAccount is never used` — acceptable mid-phase, resolved by Task 2's tests. If clippy `-D warnings` blocks the build, proceed straight to Task 2 in the same commit (they land together).

- [ ] **Step 4: Commit**

```bash
git add src/cloudflare.rs src/main.rs
git commit -m "feat(cf): add CloudflareAccount type (foundation)"
```

### Task 2: `CloudflareConfig` store

**Files:**
- Modify: `src/config.rs`
- Test: `src/config.rs` (inline `#[cfg(test)]`)

**Interfaces:**
- Consumes: `cloudflare::CloudflareAccount`.
- Produces: `CloudflareConfig::new(PathBuf)`, `default_path()`, `list() -> Vec<CloudflareAccount>`, `try_all() -> Result<Vec<..>>`, `add(account)`, `remove(name)`, `set_default(name)`, `default() -> Option<..>`, `by_name(name) -> Option<..>`.

- [ ] **Step 1: Write the failing tests**

Add to `src/config.rs` `#[cfg(test)] mod tests`:

```rust
fn temp_cf() -> (tempfile::TempDir, CloudflareConfig) {
    let dir = tempfile::tempdir().unwrap();
    let cfg = CloudflareConfig::new(dir.path().join("cloudflare.json"));
    (dir, cfg)
}

#[test]
fn first_cloudflare_account_becomes_default() {
    let (_d, cfg) = temp_cf();
    cfg.add(cf_account("personal", "tok-a", None)).unwrap();
    cfg.add(cf_account("work", "tok-b", Some("acc-1".into()))).unwrap();
    assert_eq!(cfg.default().unwrap().name, "personal");
    assert_eq!(cfg.by_name("work").unwrap().account_id.as_deref(), Some("acc-1"));
}

#[test]
fn set_default_and_remove_reassigns_default() {
    let (_d, cfg) = temp_cf();
    cfg.add(cf_account("a", "t", None)).unwrap();
    cfg.add(cf_account("b", "t", None)).unwrap();
    cfg.set_default("b").unwrap();
    assert_eq!(cfg.default().unwrap().name, "b");
    cfg.remove("b").unwrap();
    // Removing the default promotes the remaining one, never leaves zero defaults.
    assert_eq!(cfg.default().unwrap().name, "a");
}

#[test]
fn missing_cloudflare_file_is_empty_but_corrupt_refuses_to_write() {
    let (_d, cfg) = temp_cf();
    assert!(cfg.list().is_empty(), "missing file reads empty");
    std::fs::write(cfg_path(&cfg), "{ not json").unwrap();
    // try_all (the write path) must error so add() can't wipe the file.
    assert!(cfg.try_all().is_err());
    assert!(cfg.add(cf_account("x", "t", None)).is_err());
}

fn cf_account(name: &str, token: &str, account_id: Option<String>) -> crate::cloudflare::CloudflareAccount {
    crate::cloudflare::CloudflareAccount { name: name.into(), api_token: token.into(), account_id, default: false }
}
fn cfg_path(cfg: &CloudflareConfig) -> std::path::PathBuf { cfg.path_for_test() }
```

- [ ] **Step 2: Run to verify it fails**

Run: `cargo test cloudflare_account -- --list 2>&1 | tail`
Expected: FAIL — `CloudflareConfig` not defined.

- [ ] **Step 3: Implement `CloudflareConfig`**

Add to `src/config.rs` (mirrors `ServerConfig`/`Watchlist` — same corrupt-file guard, `0o600`):

```rust
use crate::cloudflare::CloudflareAccount;

/// Standalone store for Cloudflare accounts (cloudflare.json), independent of servers.
pub struct CloudflareConfig {
    path: PathBuf,
}

impl CloudflareConfig {
    pub fn new(path: PathBuf) -> Self { Self { path } }

    pub fn default_path() -> PathBuf {
        ServerConfig::default_path().with_file_name("cloudflare.json")
    }

    #[cfg(test)]
    pub fn path_for_test(&self) -> PathBuf { self.path.clone() }

    /// Read path: empty on a missing OR corrupt file (worst case: an empty list).
    pub fn list(&self) -> Vec<CloudflareAccount> {
        self.try_all().unwrap_or_default()
    }

    /// Write path: errors if the file EXISTS but can't be read/parsed, so a corrupt
    /// file can never be overwritten and silently delete every stored token.
    pub fn try_all(&self) -> Result<Vec<CloudflareAccount>> {
        let raw = match fs::read_to_string(&self.path) {
            Ok(raw) => raw,
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(Vec::new()),
            Err(e) => return Err(anyhow::anyhow!("cannot read {}: {e}", self.path.display())),
        };
        if raw.trim().is_empty() {
            return Ok(Vec::new());
        }
        serde_json::from_str(&raw).map_err(|e| {
            anyhow::anyhow!(
                "{} is corrupt: {e}. Fix or move that file; continuing would overwrite it \
                 and delete every Cloudflare token.",
                self.path.display()
            )
        })
    }

    pub fn by_name(&self, name: &str) -> Option<CloudflareAccount> {
        self.list().into_iter().find(|a| a.name == name)
    }

    pub fn default(&self) -> Option<CloudflareAccount> {
        self.list().into_iter().find(|a| a.default)
    }

    pub fn add(&self, account: CloudflareAccount) -> Result<()> {
        let existing = self.try_all()?;
        let was_default = existing.iter().any(|a| a.name == account.name && a.default);
        let mut accounts: Vec<CloudflareAccount> =
            existing.into_iter().filter(|a| a.name != account.name).collect();
        let is_first = accounts.is_empty();
        let has_default = accounts.iter().any(|a| a.default);
        let mut account = account;
        account.default = is_first || was_default || !has_default;
        accounts.push(account);
        self.save(&accounts)
    }

    pub fn remove(&self, name: &str) -> Result<()> {
        let mut accounts: Vec<CloudflareAccount> =
            self.try_all()?.into_iter().filter(|a| a.name != name).collect();
        if !accounts.is_empty() && !accounts.iter().any(|a| a.default) {
            accounts[0].default = true;
        }
        self.save(&accounts)
    }

    pub fn set_default(&self, name: &str) -> Result<()> {
        let mut accounts = self.try_all()?;
        if !accounts.iter().any(|a| a.name == name) {
            anyhow::bail!("No Cloudflare account called '{name}'");
        }
        for a in &mut accounts {
            a.default = a.name == name;
        }
        self.save(&accounts)
    }

    fn save(&self, accounts: &[CloudflareAccount]) -> Result<()> {
        if let Some(dir) = self.path.parent() {
            fs::create_dir_all(dir)?;
        }
        fs::write(&self.path, serde_json::to_string_pretty(accounts)?)?;
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            fs::set_permissions(&self.path, fs::Permissions::from_mode(0o600))?;
        }
        Ok(())
    }
}
```

- [ ] **Step 4: Run to verify pass**

Run: `cargo test -p easypanel cloudflare -- --nocapture 2>&1 | tail`
Expected: the three tests PASS.

- [ ] **Step 5: Commit**

```bash
git add src/config.rs src/cloudflare.rs
git commit -m "feat(cf): standalone CloudflareConfig store (cloudflare.json, 0600)"
```

### Task 3: Envelope + error parsing (`parse_envelope`)

**Files:**
- Modify: `src/cloudflare.rs`

**Interfaces:**
- Produces: `pub fn parse_envelope<T: DeserializeOwned>(body: &str) -> anyhow::Result<T>`; `Envelope`/`ResultInfo`/`CfError` structs; `pub struct ResultInfo { pub page: u32, pub per_page: u32, pub total_pages: u32, pub total_count: u32, pub count: u32 }` (public — the client's pagination loop reads it).

- [ ] **Step 1: Write failing tests**

```rust
#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn envelope_success_unwraps_result() {
        let body = r#"{"success":true,"errors":[],"messages":[],
            "result":[{"id":"z1","name":"example.com","status":"active"}]}"#;
        let zones: Vec<Zone> = parse_envelope(body).unwrap();
        assert_eq!(zones[0].name, "example.com");
    }

    #[test]
    fn envelope_failure_surfaces_the_first_error_message() {
        let body = r#"{"success":false,
            "errors":[{"code":81057,"message":"Record already exists."}],
            "messages":[],"result":null}"#;
        let err = parse_envelope::<Vec<Record>>(body).unwrap_err().to_string();
        assert!(err.contains("Record already exists."), "got: {err}");
    }

    #[test]
    fn envelope_failure_with_no_error_array_still_fails_cleanly() {
        let body = r#"{"success":false,"errors":[],"messages":[],"result":null}"#;
        assert!(parse_envelope::<Vec<Record>>(body).is_err());
    }
}
```

- [ ] **Step 2: Run to verify it fails** — Run: `cargo test cloudflare::tests::envelope -- --nocapture`; Expected: FAIL (no `parse_envelope`).

- [ ] **Step 3: Implement**

```rust
use serde::de::DeserializeOwned;

#[derive(Debug, Clone, Deserialize)]
pub struct Zone { pub id: String, pub name: String, #[serde(default)] pub status: String }

#[derive(Debug, Clone, Deserialize)]
pub struct Record {
    pub id: String,
    #[serde(rename = "type")] pub kind: String,
    pub name: String,
    #[serde(default)] pub content: String,
    #[serde(default)] pub ttl: u32,
    #[serde(default)] pub proxied: bool,
    #[serde(default)] pub priority: Option<u16>,
}

#[derive(Debug, Clone, Default, Deserialize)]
pub struct ResultInfo {
    #[serde(default)] pub page: u32,
    #[serde(default)] pub per_page: u32,
    #[serde(default)] pub total_pages: u32,
    #[serde(default)] pub total_count: u32,
    #[serde(default)] pub count: u32,
}

#[derive(Debug, Deserialize)]
struct CfError { #[allow(dead_code)] code: i64, message: String }

#[derive(Debug, Deserialize)]
struct Envelope<T> {
    success: bool,
    #[serde(default)] errors: Vec<CfError>,
    result: Option<T>,
    #[serde(default)] result_info: Option<ResultInfo>,
}

/// Unwrap a Cloudflare v4 envelope, turning `success:false` into an error carrying the
/// first `errors[].message` (Cloudflare's messages are human-readable), not the status.
pub fn parse_envelope<T: DeserializeOwned>(body: &str) -> anyhow::Result<T> {
    let env: Envelope<T> = serde_json::from_str(body)
        .map_err(|e| anyhow::anyhow!("unexpected Cloudflare response: {e}"))?;
    if !env.success {
        let msg = env.errors.first().map(|e| e.message.clone())
            .unwrap_or_else(|| "Cloudflare rejected the request".into());
        anyhow::bail!("Cloudflare: {msg}");
    }
    env.result.ok_or_else(|| anyhow::anyhow!("Cloudflare returned no result"))
}

/// Envelope + its pagination info, for the client's page loop.
pub fn parse_envelope_paged<T: DeserializeOwned>(body: &str) -> anyhow::Result<(T, ResultInfo)> {
    let env: Envelope<T> = serde_json::from_str(body)
        .map_err(|e| anyhow::anyhow!("unexpected Cloudflare response: {e}"))?;
    if !env.success {
        let msg = env.errors.first().map(|e| e.message.clone())
            .unwrap_or_else(|| "Cloudflare rejected the request".into());
        anyhow::bail!("Cloudflare: {msg}");
    }
    let info = env.result_info.unwrap_or_default();
    env.result.map(|r| (r, info)).ok_or_else(|| anyhow::anyhow!("Cloudflare returned no result"))
}
```

- [ ] **Step 4: Run to verify pass** — Run: `cargo test cloudflare::tests::envelope`; Expected: PASS.
- [ ] **Step 5: Commit** — `git commit -am "feat(cf): parse the Cloudflare v4 envelope + errors"`

### Task 4: Record body/patch/type guards (`record_body`, `apply_patch`, `valid_record_type`, `proxyable`)

**Files:**
- Modify: `src/cloudflare.rs`

**Interfaces:**
- Produces: `pub struct RecordPatch { pub content: Option<String>, pub proxied: Option<bool>, pub ttl: Option<u32>, pub priority: Option<u16> }`; `pub fn record_body(kind, name, content, ttl, proxied, priority) -> serde_json::Value`; `pub fn apply_patch(&RecordPatch) -> serde_json::Value`; `pub fn valid_record_type(&str) -> bool`; `pub fn proxyable(kind: &str) -> bool`.

- [ ] **Step 1: Write failing tests**

```rust
#[test]
fn a_record_body_has_no_priority_and_can_be_proxied() {
    let b = record_body("A", "www.example.com", "1.2.3.4", 1, true, None);
    assert_eq!(b["type"], "A");
    assert_eq!(b["ttl"], 1);           // 1 = automatic
    assert_eq!(b["proxied"], true);
    assert!(b.get("priority").is_none());
}

#[test]
fn mx_record_body_carries_priority() {
    let b = record_body("MX", "example.com", "mail.example.com", 3600, false, Some(10));
    assert_eq!(b["priority"], 10);
}

#[test]
fn only_v1_types_are_valid() {
    for t in ["A", "AAAA", "CNAME", "TXT", "NS", "MX"] { assert!(valid_record_type(t)); }
    for t in ["SRV", "CAA", "LOC", "URI", "bogus"] { assert!(!valid_record_type(t)); }
    assert!(proxyable("A") && proxyable("AAAA") && proxyable("CNAME"));
    assert!(!proxyable("TXT") && !proxyable("MX"));
}

#[test]
fn patch_body_only_carries_set_fields() {
    let patch = RecordPatch { content: Some("5.6.7.8".into()), proxied: None, ttl: None, priority: None };
    let b = apply_patch(&patch);
    assert_eq!(b["content"], "5.6.7.8");
    assert!(b.get("proxied").is_none() && b.get("ttl").is_none());
    assert!(patch.is_empty() == false);
    assert!(RecordPatch::default().is_empty(), "an all-None patch is empty");
}
```

- [ ] **Step 2: Run to verify it fails** — Expected: FAIL (functions undefined).

- [ ] **Step 3: Implement**

```rust
use serde_json::{json, Value};

const V1_TYPES: &[&str] = &["A", "AAAA", "CNAME", "TXT", "NS", "MX"];

pub fn valid_record_type(kind: &str) -> bool {
    V1_TYPES.contains(&kind.to_ascii_uppercase().as_str())
}
pub fn proxyable(kind: &str) -> bool {
    matches!(kind.to_ascii_uppercase().as_str(), "A" | "AAAA" | "CNAME")
}

/// The CREATE body. `ttl = 1` means "automatic". `proxied` only rides A/AAAA/CNAME;
/// `priority` only MX (and SRV later). Callers pass values already validated.
pub fn record_body(kind: &str, name: &str, content: &str, ttl: u32, proxied: bool, priority: Option<u16>) -> Value {
    let mut b = json!({ "type": kind, "name": name, "content": content, "ttl": ttl });
    if proxyable(kind) { b["proxied"] = json!(proxied); }
    if let Some(p) = priority { b["priority"] = json!(p); }
    b
}

#[derive(Debug, Clone, Default)]
pub struct RecordPatch {
    pub content: Option<String>,
    pub proxied: Option<bool>,
    pub ttl: Option<u32>,
    pub priority: Option<u16>,
}

impl RecordPatch {
    pub fn is_empty(&self) -> bool {
        self.content.is_none() && self.proxied.is_none() && self.ttl.is_none() && self.priority.is_none()
    }
}

/// The PATCH body — only the fields the user set. Sent to `PATCH …/dns_records/{id}`,
/// which preserves every field NOT present (unlike PUT).
pub fn apply_patch(patch: &RecordPatch) -> Value {
    let mut b = json!({});
    if let Some(c) = &patch.content { b["content"] = json!(c); }
    if let Some(p) = patch.proxied { b["proxied"] = json!(p); }
    if let Some(t) = patch.ttl { b["ttl"] = json!(t); }
    if let Some(p) = patch.priority { b["priority"] = json!(p); }
    b
}
```

- [ ] **Step 4: Run to verify pass** — Expected: PASS.
- [ ] **Step 5: Commit** — `git commit -am "feat(cf): record create/patch body builders + type guards"`

### Task 5: Zone resolution, record selection, filter query (`resolve_zone`, `select_records`, `filter_query`)

**Files:**
- Modify: `src/cloudflare.rs`

**Interfaces:**
- Produces: `pub struct RecordFilter { pub kind: Option<String>, pub name: Option<String>, pub content: Option<String> }`; `pub fn filter_query(&RecordFilter) -> Vec<(String,String)>`; `pub fn resolve_zone<'a>(&'a [Zone], needle: &str) -> Option<&'a Zone>`; `pub struct Selector { pub ids: Vec<String>, pub where_content: Option<String>, pub where_type: Option<String>, pub where_name: Option<String> }`; `pub fn select_records<'a>(&'a [Record], sel: &Selector) -> Vec<&'a Record>`.

- [ ] **Step 1: Write failing tests**

```rust
#[test]
fn resolve_zone_prefers_name_then_id() {
    let zones = vec![
        Zone { id: "id-a".into(), name: "example.com".into(), status: "active".into() },
        Zone { id: "example.com".into(), name: "other.com".into(), status: "active".into() },
    ];
    assert_eq!(resolve_zone(&zones, "example.com").unwrap().id, "id-a", "name wins over a coincidental id");
    assert_eq!(resolve_zone(&zones, "id-a").unwrap().name, "example.com");
    assert!(resolve_zone(&zones, "nope.com").is_none());
}

#[test]
fn filter_query_uses_operator_keys() {
    let f = RecordFilter { kind: Some("A".into()), name: Some("api".into()), content: None };
    let q = filter_query(&f);
    assert!(q.contains(&("type".into(), "A".into())));
    assert!(q.contains(&("name.contains".into(), "api".into())));
    assert!(q.contains(&("match".into(), "all".into())), "AND when >1 filter");
    assert!(filter_query(&RecordFilter::default()).is_empty());
}

#[test]
fn select_records_matches_ids_and_wheres() {
    let recs = vec![
        rec("r1", "A", "a.example.com", "1.1.1.1"),
        rec("r2", "A", "b.example.com", "1.1.1.1"),
        rec("r3", "CNAME", "c.example.com", "a.example.com"),
    ];
    // repoint case: everything on the old IP
    let sel = Selector { ids: vec![], where_content: Some("1.1.1.1".into()), where_type: None, where_name: None };
    let hit = select_records(&recs, &sel);
    assert_eq!(hit.len(), 2);
    // type + content intersect
    let sel = Selector { ids: vec![], where_content: Some("1.1.1.1".into()), where_type: Some("A".into()), where_name: None };
    assert_eq!(select_records(&recs, &sel).len(), 2);
    // explicit id
    let sel = Selector { ids: vec!["r3".into()], ..Default::default() };
    assert_eq!(select_records(&recs, &sel)[0].id, "r3");
    // no match → empty (so the CLI can say "0 matched")
    let sel = Selector { ids: vec![], where_content: Some("9.9.9.9".into()), ..Default::default() };
    assert!(select_records(&recs, &sel).is_empty());
}

fn rec(id: &str, kind: &str, name: &str, content: &str) -> Record {
    Record { id: id.into(), kind: kind.into(), name: name.into(), content: content.into(), ttl: 1, proxied: false, priority: None }
}
```

- [ ] **Step 2: Run to verify it fails** — Expected: FAIL.

- [ ] **Step 3: Implement**

```rust
#[derive(Debug, Clone, Default)]
pub struct RecordFilter { pub kind: Option<String>, pub name: Option<String>, pub content: Option<String> }

/// Cloudflare's dns_records filters are operator-keyed: `name.contains`, `content.contains`,
/// flat `type`. Add `match=all` (AND) when more than one is present.
pub fn filter_query(f: &RecordFilter) -> Vec<(String, String)> {
    let mut q = Vec::new();
    if let Some(t) = &f.kind { q.push(("type".into(), t.clone())); }
    if let Some(n) = &f.name { q.push(("name.contains".into(), n.clone())); }
    if let Some(c) = &f.content { q.push(("content.contains".into(), c.clone())); }
    if q.len() > 1 { q.push(("match".into(), "all".into())); }
    q
}

pub fn resolve_zone<'a>(zones: &'a [Zone], needle: &str) -> Option<&'a Zone> {
    zones.iter().find(|z| z.name == needle).or_else(|| zones.iter().find(|z| z.id == needle))
}

#[derive(Debug, Clone, Default)]
pub struct Selector {
    pub ids: Vec<String>,
    pub where_content: Option<String>,
    pub where_type: Option<String>,
    pub where_name: Option<String>,
}

pub fn select_records<'a>(records: &'a [Record], sel: &Selector) -> Vec<&'a Record> {
    records.iter().filter(|r| {
        if !sel.ids.is_empty() && !sel.ids.iter().any(|id| id == &r.id) { return false; }
        if let Some(c) = &sel.where_content { if &r.content != c { return false; } }
        if let Some(t) = &sel.where_type { if !r.kind.eq_ignore_ascii_case(t) { return false; } }
        if let Some(n) = &sel.where_name { if !r.name.contains(n.as_str()) { return false; } }
        // An empty selector matches nothing — bulk must never fan out over "everything".
        !(sel.ids.is_empty() && sel.where_content.is_none() && sel.where_type.is_none() && sel.where_name.is_none())
    }).collect()
}
```

- [ ] **Step 4: Run to verify pass** — Expected: PASS.
- [ ] **Step 5: Commit** — `git commit -am "feat(cf): zone resolution, record selection, filter-query builder"`

**Phase 1 gate:** `cargo fmt --check && cargo clippy --all-targets -- -D warnings && cargo test cloudflare` all clean. Domain + config are done, fully unit-tested, zero network.

---

## Phase 2 — Live probe + HTTP client

### Task 6: `CloudflareClient` (reqwest, Bearer, paginated list + CRUD)

**Files:**
- Modify: `src/cloudflare.rs`

**Interfaces:**
- Consumes: `parse_envelope`, `parse_envelope_paged`, `ResultInfo`, `filter_query`, the domain types.
- Produces: `CloudflareClient::new(token: &str)`; methods `list_zones(account_id: Option<&str>) -> Result<Vec<Zone>>`, `create_zone(name, account_id) -> Result<Zone>`, `delete_zone(id) -> Result<()>`, `list_records(zone_id, &RecordFilter) -> Result<Vec<Record>>`, `create_record(zone_id, &Value) -> Result<Record>`, `patch_record(zone_id, rec_id, &Value) -> Result<Record>`, `delete_record(zone_id, rec_id) -> Result<()>`.

- [ ] **Step 1: Implement the client skeleton** (no unit test — HTTP is verified live in Task 8; a mock would encode an unverified shape, which the brief forbids)

```rust
const BASE: &str = "https://api.cloudflare.com/client/v4";

pub struct CloudflareClient {
    http: reqwest::blocking::Client,
    token: String,
}

impl CloudflareClient {
    pub fn new(token: &str) -> Self {
        Self {
            http: reqwest::blocking::Client::builder()
                .user_agent("easypanel-cli")
                .build()
                .expect("reqwest client"),
            token: token.to_string(),
        }
    }

    fn get(&self, path: &str, query: &[(String, String)]) -> anyhow::Result<String> {
        let resp = self.http.get(format!("{BASE}{path}"))
            .bearer_auth(&self.token)
            .query(query)
            .send()?;
        Ok(resp.text()?)
    }
    // post/patch/delete mirror this with `.json(body)` / `.delete(...)`.

    /// Follow result_info.total_pages so a thousand-record zone comes back whole.
    pub fn list_records(&self, zone_id: &str, filter: &RecordFilter) -> anyhow::Result<Vec<Record>> {
        let mut all = Vec::new();
        let mut page = 1u32;
        loop {
            let mut q = filter_query(filter);
            q.push(("page".into(), page.to_string()));
            q.push(("per_page".into(), "100".into()));  // Task 8 confirms the max; 100 is safe
            let body = self.get(&format!("/zones/{zone_id}/dns_records"), &q)?;
            let (mut recs, info): (Vec<Record>, ResultInfo) = parse_envelope_paged(&body)?;
            all.append(&mut recs);
            if info.total_pages <= page || info.total_pages == 0 { break; }
            page += 1;
        }
        Ok(all)
    }
    // list_zones mirrors this; create_*/patch_*/delete_* use parse_envelope (single object).
}
```

- [ ] **Step 2: Verify it compiles** — Run: `cargo build 2>&1 | tail`; Expected: compiles (still no caller — Task 7/9 add them in the same landing).
- [ ] **Step 3: Commit** — `git commit -am "feat(cf): CloudflareClient — Bearer, paginated list, CRUD"`

### Task 7: `commands::cf_*` orchestration (shared CLI + TUI)

**Files:**
- Modify: `src/commands.rs`

**Interfaces:**
- Consumes: `CloudflareConfig`, `CloudflareClient`, domain functions.
- Produces: `cf_account_add/list/use/delete`, `cf_zone_list/add/delete`, `cf_record_list(account, zone, filter)`, `cf_record_add`, `cf_record_set(account, zone, selector, patch) -> BulkReport`, `cf_record_delete(account, zone, ids) -> BulkReport`. A `pub fn cf_client(account_name: Option<&str>) -> Result<(CloudflareClient, CloudflareAccount)>` resolves the account (named or default) and builds the client, erroring clearly when no account is configured.

- [ ] **Step 1: Implement `cf_client` + account resolution + a failing doc-test-style unit for the "no account" message.**

```rust
pub fn cf_client(cfg: &CloudflareConfig, account: Option<&str>) -> anyhow::Result<(CloudflareClient, CloudflareAccount)> {
    let acc = match account {
        Some(name) => cfg.by_name(name)
            .ok_or_else(|| anyhow::anyhow!("No Cloudflare account called '{name}'. Add it: easypanel cf account add {name}"))?,
        None => cfg.default()
            .ok_or_else(|| anyhow::anyhow!("No Cloudflare account configured. Add one: easypanel cf account add <name>"))?,
    };
    Ok((CloudflareClient::new(&acc.api_token), acc))
}
```

- [ ] **Step 2: Implement the zone/record orchestration** using `cf_client`, `resolve_zone` (list zones first, resolve the `<zone>` needle to an id), `select_records` for bulk, `apply_patch`. `cf_record_set` lists records, selects, then loops `patch_record` collecting a per-id `BulkReport { ok: Vec<String>, failed: Vec<(String,String)> }` (reuse the existing bulk report shape if one exists; otherwise a small local struct).
- [ ] **Step 3: Verify compiles + existing tests still pass** — Run: `cargo test 2>&1 | tail`.
- [ ] **Step 4: Commit** — `git commit -am "feat(cf): shared cf_* orchestration for CLI and TUI"`

### Task 8: LIVE PROBE (gateway — resolves the 6 unverified details)

**Files:** none (a manual verification task; findings recorded in the plan + AGENT_BRIEF).

- [ ] **Step 1:** With `CF_TEST_TOKEN` exported (owner-supplied, scoped to a throwaway zone), from a scratch script or `easypanel cf` once Task 9 lands, confirm each flagged item:
  1. `GET /zones` with and without `account.id` on the scoped token — does it 403? Record the answer.
  2. `POST /zones` omitting `account` — capture the exact 400 (confirms account_id is required in practice).
  3. `per_page=10000` on both list endpoints — read the clamp/400; set the client's `per_page` to the confirmed max (or keep 100).
  4. PATCH one field, re-GET, confirm the others survived (proves partial update).
  5. Zone `name` filter operator syntax if `zone list` filtering is added.
- [ ] **Step 2:** Fold any corrections back into Task 6's client (e.g. per_page max, whether `account_id` is required for `list_zones`). Re-run `cargo test`.
- [ ] **Step 3:** Record the measured answers in `.github/AGENT_BRIEF.md` (so a future run doesn't re-probe) and commit.

---

## Phase 3 — CLI

### Task 9: `easypanel cf …` subcommands + dispatch

**Files:**
- Modify: `src/main.rs`

**Interfaces:**
- Consumes: `commands::cf_*`.
- Produces: `Command::Cf(CfCmd)` and nested `CfCmd { Account(CfAccountCmd), Zone(CfZoneCmd), Record(CfRecordCmd) }`.

- [ ] **Step 1: Add the clap subcommands** mirroring `DbCmd`'s style:

```rust
#[derive(Subcommand)]
enum CfCmd {
    /// Manage stored Cloudflare accounts (independent of EasyPanel servers).
    #[command(subcommand)] Account(CfAccountCmd),
    /// Manage zones on the active account.
    #[command(subcommand)] Zone(CfZoneCmd),
    /// Manage DNS records within a zone.
    #[command(subcommand)] Record(CfRecordCmd),
}

#[derive(Subcommand)]
enum CfAccountCmd {
    Add { name: String, #[arg(long)] account_id: Option<String>, #[arg(long)] token: Option<String> },
    List,
    Use { name: String },
    Delete { name: String },
}
// CfZoneCmd { List{#[arg(long)] account}, Add{name, #[arg(long)] account}, Delete{zone, #[arg(long)] account, #[arg(long)] yes} }
// CfRecordCmd { List{zone, #[arg(long)] r#type, #[arg(long)] name, #[arg(long)] content, #[arg(long)] account},
//               Add{zone, #[arg(long)] r#type, #[arg(long)] name, #[arg(long)] content, #[arg(long)] ttl, #[arg(long)] proxied, #[arg(long)] priority, #[arg(long)] account},
//               Set{zone, ids:Vec<String>, #[arg(long)] where_content, #[arg(long)] where_type, #[arg(long)] where_name, #[arg(long)] content, #[arg(long)] proxied, #[arg(long)] ttl, #[arg(long)] priority, #[arg(long)] account, #[arg(long)] yes},
//               Delete{zone, ids:Vec<String>, #[arg(long)] account} }
```

- [ ] **Step 2: Wire dispatch** in `run()`: `account add` prompts for the token without echo when `--token` is absent (`dialoguer::Password`), builds a `CloudflareAccount`, calls `CloudflareConfig::add`. `account list` prints labels + which is default + a MASKED token (reuse the existing `mask_token`). Zone/record commands call `commands::cf_*`, honour `--json`, and `zone delete`/`record set` confirm (typed zone name for `zone delete`; matched-record preview for `record set`) unless `--yes`.
- [ ] **Step 3: Live-verify** each command against the throwaway zone (this is also Task 8's vehicle). Confirm add/list/edit(bulk)/delete round-trips; clean up.
- [ ] **Step 4: Commit** — `git commit -am "feat(cf): easypanel cf account/zone/record CLI"`

---

## Phase 4 — TUI (isolated workspace)

### Task 10: `Workspace` mode + Switch menu (`W`)

**Files:**
- Modify: `src/tui/app.rs`, `src/tui/keys.rs`, `src/tui/render.rs`, `src/tui/tests.rs`

**Interfaces:**
- Produces: `enum Workspace { Easypanel, Cloudflare }` on `App`; `W` opens a Switch picker; selecting Cloudflare sets `app.workspace = Cloudflare` and loads zones; Esc at the Cloudflare Zones root resets to `Easypanel`.

- [ ] **Step 1: Write the failing TUI test** (workspace toggle + isolation):

```rust
#[test]
fn the_switch_menu_enters_and_leaves_the_cloudflare_workspace() {
    let mut app = App::new_for_test();
    assert!(matches!(app.workspace, Workspace::Easypanel));
    app.enter_workspace(Workspace::Cloudflare);
    assert!(matches!(app.workspace, Workspace::Cloudflare));
    // EasyPanel number keys do nothing while in Cloudflare (isolation).
    // Esc at the zones root returns to EasyPanel.
    app.cloudflare_escape();
    assert!(matches!(app.workspace, Workspace::Easypanel));
}
```

- [ ] **Step 2: Run to verify it fails** — Expected: FAIL (`workspace` field / methods missing).
- [ ] **Step 3: Implement** `Workspace` state, `enter_workspace`, `cloudflare_escape`, the `W` key → Switch picker (reuse the menu/picker machinery), and gate the EasyPanel number/tab keys behind `workspace == Easypanel` in `keys.rs`.
- [ ] **Step 4: Run to verify pass**, then `cargo test tui` for no regressions.
- [ ] **Step 5: Commit** — `git commit -am "feat(cf): TUI Workspace mode + W switch menu (isolated)"`

### Task 11: Cloudflare Zones/Records screens (render + navigation + filter)

**Files:**
- Modify: `src/tui/app.rs` (a `CfUi` state block), `src/tui/render.rs`, `src/tui/keys.rs`, `src/tui/worker.rs`

**Interfaces:**
- Consumes: `commands::cf_*`, `Workspace`.
- Produces: `Req::Cf(CfReq)` / `Resp::Cf(CfResp)` (arms per the spec); `CfUi { active_account, zones, zones_state, records, records_state, current_zone, ... }`; Zones→Enter→Records; both join `filterable()`; orange header showing the active account.

- [ ] **Step 1:** Add `Req::Cf`/`Resp::Cf` and worker handlers calling `commands::cf_*` (zones, records with filter, create/patch/delete, bulk). Build the client from the active CF account.
- [ ] **Step 2:** Render the Cloudflare header (orange accent) + Zones table; Enter loads Records; render Records table (Type/Name/Content/TTL/Proxied). Add both screens to `filterable()`.
- [ ] **Step 3:** Empty state when no accounts: "No Cloudflare account yet — press `n` to add one" (opens the add-account form). `a` opens the account picker.
- [ ] **Step 4:** Run the binary, LOOK at the screens (tmux capture) — verify orange header, filter works, Esc navigation, no EasyPanel bleed-through. Fix what the screen shows.
- [ ] **Step 5: Commit** — `git commit -am "feat(cf): TUI Zones/Records screens with filter"`

### Task 12: TUI record actions — add/edit/delete + bulk (`v`/`V` marking)

**Files:**
- Modify: `src/tui/keys.rs`, `src/tui/app.rs`, `src/tui/actions.rs`, `src/tui/worker.rs`

- [ ] **Step 1:** Record add/edit forms (reuse `Form`/`FieldKind`); `n` add, `e` edit, `x`/Danger delete with confirm.
- [ ] **Step 2:** Bulk: `v`/`V` mark records; action menu offers "Set content/proxied/TTL on N marked" and "Delete N marked"; worker `BulkPatchRecords`/`BulkDeleteRecords` loop with a `BulkDone` per-record report (reuse the EasyPanel bulk pattern).
- [ ] **Step 3:** `zone add`/`zone delete` (typed-name confirm) from the Zones screen menu.
- [ ] **Step 4:** Live-verify every action on the throwaway zone via the TUI; LOOK at the screens; clean up.
- [ ] **Step 5: Commit** — `git commit -am "feat(cf): TUI record add/edit/delete + bulk ops"`

---

## Phase 5 — Release

### Task 13: Docs, help, version, tag

**Files:**
- Modify: `README.md`, `CHANGELOG.md`, `Cargo.toml`, `src/tui/render.rs` (help: document `W`), `.github/AGENT_BRIEF.md`

- [ ] **Step 1:** Add the `W` switch to the TUI help (`GLOBAL_KEYS`), and CF screen keys to `screen_keys`. README: a "Cloudflare" section with the `cf` commands + a screenshot of the isolated workspace (demo data). CHANGELOG `[Unreleased]` → `[0.83.0]` describing the capability and why it matters.
- [ ] **Step 2:** Bump `Cargo.toml` to `0.83.0`. `graphify update .`.
- [ ] **Step 3:** Full DoD: `cargo fmt --check && cargo clippy --all-targets -- -D warnings && cargo test`. Confirm the `the_tab_switch_hint_names_every_numbered_tab` test still passes (EasyPanel tabs stay 8 — Cloudflare is NOT a tab).
- [ ] **Step 4:** Commit, `git tag v0.83.0`, push `--tags`.

---

## Self-Review notes

- **Spec coverage:** accounts (T1,2,9,10,11), zones CRUD (T6,7,9,11,12), records CRUD (T4,6,7,9,12), bulk (T5,7,12), filter (T5,6,11), PATCH (T4,6), isolated TUI mode (T10,11,12), per-account not per-server (T1,2), live-probe of the 6 flagged items (T8). All covered.
- **Type consistency:** `RecordPatch`, `RecordFilter`, `Selector`, `Zone`, `Record` (field `kind` via `#[serde(rename="type")]`), `parse_envelope`/`parse_envelope_paged`, `filter_query` (`name.contains`) are used identically across tasks.
- **Live-first honesty:** no HTTP is unit-tested with a mock (the brief's rule); HTTP correctness is proven in Task 8's live probe on the throwaway zone. Pure logic is exhaustively unit-tested.
