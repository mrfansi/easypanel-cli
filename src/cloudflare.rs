//! Cloudflare — a bounded context OUTSIDE EasyPanel: manage one or more Cloudflare
//! accounts' zones and DNS records. Nothing here touches the EasyPanel domain; the two
//! share only the TUI event loop and the config directory (separate files).

use anyhow::Result;
use chrono::{Duration, Utc};
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
    /// R2 delimiter-mode common prefixes: the subfolders at this level. Present only
    /// when the request carries `delimiter=/`; each entry is a FULL key prefix ending
    /// in `/` (e.g. `assets/css/`). Absent otherwise → empty.
    #[serde(default)]
    pub delimited: Vec<String>,
}

/// One level of an R2 bucket browsed as a folder tree: the subfolders at this level
/// (`folders`, full key prefixes ending in `/`) and the files directly here (`files`).
/// `truncated` is set when a single level held more than one page. Built by
/// [`CloudflareClient::list_r2_level`] with `delimiter=/`, so `/`-delimited object keys
/// browse as nested folders instead of one flat 1000-row dump.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct R2Level {
    pub folders: Vec<String>,
    pub files: Vec<R2Object>,
    pub truncated: bool,
}

/// Sort a level for display: folders alphabetically (ascending), files newest-first
/// (by `last_modified`, ISO-8601 so a string compare is chronological). Pure, so the
/// ordering is unit-tested from a fixture without a live call.
pub fn sort_r2_level(level: &mut R2Level) {
    level.folders.sort();
    level
        .files
        .sort_by(|a, b| b.last_modified.cmp(&a.last_modified));
}

/// The status-bar feedback for marked rows — EasyPanel's exact wording
/// (`report_marks`), with the noun swapped per screen ("record" on DNS records,
/// "file" on R2 objects). Pure, so the wording parity is unit-tested.
pub fn marks_status(noun: &str, n: usize) -> String {
    format!("{n} {noun}(s) marked — [Space] to act on them, [Esc] to clear")
}

/// The Cloudflare REST object endpoints (`PUT`/`GET`/`DELETE …/objects/{key}`) cap a
/// single request at 300 MB. Larger objects need the S3 API (which this tool uses only
/// for DB dumps), so an oversized upload is rejected up front rather than attempted.
pub const MAX_REST_OBJECT_BYTES: u64 = 300 * 1024 * 1024;

/// Percent-encode an object key for the REST path. Slashes stay LITERAL so the key still
/// browses as a folder tree (`dir/a.gz`, not `dir%2Fa.gz`); every byte outside the
/// unreserved set `A-Za-z0-9-._~` becomes `%XX` (uppercase hex). Pure.
pub fn encode_object_key(key: &str) -> String {
    let mut out = String::with_capacity(key.len());
    for &b in key.as_bytes() {
        match b {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'.' | b'_' | b'~' | b'/' => {
                out.push(b as char);
            }
            _ => out.push_str(&format!("%{b:02X}")),
        }
    }
    out
}

/// The last path segment of an object key (the whole key when it has no `/`) — used as the
/// default local filename on download. Pure.
pub fn object_basename(key: &str) -> &str {
    key.rsplit('/').next().unwrap_or(key)
}

/// The destination object key for an upload: `prefix` (already `""` or ending in `/`) plus
/// the OS basename of `local_path` (any directory stripped). Pure. Used by the TUI (Phase
/// B) to upload into the currently-browsed folder; the CLI `put` takes an explicit key.
pub fn upload_key(prefix: &str, local_path: &str) -> String {
    let base = std::path::Path::new(local_path)
        .file_name()
        .map(|n| n.to_string_lossy().into_owned())
        .unwrap_or_else(|| local_path.to_string());
    format!("{prefix}{base}")
}

#[derive(Debug, Clone, Default, PartialEq)]
pub struct AnalyticsMetric {
    pub label: String,
    pub value: u64,
}

#[derive(Debug, Clone, Default, PartialEq)]
pub struct CountryTraffic {
    pub country: String,
    pub requests: u64,
    pub bandwidth: u64,
}

