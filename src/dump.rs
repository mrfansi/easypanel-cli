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

/// A dump key read back apart: which service wrote it, and when.
///
/// The wide "every service on this host" listing exists for the same reason
/// `backup::history_all` does — a dump of `shop/db` is a perfectly good thing to
/// load into `shop-staging/db` — and for the same reason it must carry WHERE it
/// came from: once a row can name a service other than the destination, a row
/// that does not say so is ambiguous about what is being overwritten with what.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DumpKey {
    pub project: String,
    pub service: String,
    /// The stamp exactly as `dump_key` wrote it: `%Y%m%d-%H%M%S`.
    pub ts: String,
    /// The whole object key, which is what a restore is actually aimed at.
    pub key: String,
}

impl DumpKey {
    /// The stamp as a human reads a clock: `20260905-101010` → `2026-09-05 10:10:10`.
    /// Kept a pure reformat of the stamp rather than a parsed datetime: the stamp is
    /// UTC by construction (`dump_to_r2` formats `Utc::now()`) and re-parsing it
    /// could only fail on a key `parse_dump_key` has already accepted.
    pub fn when(&self) -> String {
        let (d, t) = self.ts.split_once('-').unwrap_or((&self.ts, ""));
        format!(
            "{}-{}-{} {}:{}:{}",
            &d[0..4],
            &d[4..6],
            &d[6..8],
            &t[0..2],
            &t[2..4],
            &t[4..6]
        )
    }

    /// `{project}/{service}` — how every row and every confirmation names an origin.
    pub fn origin(&self) -> String {
        format!("{}/{}", self.project, self.service)
    }
}

/// Read a dump key back into the three things it was built from, or `None` when
/// the object is not one of ours.
///
/// The inverse of [`dump_key`], living beside it so the shape has ONE definition
/// for the writer and for every reader. A host-wide listing sees whatever else
/// shares the bucket (EasyPanel's own backups, unrelated files), and a key that
/// does not match this shape is SKIPPED rather than rendered half-parsed: a row
/// with a blank service is exactly the ambiguity the origin column exists to
/// remove.
pub fn parse_dump_key(key: &str) -> Option<DumpKey> {
    let (project, rest) = key.split_once('/')?;
    // One level only: `{project}/{service}-{ts}.sql.gz` never nests deeper, so a
    // deeper key belongs to something else.
    if project.is_empty() || rest.contains('/') {
        return None;
    }
    let stem = rest.strip_suffix(".sql.gz")?;
    // The stamp itself holds a `-` (`%Y%m%d-%H%M%S`), so the two right-most
    // fields are the stamp and everything left of them is the service name —
    // which may itself contain dashes (`mysql-r2` does).
    let mut parts = stem.rsplitn(3, '-');
    let time = parts.next()?;
    let date = parts.next()?;
    let service = parts.next()?;
    let digits = |s: &str, n: usize| s.len() == n && s.bytes().all(|b| b.is_ascii_digit());
    if service.is_empty() || !digits(date, 8) || !digits(time, 6) {
        return None;
    }
    Some(DumpKey {
        project: project.to_string(),
        service: service.to_string(),
        ts: format!("{date}-{time}"),
        key: key.to_string(),
    })
}

