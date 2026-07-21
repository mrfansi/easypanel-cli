//! Rewriting many domains at once.
//!
//! Moving a fleet from one hostname to another, or repointing every domain that
//! feeds a service being replaced, is a per-domain edit form repeated twenty
//! times — twenty chances to fat-finger one and not notice which.
//!
//! The rewrite is plain find-and-replace over ONE named part of the domain, not
//! a regex: the strings involved are hostnames and service names, where a stray
//! `.` matching any character is a silent wrong answer rather than a helpful
//! one. Everything else in the domain — middlewares, the certificate resolver,
//! the other servers of a custom destination — is carried through untouched,
//! because a bulk edit that quietly resets a field nobody was looking at is
//! worse than no bulk edit at all.
//!
//! This module is the rule, not the screen: it decides what a rewrite MEANS and
//! refuses the ones that would produce a broken domain. What it never does is
//! send anything — the caller previews the plan, and only then applies it.

use std::collections::HashSet;

use serde_json::{json, Value};

use crate::output::field;

/// The parts a bulk rewrite can touch, as the user picks them.
pub const TARGETS: &[&str] = &["host", "destination service", "destination url"];

/// One domain's rewrite: what it reads now, what it would read, and the body
/// that gets it there.
#[derive(Debug, Clone, PartialEq)]
pub struct Change {
    pub id: String,
    /// The domain this rewrite belongs to. Carried even when the host is not what
    /// is being rewritten: repointing five domains at a new service gives five
    /// IDENTICAL before → after lines, and without the host the preview cannot
    /// say which five.
    pub host: String,
    pub before: String,
    pub after: String,
    pub body: Value,
}

/// Does this domain point at a service that no longer exists?
///
/// `services` is every (project, service) pair on the host. `None` means the
/// list has not loaded yet — and then NOTHING is judged, because an empty list
/// would condemn every domain on the panel at once. That guard is the whole
/// difference between a useful flag and 713 false alarms.
///
/// Only a service destination can be orphaned: a custom destination points at a
/// URL this tool knows nothing about, and calling it dead would be a guess.
///
/// Measured on a live host: exactly ONE of 713 domains was orphaned. That is the
/// point of it — one dead route is invisible among seven hundred live ones, and
/// it is the kind of thing nobody finds until a deploy quietly stops arriving.
pub fn is_orphan(d: &Value, services: Option<&HashSet<(String, String)>>) -> bool {
    let Some(services) = services else {
        return false;
    };
    if field(d, "/destinationType") != "service" {
        return false;
    }
    let pair = (
        field(d, "/serviceDestination/projectName"),
        field(d, "/serviceDestination/serviceName"),
    );
    !services.contains(&pair)
}

// ---------- What a domain reads as ----------
//
// These were in `commands.rs`, the CLI layer, and the TUI reached across into it
// for them — one presentation module borrowing another's idea of what a domain
// is. They are the domain's own vocabulary: both surfaces now depend on this
// context instead of on each other.

/// Domain source: "https://host/path", with a "*." in front of a wildcard host.
///
/// EasyPanel stores `*.edu.example` as `{ host: "edu.example", wildcard: true }`
/// — the star is a separate flag, not part of the host. Ignoring it renders a
/// wildcard and its apex identically ("https://edu.example/" for both), so two
/// different routes look like one duplicate. The panel's own UI shows the star;
/// so does this now.
pub fn domain_source(d: &Value) -> String {
    let scheme = if d.get("https").and_then(Value::as_bool).unwrap_or(false) {
        "https"
    } else {
        "http"
    };
    let host = field(d, "/host");
    let host = if d.get("wildcard").and_then(Value::as_bool).unwrap_or(false) {
        format!("*.{host}")
    } else {
        host
    };
    format!("{scheme}://{host}{}", field(d, "/path"))
}

