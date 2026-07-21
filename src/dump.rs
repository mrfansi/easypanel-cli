//! Building the in-container shell commands for a non-locking database dump
//! straight to object storage — and the matching restore.
//!
//! Why this exists: EasyPanel's own `createDatabaseBackup` locks the running
//! database (no `--single-transaction`) so apps using it error out, files one
//! backup PER database, and restores only INTO a database that already exists.
//! These commands instead run our OWN `mysqldump --single-transaction` inside the
//! service container, gzip it, and push it to the existing R2 storage with a
//! presigned URL — non-locking, one self-contained file for several databases,
//! and (because `--databases` embeds `CREATE DATABASE`) restorable onto a host
//! where the schema never existed.
//!
//! The data flows container→R2 directly over the presigned URL, so it never
//! crosses our WebSocket and never hits a proxy's ~125 s timeout. R2 rejects a
//! streamed/chunked PUT with `411 MissingContentLength`, so we buffer the dump to
//! a temp file first (known Content-Length) and `curl -T` that.

/// Standard MySQL/MariaDB bookkeeping schemas — never dumped by `--all` and never
/// what a user means to move. (Deliberately NOT `sys`-only: the four together are
/// the engine's own, present on every server.)
const SYSTEM_DBS: &[&str] = &["information_schema", "mysql", "performance_schema", "sys"];

pub fn is_system_db(name: &str) -> bool {
    SYSTEM_DBS.contains(&name.to_ascii_lowercase().as_str())
}

/// Is a database name safe to splice into the container shell command?
///
/// These commands are run as `sh -c` inside the container. EasyPanel's own names
/// match `^[A-Za-z][A-Za-z0-9_-]*`, but `--databases` can be typed by hand, so
/// every name is gated to a strict charset first: an unchecked name would be a
/// shell-injection hole (the dump runs as root inside the DB container).
pub fn valid_db_name(name: &str) -> bool {
    !name.is_empty()
        && name.len() <= 64
        && name
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || b == b'_' || b == b'-')
}

/// Single-quote a value for `sh`: `'` → `'\''`, so a credential is safe inside a
/// single-quoted argument.
fn sh_quote(s: &str) -> String {
    s.replace('\'', "'\\''")
}

/// The dump+upload command for mysql/mariadb, or `None` for an engine we don't do
/// yet (postgres/mongo come later — mongo is not one SQL file). Callers MUST have
/// validated every database name with [`valid_db_name`] first. `tmp_file` is the
/// final `.sql.gz` path in the container.
///
/// Non-locking (`--single-transaction --quick`), self-contained (`--databases`
/// embeds `CREATE DATABASE`/`USE`, plus routines/triggers/events).
///
/// mysqldump writes to a plain FILE, which is then gzipped — NOT piped into gzip.
/// The container shell EasyPanel gives us is a PTY (`docker exec -it`); piping a
/// real-sized dump through it hits `errno 11 (EAGAIN)` on the write and corrupts
/// the stream (an empty schema slips through, real data does not). A regular-file
/// write never blocks, so dump→file→gzip→upload is the reliable shape.
pub fn dump_command(
    stype: &str,
    root_password: &str,
    databases: &[String],
    tmp_file: &str,
    presigned_put_url: &str,
) -> Option<String> {
    // mysql: `--skip-lock-tables` (with single-transaction) and drop GTIDs so the
    // dump restores onto any server without GTID conflicts. mariadb-dump wants
    // `--hex-blob` + utf8mb4 for clean blobs and has no GTID flag.
    let (tool, extra) = match stype {
        "mysql" => ("mysqldump", "--skip-lock-tables --set-gtid-purged=OFF"),
        "mariadb" => ("mariadb-dump", "--hex-blob --default-character-set=utf8mb4"),
        _ => return None,
    };
    let dbs = databases.join(" ");
    // `tmp_file` ends in .gz; mysqldump writes the .sql, then `gzip` produces the .gz.
    // `--set-gtid-purged=OFF` also silences mysqldump's GTID warning, so nothing is
    // written to stderr on success — important because the container shell is a PTY
    // and a stray write racing our read can EAGAIN.
    let sql_file = tmp_file.strip_suffix(".gz").unwrap_or(tmp_file);
    let work = format!(
        "MYSQL_PWD='{pw}' {tool} -uroot --databases {dbs} \
         --single-transaction --quick --routines --triggers --events {extra} > '{sql}' \
         && gzip -f '{sql}' && curl -sfS -T '{gz}' '{url}'",
        pw = sh_quote(root_password),
        sql = sql_file,
        gz = tmp_file,
        url = presigned_put_url,
    );
    Some(cleanup(&work, sql_file, tmp_file))
}

/// Wrap an in-container command so its temp files are removed WHATEVER happens —
/// on a failed upload the gzip (~the dump's compressed size) would otherwise sit in
/// the container's `/tmp` forever. The command's real exit status is preserved for
/// [`crate::container::run_until_done`]'s marker: `(exit $ec)` sets `$?` without
/// ending the shell, so `rm` between them can't mask a dump/curl failure.
///
/// Safe to brace/subshell now that commands travel as WebSocket INPUT (v0.82.0), not
/// baked into the connection URL where a long one used to be truncated.
fn cleanup(work: &str, a: &str, b: &str) -> String {
    format!("{work}; ec=$?; rm -f '{a}' '{b}' 2>/dev/null; (exit $ec)")
}

