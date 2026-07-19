//! What a lifecycle action MEANS for a given service type.
//!
//! EasyPanel does not give every service type the same lifecycle verbs, and the
//! difference is not cosmetic — the missing ones are not routes at all. Probed
//! against a live panel (2.32.2), sending an operation that does not exist for a
//! type answers with the bare `{"error":"Not found"}` of an unknown route, as
//! opposed to the tRPC-shaped 400 a real operation gives for a bad argument:
//!
//! | type                                    | deploy | restart | stop | start | enable/disable |
//! |-----------------------------------------|--------|---------|------|-------|----------------|
//! | app, compose                            | yes    | yes     | yes  | yes   | no             |
//! | box, wordpress                          | NO     | yes     | yes  | yes   | no             |
//! | mysql, mariadb, postgres, mongo, redis  | NO     | NO      | NO   | NO    | yes            |
//!
//! So the tool used to send `services/mysql/restartService` for every database —
//! a route that has never existed — and the whole Lifecycle menu 404'd on every
//! database in the panel. Worse, that left the Config File editor with no way to
//! take effect: a config change is only picked up when the process restarts, and
//! nothing in the tool could restart a database.
//!
//! A database stops and starts through `enabled`, which is a config flag rather
//! than a running state. Cycling it is what a restart IS for a database: verified
//! live by setting `max_connections = 999` in a MySQL config file — the file
//! appeared in the container immediately but the server still reported 151 for
//! nearly two hours, and only `disableService` + `enableService` brought it up
//! with the new value.

/// Service types that are a database: lifecycle runs through `enabled`.
const DATABASES: &[&str] = &["mysql", "mariadb", "postgres", "mongo", "redis"];

/// Service types with no build step, so nothing to deploy.
const NOT_BUILDABLE: &[&str] = &["box", "wordpress"];

pub fn is_database(stype: &str) -> bool {
    DATABASES.contains(&stype)
}

/// The API operations `action` needs for `stype`, in the order they must run.
///
/// `None` means this service type has no such action — the caller must not send
/// it and the UI must not offer it. An unknown type is treated as app-like, so a
/// service type this tool has never heard of keeps working as before rather than
/// losing its menu.
pub fn ops(stype: &str, action: &str) -> Option<Vec<&'static str>> {
    if is_database(stype) {
        return match action {
            // Cycling `enabled` is a database's restart — and the only way a
            // config-file change ever takes effect.
            "restart" => Some(vec!["disableService", "enableService"]),
            "stop" => Some(vec!["disableService"]),
            "start" => Some(vec!["enableService"]),
            "destroy" => Some(vec!["destroyService"]),
            // A database is pulled, never built.
            _ => None,
        };
    }
    match action {
        "deploy" if NOT_BUILDABLE.contains(&stype) => None,
        "deploy" => Some(vec!["deployService"]),
        "restart" => Some(vec!["restartService"]),
        "stop" => Some(vec!["stopService"]),
        "start" => Some(vec!["startService"]),
        "destroy" => Some(vec!["destroyService"]),
        _ => None,
    }
}

/// Does this type have `updateResources`? Everything but `compose`, whose limits
/// live in its compose file rather than in a panel field.
pub fn has_resource_limits(stype: &str) -> bool {
    stype != "compose"
}

/// Does this type have `updateAdvanced` — the "Config File (Advanced)" editor?
///
/// The databases (where EasyPanel keeps e.g. a MySQL replication config) and
/// `box`. NOT app or compose, which answer the bare 404 of a missing route.
pub fn has_config_file(stype: &str) -> bool {
    is_database(stype) || stype == "box"
}

/// Why an action isn't offered, in the user's words. Shown instead of sending a
/// request that could only ever come back as a 404.
pub fn unavailable(stype: &str, action: &str) -> String {
    if is_database(stype) && action == "deploy" {
        return format!("A {stype} service is pulled, not built — there is nothing to deploy. Use Restart to pick up a config change.");
    }
    if action == "deploy" {
        return format!("A {stype} service has no build step, so it cannot be deployed.");
    }
    format!("A {stype} service has no '{action}' action.")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_database_restarts_by_cycling_enabled() {
        // The route the tool used to send does not exist; this pair does, and is
        // the only thing that makes a config-file change take effect.
        assert_eq!(
            ops("mysql", "restart"),
            Some(vec!["disableService", "enableService"])
        );
        assert_eq!(ops("postgres", "stop"), Some(vec!["disableService"]));
        assert_eq!(ops("redis", "start"), Some(vec!["enableService"]));
        // Destroy is the one lifecycle route databases DO share with apps.
        assert_eq!(ops("mongo", "destroy"), Some(vec!["destroyService"]));
    }

    #[test]
    fn nothing_that_cannot_be_built_offers_a_deploy() {
        assert_eq!(ops("mysql", "deploy"), None);
        assert_eq!(ops("box", "deploy"), None);
        assert_eq!(ops("wordpress", "deploy"), None);
        assert_eq!(ops("app", "deploy"), Some(vec!["deployService"]));
        assert_eq!(ops("compose", "deploy"), Some(vec!["deployService"]));
    }

    #[test]
    fn resource_limits_and_config_file_follow_the_routes_that_exist() {
        // Probed live: compose has no updateResources, and updateAdvanced exists
        // for the databases and `box` but NOT for app or compose.
        assert!(!has_resource_limits("compose"));
        assert!(has_resource_limits("app") && has_resource_limits("box"));
        assert!(has_config_file("mysql") && has_config_file("box"));
        assert!(!has_config_file("app") && !has_config_file("compose"));
    }

    #[test]
    fn an_unknown_service_type_keeps_the_app_like_verbs() {
        // A type added by a future EasyPanel must not silently lose its menu.
        assert_eq!(ops("newthing", "restart"), Some(vec!["restartService"]));
        assert_eq!(ops("newthing", "deploy"), Some(vec!["deployService"]));
        assert!(!is_database("newthing"));
    }
}
