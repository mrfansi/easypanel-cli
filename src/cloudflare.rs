//! Cloudflare — a bounded context OUTSIDE EasyPanel: manage one or more Cloudflare
//! accounts' zones and DNS records. Nothing here touches the EasyPanel domain; the two
//! share only the TUI event loop and the config directory (separate files).

use anyhow::Result;
use serde::de::DeserializeOwned;
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};

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

// ---------- Domain types (Cloudflare API shapes) ----------

#[derive(Debug, Clone, Deserialize)]
pub struct Zone {
    pub id: String,
    pub name: String,
    #[serde(default)]
    pub status: String,
}

#[derive(Debug, Clone, Deserialize)]
pub struct Record {
    pub id: String,
    #[serde(rename = "type")]
    pub kind: String,
    pub name: String,
    #[serde(default)]
    pub content: String,
    #[serde(default)]
    pub ttl: u32,
    #[serde(default)]
    pub proxied: bool,
    #[serde(default)]
    pub priority: Option<u16>,
}

/// An R2 bucket (Cloudflare's REST view, not the S3 API). Object browsing is a
/// separate future slice — this slice manages buckets only.
#[derive(Debug, Clone, Deserialize)]
pub struct R2Bucket {
    pub name: String,
    #[serde(default)]
    pub creation_date: String,
    /// Optional — the list endpoint may omit it.
    #[serde(default)]
    pub location: Option<String>,
    #[serde(default)]
    pub storage_class: String,
    #[serde(default)]
    pub jurisdiction: String,
}

/// The list-buckets `result` is NOT a bare array: it is an object wrapping a
/// `buckets` array. Deserialize `result` into this, not `Vec<R2Bucket>`.
#[derive(Debug, Deserialize)]
struct BucketsResult {
    #[serde(default)]
    buckets: Vec<R2Bucket>,
}

/// One object inside an R2 bucket, from the Cloudflare REST API (`GET
/// /accounts/{account_id}/r2/buckets/{bucket}/objects`) — the SAME Bearer token as
/// buckets/DNS, no S3 credentials. `result` is a bare array of these. Browsing only for
/// now; upload/download/delete are a later slice. serde ignores the other keys the API
/// sends (etag/custom_metadata/http_metadata/ssec).
#[derive(Debug, Clone, PartialEq, Deserialize)]
pub struct R2Object {
    pub key: String,
    #[serde(default)]
    pub size: u64,
    #[serde(default)]
    pub last_modified: String,
    #[serde(default)]
    pub storage_class: String,
}

/// Cloudflare's pagination block. `total_pages` drives the DNS page loop; `cursor` +
/// `is_truncated` drive R2's cursor pagination (`is_truncated` is the authoritative
/// "more pages" flag). serde ignores the other keys (page/per_page/count/total_count)
/// the API also sends.
#[derive(Debug, Clone, Default, Deserialize)]
pub struct ResultInfo {
    #[serde(default)]
    pub total_pages: u32,
    /// R2's next-page cursor: pass it back as the `cursor` query param.
    #[serde(default)]
    pub cursor: Option<String>,
    /// R2 objects: true while more pages remain. Absent (buckets) defaults to false.
    #[serde(default)]
    pub is_truncated: bool,
}

// ---------- Envelope parsing ----------

#[derive(Debug, Deserialize)]
struct CfError {
    #[allow(dead_code)]
    code: i64,
    message: String,
}

#[derive(Debug, Deserialize)]
struct Envelope<T> {
    success: bool,
    #[serde(default)]
    errors: Vec<CfError>,
    result: Option<T>,
    #[serde(default)]
    result_info: Option<ResultInfo>,
}

fn envelope_error(errors: &[CfError]) -> String {
    errors
        .first()
        .map(|e| e.message.clone())
        .unwrap_or_else(|| "Cloudflare rejected the request".into())
}

/// R2 buckets are account-scoped: list needs the "Workers R2 Storage" Read
/// permission, create/delete its Edit. A token scoped only to Zone:DNS gets
/// Cloudflare's generic "Authentication error" here, which never says why — so
/// append the specific permission that is missing. Mirrors how DNS surfaces
/// Cloudflare's own message, but R2-specific.
fn r2_hint(e: anyhow::Error) -> anyhow::Error {
    let msg = e.to_string();
    if msg.to_ascii_lowercase().contains("authentication error") {
        anyhow::anyhow!(
            "{msg} — the token may lack the Workers R2 Storage permission (add it at Account scope)"
        )
    } else {
        e
    }
}

