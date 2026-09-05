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

/// The object-key prefix every dump of one service shares: `{project}/{service}-`.
///
/// ONE definition, because a dump is WRITTEN under this prefix and later FOUND by
/// it (`commands::list_r2_dumps` signs a ListObjectsV2 for exactly this string).
/// Had the two drifted, a dump would be filed where nothing looks for it.
pub fn dump_prefix(project: &str, service: &str) -> String {
    format!("{project}/{service}-")
}

/// Where a dump lands: under the prefix of the service it came FROM, timestamped.
///
/// A copy between two services keeps the SOURCE's prefix rather than filing under
/// the target, so `db list <source>` stays a truthful list of that service's
/// dumps. The consequence is deliberate and has to be told to the operator: after
/// a copy the dump does NOT appear under `db list <target>`, so the copy prints
/// the full key.
pub fn dump_key(project: &str, service: &str, ts: &str) -> String {
    format!("{}{ts}.sql.gz", dump_prefix(project, service))
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

/// Where a running dump has got to, read from the sizes of its two temp files.
///
/// The command writes `.sql`, then `gzip -f` turns it into `.sql.gz` and REMOVES
/// the `.sql`, then `curl` uploads. So the pair is an exact three-phase signal and
/// costs nothing to sample: `.sql` alone = still dumping, both = compressing,
/// `.sql.gz` alone = uploading. Neither yet means the dump has only just started.
pub fn dump_phase(sql: Option<u64>, gz: Option<u64>) -> String {
    let mb = |b: u64| crate::output::format_bytes(b as f64);
    match (sql, gz) {
        (Some(s), None) => format!("dumping — {} written", mb(s)),
        (Some(s), Some(g)) => format!("compressing — {} of {}", mb(g), mb(s)),
        (None, Some(g)) => format!("uploading — {}", mb(g)),
        (None, None) => "starting…".to_string(),
    }
}

/// Where a running restore has got to, read from the sizes of its two temp files.
///
/// The mirror of [`dump_phase`], and the same three-phase signal read backwards:
/// `curl` fills the `.sql.gz`, `gunzip -f` writes the `.sql` and REMOVES the
/// `.sql.gz`, then the client reads the `.sql`. So `.gz` alone = still
/// downloading, both = decompressing, `.sql` alone = importing.
///
/// The import phase reports the file's total size rather than how much of it has
/// been read: the client consumes the file without shrinking it, and nothing in
/// the container reports its read offset. Naming the total is honest; inventing a
/// percentage would not be.
pub fn restore_phase(sql: Option<u64>, gz: Option<u64>) -> String {
    let mb = |b: u64| crate::output::format_bytes(b as f64);
    match (sql, gz) {
        (None, Some(g)) => format!("downloading — {} fetched", mb(g)),
        (Some(s), Some(g)) => format!("decompressing — {} from {}", mb(s), mb(g)),
        (Some(s), None) => format!("importing — {} to read", mb(s)),
        (None, None) => "starting…".to_string(),
    }
}

/// Wrap an in-container command so its temp files are removed WHATEVER happens —
/// on a failed upload the gzip (~the dump's compressed size) would otherwise sit in
/// the container's `/tmp` forever. The command's real exit status is preserved so
/// [`crate::container::run_until_done`] can record it: `(exit $ec)` sets `$?` without
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

/// Why one database service cannot be copied into another, or `None` when it can.
///
/// One place, so the CLI and the TUI cannot disagree about what is offered — the
/// shape [`crate::dbms::unsupported_reason`] uses for the browse screen, and for
/// the same reason: a refusal has to name the thing that DOES work instead of
/// shrugging.
///
/// Two refusals live here and they are not the same mistake:
///
/// - an engine this path has no dump for (see [`engine_refusal`]), and
/// - two engines that each dump perfectly well, but not into each other.
///
/// The second is a REFUSAL and not a warning because of how the two halves are
/// built: the file is written by the SOURCE's tool ([`dump_command`] picks
/// `mysqldump` or `mariadb-dump`) and read by the TARGET's client
/// ([`restore_command`] picks `mysql` or `mariadb`). A mismatch therefore fails
/// somewhere INSIDE the load, statement by statement, which is the one failure
/// that leaves a target half-written. Refusing before anything is dumped is the
/// only point at which it costs nothing.
pub fn copy_refusal(
    src_stype: &str,
    src_name: &str,
    dst_stype: &str,
    dst_name: &str,
) -> Option<String> {
    if let Some(r) = engine_refusal(src_stype, src_name, "source") {
        return Some(r);
    }
    if let Some(r) = engine_refusal(dst_stype, dst_name, "target") {
        return Some(r);
    }
    if src_stype != dst_stype {
        return Some(format!(
            "{src_name} is '{src_stype}' and {dst_name} is '{dst_stype}'. A copy \
             across engines is refused: the dump is written by the source's own \
             tool and read by the target's own client, so a mismatch fails inside \
             the load and leaves the target partially written. Copy between two \
             '{src_stype}' services, or move the data by hand."
        ));
    }
    None
}

/// The engines this dump/copy path handles at all.
///
/// ONE definition, shared with [`engine_refusal`] below, so a menu cannot offer
/// what the core will refuse — the gate and the refusal are the same predicate
/// read two ways, the way `backup::can_back_up` states its own exclusion once.
pub fn can_dump(stype: &str) -> bool {
    matches!(stype, "mysql" | "mariadb")
}

/// Why ONE end of a copy is not a service this path can dump or load, or `None`.
///
/// `side` names which end it is ("source" / "target"), so the message says which
/// of the two services the operator has to change.
///
/// postgres and mongo are refused as NOT YET rather than never: they are real
/// databases whose dumps are simply not one `.sql.gz` (`pg_dump`'s own format,
/// and mongo is not a single SQL file at all — see [`dump_command`]). EasyPanel's
/// native backup does accept them (`crate::backup::can_back_up`), so the message
/// points there instead of leaving the operator with nothing.
///
/// redis is refused outright: it holds keys, not schemas, and EasyPanel's own
/// backup rejects it too (`crate::backup::can_back_up` excludes it).
fn engine_refusal(stype: &str, name: &str, side: &str) -> Option<String> {
    match stype {
        s if can_dump(s) => None,
        "postgres" | "mongo" => Some(format!(
            "The {side} {name} is '{stype}', which cannot be copied yet: this path \
             writes one self-contained '.sql.gz', and {stype} needs its own dump \
             format. EasyPanel's own backup does support {stype} — see: easypanel \
             backup providers, then easypanel backup db-restore."
        )),
        "redis" => Some(format!(
            "The {side} {name} is 'redis': it holds keys, not schemas, so there is \
             no database dump to copy, and EasyPanel's own backup refuses it too. \
             Move its data with redis-cli through the service shell instead."
        )),
        other => Some(format!(
            "The {side} {name} is '{other}', which is not a database service. \
             db copy moves databases between two mysql or mariadb services."
        )),
    }
}

/// Why a dump written to the source's storage could not be read back by the
/// target, or `None` when both panels describe the same objects.
///
/// The second leg of a copy presigns its GET against the TARGET panel's OWN
/// provider, so the two sides must name the same bucket at the same endpoint or
/// the load fetches nothing. Checked BEFORE the dump, because the alternative is
/// discovering it as a `curl` exit code an hour and 25 GB later.
///
/// Provider IDS are deliberately NOT compared. They are per-panel — the TUI's
/// cross-host restore already re-points a file for exactly that reason — so an id
/// difference is normal and proves nothing, while a bucket difference is decisive.
///
/// What this rests on: that endpoint + bucket identify the objects. It is not a
/// proof that the target's credentials can READ them; nothing short of a request
/// shows that, and a request now would not show it an hour later when the GET
/// actually runs.
pub fn store_refusal(
    src_endpoint: &str,
    src_bucket: &str,
    dst_endpoint: &str,
    dst_bucket: &str,
) -> Option<String> {
    // The endpoint is written by hand into each panel, so one side carrying a
    // scheme or a trailing slash and the other not is a formatting difference,
    // not a different bucket. `s3::presign` accepts either form.
    let host = |e: &str| {
        e.trim()
            .trim_start_matches("https://")
            .trim_start_matches("http://")
            .trim_end_matches('/')
            .to_ascii_lowercase()
    };
    if host(src_endpoint) != host(dst_endpoint) {
        return Some(format!(
            "The two panels' storage is at different endpoints — source \
             '{src_endpoint}', target '{dst_endpoint}'. A copy carries the dump \
             through storage BOTH sides can read, so point them at the same one \
             (or dump, then easypanel db download and restore by hand)."
        ));
    }
    if src_bucket != dst_bucket {
        return Some(format!(
            "The two panels' storage names different buckets — source \
             '{src_bucket}', target '{dst_bucket}'. The dump would be written \
             where the target cannot read it, so register the same bucket on both \
             panels (the provider IDs may differ; only the bucket must match)."
        ));
    }
    None
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

    /// The three phases a 25 GB dump walks through, told apart by which temp files
    /// exist — the only progress a detached in-container job gives away.
    #[test]
    fn the_two_temp_files_say_which_phase_a_dump_is_in() {
        assert!(dump_phase(Some(25_298_331_739), None).starts_with("dumping"));
        let mid = dump_phase(Some(25_298_331_739), Some(3_084_386_304));
        assert!(mid.starts_with("compressing"), "{mid}");
        assert!(mid.contains("2.9 GB") && mid.contains("23.6 GB"), "{mid}");
        assert!(dump_phase(None, Some(3_901_300_000)).starts_with("uploading"));
        assert_eq!(dump_phase(None, None), "starting…");
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

    /// A dump is filed under the service it came FROM, and found again by the
    /// same prefix. If the two ever disagreed a dump would be written where
    /// nothing looks for it — which is why they share one definition.
    #[test]
    fn a_dump_is_filed_under_its_source_service_and_found_by_that_prefix() {
        let key = dump_key("shop", "mysql", "20260905-101010");
        assert_eq!(key, "shop/mysql-20260905-101010.sql.gz");
        assert!(key.starts_with(&dump_prefix("shop", "mysql")));
        // A copy INTO shop-staging keeps the source's prefix, so the dump is not
        // listed under the target — the reason `db copy` prints the whole key.
        assert!(!key.starts_with(&dump_prefix("shop-staging", "mysql")));
    }

    /// The mirror of the dump's phase signal, read in the other direction. The
    /// `.sql`-alone case is load-bearing twice: it is the "importing" line the
    /// operator sees, and it is what tells `restore_from_r2` that a failure from
    /// here on can have written rows.
    #[test]
    fn the_two_temp_files_say_which_phase_a_restore_is_in() {
        assert!(restore_phase(None, Some(3_084_386_304)).starts_with("downloading"));
        let mid = restore_phase(Some(25_298_331_739), Some(3_084_386_304));
        assert!(mid.starts_with("decompressing"), "{mid}");
        assert!(mid.contains("23.6 GB") && mid.contains("2.9 GB"), "{mid}");
        assert!(restore_phase(Some(25_298_331_739), None).starts_with("importing"));
        assert_eq!(restore_phase(None, None), "starting…");
    }

    /// Copying between two engines is refused BEFORE anything is dumped: the file
    /// is written by the source's tool and read by the target's client, so a
    /// mismatch fails inside the load, which is the one failure that half-writes
    /// the target. The message must name both engines — "engine mismatch" alone
    /// does not tell the operator which end to change.
    #[test]
    fn copying_across_engines_is_refused_and_names_both_sides() {
        let r = copy_refusal("mysql", "shop/db", "mariadb", "staging/db")
            .expect("mysql → mariadb must be refused");
        assert!(r.contains("mysql") && r.contains("mariadb"), "{r}");
        assert!(r.contains("shop/db") && r.contains("staging/db"), "{r}");
        assert!(r.contains("partially written"), "{r}");
        // The reverse is refused too — neither direction is privileged.
        assert!(copy_refusal("mariadb", "a/db", "mysql", "b/db").is_some());
        // Matching engines pass.
        assert!(copy_refusal("mysql", "a/db", "mysql", "b/db").is_none());
        assert!(copy_refusal("mariadb", "a/db", "mariadb", "b/db").is_none());
    }

    /// An engine with no dump on this path is refused with a NAMED reason that
    /// points at what does work, never a shrug — the rule `dbms::unsupported_reason`
    /// already follows. Each engine gets its own sentence because the reasons
    /// genuinely differ: postgres/mongo have a dump we have not written yet,
    /// redis has no schemas to dump at all.
    #[test]
    fn an_engine_with_no_dump_is_refused_by_name() {
        for stype in ["postgres", "mongo"] {
            let r = copy_refusal(stype, "shop/db", "mysql", "b/db")
                .unwrap_or_else(|| panic!("{stype} must be refused as a source"));
            assert!(r.contains(stype), "{r}");
            // Points at the thing that DOES accept postgres/mongo.
            assert!(r.contains("backup"), "{r}");
            // And refused as a TARGET too, naming that end instead.
            let r = copy_refusal("mysql", "a/db", stype, "staging/db")
                .unwrap_or_else(|| panic!("{stype} must be refused as a target"));
            assert!(r.contains("staging/db") && r.contains("target"), "{r}");
        }
        let r = copy_refusal("redis", "shop/cache", "mysql", "b/db").expect("redis is refused");
        assert!(r.contains("keys, not schemas"), "{r}");
        assert!(r.contains("redis-cli"), "{r}");
        // A non-database service is a different mistake again, and says so.
        let r = copy_refusal("app", "shop/web", "mysql", "b/db").expect("app is refused");
        assert!(r.contains("not a database service"), "{r}");
    }

    /// The load leg presigns against the TARGET panel's own provider, so a dump
    /// written to a different bucket or endpoint can never be read back. Caught
    /// before the dump, because the alternative is a curl exit code an hour later.
    #[test]
    fn storage_the_target_cannot_read_is_refused_before_dumping() {
        // Same bucket at the same endpoint: nothing to say.
        assert!(
            store_refusal("https://acc.r2.com", "dumps", "https://acc.r2.com", "dumps").is_none()
        );
        // A scheme or trailing slash written differently in the two panels is a
        // formatting difference, not a different bucket.
        assert!(store_refusal("https://acc.r2.com/", "dumps", "acc.r2.com", "dumps").is_none());
        assert!(store_refusal("https://ACC.r2.com", "dumps", "acc.r2.com", "dumps").is_none());
        // Different bucket → refused, naming both so the operator can fix one.
        let r = store_refusal("https://acc.r2.com", "dumps", "https://acc.r2.com", "other")
            .expect("a different bucket must be refused");
        assert!(r.contains("dumps") && r.contains("other"), "{r}");
        // Provider IDS differ per panel and must NOT be what is compared — the
        // bucket alone decides, so identical buckets pass whatever else differs.
        let r = store_refusal("https://a.r2.com", "dumps", "https://b.r2.com", "dumps")
            .expect("a different endpoint must be refused");
        assert!(r.contains("a.r2.com") && r.contains("b.r2.com"), "{r}");
    }
}