/// The download+import command for a dump this tool wrote. Buffers the object to
/// a temp file first (a broken mid-stream download would half-import), then feeds
/// it to the client. The dump's own `CREATE DATABASE` means the target schema
/// need not pre-exist — this is exactly the cross-server restore EasyPanel can't do.
pub fn restore_command(
    stype: &str,
    root_password: &str,
    tmp_file: &str,
    presigned_get_url: &str,
) -> Option<String> {
    let client = match stype {
        "mysql" => "mysql",
        "mariadb" => "mariadb",
        _ => return None,
    };
    // Download to a file, gunzip to a file, then feed mysql via `< file` — NOT
    // `gunzip -c | mysql`. Same reason as the dump: a pipe through the container
    // PTY hits EAGAIN on a real-sized stream. `tmp_file` is the `.sql.gz`.
    let sql_file = tmp_file.strip_suffix(".gz").unwrap_or(tmp_file);
    let work = format!(
        "curl -sfS '{url}' -o '{gz}' && gunzip -f '{gz}' \
         && MYSQL_PWD='{pw}' {client} -uroot < '{sql}'",
        pw = sh_quote(root_password),
        gz = tmp_file,
        sql = sql_file,
        url = presigned_get_url,
    );
    Some(cleanup(&work, sql_file, tmp_file))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn only_the_engines_own_schemas_are_system() {
        assert!(is_system_db("mysql") && is_system_db("information_schema"));
        assert!(is_system_db("PERFORMANCE_SCHEMA"), "case-insensitive");
        assert!(!is_system_db("shop") && !is_system_db("hscom_main"));
    }

    #[test]
    fn a_db_name_must_be_a_safe_shell_token() {
        assert!(valid_db_name("hscom_main") && valid_db_name("a-b_1"));
        // Anything that could break out of the command is rejected.
        assert!(!valid_db_name(""));
        assert!(!valid_db_name("a; rm -rf /"));
        assert!(!valid_db_name("a b"));
        assert!(!valid_db_name("a'b"));
        assert!(!valid_db_name("db$(whoami)"));
        assert!(!valid_db_name(&"x".repeat(65)));
    }

    #[test]
    fn mysql_dump_is_non_locking_self_contained_and_buffers_before_upload() {
        let cmd = dump_command(
            "mysql",
            "p@ss'w",
            &["shop".into(), "blog".into()],
            "/tmp/ezp-dump.sql.gz",
            "https://r2.example/bucket/key?X-Amz-Signature=abc",
        )
        .unwrap();
        assert!(cmd.contains("mysqldump -uroot --databases shop blog"));
        assert!(cmd.contains("--single-transaction"), "must not lock the DB");
        assert!(
            cmd.contains("--databases"),
            "embeds CREATE DATABASE for a fresh host"
        );
        // mysqldump writes a plain file, THEN it's gzipped, THEN uploaded — piping
        // a real-sized dump through the container PTY hits EAGAIN and corrupts it.
        let dump_at = cmd.find("> '/tmp/ezp-dump.sql'").unwrap();
        let gzip_at = cmd.find("gzip -f '/tmp/ezp-dump.sql'").unwrap();
        let curl_at = cmd.find("curl -sfS -T '/tmp/ezp-dump.sql.gz'").unwrap();
        assert!(
            dump_at < gzip_at && gzip_at < curl_at,
            "dump → gzip → upload"
        );
        assert!(
            !cmd.contains("| gzip"),
            "no pipe into gzip (PTY corrupts it)"
        );
        // Temp files are removed WHATEVER happens (a failed upload must not leave the
        // gzip in the container's /tmp), and the real exit status is preserved so a
        // failure still surfaces — the cleanup sits AFTER the work, not `&&`-chained.
        assert!(cmd.contains("; ec=$?; rm -f '/tmp/ezp-dump.sql' '/tmp/ezp-dump.sql.gz'"));
        assert!(
            cmd.trim_end().ends_with("(exit $ec)"),
            "exit status preserved"
        );
        assert!(!cmd.contains("&& rm"), "cleanup must not depend on success");
        // The password is single-quote-escaped, not raw.
        assert!(cmd.contains("MYSQL_PWD='p@ss'\\''w'"));
    }

    #[test]
    fn mariadb_uses_its_own_dumper_and_blob_flags() {
        let cmd = dump_command("mariadb", "pw", &["app".into()], "/tmp/d", "URL").unwrap();
        assert!(cmd.contains("mariadb-dump"));
        assert!(cmd.contains("--hex-blob") && cmd.contains("utf8mb4"));
    }

    #[test]
    fn restore_downloads_then_imports_and_needs_no_existing_schema() {
        let cmd = restore_command("mysql", "pw", "/tmp/r.sql.gz", "https://r2/get").unwrap();
        let dl = cmd.find("-o '/tmp/r.sql.gz'").unwrap();
        let imp = cmd
            .find("MYSQL_PWD='pw' mysql -uroot < '/tmp/r.sql'")
            .unwrap();
        assert!(dl < imp, "download fully before importing");
        // Feed mysql from a file, not `gunzip -c | mysql` (PTY corrupts a pipe).
        assert!(!cmd.contains("| mysql") && !cmd.contains("| MYSQL_PWD"));
        // Temp files cleaned whatever happens, exit status preserved.
        assert!(cmd.contains("; ec=$?; rm -f '/tmp/r.sql' '/tmp/r.sql.gz'"));
        assert!(cmd.trim_end().ends_with("(exit $ec)"));
    }

    #[test]
    fn unsupported_engines_have_no_command_yet() {
        assert!(dump_command("postgres", "pw", &["a".into()], "/t", "u").is_none());
        assert!(dump_command("mongo", "pw", &["a".into()], "/t", "u").is_none());
        assert!(restore_command("redis", "pw", "/t", "u").is_none());
    }
}