/// Unwrap a Cloudflare v4 envelope, turning `success:false` into an error carrying the
/// first `errors[].message` (Cloudflare's messages are human-readable), not the status.
pub fn parse_envelope<T: DeserializeOwned>(body: &str) -> Result<T> {
    let env: Envelope<T> = serde_json::from_str(body)
        .map_err(|e| anyhow::anyhow!("unexpected Cloudflare response: {e}"))?;
    if !env.success {
        anyhow::bail!("Cloudflare: {}", envelope_error(&env.errors));
    }
    env.result
        .ok_or_else(|| anyhow::anyhow!("Cloudflare returned no result"))
}

/// Envelope + its pagination info, for the client's page loop.
pub fn parse_envelope_paged<T: DeserializeOwned>(body: &str) -> Result<(T, ResultInfo)> {
    let env: Envelope<T> = serde_json::from_str(body)
        .map_err(|e| anyhow::anyhow!("unexpected Cloudflare response: {e}"))?;
    if !env.success {
        anyhow::bail!("Cloudflare: {}", envelope_error(&env.errors));
    }
    let info = env.result_info.unwrap_or_default();
    env.result
        .map(|r| (r, info))
        .ok_or_else(|| anyhow::anyhow!("Cloudflare returned no result"))
}

// ---------- Record bodies + type guards ----------

const V1_TYPES: &[&str] = &["A", "AAAA", "CNAME", "TXT", "NS", "MX"];

/// v1 supports the flat-`content` types + MX. The `data`-object types (SRV, CAA, LOC, …)
/// need a structured body and are deferred; reject them clearly rather than send garbage.
pub fn valid_record_type(kind: &str) -> bool {
    V1_TYPES.contains(&kind.to_ascii_uppercase().as_str())
}

/// Only A/AAAA/CNAME can ride the orange-cloud proxy.
pub fn proxyable(kind: &str) -> bool {
    matches!(kind.to_ascii_uppercase().as_str(), "A" | "AAAA" | "CNAME")
}

/// The CREATE body. `ttl = 1` means "automatic". `proxied` only rides A/AAAA/CNAME;
/// `priority` only MX (and SRV later). Callers pass values already validated.
pub fn record_body(
    kind: &str,
    name: &str,
    content: &str,
    ttl: u32,
    proxied: bool,
    priority: Option<u16>,
) -> Value {
    let mut b = json!({ "type": kind, "name": name, "content": content, "ttl": ttl });
    if proxyable(kind) {
        b["proxied"] = json!(proxied);
    }
    if let Some(p) = priority {
        b["priority"] = json!(p);
    }
    b
}

/// A partial field-change for a record.
#[derive(Debug, Clone, Default)]
pub struct RecordPatch {
    pub content: Option<String>,
    pub proxied: Option<bool>,
    pub ttl: Option<u32>,
    pub priority: Option<u16>,
}

impl RecordPatch {
    pub fn is_empty(&self) -> bool {
        self.content.is_none()
            && self.proxied.is_none()
            && self.ttl.is_none()
            && self.priority.is_none()
    }
}

/// The PATCH body — only the fields the user set. Sent to `PATCH …/dns_records/{id}`,
/// which preserves every field NOT present (unlike PUT, which overwrites).
pub fn apply_patch(patch: &RecordPatch) -> Value {
    let mut b = json!({});
    if let Some(c) = &patch.content {
        b["content"] = json!(c);
    }
    if let Some(p) = patch.proxied {
        b["proxied"] = json!(p);
    }
    if let Some(t) = patch.ttl {
        b["ttl"] = json!(t);
    }
    if let Some(p) = patch.priority {
        b["priority"] = json!(p);
    }
    b
}

// ---------- Zone resolution, record selection, filter query ----------

#[derive(Debug, Clone, Default)]
pub struct RecordFilter {
    pub kind: Option<String>,
    pub name: Option<String>,
    pub content: Option<String>,
}

/// Cloudflare's dns_records filters are operator-keyed: `name.contains`, `content.contains`,
/// flat `type`. Add `match=all` (AND) when more than one is present.
pub fn filter_query(f: &RecordFilter) -> Vec<(String, String)> {
    let mut q = Vec::new();
    if let Some(t) = &f.kind {
        q.push(("type".into(), t.clone()));
    }
    if let Some(n) = &f.name {
        q.push(("name.contains".into(), n.clone()));
    }
    if let Some(c) = &f.content {
        q.push(("content.contains".into(), c.clone()));
    }
    if q.len() > 1 {
        q.push(("match".into(), "all".into()));
    }
    q
}