/// One page of an S3 `ListObjectsV2` body: the `.sql.gz` keys on it, and the
/// token for the NEXT page when the answer was truncated.
///
/// Pure and here rather than in the caller so a truncated listing can be tested
/// without a bucket — reading only the first page is what made a host-wide
/// listing report "no dumps" while the dumps were sitting past key 1000. One tag
/// each, scraped rather than parsed, for the same reason the key scan is: an XML
/// dependency for three tags is not worth its weight.
pub fn parse_key_page(body: &str) -> (Vec<String>, Option<String>) {
    let tag = |name: &str| {
        body.split_once(&format!("<{name}>"))
            .and_then(|(_, rest)| rest.split_once(&format!("</{name}>")))
            .map(|(v, _)| v.to_string())
    };
    let keys: Vec<String> = body
        .split("<Key>")
        .skip(1)
        .filter_map(|s| s.split("</Key>").next())
        .filter(|k| k.ends_with(".sql.gz"))
        .map(String::from)
        .collect();
    // The token is only meaningful while truncated: S3 may echo one on the last
    // page, and following it would loop over the tail forever.
    let truncated = tag("IsTruncated").as_deref() == Some("true");
    let next = truncated.then(|| tag("NextContinuationToken")).flatten();
    (keys, next)
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
/// Non-locking (`--single-transaction --quick`) and self-contained: `--databases`
/// embeds `USE`, and `--add-drop-database` puts a `DROP DATABASE IF EXISTS`
/// before each `CREATE DATABASE`.
///
/// That DROP is a deliberate product decision, not a default: loading this dump
/// REPLACES each database it holds. Without it the load was a MERGE — every table
/// in the dump was replaced (mysqldump's own `DROP TABLE IF EXISTS`), but a table
/// that existed only on the target survived, so a "copy" left the target holding a
/// mixture of both sides that matched neither. A copy is asked for to make one
/// service look like another, and the confirmation has always said the target's
/// copies are OVERWRITTEN and cannot be recovered; this is what makes that true.
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
        "MYSQL_PWD='{pw}' {tool} -uroot --databases {dbs} --add-drop-database \
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
/// it to the client. The dump's own `DROP DATABASE`/`CREATE DATABASE` means the
/// target schema need not pre-exist — this is exactly the cross-server restore
/// EasyPanel can't do — and that where it DOES exist it is replaced, not merged
/// into (see [`dump_command`]).
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
        // REPLACE, not merge. Without `--add-drop-database` a load left any table
        // that existed only on the target in place, so the target ended up holding
        // a mixture of both sides that matched neither — while the confirmation
        // said its databases would be OVERWRITTEN.
        assert!(
            cmd.contains("--databases shop blog --add-drop-database"),
            "each database is dropped and recreated: {cmd}"
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
        // Same replace semantics as mysql: mariadb-dump takes the flag too.
        assert!(cmd.contains("--add-drop-database"), "{cmd}");
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

    /// Reading a key back is the inverse of writing one, and a wide listing sees
    /// objects that are NOT ours — EasyPanel's own backups share the bucket. A
    /// key that does not match the shape must be skipped, never half-parsed into
    /// a row with a blank origin, because the origin is the only thing telling
    /// the operator whose data a cross-service restore would load.
    #[test]
    fn a_dump_key_reads_back_as_its_source_and_a_foreign_key_is_refused() {
        // A service name with a dash of its own ("mysql-r2" exists on this host),
        // so the stamp's own dash cannot be mistaken for the name's.
        for (project, service) in [("shop", "mysql"), ("viding-co-db", "mysql-r2")] {
            let key = dump_key(project, service, "20260905-101010");
            let d = parse_dump_key(&key).expect("our own key parses");
            assert_eq!((d.project.as_str(), d.service.as_str()), (project, service));
            assert_eq!(d.ts, "20260905-101010");
            assert_eq!(dump_key(&d.project, &d.service, &d.ts), key, "round trip");
            assert_eq!(d.origin(), format!("{project}/{service}"));
            assert_eq!(d.when(), "2026-09-05 10:10:10");
        }
        for foreign in [
            "shop/mysql-20260905-101010.sql",           // not gzipped: not ours
            "shop/mysql.sql.gz",                        // no stamp
            "shop/mysql-2026095-101010.sql.gz",         // 7-digit date
            "shop/mysql-20260905-10101.sql.gz",         // 5-digit time
            "shop/nested/mysql-20260905-101010.sql.gz", // a deeper key is someone else's
            "backups/shop/db/2026-09-05.sql.gz",        // EasyPanel's own layout
            "mysql-20260905-101010.sql.gz",             // no project
        ] {
            assert!(
                parse_dump_key(foreign).is_none(),
                "a foreign key must be skipped: {foreign}"
            );
        }
    }

    /// A `ListObjectsV2` answer, shaped like the real one.
    fn list_page(keys: &[&str], truncated: bool, token: Option<&str>) -> String {
        let objects: String = keys
            .iter()
            .map(|k| format!("<Contents><Key>{k}</Key><Size>1</Size></Contents>"))
            .collect();
        let next = token
            .map(|t| format!("<NextContinuationToken>{t}</NextContinuationToken>"))
            .unwrap_or_default();
        format!(
            "<?xml version=\"1.0\" encoding=\"UTF-8\"?>\
             <ListBucketResult><Name>viding-db</Name>\
             <IsTruncated>{truncated}</IsTruncated>{next}{objects}</ListBucketResult>"
        )
    }

    /// A listing answers at most 1000 keys, so the page's token is the ONLY way
    /// to reach the rest. Reading one page reported "no dumps" against the real
    /// bucket, whose first thousand keys are another service's rotated logs.
    #[test]
    fn a_truncated_listing_hands_back_the_token_for_the_next_page() {
        let first = dump_key("viding-co-db", "mysql", "20260905-101010");
        let second = dump_key("viding-co-db", "mysql", "20260904-070707");
        let (keys, next) = parse_key_page(&list_page(
            &[&first, &second],
            true,
            Some("1ueGcxLPRx1Tr/XYExHnhbYLgveDs2J/wm36Hy4vbOwM="),
        ));
        assert_eq!(keys, vec![first, second], "both keys on the page");
        assert_eq!(
            next.as_deref(),
            Some("1ueGcxLPRx1Tr/XYExHnhbYLgveDs2J/wm36Hy4vbOwM="),
            "and the token that reaches the rest"
        );
    }

    /// The last page can still carry a token. Following it re-asks for the same
    /// tail forever, so `IsTruncated` — not the token's presence — is what says
    /// there is more.
    #[test]
    fn a_finished_listing_hands_back_no_token_even_when_it_echoes_one() {
        let key = dump_key("viding-co-db", "mysql", "20260905-101010");
        let (keys, next) = parse_key_page(&list_page(&[&key], false, Some("echoed-token")));
        assert_eq!(keys, vec![key]);
        assert_eq!(
            next, None,
            "a token without IsTruncated must not be followed"
        );
    }

    /// The bucket is shared. These are the real neighbours from the reported
    /// host: rotated app logs of an unrelated static site, and EasyPanel's own
    /// backup layout. Neither is a `.sql.gz` dump of ours, and neither may reach
    /// a picker row.
    #[test]
    fn a_shared_buckets_foreign_objects_never_reach_the_dump_list() {
        let ours = dump_key("viding-org-db", "mysql", "20260906-020202");
        let (keys, next) = parse_key_page(&list_page(
            &[
                "/viding-org_website-static/app.log.1",
                "/viding-org_website-static/app.log.2.gz",
                "backups/viding-co-db/mysql/2026-09-05T00-00-00.tar.gz",
                &ours,
            ],
            false,
            None,
        ));
        assert_eq!(keys, vec![ours.clone()], "only our dump survives the page");
        assert_eq!(next, None);
        for foreign in [
            "/viding-org_website-static/app.log.1",
            "backups/viding-co-db/mysql/2026-09-05T00-00-00.tar.gz",
        ] {
            assert!(parse_dump_key(foreign).is_none(), "{foreign}");
        }
        assert!(parse_dump_key(&ours).is_some());
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

    /// A listing answers at most 1000 keys. Reading only the first page reported
    /// "no dumps found" against a bucket whose first 1000 keys were somebody
    /// else's rotated logs — the dumps were real and sitting past the cut, so the
    /// page-following is what makes a wide listing true rather than merely short.
    #[test]
    fn a_truncated_listing_hands_back_its_keys_and_the_way_to_the_next_page() {
        let page = "<ListBucketResult>\
             <Contents><Key>shop/db-20260905-101010.sql.gz</Key></Contents>\
             <Contents><Key>shop/db-20260904-070707.sql.gz</Key></Contents>\
             <IsTruncated>true</IsTruncated>\
             <NextContinuationToken>1ueGcxL/tok</NextContinuationToken>\
             </ListBucketResult>";
        let (keys, next) = parse_key_page(page);
        assert_eq!(
            keys,
            vec![
                "shop/db-20260905-101010.sql.gz",
                "shop/db-20260904-070707.sql.gz"
            ]
        );
        assert_eq!(next.as_deref(), Some("1ueGcxL/tok"));
    }

    /// S3 may echo a token on the LAST page. Following it re-asks for the same
    /// tail forever, so the token counts only while `IsTruncated` says so.
    #[test]
    fn a_token_on_a_final_page_is_not_followed() {
        let page = "<ListBucketResult>\
             <Contents><Key>shop/db-20260905-101010.sql.gz</Key></Contents>\
             <IsTruncated>false</IsTruncated>\
             <NextContinuationToken>1ueGcxL/tok</NextContinuationToken>\
             </ListBucketResult>";
        let (keys, next) = parse_key_page(page);
        assert_eq!(keys.len(), 1);
        assert!(next.is_none(), "a finished listing must not be re-asked");
    }

    /// The bucket is shared. These are the real neighbours from the host this was
    /// found on: rotated app logs and EasyPanel's own backups. Only our own keys
    /// may reach a row, because the row's whole job is to name where a dump came
    /// from.
    #[test]
    fn objects_that_are_not_our_dumps_never_become_rows() {
        let page = "<ListBucketResult>\
             <Contents><Key>/viding-org_website-static/app.log.1</Key></Contents>\
             <Contents><Key>viding-co-db/2026-08-10T05:01:23.000Z.sql.gz</Key></Contents>\
             <Contents><Key>viding-co-db/mysql-20260905-204845.sql.gz</Key></Contents>\
             <IsTruncated>false</IsTruncated></ListBucketResult>";
        let (keys, _) = parse_key_page(page);
        // The log is dropped by the suffix; the panel's own backup survives that
        // and is dropped by the key SHAPE, which is why both filters exist.
        assert_eq!(keys.len(), 2, "{keys:?}");
        let ours: Vec<DumpKey> = keys.iter().filter_map(|k| parse_dump_key(k)).collect();
        assert_eq!(ours.len(), 1, "{ours:?}");
        assert_eq!(ours[0].origin(), "viding-co-db/mysql");
        assert_eq!(ours[0].when(), "2026-09-05 20:48:45");
    }
}
