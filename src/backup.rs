//! Database backups: running one now, and restoring one.
//!
//! Everything here was probed against a live panel (EasyPanel 2.32.2), because
//! the shapes are not guessable:
//!
//! - `createDatabaseBackup` takes a SCHEDULE (projectName, serviceName,
//!   databaseName, schedule, enabled, storageProviderId, storageProviderPath).
//! - `runDatabaseBackup` takes only `{id}` — the id of a schedule. There is no
//!   "back up this database once" endpoint at all.
//! - `deleteDatabaseBackup` removes a schedule (note the name: `delete`, where
//!   every other destructive op in this API is `destroy`).
//! - `restoreDatabaseBackup` takes projectName, serviceName, databaseName,
//!   storageProviderId and `path` — so it can restore INTO any database service,
//!   not only the one the backup came from.
//!
//! Two consequences shape this module.
//!
//! **Backing up "now" needs a schedule.** So one is created disabled, run, and
//! deleted again. Verified live: a DISABLED schedule runs perfectly well (an
//! earlier failure looked like the `enabled` flag but was the database simply not
//! being up yet — a backup of a database that isn't running answers "Invariant
//! failed").
//!
//! **Nothing lists the backup FILES.** There is no `listDatabaseBackupFiles`, so
//! a restore would mean typing a path like
//! `myproject/2026-07-19T20:37:49.317Z.sql.gz` from memory. But every run records
//! an action whose `meta` carries exactly what a restore needs:
//!
//! ```json
//! { "type": "backup", "status": "done",
//!   "meta": { "databaseName": "shop", "path": "…sql.gz", "storageProviderId": "…" } }
//! ```
//!
//! so the history IS the file list.

use serde_json::{json, Value};

use crate::output::field;

/// A backup that actually exists, recovered from the action history.
pub struct BackupFile {
    pub when: String,
    pub database: String,
    pub path: String,
    pub storage_provider_id: String,
}

impl BackupFile {
    /// One row, for a picker. The timestamp leads because that is what a person
    /// chooses by; the path is the machine's business and comes last.
    pub fn row(&self) -> String {
        format!("{:<21}{:<18}{}", self.when, self.database, self.path)
    }
}

/// The backups that exist for one service, newest first.
///
/// Built from `listActions`, filtered to this service's SUCCESSFUL backup runs —
/// a failed run still writes an action, and offering its path for restore would
/// hand the user a file that was never finished.
pub fn history(actions: &[Value], project: &str, service: &str) -> Vec<BackupFile> {
    actions
        .iter()
        .filter(|a| {
            field(a, "/type") == "backup"
                && field(a, "/status") == "done"
                && field(a, "/projectName") == project
                && field(a, "/serviceName") == service
        })
        .filter_map(|a| {
            let path = field(a, "/meta/path");
            if path.is_empty() {
                return None;
            }
            Some(BackupFile {
                when: field(a, "/createdAt"),
                database: field(a, "/meta/databaseName"),
                path,
                storage_provider_id: field(a, "/meta/storageProviderId"),
            })
        })
        .collect()
}

/// Every restorable backup on a host, whatever project it came from.
///
/// Restoring ACROSS hosts must not assume the two sides use the same names: a
/// backup of `shop/db` is a perfectly good thing to load into `shop-staging/db`
/// on another machine. Filtering the source by the destination's names would
/// have shown an empty list and explained nothing. The project and service are
/// carried in the row instead, so the user can see where each one came from.
pub fn history_all(actions: &[Value]) -> Vec<(String, BackupFile)> {
    actions
        .iter()
        .filter(|a| field(a, "/type") == "backup" && field(a, "/status") == "done")
        .filter_map(|a| {
            let path = field(a, "/meta/path");
            if path.is_empty() {
                return None;
            }
            let origin = format!("{}/{}", field(a, "/projectName"), field(a, "/serviceName"));
            Some((
                origin,
                BackupFile {
                    when: field(a, "/createdAt"),
                    database: field(a, "/meta/databaseName"),
                    path,
                    storage_provider_id: field(a, "/meta/storageProviderId"),
                },
            ))
        })
        .collect()
}

/// The body for `createDatabaseBackup` / `updateDatabaseBackup`.
pub fn schedule_body(
    project: &str,
    service: &str,
    database: &str,
    schedule: &str,
    enabled: bool,
    provider: &str,
    path: &str,
) -> Value {
    json!({
        "projectName": project,
        "serviceName": service,
        "databaseName": database,
        "schedule": schedule,
        "enabled": enabled,
        "storageProviderId": provider,
        "storageProviderPath": path,
    })
}