#[derive(Debug, Clone, Default, PartialEq)]
pub struct AnalyticsSummary {
    pub days: u16,
    pub requests: u64,
    pub bandwidth: u64,
    pub visits: u64,
    pub countries: Vec<CountryTraffic>,
    pub ssl: Vec<AnalyticsMetric>,
    pub cache: Vec<AnalyticsMetric>,
    pub status: Vec<AnalyticsMetric>,
    pub protocols: Vec<AnalyticsMetric>,
    pub content_types: Vec<AnalyticsMetric>,
}

#[derive(Debug, Clone, Default, PartialEq)]
pub struct WebAnalyticsSite {
    pub site_tag: String,
    pub site_token: String,
    pub host: String,
    pub created: String,
    pub auto_install: bool,
    pub enabled: bool,
    pub zone_name: String,
    pub zone_tag: String,
    pub page_views_24h: Option<u64>,
    pub visits_24h: Option<u64>,
}

#[derive(Debug, Clone, Default, Deserialize)]
struct WebAnalyticsRule {
    #[serde(default)]
    host: String,
    #[serde(default)]
    is_paused: bool,
}

#[derive(Debug, Clone, Default, Deserialize)]
struct WebAnalyticsRuleset {
    #[serde(default)]
    enabled: bool,
    #[serde(default)]
    zone_name: String,
    #[serde(default)]
    zone_tag: String,
}

#[derive(Debug, Clone, Default, Deserialize)]
struct WebAnalyticsSiteRaw {
    #[serde(default)]
    auto_install: bool,
    #[serde(default)]
    created: String,
    #[serde(default)]
    rules: Vec<WebAnalyticsRule>,
    #[serde(default)]
    ruleset: Option<WebAnalyticsRuleset>,
    #[serde(default)]
    site_tag: String,
    #[serde(default)]
    site_token: String,
    #[serde(default)]
    page_views_24h: Option<u64>,
    #[serde(default)]
    visits_24h: Option<u64>,
}

impl From<WebAnalyticsSiteRaw> for WebAnalyticsSite {
    fn from(raw: WebAnalyticsSiteRaw) -> Self {
        let ruleset = raw.ruleset.unwrap_or_default();
        let first_rule = raw.rules.iter().find(|r| !r.host.is_empty());
        let host = if !ruleset.zone_name.is_empty() {
            ruleset.zone_name.clone()
        } else {
            first_rule.map(|r| r.host.clone()).unwrap_or_default()
        };
        let enabled = ruleset.enabled || raw.rules.iter().any(|r| !r.is_paused);
        Self {
            site_tag: raw.site_tag,
            site_token: raw.site_token,
            host,
            created: raw.created,
            auto_install: raw.auto_install,
            enabled,
            zone_name: ruleset.zone_name,
            zone_tag: ruleset.zone_tag,
            page_views_24h: raw.page_views_24h,
            visits_24h: raw.visits_24h,
        }
    }
}

fn graphql_error(body: &str) -> Option<String> {
    let v: Value = serde_json::from_str(body).ok()?;
    v.get("errors")
        .and_then(Value::as_array)
        .and_then(|errs| errs.first())
        .and_then(|e| e.get("message"))
        .and_then(Value::as_str)
        .map(|s| format!("Cloudflare GraphQL: {s}"))
}

fn group_count(g: &Value) -> u64 {
    g.get("count").and_then(Value::as_u64).unwrap_or(0)
}

fn group_sum(g: &Value, key: &str) -> u64 {
    g.get("sum")
        .and_then(|s| s.get(key))
        .and_then(Value::as_u64)
        .unwrap_or(0)
}

fn group_dimension(g: &Value, key: &str) -> String {
    let Some(value) = g.get("dimensions").and_then(|d| d.get(key)) else {
        return "-".into();
    };
    match value {
        Value::String(s) if !s.is_empty() => s.clone(),
        Value::Number(n) => n.to_string(),
        Value::Bool(b) => b.to_string(),
        _ => "-".into(),
    }
}

