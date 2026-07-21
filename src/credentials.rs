//! What a database service's connection identity reads as — the Credentials context.
//!
//! EasyPanel's own dashboard has a Credentials screen for every database: user,
//! password, the internal host and port, and a ready-to-paste connection URL.
//! The tool already KNOWS these — the DB shell (`y`) logs in with them — but it
//! only ever used them, never showed them. This context turns the same
//! `inspectService` fields into something an operator can read and copy.
//!
//! No I/O here: the caller inspects the service, this decides how the identity
//! reads. It never logs and never masks — masking is a display choice the screen
//! makes; the value carried here is the real one, because a copy has to be.

use serde_json::Value;

use crate::output::field;

/// One labelled credential line. A `secret` line is masked on screen until
/// revealed, and is the one worth a one-key copy.
pub struct Cred {
    pub label: String,
    pub value: String,
    pub secret: bool,
}

/// The connection identity of a database service, in the panel's own order, or
/// `None` for a non-database type.
///
/// Built from the same `inspectService` fields the DB shell authenticates with,
/// so what is shown is what actually connects. For mysql/mariadb the app user is
/// shown when it exists, falling back to `root`; postgres falls back to
/// `postgres` — matching how each image seeds its default account.
pub fn credentials(stype: &str, s: &Value) -> Option<Vec<Cred>> {
    let f = |k: &str| field(s, &format!("/{k}"));
    let host = format!("{}_{}", f("projectName"), f("name"));

    let (user, password, port, db) = match stype {
        "mysql" | "mariadb" => {
            if f("user").is_empty() {
                (
                    String::from("root"),
                    f("rootPassword"),
                    "3306",
                    f("databaseName"),
                )
            } else {
                (f("user"), f("password"), "3306", f("databaseName"))
            }
        }
        "postgres" => {
            let user = if f("user").is_empty() {
                String::from("postgres")
            } else {
                f("user")
            };
            (user, f("password"), "5432", f("databaseName"))
        }
        "mongo" => (f("user"), f("password"), "27017", String::new()),
        // Redis has no user account; the client authenticates as `default`.
        "redis" => (
            String::from("default"),
            f("password"),
            "6379",
            String::new(),
        ),
        _ => return None,
    };

    let url = connection_url(stype, &user, &password, &host, port, &db);
    Some(vec![
        Cred {
            label: "User".into(),
            value: user,
            secret: false,
        },
        Cred {
            label: "Password".into(),
            value: password,
            secret: true,
        },
        Cred {
            label: "Internal Host".into(),
            value: host,
            secret: false,
        },
        Cred {
            label: "Internal Port".into(),
            value: port.into(),
            secret: false,
        },
        Cred {
            label: "Connection URL".into(),
            value: url,
            secret: true,
        },
    ])
}

/// The internal connection URL for the client each type speaks, with user and
/// password percent-encoded so a password containing `@`, `:` or `/` still
/// parses as one credential rather than splitting the authority.
fn connection_url(
    stype: &str,
    user: &str,
    password: &str,
    host: &str,
    port: &str,
    db: &str,
) -> String {
    let (u, p) = (pct(user), pct(password));
    match stype {
        "mysql" | "mariadb" => format!("mysql://{u}:{p}@{host}:{port}/{db}"),
        "postgres" => format!("postgres://{u}:{p}@{host}:{port}/{db}"),
        "mongo" => format!("mongodb://{u}:{p}@{host}:{port}/?authSource=admin"),
        "redis" => format!("redis://{u}:{p}@{host}:{port}"),
        _ => String::new(),
    }
}

/// Percent-encode everything outside the URL "unreserved" set (RFC 3986:
/// ALPHA / DIGIT / `-` `.` `_` `~`). Small on purpose — this only ever sees a
/// username or password, not a whole path, so there is no query or slash to keep.
fn pct(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    for b in s.bytes() {
        if b.is_ascii_alphanumeric() || matches!(b, b'-' | b'.' | b'_' | b'~') {
            out.push(b as char);
        } else {
            out.push_str(&format!("%{b:02X}"));
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn a_non_database_has_no_credentials() {
        assert!(credentials("app", &json!({ "projectName": "p", "name": "web" })).is_none());
    }

    #[test]
    fn redis_authenticates_as_default_with_only_a_password() {
        let s = json!({ "projectName": "viding-org-db", "name": "redis_planner", "password": "s3cr3t" });
        let c = credentials("redis", &s).unwrap();
        assert_eq!(c[0].label, "User");
        assert_eq!(c[0].value, "default");
        assert_eq!(c[1].label, "Password");
        assert!(c[1].secret);
        assert_eq!(c[1].value, "s3cr3t");
        assert_eq!(c[2].value, "viding-org-db_redis_planner");
        assert_eq!(c[3].value, "6379");
        // The connection URL carries the real password and is itself a secret.
        assert_eq!(
            c[4].value,
            "redis://default:s3cr3t@viding-org-db_redis_planner:6379"
        );
        assert!(c[4].secret);
    }

    #[test]
    fn mysql_shows_the_app_user_or_falls_back_to_root() {
        let app_user = json!({
            "projectName": "shop", "name": "mysql",
            "user": "app", "password": "pw", "rootPassword": "rp", "databaseName": "shop"
        });
        let c = credentials("mysql", &app_user).unwrap();
        assert_eq!(c[0].value, "app");
        assert_eq!(c[1].value, "pw");
        assert_eq!(c[4].value, "mysql://app:pw@shop_mysql:3306/shop");

        // No app user seeded → root is the account that exists.
        let root_only = json!({
            "projectName": "shop", "name": "mysql",
            "user": "", "password": "", "rootPassword": "rp", "databaseName": "shop"
        });
        let c = credentials("mysql", &root_only).unwrap();
        assert_eq!(c[0].value, "root");
        assert_eq!(c[1].value, "rp");
    }

    #[test]
    fn a_password_with_url_reserved_characters_is_encoded_in_the_url() {
        // A generated password is usually alphanumeric, but nothing guarantees it —
        // an `@` unencoded would split the authority and the URL would connect to
        // the wrong host.
        let s = json!({ "projectName": "p", "name": "pg", "user": "u", "password": "a@b:c/d", "databaseName": "db" });
        let c = credentials("postgres", &s).unwrap();
        assert_eq!(c[4].value, "postgres://u:a%40b%3Ac%2Fd@p_pg:5432/db");
        // The Password line itself stays the raw value — it is what you paste into
        // a client's password box, not into a URL.
        assert_eq!(c[1].value, "a@b:c/d");
    }
}