/// The body for `restoreDatabaseBackup`.
pub fn restore_body(
    project: &str,
    service: &str,
    database: &str,
    provider: &str,
    path: &str,
) -> Value {
    json!({
        "projectName": project,
        "serviceName": service,
        "databaseName": database,
        "storageProviderId": provider,
        "path": path,
    })
}

/// Can this service type be backed up at all?
///
/// Redis cannot: `createDatabaseBackup` on a real redis service answers
/// `Service is not supported` (verified live), and redis has no `databaseName`
/// field either — so the picker was offering a database called "-", the
/// placeholder for a missing value, which could only ever fail. mysql, mariadb,
/// postgres and mongo are all accepted.
pub fn can_back_up(stype: &str) -> bool {
    matches!(stype, "mysql" | "mariadb" | "postgres" | "mongo")
}

/// Is this a real database name, rather than a missing one?
///
/// `field()` yields "-" for an absent value, and an empty or "-" name is
/// rejected by the endpoint — so neither belongs in a list of things to back up.
pub fn is_named(database: &str) -> bool {
    !database.is_empty() && database != "-"
}

/// MySQL/MariaDB and PostgreSQL keep their own bookkeeping schemas; nobody wants
/// to back those up, and restoring one would be actively harmful.
const INTERNAL: &[&str] = &[
    "information_schema",
    "mysql",
    "performance_schema",
    "sys",
    "postgres",
    "template0",
    "template1",
];

/// The databases a service actually holds, from a `SHOW DATABASES`-style listing.
///
/// EasyPanel records ONE `databaseName` per service — the one it created — but a
/// server happily holds many, and `createDatabaseBackup` accepts any of them
/// (verified live: backing up a schema the panel never heard of produced a real
/// dump of exactly that schema). There is no API that lists them, so the list is
/// read from the database itself through the container shell.
pub fn parse_databases(output: &str) -> Vec<String> {
    output
        .lines()
        .map(|l| l.trim())
        .filter(|l| {
            !l.is_empty()
                && !INTERNAL.contains(&l.to_ascii_lowercase().as_str())
                // Shell noise: warnings, prompts, anything with a space or a
                // colon is not a database name.
                && !l.contains(' ')
                && !l.contains(':')
        })
        .map(String::from)
        .collect()
}

/// The command that lists a service's databases, for the engines where one
/// exists. `None` = this engine has no such listing here, so the caller falls
/// back to the single name the panel recorded.
pub fn list_databases_command(stype: &str, user: &str, password: &str) -> Option<String> {
    let q = |s: &str| s.replace('\'', "'\\''");
    match stype {
        "mysql" | "mariadb" => Some(format!(
            "MYSQL_PWD='{}' mysql -uroot -N -B -e 'SHOW DATABASES'",
            q(password)
        )),
        "postgres" => Some(format!(
            "PGPASSWORD='{}' psql -U {} -tAc 'SELECT datname FROM pg_database WHERE NOT datistemplate'",
            q(password),
            if user.is_empty() { "postgres" } else { user }
        )),
        _ => None,
    }
}

/// A `local` provider writes to the host's own disk, so nothing on another host
/// can read it. Anything else is remote and therefore shareable.
pub fn is_remote(provider_type: &str) -> bool {
    provider_type != "local"
}

/// Which provider a backup should go to, given what the panel has.
///
/// A REMOTE one wins when there is one: a backup you can restore anywhere is
/// strictly more useful than one locked to a single host, and choosing local
/// silently is how someone discovers on the bad day that their backup cannot
/// leave the machine. The caller still names the choice before running.
pub fn preferred_provider(
    providers: &[(String, String, String)],
) -> Option<&(String, String, String)> {
    providers
        .iter()
        .find(|(_, _, t)| is_remote(t))
        .or_else(|| providers.first())
}