fn country_label(raw: &str) -> String {
    match raw {
        "AD" => "Andorra",
        "AE" => "United Arab Emirates",
        "AF" => "Afghanistan",
        "AG" => "Antigua and Barbuda",
        "AI" => "Anguilla",
        "AL" => "Albania",
        "AM" => "Armenia",
        "AO" => "Angola",
        "AR" => "Argentina",
        "AT" => "Austria",
        "AU" => "Australia",
        "AZ" => "Azerbaijan",
        "BA" => "Bosnia and Herzegovina",
        "BB" => "Barbados",
        "BD" => "Bangladesh",
        "BE" => "Belgium",
        "BF" => "Burkina Faso",
        "BG" => "Bulgaria",
        "BH" => "Bahrain",
        "BI" => "Burundi",
        "BJ" => "Benin",
        "BN" => "Brunei",
        "BO" => "Bolivia",
        "BR" => "Brazil",
        "BS" => "Bahamas",
        "BT" => "Bhutan",
        "BW" => "Botswana",
        "BY" => "Belarus",
        "BZ" => "Belize",
        "CA" => "Canada",
        "CD" => "Congo, Democratic Republic",
        "CG" => "Congo",
        "CH" => "Switzerland",
        "CI" => "Cote d'Ivoire",
        "CL" => "Chile",
        "CM" => "Cameroon",
        "CN" => "China",
        "CO" => "Colombia",
        "CR" => "Costa Rica",
        "CU" => "Cuba",
        "CV" => "Cabo Verde",
        "CY" => "Cyprus",
        "CZ" => "Czechia",
        "DE" => "Germany",
        "DK" => "Denmark",
        "DO" => "Dominican Republic",
        "DZ" => "Algeria",
        "EC" => "Ecuador",
        "EE" => "Estonia",
        "EG" => "Egypt",
        "ES" => "Spain",
        "ET" => "Ethiopia",
        "FI" => "Finland",
        "FR" => "France",
        "GB" => "United Kingdom",
        "GE" => "Georgia",
        "GH" => "Ghana",
        "GR" => "Greece",
        "GT" => "Guatemala",
        "HK" => "Hong Kong",
        "HN" => "Honduras",
        "HR" => "Croatia",
        "HU" => "Hungary",
        "ID" => "Indonesia",
        "IE" => "Ireland",
        "IL" => "Israel",
        "IN" => "India",
        "IQ" => "Iraq",
        "IR" => "Iran",
        "IS" => "Iceland",
        "IT" => "Italy",
        "JM" => "Jamaica",
        "JO" => "Jordan",
        "JP" => "Japan",
        "KE" => "Kenya",
        "KH" => "Cambodia",
        "KR" => "South Korea",
        "KW" => "Kuwait",
        "KZ" => "Kazakhstan",
        "LA" => "Laos",
        "LB" => "Lebanon",
        "LK" => "Sri Lanka",
        "LT" => "Lithuania",
        "LU" => "Luxembourg",
        "LV" => "Latvia",
        "MA" => "Morocco",
        "MD" => "Moldova",
        "MG" => "Madagascar",
        "MM" => "Myanmar",
        "MN" => "Mongolia",
        "MO" => "Macao",
        "MT" => "Malta",
        "MU" => "Mauritius",
        "MX" => "Mexico",
        "MY" => "Malaysia",
        "NG" => "Nigeria",
        "NL" => "Netherlands",
        "NO" => "Norway",
        "NP" => "Nepal",
        "NZ" => "New Zealand",
        "OM" => "Oman",
        "PA" => "Panama",
        "PE" => "Peru",
        "PH" => "Philippines",
        "PK" => "Pakistan",
        "PL" => "Poland",
        "PR" => "Puerto Rico",
        "PT" => "Portugal",
        "QA" => "Qatar",
        "RO" => "Romania",
        "RS" => "Serbia",
        "RU" => "Russia",
        "SA" => "Saudi Arabia",
        "SE" => "Sweden",
        "SG" => "Singapore",
        "SI" => "Slovenia",
        "SK" => "Slovakia",
        "TH" => "Thailand",
        "TN" => "Tunisia",
        "TR" => "Turkey",
        "TW" => "Taiwan",
        "UA" => "Ukraine",
        "US" => "United States",
        "UY" => "Uruguay",
        "UZ" => "Uzbekistan",
        "VE" => "Venezuela",
        "VN" => "Vietnam",
        "ZA" => "South Africa",
        _ => raw,
    }
    .to_string()
}

fn metric_groups(account: &Value, key: &str, dimension: &str) -> Vec<AnalyticsMetric> {
    account
        .get(key)
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
        .map(|g| AnalyticsMetric {
            label: group_dimension(g, dimension),
            value: group_count(g),
        })
        .collect()
}