/// Domain destination: an internal service, or a list of custom servers with their weights.
pub fn domain_destination(d: &Value) -> String {
    match field(d, "/destinationType").as_str() {
        "service" => format!(
            "{}://{}_{}:{}{}",
            field(d, "/serviceDestination/protocol"),
            field(d, "/serviceDestination/projectName"),
            field(d, "/serviceDestination/serviceName"),
            field(d, "/serviceDestination/port"),
            field(d, "/serviceDestination/path"),
        ),
        "custom" => d
            .pointer("/customDestination/servers")
            .and_then(Value::as_array)
            .map(|servers| {
                // A weight only means something RELATIVE to the other servers in
                // the set. On a lone destination it always takes 100% of traffic,
                // so a trailing "(1)" carries no information while reading like an
                // unexplained token on the routing screen. Show weights only when
                // there is more than one server to weigh against.
                let show_weight = servers.len() > 1;
                servers
                    .iter()
                    .map(|s| {
                        let url = field(s, "/url");
                        if show_weight {
                            format!("{url} ({})", field(s, "/weight"))
                        } else {
                            url
                        }
                    })
                    .collect::<Vec<_>>()
                    .join(", ")
            })
            .unwrap_or_else(|| "-".to_string()),
        _ => "-".to_string(),
    }
}

pub const DOMAIN_HEADERS: [&str; 3] = ["Source", "Destination", "ID"];

pub fn domain_row(d: &Value) -> Vec<String> {
    vec![domain_source(d), domain_destination(d), field(d, "/id")]
}

/// The rewrites `find` → `replace` would make across `domains`.
///
/// Domains the search doesn't appear in are simply absent from the plan — the
/// caller shows what WILL change, not a list mostly full of "unchanged". An
/// error means the rewrite is unsafe for at least one domain, and then nothing
/// is planned at all: a partial rewrite of a hostname across a fleet is a
/// half-migrated mess to untangle by hand.
pub fn plan(
    domains: &[&Value],
    target: &str,
    find: &str,
    replace: &str,
) -> Result<Vec<Change>, String> {
    if find.is_empty() {
        return Err("Find is required — an empty search would match every domain".into());
    }
    if !TARGETS.contains(&target) {
        return Err(format!("Unknown part '{target}'"));
    }
    let mut out = Vec::new();
    for d in domains {
        if let Some(change) = change(d, target, find, replace)? {
            out.push(change);
        }
    }
    Ok(out)
}

