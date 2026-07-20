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