/// Resolve a zone by NAME (preferred) or id.
pub fn resolve_zone<'a>(zones: &'a [Zone], needle: &str) -> Option<&'a Zone> {
    zones
        .iter()
        .find(|z| z.name == needle)
        .or_else(|| zones.iter().find(|z| z.id == needle))
}

/// A bulk selection over records: explicit ids and/or where-clauses.
#[derive(Debug, Clone, Default)]
pub struct Selector {
    pub ids: Vec<String>,
    pub where_content: Option<String>,
    pub where_type: Option<String>,
    pub where_name: Option<String>,
}

impl Selector {
    fn is_empty(&self) -> bool {
        self.ids.is_empty()
            && self.where_content.is_none()
            && self.where_type.is_none()
            && self.where_name.is_none()
    }
}

/// The records a selector matches. An EMPTY selector matches NOTHING — a bulk op must
/// never silently fan out over every record in the zone.
pub fn select_records<'a>(records: &'a [Record], sel: &Selector) -> Vec<&'a Record> {
    if sel.is_empty() {
        return Vec::new();
    }
    records
        .iter()
        .filter(|r| {
            if !sel.ids.is_empty() && !sel.ids.iter().any(|id| id == &r.id) {
                return false;
            }
            if let Some(c) = &sel.where_content {
                if &r.content != c {
                    return false;
                }
            }
            if let Some(t) = &sel.where_type {
                if !r.kind.eq_ignore_ascii_case(t) {
                    return false;
                }
            }
            if let Some(n) = &sel.where_name {
                if !r.name.contains(n.as_str()) {
                    return false;
                }
            }
            true
        })
        .collect()
}

// ---------- HTTP client ----------

const BASE: &str = "https://api.cloudflare.com/client/v4";