/// The rewrite for ONE domain: `None` when the search doesn't appear in it, or
/// when the domain has no such part (a service destination has no URL).
fn change(d: &Value, target: &str, find: &str, replace: &str) -> Result<Option<Change>, String> {
    let id = field(d, "/id");
    let mut body = d.clone();
    let (before, after) = match target {
        "host" => {
            let before = field(d, "/host");
            if !before.contains(find) {
                return Ok(None);
            }
            let after = before.replace(find, replace);
            if after.is_empty() {
                return Err(format!("Rewriting '{before}' would leave it with no host"));
            }
            body["host"] = json!(after);
            (before, after)
        }
        "destination service" => {
            if field(d, "/destinationType") != "service" {
                return Ok(None);
            }
            // Presented as "project/service" so one search can move a domain
            // between projects as well as between services.
            let before = format!(
                "{}/{}",
                field(d, "/serviceDestination/projectName"),
                field(d, "/serviceDestination/serviceName")
            );
            if !before.contains(find) {
                return Ok(None);
            }
            let after = before.replace(find, replace);
            let (project, service) = after
                .split_once('/')
                .filter(|(p, s)| !p.is_empty() && !s.is_empty() && !s.contains('/'))
                .ok_or_else(|| {
                    format!("'{after}' is not a project/service pair — the rewrite would break it")
                })?;
            body["serviceDestination"]["projectName"] = json!(project);
            body["serviceDestination"]["serviceName"] = json!(service);
            (before, after)
        }
        _ => {
            let servers = d
                .pointer("/customDestination/servers")
                .and_then(Value::as_array)
                .cloned()
                .unwrap_or_default();
            let urls: Vec<String> = servers.iter().map(|s| field(s, "/url")).collect();
            if !urls.iter().any(|u| u.contains(find)) {
                return Ok(None);
            }
            // Every server of the domain is rewritten, not just the first: a
            // custom destination is a load-balanced set, and moving half of it
            // is how traffic ends up split across two hostnames.
            let mut rewritten = servers.clone();
            for (i, url) in urls.iter().enumerate() {
                let new = url.replace(find, replace);
                if new.is_empty() {
                    return Err(format!("Rewriting '{url}' would leave it with no URL"));
                }
                rewritten[i]["url"] = json!(new);
            }
            body["customDestination"]["servers"] = json!(rewritten);
            let after = rewritten
                .iter()
                .map(|s| field(s, "/url"))
                .collect::<Vec<_>>()
                .join(", ");
            (urls.join(", "), after)
        }
    };
    Ok(Some(Change {
        id,
        host: field(d, "/host"),
        before,
        after,
        body,
    }))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn domain_destination_handles_service_and_custom() {
        let service = json!({
            "destinationType": "service",
            "serviceDestination": {
                "protocol": "http", "projectName": "proj", "serviceName": "api",
                "port": 8000, "path": "/"
            }
        });
        assert_eq!(domain_destination(&service), "http://proj_api:8000/");

        let custom = json!({
            "destinationType": "custom",
            "customDestination": { "servers": [
                { "url": "https://a.test", "weight": 1 },
                { "url": "https://b.test", "weight": 2 }
            ]}
        });
        assert_eq!(
            domain_destination(&custom),
            "https://a.test (1), https://b.test (2)",
            "with two servers the weights disambiguate the split"
        );

        // One server: the weight is always 100% of traffic, so it is dropped —
        // no unexplained "(1)" on the destination.
        let single = json!({
            "destinationType": "custom",
            "customDestination": { "servers": [
                { "url": "https://only.test", "weight": 1 }
            ]}
        });
        assert_eq!(domain_destination(&single), "https://only.test");

        assert_eq!(
            domain_destination(&json!({ "destinationType": "unknown" })),
            "-"
        );
    }

    #[test]
    fn domain_source_uses_scheme_from_https_flag() {
        assert_eq!(
            domain_source(&json!({ "https": true, "host": "a.test", "path": "/x" })),
            "https://a.test/x"
        );
        assert_eq!(
            domain_source(&json!({ "https": false, "host": "a.test", "path": "/" })),
            "http://a.test/"
        );
    }

    #[test]
    fn a_wildcard_host_shows_its_star_prefix() {
        // EasyPanel stores *.edu.example as { host: "edu.example", wildcard: true }.
        // Without the star, a wildcard and its apex render identically — two
        // different routes that look like one.
        assert_eq!(
            domain_source(
                &json!({ "https": true, "host": "edu.example", "path": "/", "wildcard": true })
            ),
            "https://*.edu.example/"
        );
        // A non-wildcard host keeps its bare name.
        assert_eq!(
            domain_source(
                &json!({ "https": true, "host": "edu.example", "path": "/", "wildcard": false })
            ),
            "https://edu.example/"
        );
    }

    fn service_domain() -> Value {
        json!({
            "id": "d1",
            "host": "app.old.com",
            "path": "/api",
            "https": true,
            "middlewares": ["rate-limit"],
            "destinationType": "service",
            "serviceDestination": {
                "projectName": "shop", "serviceName": "api",
                "port": 3000, "protocol": "http", "path": "/"
            }
        })
    }

    fn custom_domain() -> Value {
        json!({
            "id": "d2",
            "host": "cdn.old.com",
            "destinationType": "custom",
            "customDestination": { "servers": [
                { "url": "http://a.old.com", "weight": 1 },
                { "url": "http://b.old.com", "weight": 2 }
            ]}
        })
    }

    #[test]
    fn a_domain_pointing_at_a_service_that_is_gone_is_orphaned() {
        let live: HashSet<(String, String)> = [("shop".to_string(), "api".to_string())]
            .into_iter()
            .collect();
        let alive = service_domain();
        assert!(!is_orphan(&alive, Some(&live)));

        let mut dead = service_domain();
        dead["serviceDestination"]["serviceName"] = json!("api-old");
        assert!(is_orphan(&dead, Some(&live)));
    }

    #[test]
    fn nothing_is_orphaned_until_the_service_list_has_loaded() {
        // The dangerous case: judging against an empty list would mark every
        // domain on the panel dead at once — a confident wrong answer on 713
        // rows, which is far worse than saying nothing.
        let d = service_domain();
        assert!(!is_orphan(&d, None));
        // A custom destination points at a URL this tool knows nothing about, so
        // it can never be judged either.
        let live: HashSet<(String, String)> = HashSet::new();
        assert!(!is_orphan(&custom_domain(), Some(&live)));
        // ...while a service destination IS judged against a loaded list.
        assert!(is_orphan(&d, Some(&live)));
    }

    #[test]
    fn a_host_rewrite_leaves_everything_else_alone() {
        let d = service_domain();
        let plan = plan(&[&d], "host", "old.com", "new.com").unwrap();
        assert_eq!(plan.len(), 1);
        assert_eq!(plan[0].before, "app.old.com");
        assert_eq!(plan[0].after, "app.new.com");
        // The path, the resolver flag and the middlewares are what a bulk edit
        // must NOT quietly reset while renaming a host.
        assert_eq!(field(&plan[0].body, "/path"), "/api");
        assert_eq!(plan[0].body["https"], json!(true));
        assert_eq!(plan[0].body["middlewares"], json!(["rate-limit"]));
    }

    #[test]
    fn domains_the_search_misses_are_not_in_the_plan() {
        let (a, b) = (service_domain(), custom_domain());
        // Only the custom one has servers, so a URL rewrite plans just that one.
        let urls = plan(&[&a, &b], "destination url", "old.com", "new.com").unwrap();
        assert_eq!(urls.len(), 1);
        assert_eq!(urls[0].id, "d2");
        // And a search that matches nothing plans nothing, rather than
        // "changing" every domain to what it already said.
        assert!(plan(&[&a, &b], "host", "absent.example", "x")
            .unwrap()
            .is_empty());
    }

    #[test]
    fn every_server_of_a_custom_destination_moves_together() {
        let d = custom_domain();
        let plan = plan(&[&d], "destination url", "old.com", "new.com").unwrap();
        assert_eq!(plan[0].after, "http://a.new.com, http://b.new.com");
        // Weights are part of the destination, not of the rewrite.
        assert_eq!(plan[0].body["customDestination"]["servers"][1]["weight"], 2);
    }

    #[test]
    fn a_destination_can_move_project_as_well_as_service() {
        let d = service_domain();
        let plan = plan(&[&d], "destination service", "shop/api", "shop-v2/api").unwrap();
        assert_eq!(plan[0].before, "shop/api");
        assert_eq!(
            field(&plan[0].body, "/serviceDestination/projectName"),
            "shop-v2"
        );
        // The port and protocol belong to the destination, not to the rename.
        assert_eq!(plan[0].body["serviceDestination"]["port"], 3000);
    }

    #[test]
    fn a_rewrite_that_would_break_a_domain_plans_nothing_at_all() {
        let d = service_domain();
        // Deleting the host outright.
        assert!(plan(&[&d], "host", "app.old.com", "").is_err());
        // Losing the separator turns a destination into an unusable single name.
        assert!(plan(&[&d], "destination service", "shop/api", "shopapi").is_err());
        // An empty search would "match" every domain in the panel.
        assert!(plan(&[&d], "host", "", "x").is_err());
    }
}