/// Where a one-off backup is filed under the storage provider.
///
/// The project name, so backups group the way the panel does. Same shape the
/// dashboard uses when you set a schedule up by hand.
pub fn default_path(project: &str) -> String {
    project.to_string()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn action(t: &str, status: &str, service: &str, path: &str) -> Value {
        json!({
            "type": t, "status": status,
            "projectName": "shopco", "serviceName": service,
            "createdAt": "2026-07-19 20:37:49",
            "meta": { "databaseName": "shop", "path": path,
                      "storageProviderId": "prov1" }
        })
    }

    #[test]
    fn history_is_built_from_successful_backup_actions_only() {
        // The action log is the only list of backup FILES this API has; a failed
        // run must not appear, or restore would offer a file that never finished.
        let actions = vec![
            action("backup", "done", "db", "shopco/a.sql.gz"),
            action("backup", "error", "db", "shopco/broken.sql.gz"),
            action("deployment", "done", "db", "shopco/x.sql.gz"),
            action("backup", "done", "other", "shopco/elsewhere.sql.gz"),
        ];
        let h = history(&actions, "shopco", "db");
        assert_eq!(h.len(), 1, "only this service's successful backups");
        assert_eq!(h[0].path, "shopco/a.sql.gz");
        assert_eq!(h[0].storage_provider_id, "prov1");
        assert_eq!(h[0].database, "shop");
    }

    #[test]
    fn an_action_without_a_path_is_not_a_restorable_backup() {
        let actions = vec![action("backup", "done", "db", "")];
        assert!(history(&actions, "shopco", "db").is_empty());
    }

    #[test]
    fn a_cross_host_list_keeps_every_project_and_says_where_each_came_from() {
        // Across hosts the names need not match, so filtering by the
        // destination's project/service would show an empty list and explain
        // nothing.
        let actions = vec![
            action("backup", "done", "db", "shopco/a.sql.gz"),
            action("backup", "done", "other", "shopco/b.sql.gz"),
            action("backup", "error", "db", "shopco/bad.sql.gz"),
        ];
        let all = history_all(&actions);
        assert_eq!(all.len(), 2, "both services, no failed run");
        assert_eq!(all[0].0, "shopco/db");
        assert_eq!(all[1].0, "shopco/other");
    }

    #[test]
    fn redis_is_not_offered_a_backup_it_cannot_have() {
        // Verified live: redis answers "Service is not supported", and carries no
        // databaseName — so the picker had been offering "-", the placeholder for
        // a value that isn't there.
        assert!(!can_back_up("redis"));
        for t in ["mysql", "mariadb", "postgres", "mongo"] {
            assert!(can_back_up(t), "{t}");
        }
        assert!(!is_named("") && !is_named("-"));
        assert!(is_named("shop"));
    }

    #[test]
    fn listing_databases_skips_the_engines_own_bookkeeping() {
        // Real `SHOW DATABASES` output, plus the warning mysql prints on stderr.
        let out =
            "mysql: [Warning] Using a password on the command line interface can be insecure.\n\
                   information_schema\nmysql\nperformance_schema\nsys\ntoko\ngudang\nutama\n";
        assert_eq!(parse_databases(out), vec!["toko", "gudang", "utama"]);
        // Postgres bookkeeping goes too.
        assert_eq!(
            parse_databases("postgres\ntemplate0\ntemplate1\nshop\n"),
            vec!["shop"]
        );
    }

    #[test]
    fn only_engines_that_can_list_their_databases_are_asked_to() {
        assert!(list_databases_command("mysql", "app", "pw").is_some());
        assert!(list_databases_command("postgres", "app", "pw").is_some());
        // Redis has no schemas to choose between; mongo is not offered one here.
        assert!(list_databases_command("redis", "", "pw").is_none());
        assert!(list_databases_command("mongo", "u", "pw").is_none());
    }

    #[test]
    fn a_remote_provider_is_preferred_over_the_local_disk() {
        let p = |i: &str, n: &str, t: &str| (i.into(), n.into(), t.into());
        let both = vec![p("1", "Local Disk", "local"), p("2", "R2", "s3")];
        assert_eq!(preferred_provider(&both).unwrap().1, "R2");
        // Order must not decide it: a backup that can leave the host wins either way.
        let flipped = vec![p("2", "R2", "s3"), p("1", "Local Disk", "local")];
        assert_eq!(preferred_provider(&flipped).unwrap().1, "R2");
        // Only a local one? Still usable — just not off this host.
        let only_local = vec![p("1", "Local Disk", "local")];
        assert_eq!(preferred_provider(&only_local).unwrap().1, "Local Disk");
        assert!(preferred_provider(&[]).is_none());
        assert!(!is_remote("local") && is_remote("s3"));
    }

    #[test]
    fn a_restore_names_its_target_not_its_origin() {
        // The target service is whatever the user picked — restoring into a
        // DIFFERENT service is the point, not an accident.
        let b = restore_body("proj", "target-db", "shop", "prov1", "proj/a.sql.gz");
        assert_eq!(b["serviceName"], json!("target-db"));
        assert_eq!(b["databaseName"], json!("shop"));
        assert_eq!(b["path"], json!("proj/a.sql.gz"));
        assert_eq!(b["storageProviderId"], json!("prov1"));
    }
}