pub fn parse_account_analytics(body: &str, days: u16) -> Result<AnalyticsSummary> {
    if let Some(e) = graphql_error(body) {
        anyhow::bail!("{e}");
    }
    let v: Value = serde_json::from_str(body)
        .map_err(|e| anyhow::anyhow!("unexpected Cloudflare GraphQL response: {e}"))?;
    let account = v
        .pointer("/data/viewer/accounts")
        .and_then(Value::as_array)
        .and_then(|a| a.first())
        .ok_or_else(|| anyhow::anyhow!("Cloudflare GraphQL returned no account analytics"))?;
    let total = account
        .get("totals")
        .and_then(Value::as_array)
        .and_then(|a| a.first())
        .cloned()
        .unwrap_or(Value::Null);
    let countries = account
        .get("countries")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
        .map(|g| CountryTraffic {
            country: country_label(&group_dimension(g, "clientCountryName")),
            requests: group_count(g),
            bandwidth: group_sum(g, "edgeResponseBytes"),
        })
        .collect();
    Ok(AnalyticsSummary {
        days,
        requests: group_count(&total),
        bandwidth: group_sum(&total, "edgeResponseBytes"),
        visits: group_sum(&total, "visits"),
        countries,
        ssl: metric_groups(account, "ssl", "clientSSLProtocol"),
        cache: metric_groups(account, "cache", "cacheStatus"),
        status: metric_groups(account, "status", "edgeResponseStatus"),
        protocols: metric_groups(account, "protocols", "clientRequestHTTPProtocol"),
        content_types: metric_groups(account, "contentTypes", "edgeResponseContentTypeName"),
    })
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

/// A zone's health category, for the at-a-glance status colour in the Zones list — the
/// CF analogue of EasyPanel's coloured Status column. Classifying the raw status string
/// is a domain decision, so it lives here (pure, testable); the renderer only maps a
/// category to a colour.
///
/// `active` serves through Cloudflare; `pending`/`initializing` are not live yet (the
/// nameservers have not been moved to Cloudflare — the operator must act);
/// `moved`/`deactivated`/`deleted` no longer serve. Anything unrecognised stays neutral.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ZoneHealth {
    Active,
    Pending,
    Inactive,
    Unknown,
}

pub fn zone_health(status: &str) -> ZoneHealth {
    match status {
        "active" => ZoneHealth::Active,
        "pending" | "initializing" => ZoneHealth::Pending,
        "moved" | "deactivated" | "deleted" => ZoneHealth::Inactive,
        _ => ZoneHealth::Unknown,
    }
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

// ---------- Client-side list filters (narrow an already-loaded list) ----------
//
// The CF-local `/` filter narrows the list already in hand rather than refetching
// (a zone can hold thousands of records). Which fields a needle matches is a domain
// rule, so it lives here; the TUI's `cf_*_shown` selectors are thin callers.

/// The zones whose name/status/id contains `needle` (case-insensitive). An empty
/// needle keeps everything.
pub fn filter_zones<'a>(zones: &'a [Zone], needle: &str) -> Vec<&'a Zone> {
    let n = needle.to_ascii_lowercase();
    zones
        .iter()
        .filter(|z| {
            n.is_empty()
                || z.name.to_ascii_lowercase().contains(&n)
                || z.status.to_ascii_lowercase().contains(&n)
                || z.id.to_ascii_lowercase().contains(&n)
        })
        .collect()
}

/// The records whose type/name/content contains `needle` (case-insensitive). An
/// empty needle keeps everything.
pub fn filter_records<'a>(records: &'a [Record], needle: &str) -> Vec<&'a Record> {
    let n = needle.to_ascii_lowercase();
    records
        .iter()
        .filter(|r| {
            n.is_empty()
                || r.kind.to_ascii_lowercase().contains(&n)
                || r.name.to_ascii_lowercase().contains(&n)
                || r.content.to_ascii_lowercase().contains(&n)
        })
        .collect()
}