/// A Cloudflare API client bound to one account's scoped token. Separate from
/// EasypanelClient — different API, auth, and base URL.
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

    fn get(&self, path: &str, query: &[(String, String)]) -> Result<String> {
        Ok(self
            .http
            .get(format!("{BASE}{path}"))
            .bearer_auth(&self.token)
            .query(query)
            .send()?
            .text()?)
    }

    /// List every zone the token can see. `account_id` narrows to one account when set.
    pub fn list_zones(&self, account_id: Option<&str>) -> Result<Vec<Zone>> {
        let mut all = Vec::new();
        let mut page = 1u32;
        loop {
            let mut q: Vec<(String, String)> = vec![
                ("page".into(), page.to_string()),
                ("per_page".into(), "50".into()),
            ];
            if let Some(acc) = account_id {
                q.push(("account.id".into(), acc.to_string()));
            }
            let body = self.get("/zones", &q)?;
            let (mut zones, info): (Vec<Zone>, ResultInfo) = parse_envelope_paged(&body)?;
            all.append(&mut zones);
            if info.total_pages <= page || info.total_pages == 0 {
                break;
            }
            page += 1;
        }
        Ok(all)
    }

    /// List a zone's DNS records, following pagination, with an optional server-side filter.
    pub fn list_records(&self, zone_id: &str, filter: &RecordFilter) -> Result<Vec<Record>> {
        let mut all = Vec::new();
        let mut page = 1u32;
        loop {
            let mut q = filter_query(filter);
            q.push(("page".into(), page.to_string()));
            q.push(("per_page".into(), "100".into()));
            let body = self.get(&format!("/zones/{zone_id}/dns_records"), &q)?;
            let (mut recs, info): (Vec<Record>, ResultInfo) = parse_envelope_paged(&body)?;
            all.append(&mut recs);
            if info.total_pages <= page || info.total_pages == 0 {
                break;
            }
            page += 1;
        }
        Ok(all)
    }

    fn send(&self, method: reqwest::Method, path: &str, body: Option<&Value>) -> Result<String> {
        let mut req = self
            .http
            .request(method, format!("{BASE}{path}"))
            .bearer_auth(&self.token);
        if let Some(b) = body {
            req = req.json(b);
        }
        Ok(req.send()?.text()?)
    }

    /// Create a zone under `account_id` (required in practice for a token).
    pub fn create_zone(&self, name: &str, account_id: &str) -> Result<Zone> {
        let body = json!({ "name": name, "account": { "id": account_id } });
        parse_envelope(&self.send(reqwest::Method::POST, "/zones", Some(&body))?)
    }

    pub fn delete_zone(&self, zone_id: &str) -> Result<()> {
        let _: Value = parse_envelope(&self.send(
            reqwest::Method::DELETE,
            &format!("/zones/{zone_id}"),
            None,
        )?)?;
        Ok(())
    }

    pub fn create_record(&self, zone_id: &str, body: &Value) -> Result<Record> {
        parse_envelope(&self.send(
            reqwest::Method::POST,
            &format!("/zones/{zone_id}/dns_records"),
            Some(body),
        )?)
    }

    /// PATCH — a partial update; only the fields in `patch` change, the rest are preserved.
    pub fn patch_record(&self, zone_id: &str, record_id: &str, patch: &Value) -> Result<Record> {
        parse_envelope(&self.send(
            reqwest::Method::PATCH,
            &format!("/zones/{zone_id}/dns_records/{record_id}"),
            Some(patch),
        )?)
    }

    pub fn delete_record(&self, zone_id: &str, record_id: &str) -> Result<()> {
        let _: Value = parse_envelope(&self.send(
            reqwest::Method::DELETE,
            &format!("/zones/{zone_id}/dns_records/{record_id}"),
            None,
        )?)?;
        Ok(())
    }

    // ---------- R2 buckets (account-scoped) ----------

    /// List every R2 bucket under `account_id`, following R2's CURSOR pagination.
    /// The list `result` wraps its array in a `buckets` object; the next cursor is
    /// in `result_info.cursor` and loops back as the `cursor` query param until it
    /// is absent/empty.
    pub fn list_r2_buckets(&self, account_id: &str) -> Result<Vec<R2Bucket>> {
        let mut all = Vec::new();
        let mut cursor: Option<String> = None;
        loop {
            let mut q: Vec<(String, String)> = vec![("per_page".into(), "100".into())];
            if let Some(c) = &cursor {
                q.push(("cursor".into(), c.clone()));
            }
            let body = self
                .get(&format!("/accounts/{account_id}/r2/buckets"), &q)
                .map_err(r2_hint)?;
            let (res, info): (BucketsResult, ResultInfo) =
                parse_envelope_paged(&body).map_err(r2_hint)?;
            all.extend(res.buckets);
            match info.cursor.filter(|c| !c.is_empty()) {
                Some(c) => cursor = Some(c),
                None => break,
            }
        }
        Ok(all)
    }

    /// Create an R2 bucket by name under `account_id`.
    pub fn create_r2_bucket(&self, account_id: &str, name: &str) -> Result<R2Bucket> {
        let body = json!({ "name": name });
        parse_envelope(
            &self
                .send(
                    reqwest::Method::POST,
                    &format!("/accounts/{account_id}/r2/buckets"),
                    Some(&body),
                )
                .map_err(r2_hint)?,
        )
        .map_err(r2_hint)
    }

    /// Delete an R2 bucket. Cloudflare requires the bucket be EMPTY — deleting a
    /// non-empty one errors, and that message is surfaced as-is.
    pub fn delete_r2_bucket(&self, account_id: &str, name: &str) -> Result<()> {
        let _: Value = parse_envelope(
            &self
                .send(
                    reqwest::Method::DELETE,
                    &format!("/accounts/{account_id}/r2/buckets/{name}"),
                    None,
                )
                .map_err(r2_hint)?,
        )
        .map_err(r2_hint)?;
        Ok(())
    }

    /// List a bucket's objects, following R2's CURSOR pagination. Unlike buckets, the
    /// objects `result` is a BARE array (not wrapped) — `is_truncated` says whether to
    /// loop, `cursor` is the next page. Same Bearer token as buckets; `prefix` narrows.
    pub fn list_r2_objects(
        &self,
        account_id: &str,
        bucket: &str,
        prefix: Option<&str>,
    ) -> Result<Vec<R2Object>> {
        let path = format!("/accounts/{account_id}/r2/buckets/{bucket}/objects");
        let mut all = Vec::new();
        let mut cursor: Option<String> = None;
        loop {
            let mut q: Vec<(String, String)> = vec![("per_page".into(), "1000".into())];
            if let Some(p) = prefix.filter(|p| !p.is_empty()) {
                q.push(("prefix".into(), p.to_string()));
            }
            if let Some(c) = &cursor {
                q.push(("cursor".into(), c.clone()));
            }
            let body = self.get(&path, &q).map_err(r2_hint)?;
            let (mut objs, info): (Vec<R2Object>, ResultInfo) =
                parse_envelope_paged(&body).map_err(r2_hint)?;
            all.append(&mut objs);
            match info.cursor.filter(|c| info.is_truncated && !c.is_empty()) {
                Some(c) => cursor = Some(c),
                None => break,
            }
        }
        Ok(all)
    }
}

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

    // The list-objects REST envelope as Cloudflare documents it: `result` is a BARE
    // array of objects (NOT wrapped like buckets), and pagination lives in
    // `result_info` as `cursor` + `is_truncated`. A fixture pins the verified shape.
    #[test]
    fn r2_objects_result_is_a_bare_array_with_cursor_pagination() {
        let body = r#"{"success":true,"errors":[],"messages":[],
            "result":[
                {"key":"img/logo.png","size":2048,"last_modified":"2026-01-02T03:04:05.000Z",
                 "storage_class":"Standard","etag":"e1"},
                {"key":"img/hero.jpg","size":10485760,"last_modified":"2026-01-03T00:00:00.000Z",
                 "storage_class":"Standard"}
            ],
            "result_info":{"cursor":"next-page-cursor","is_truncated":true,"per_page":1000}}"#;
        let (objects, info): (Vec<R2Object>, ResultInfo) = parse_envelope_paged(body).unwrap();
        assert_eq!(objects.len(), 2);
        assert_eq!(objects[0].key, "img/logo.png");
        assert_eq!(objects[0].size, 2048);
        assert_eq!(objects[0].last_modified, "2026-01-02T03:04:05.000Z");
        assert_eq!(objects[0].storage_class, "Standard");
        assert_eq!(objects[1].size, 10_485_760);
        // Truncated → the caller loops with this cursor.
        assert!(info.is_truncated);
        assert_eq!(info.cursor.as_deref(), Some("next-page-cursor"));
    }

    #[test]
    fn r2_objects_last_page_is_not_truncated_and_empty_is_empty() {
        let last = r#"{"success":true,"errors":[],"messages":[],
            "result":[{"key":"only.txt","size":5,"last_modified":"2026-01-01T00:00:00.000Z",
                       "storage_class":"Standard"}],
            "result_info":{"is_truncated":false,"per_page":1000}}"#;
        let (objects, info): (Vec<R2Object>, ResultInfo) = parse_envelope_paged(last).unwrap();
        assert_eq!(objects.len(), 1);
        assert!(!info.is_truncated, "a non-truncated page never loops");

        let empty = r#"{"success":true,"errors":[],"messages":[],
            "result":[],"result_info":{"is_truncated":false}}"#;
        let (objects, _): (Vec<R2Object>, ResultInfo) = parse_envelope_paged(empty).unwrap();
        assert!(objects.is_empty());
    }

    #[test]
    fn a_record_body_has_no_priority_and_can_be_proxied() {
        let b = record_body("A", "www.example.com", "1.2.3.4", 1, true, None);
        assert_eq!(b["type"], "A");
        assert_eq!(b["ttl"], 1); // 1 = automatic
        assert_eq!(b["proxied"], true);
        assert!(b.get("priority").is_none());
    }

    #[test]
    fn mx_record_body_carries_priority_and_is_not_proxied() {
        let b = record_body(
            "MX",
            "example.com",
            "mail.example.com",
            3600,
            false,
            Some(10),
        );
        assert_eq!(b["priority"], 10);
        assert!(b.get("proxied").is_none(), "MX is not proxyable");
    }

    #[test]
    fn only_v1_types_are_valid() {
        for t in ["A", "AAAA", "CNAME", "TXT", "NS", "MX"] {
            assert!(valid_record_type(t));
        }
        for t in ["SRV", "CAA", "LOC", "URI", "bogus"] {
            assert!(!valid_record_type(t));
        }
        assert!(proxyable("A") && proxyable("aaaa") && proxyable("CNAME"));
        assert!(!proxyable("TXT") && !proxyable("MX"));
    }

    #[test]
    fn patch_body_only_carries_set_fields() {
        let patch = RecordPatch {
            content: Some("5.6.7.8".into()),
            ..Default::default()
        };
        let b = apply_patch(&patch);
        assert_eq!(b["content"], "5.6.7.8");
        assert!(b.get("proxied").is_none() && b.get("ttl").is_none());
        assert!(!patch.is_empty());
        assert!(
            RecordPatch::default().is_empty(),
            "an all-None patch is empty"
        );
    }

    #[test]
    fn resolve_zone_prefers_name_then_id() {
        let zones = vec![
            Zone {
                id: "id-a".into(),
                name: "example.com".into(),
                status: "active".into(),
            },
            Zone {
                id: "example.com".into(),
                name: "other.com".into(),
                status: "active".into(),
            },
        ];
        assert_eq!(
            resolve_zone(&zones, "example.com").unwrap().id,
            "id-a",
            "name wins over a coincidental id"
        );
        assert_eq!(resolve_zone(&zones, "id-a").unwrap().name, "example.com");
        assert!(resolve_zone(&zones, "nope.com").is_none());
    }

    #[test]
    fn filter_query_uses_operator_keys() {
        let f = RecordFilter {
            kind: Some("A".into()),
            name: Some("api".into()),
            content: None,
        };
        let q = filter_query(&f);
        assert!(q.contains(&("type".into(), "A".into())));
        assert!(q.contains(&("name.contains".into(), "api".into())));
        assert!(
            q.contains(&("match".into(), "all".into())),
            "AND when >1 filter"
        );
        assert!(filter_query(&RecordFilter::default()).is_empty());
    }

    #[test]
    fn select_records_matches_ids_and_wheres() {
        let recs = vec![
            rec("r1", "A", "a.example.com", "1.1.1.1"),
            rec("r2", "A", "b.example.com", "1.1.1.1"),
            rec("r3", "CNAME", "c.example.com", "a.example.com"),
        ];
        let sel = Selector {
            where_content: Some("1.1.1.1".into()),
            ..Default::default()
        };
        assert_eq!(
            select_records(&recs, &sel).len(),
            2,
            "repoint: everything on the old IP"
        );
        let sel = Selector {
            where_content: Some("1.1.1.1".into()),
            where_type: Some("A".into()),
            ..Default::default()
        };
        assert_eq!(
            select_records(&recs, &sel).len(),
            2,
            "type + content intersect"
        );
        let sel = Selector {
            ids: vec!["r3".into()],
            ..Default::default()
        };
        assert_eq!(select_records(&recs, &sel)[0].id, "r3");
        let sel = Selector {
            where_content: Some("9.9.9.9".into()),
            ..Default::default()
        };
        assert!(select_records(&recs, &sel).is_empty(), "no match → empty");
        assert!(
            select_records(&recs, &Selector::default()).is_empty(),
            "empty selector matches nothing"
        );
    }

    #[test]
    fn r2_list_result_is_wrapped_in_a_buckets_object() {
        // The list-buckets `result` is NOT a bare array — it wraps a `buckets` array,
        // and `location` may be omitted. Feed a documented sample envelope and assert
        // the wrapper yields the buckets. result_info.cursor drives the page loop.
        let body = r#"{"success":true,"errors":[],"messages":[],
            "result":{"buckets":[
                {"name":"assets","creation_date":"2024-01-02T03:04:05.000Z",
                 "storage_class":"Standard","jurisdiction":"default"},
                {"name":"backups","creation_date":"2024-02-02T03:04:05.000Z",
                 "location":"weur","storage_class":"InfrequentAccess","jurisdiction":"default"}]},
            "result_info":{"cursor":""}}"#;
        let (res, info): (BucketsResult, ResultInfo) = parse_envelope_paged(body).unwrap();
        assert_eq!(res.buckets.len(), 2);
        assert_eq!(res.buckets[0].name, "assets");
        assert!(res.buckets[0].location.is_none(), "location may be omitted");
        assert_eq!(res.buckets[1].location.as_deref(), Some("weur"));
        assert_eq!(res.buckets[1].storage_class, "InfrequentAccess");
        // An empty cursor means the last page — the loop stops.
        assert!(info.cursor.filter(|c| !c.is_empty()).is_none());
    }

    #[test]
    fn r2_auth_error_hints_at_the_workers_r2_storage_permission() {
        let e = anyhow::anyhow!("Cloudflare: Authentication error");
        let hinted = r2_hint(e).to_string();
        assert!(
            hinted.contains("Workers R2 Storage"),
            "an R2 auth failure must name the missing permission: {hinted}"
        );
        // A non-auth error is passed through untouched.
        let other = r2_hint(anyhow::anyhow!(
            "Cloudflare: The bucket you tried to delete is not empty"
        ));
        assert!(!other.to_string().contains("Workers R2 Storage"));
    }

    fn rec(id: &str, kind: &str, name: &str, content: &str) -> Record {
        Record {
            id: id.into(),
            kind: kind.into(),
            name: name.into(),
            content: content.into(),
            ttl: 1,
            proxied: false,
            priority: None,
        }
    }
}
