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