/// The R2 buckets whose name/class/location contains `needle` (case-insensitive).
/// An empty needle keeps everything.
pub fn filter_buckets<'a>(buckets: &'a [R2Bucket], needle: &str) -> Vec<&'a R2Bucket> {
    let n = needle.to_ascii_lowercase();
    buckets
        .iter()
        .filter(|b| {
            n.is_empty()
                || b.name.to_ascii_lowercase().contains(&n)
                || b.storage_class.to_ascii_lowercase().contains(&n)
                || b.location
                    .as_deref()
                    .unwrap_or("")
                    .to_ascii_lowercase()
                    .contains(&n)
        })
        .collect()
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

    /// List all account-level Web Analytics sites. Cloudflare exposes this under the
    /// RUM Site Info REST API; the endpoint returns site metadata, not RUM traffic
    /// totals, so page-view/visit fields stay optional.
    pub fn list_web_analytics_sites(&self, account_id: &str) -> Result<Vec<WebAnalyticsSite>> {
        let mut all = Vec::new();
        let mut page = 1u32;
        loop {
            let q: Vec<(String, String)> = vec![
                ("page".into(), page.to_string()),
                ("per_page".into(), "50".into()),
                ("order_by".into(), "host".into()),
            ];
            let body = self.get(&format!("/accounts/{account_id}/rum/site_info/list"), &q)?;
            let (mut sites, info): (Vec<WebAnalyticsSiteRaw>, ResultInfo) =
                parse_envelope_paged(&body)?;
            all.extend(sites.drain(..).map(WebAnalyticsSite::from));
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

    fn graphql(&self, body: &Value) -> Result<String> {
        Ok(self
            .http
            .post(format!("{BASE}/graphql"))
            .bearer_auth(&self.token)
            .json(body)
            .send()?
            .text()?)
    }

    pub fn account_analytics(&self, account_id: &str, days: u16) -> Result<AnalyticsSummary> {
        let end = Utc::now();
        let start = end - Duration::days(i64::from(days.max(1)));
        let query = r#"
query AccountAnalytics($accountTag: string, $filter: filter) {
  viewer {
    accounts(filter: { accountTag: $accountTag }) {
      totals: httpRequestsAdaptiveGroups(limit: 1, filter: $filter) {
        count
        sum { edgeResponseBytes visits }
      }
      countries: httpRequestsAdaptiveGroups(limit: 10, orderBy: [count_DESC], filter: $filter) {
        count
        sum { edgeResponseBytes }
        dimensions { clientCountryName }
      }
      ssl: httpRequestsAdaptiveGroups(limit: 6, orderBy: [count_DESC], filter: $filter) {
        count
        dimensions { clientSSLProtocol }
      }
      cache: httpRequestsAdaptiveGroups(limit: 8, orderBy: [count_DESC], filter: $filter) {
        count
        dimensions { cacheStatus }
      }
      status: httpRequestsAdaptiveGroups(limit: 8, orderBy: [count_DESC], filter: $filter) {
        count
        dimensions { edgeResponseStatus }
      }
      protocols: httpRequestsAdaptiveGroups(limit: 6, orderBy: [count_DESC], filter: $filter) {
        count
        dimensions { clientRequestHTTPProtocol }
      }
    }
  }
}
"#;
        let body = json!({
            "query": query,
            "variables": {
                "accountTag": account_id,
                "filter": {
                    "datetime_geq": start.to_rfc3339_opts(chrono::SecondsFormat::Secs, true),
                    "datetime_lt": end.to_rfc3339_opts(chrono::SecondsFormat::Secs, true),
                    "requestSource": "eyeball"
                }
            }
        });
        let resp = self.graphql(&body)?;
        parse_account_analytics(&resp, days)
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

    /// List ONE level of a bucket browsed as a folder tree. Sends `delimiter=/` so the
    /// `/`-delimited keys group into subfolders instead of a flat 1000-row dump: `result`
    /// is the files directly at `prefix` (no further `/`), and `result_info.delimited`
    /// (VERIFIED against the R2 list-objects API docs — an array of full key prefixes
    /// ending in `/`) is the subfolders. `prefix` is "" at the bucket root, or e.g.
    /// `assets/css/` deeper. Same Bearer token as buckets — no S3 credentials.
    ///
    /// A bucket can hold millions of objects, so one level is fetched (up to 1000 rows)
    /// and `truncated` reported rather than walking every cursor page. The result is
    /// sorted for display (folders A→Z, files newest-first) inside the domain.
    pub fn list_r2_level(&self, account_id: &str, bucket: &str, prefix: &str) -> Result<R2Level> {
        let path = format!("/accounts/{account_id}/r2/buckets/{bucket}/objects");
        let mut q: Vec<(String, String)> = vec![
            ("per_page".into(), "1000".into()),
            ("delimiter".into(), "/".into()),
        ];
        if !prefix.is_empty() {
            q.push(("prefix".into(), prefix.to_string()));
        }
        let body = self.get(&path, &q).map_err(r2_hint)?;
        parse_r2_level(&body).map_err(r2_hint)
    }

    /// Upload raw bytes to an object key. `PUT …/objects/{key}` with the raw body and a
    /// `Content-Type` (default `application/octet-stream`); success is the standard
    /// envelope. The caller must enforce [`MAX_REST_OBJECT_BYTES`] before calling.
    pub fn put_object(
        &self,
        account_id: &str,
        bucket: &str,
        key: &str,
        bytes: Vec<u8>,
        content_type: Option<&str>,
    ) -> Result<()> {
        let url = format!(
            "{BASE}/accounts/{account_id}/r2/buckets/{bucket}/objects/{}",
            encode_object_key(key)
        );
        let body = self
            .http
            .put(url)
            .bearer_auth(&self.token)
            .header(
                reqwest::header::CONTENT_TYPE,
                content_type.unwrap_or("application/octet-stream"),
            )
            .body(bytes)
            .send()
            .and_then(|r| r.text())
            .map_err(|e| r2_hint(e.into()))?;
        let _: Value = parse_envelope(&body).map_err(r2_hint)?;
        Ok(())
    }

    /// Download an object, streaming its body into `out` (never buffering the whole file in
    /// memory). On success the body IS the raw object bytes; on error it is a JSON error
    /// envelope, which is parsed so "object not found" / auth errors surface properly.
    /// Returns the byte count written.
    pub fn download_object(
        &self,
        account_id: &str,
        bucket: &str,
        key: &str,
        out: &mut dyn std::io::Write,
    ) -> Result<u64> {
        let url = format!(
            "{BASE}/accounts/{account_id}/r2/buckets/{bucket}/objects/{}",
            encode_object_key(key)
        );
        let mut resp = self
            .http
            .get(url)
            .bearer_auth(&self.token)
            .send()
            .map_err(|e| r2_hint(e.into()))?;
        if resp.status().is_success() {
            Ok(resp.copy_to(out)?)
        } else {
            let body = resp.text().map_err(|e| r2_hint(e.into()))?;
            // A non-2xx body is the JSON error envelope; parse_envelope turns it into the
            // Cloudflare message. It never returns Ok on an error body.
            let _: Value = parse_envelope(&body).map_err(r2_hint)?;
            anyhow::bail!("Cloudflare returned an error status but no error message")
        }
    }

    /// Delete one object key. `DELETE …/objects/{key}`; success is the standard envelope.
    pub fn delete_object(&self, account_id: &str, bucket: &str, key: &str) -> Result<()> {
        let url = format!(
            "{BASE}/accounts/{account_id}/r2/buckets/{bucket}/objects/{}",
            encode_object_key(key)
        );
        let body = self
            .http
            .delete(url)
            .bearer_auth(&self.token)
            .send()
            .and_then(|r| r.text())
            .map_err(|e| r2_hint(e.into()))?;
        let _: Value = parse_envelope(&body).map_err(r2_hint)?;
        Ok(())
    }
}

/// Parse a delimiter-mode objects response into one browse level: the files at this
/// level (`result`) plus the subfolder prefixes (`result_info.delimited`), sorted for
/// display. Pure, so the folder/file split + sort is unit-tested from a fixture with no
/// live call.
fn parse_r2_level(body: &str) -> Result<R2Level> {
    let (files, info): (Vec<R2Object>, ResultInfo) = parse_envelope_paged(body)?;
    let mut level = R2Level {
        folders: info.delimited,
        files,
        truncated: info.is_truncated,
    };
    sort_r2_level(&mut level);
    Ok(level)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn zone_health_classifies_the_status_string() {
        assert_eq!(zone_health("active"), ZoneHealth::Active);
        assert_eq!(zone_health("pending"), ZoneHealth::Pending);
        assert_eq!(zone_health("initializing"), ZoneHealth::Pending);
        assert_eq!(zone_health("moved"), ZoneHealth::Inactive);
        assert_eq!(zone_health("deactivated"), ZoneHealth::Inactive);
        // An unrecognised status stays neutral rather than being mislabelled healthy.
        assert_eq!(zone_health("read only"), ZoneHealth::Unknown);
        assert_eq!(zone_health(""), ZoneHealth::Unknown);
    }

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

    // The delimiter-mode (`delimiter=/`) envelope: `result` is the FILES at this level and
    // `result_info.delimited` is the SUBFOLDERS (full key prefixes ending in `/`) — the
    // verified field for common prefixes. A fixture pins the shape; the parse splits and
    // sorts it into one browse level.
    #[test]
    fn r2_level_splits_files_from_folders_and_sorts_them() {
        let body = r#"{"success":true,"errors":[],"messages":[],
            "result":[
                {"key":"assets/admin-front-end/app.js","size":10,
                 "last_modified":"2026-01-02T00:00:00.000Z","storage_class":"Standard"},
                {"key":"assets/admin-front-end/index.html","size":20,
                 "last_modified":"2026-03-09T00:00:00.000Z","storage_class":"Standard"}
            ],
            "result_info":{"is_truncated":true,"per_page":1000,
                "delimited":["assets/admin-front-end/js/","assets/admin-front-end/css/"]}}"#;
        let level = parse_r2_level(body).unwrap();
        // Folders come from `delimited`, sorted A→Z.
        assert_eq!(
            level.folders,
            vec![
                "assets/admin-front-end/css/".to_string(),
                "assets/admin-front-end/js/".to_string()
            ]
        );
        // Files come from `result`, newest-first (index.html is the newer one).
        assert_eq!(level.files.len(), 2);
        assert_eq!(level.files[0].key, "assets/admin-front-end/index.html");
        assert_eq!(level.files[1].key, "assets/admin-front-end/app.js");
        // A level with more than one page reports truncated for the "narrow with /" note.
        assert!(level.truncated);
    }

    #[test]
    fn r2_level_root_with_no_subfolders_parses_empty_folders() {
        let body = r#"{"success":true,"errors":[],"messages":[],
            "result":[{"key":"readme.txt","size":3,
                       "last_modified":"2026-01-01T00:00:00.000Z","storage_class":"Standard"}],
            "result_info":{"is_truncated":false,"per_page":1000}}"#;
        let level = parse_r2_level(body).unwrap();
        assert!(level.folders.is_empty(), "no delimited → no subfolders");
        assert_eq!(level.files.len(), 1);
        assert!(!level.truncated);
    }

    #[test]
    fn sort_r2_level_folders_ascending_files_newest_first() {
        let mk = |key: &str, ts: &str| R2Object {
            key: key.into(),
            size: 1,
            last_modified: ts.into(),
            storage_class: "Standard".into(),
        };
        let mut level = R2Level {
            folders: vec!["z/".into(), "a/".into(), "m/".into()],
            files: vec![
                mk("old.txt", "2026-01-01T00:00:00.000Z"),
                mk("new.txt", "2026-06-01T00:00:00.000Z"),
                mk("mid.txt", "2026-03-01T00:00:00.000Z"),
            ],
            truncated: false,
        };
        sort_r2_level(&mut level);
        assert_eq!(level.folders, vec!["a/", "m/", "z/"]);
        let order: Vec<&str> = level.files.iter().map(|f| f.key.as_str()).collect();
        assert_eq!(order, vec!["new.txt", "mid.txt", "old.txt"]);
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
    fn web_analytics_sites_parse_documented_site_info_shape() {
        let body = r#"{"success":true,"errors":[],"messages":[],
            "result":[{
                "auto_install":true,
                "created":"2014-01-01T05:20:00.12345Z",
                "rules":[{
                    "id":"rule-1",
                    "created":"2014-01-01T05:20:00.12345Z",
                    "host":"example.com",
                    "inclusive":true,
                    "is_paused":false,
                    "paths":["*"],
                    "priority":1000
                }],
                "ruleset":{
                    "id":"ruleset-1",
                    "enabled":true,
                    "zone_name":"example.com",
                    "zone_tag":"zone-1"
                },
                "site_tag":"site-1",
                "site_token":"token-1",
                "snippet":"<script></script>"
            }],
            "result_info":{"page":1,"per_page":10,"count":1,"total_count":1,"total_pages":1}}"#;

        let (raw, info): (Vec<WebAnalyticsSiteRaw>, ResultInfo) =
            parse_envelope_paged(body).unwrap();
        let sites: Vec<WebAnalyticsSite> = raw.into_iter().map(WebAnalyticsSite::from).collect();

        assert_eq!(info.total_pages, 1);
        assert_eq!(sites.len(), 1);
        assert_eq!(sites[0].host, "example.com");
        assert_eq!(sites[0].zone_name, "example.com");
        assert_eq!(sites[0].zone_tag, "zone-1");
        assert_eq!(sites[0].site_tag, "site-1");
        assert_eq!(sites[0].site_token, "token-1");
        assert!(sites[0].auto_install);
        assert!(sites[0].enabled);
        assert_eq!(sites[0].page_views_24h, None);
        assert_eq!(sites[0].visits_24h, None);
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

    #[test]
    fn encode_object_key_percent_encodes_everything_but_slash_and_unreserved() {
        // Space → %20, colon → %3A; the slash and the unreserved bytes stay literal.
        assert_eq!(encode_object_key("a/b c:d.gz"), "a/b%20c%3Ad.gz");
        // A plain nested key is unchanged.
        assert_eq!(encode_object_key("dir/sub/x.sql.gz"), "dir/sub/x.sql.gz");
    }

    #[test]
    fn object_basename_takes_the_segment_after_the_last_slash() {
        assert_eq!(object_basename("a/b/c.gz"), "c.gz");
        assert_eq!(object_basename("x"), "x");
    }

    #[test]
    fn upload_key_joins_prefix_and_the_local_basename() {
        assert_eq!(upload_key("dir/", "/tmp/dump.sql.gz"), "dir/dump.sql.gz");
        assert_eq!(upload_key("", "x.gz"), "x.gz");
    }

    #[test]
    fn max_rest_object_bytes_is_300_mib() {
        assert_eq!(MAX_REST_OBJECT_BYTES, 300 * 1024 * 1024);
    }

    #[test]
    fn marks_status_uses_easypanels_exact_wording() {
        assert_eq!(
            marks_status("record", 7),
            "7 record(s) marked — [Space] to act on them, [Esc] to clear"
        );
        assert_eq!(
            marks_status("file", 1),
            "1 file(s) marked — [Space] to act on them, [Esc] to clear"
        );
    }

    #[test]
    fn account_analytics_parser_reads_totals_countries_and_breakdowns() {
        let body = r#"{
          "data": {"viewer": {"accounts": [{
            "totals": [{"count": 44120000, "sum": {"edgeResponseBytes": 4606400000000, "visits": 2280000}}],
            "countries": [
              {"count": 17590000, "sum": {"edgeResponseBytes": 2396400000000}, "dimensions": {"clientCountryName": "ID"}},
              {"count": 11280000, "sum": {"edgeResponseBytes": 883179520000}, "dimensions": {"clientCountryName": "Singapore"}}
            ],
            "ssl": [{"count": 39960000, "dimensions": {"clientSSLProtocol": "TLSv1.3"}}],
            "cache": [{"count": 623820, "dimensions": {"cacheStatus": "hit"}}],
            "status": [{"count": 12780000, "dimensions": {"edgeResponseStatus": 404}}],
            "protocols": [{"count": 17200000, "dimensions": {"clientRequestHTTPProtocol": "HTTP/1.1"}}]
          }]}}
        }"#;
        let s = parse_account_analytics(body, 7).unwrap();
        assert_eq!(s.days, 7);
        assert_eq!(s.requests, 44_120_000);
        assert_eq!(s.bandwidth, 4_606_400_000_000);
        assert_eq!(s.visits, 2_280_000);
        assert_eq!(s.countries[0].country, "Indonesia");
        assert_eq!(s.countries[1].country, "Singapore");
        assert_eq!(s.countries[1].bandwidth, 883_179_520_000);
        assert_eq!(s.ssl[0].label, "TLSv1.3");
        assert_eq!(s.cache[0].label, "hit");
        assert_eq!(s.status[0].label, "404");
        assert_eq!(s.protocols[0].label, "HTTP/1.1");
        assert!(s.content_types.is_empty());
    }

    #[test]
    fn account_analytics_parser_surfaces_graphql_errors() {
        let err = parse_account_analytics(
            r#"{"errors":[{"message":"permission denied: Account Analytics Read required"}]}"#,
            7,
        )
        .unwrap_err()
        .to_string();
        assert!(err.contains("Account Analytics Read required"));
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
