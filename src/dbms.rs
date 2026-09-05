//! Browsing and querying a database service through its own client — the
//! commands to send, and how to read what comes back.
//!
//! EasyPanel has no query endpoint (and no exec endpoint at all): the only way
//! into a database is the container's shell, so every question here is asked by
//! running the engine's OWN client in BATCH mode and parsing its output. This
//! module is the shape of those commands and of that output — no I/O, so each
//! piece is testable on a literal. `container::run_capture` runs them; the TUI's
//! `tui::dbms` holds the screen state. Same split `backup` / `tui::backup_ui`
//! already uses.
//!
//! The flags were chosen from each client's DOCUMENTED batch options, not
//! verified against a live server:
//!
//! - mysql/mariadb `--batch --raw`: tab-separated, no box drawing, `NULL` for a
//!   null. `--raw` also switches OFF the escaping of tabs and newlines inside a
//!   value, so a value CONTAINING a tab or a newline shifts its row — see
//!   [`parse_grid`], which keeps such a row rather than dropping it.
//! - psql `-A -F<tab> -q -P footer=off -P null=NULL`: unaligned, tab-separated,
//!   header kept, no `(N rows)` footer (it would read as one more row), and NULL
//!   spelled the way mysql spells it so it cannot be confused with an empty
//!   string.
//! - mongosh `--quiet --eval`: one JSON document per line, printed by the script
//!   itself, so the output framing is a line rather than a table.

use serde_json::Value;

use crate::output::field;

/// How many rows a preview reads. Visible in the UI — a silent cap is a lie
/// about how much data is there.
pub const PREVIEW_LIMIT: usize = 200;

/// The engines this screen can browse.
///
/// Redis is deliberately absent: it has no databases-with-tables to walk and no
/// query language whose result is a grid. Faking one (a key browser dressed as a
/// table) would be inventing a feature; the DB shell already gives `redis-cli`,
/// which is the honest answer. Stated once here, the way `backup::can_back_up`
/// states its own exclusion.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Engine {
    Mysql,
    Mariadb,
    Postgres,
    Mongo,
}

impl Engine {
    pub fn from_service_type(stype: &str) -> Option<Engine> {
        match stype {
            "mysql" => Some(Engine::Mysql),
            "mariadb" => Some(Engine::Mariadb),
            "postgres" => Some(Engine::Postgres),
            "mongo" => Some(Engine::Mongo),
            _ => None,
        }
    }

    /// What a "database" and a "table" are called here, for the screen's header
    /// and its key hints. Mongo has collections and documents, and calling them
    /// tables would be the tool guessing at a model the user does not have.
    pub fn table_word(self) -> &'static str {
        match self {
            Engine::Mongo => "Collection",
            _ => "Table",
        }
    }

    /// What the free-form box takes: SQL, or JavaScript for mongosh.
    pub fn query_word(self) -> &'static str {
        match self {
            Engine::Mongo => "JavaScript (mongosh)",
            _ => "SQL",
        }
    }
}

/// Why this service cannot be browsed, or `None` when it can.
///
/// One place, so the key, the menu item and the palette cannot disagree about
/// what is offered — and so redis gets a sentence that names the thing that DOES
/// work instead of a shrug.
pub fn unsupported_reason(stype: &str) -> Option<String> {
    if Engine::from_service_type(stype).is_some() {
        return None;
    }
    if stype == "redis" {
        return Some(
            "Redis has no tables to browse — press `y` for the redis-cli shell instead".into(),
        );
    }
    Some(format!(
        "Browsing is only for mysql/mariadb/postgres/mongo services (this is {stype})"
    ))
}

/// The credentials a client needs, read from `inspectService`.
///
/// The same fields the DB shell authenticates with (`tui::terminal::db_command`)
/// and credentials shows, in ONE place: mysql/mariadb browse as `root` (a
/// browser that cannot see the other schemas is pointless), postgres falls back
/// to `postgres` the way its image seeds it, mongo authenticates against
/// `admin`.
#[derive(Debug, PartialEq, Eq)]
pub struct Creds {
    pub user: String,
    pub password: String,
    /// The one database EasyPanel created, if it recorded one. Used as the
    /// starting point and as the connection's default database.
    pub database: String,
}

pub fn creds(engine: Engine, inspect: &Value) -> Creds {
    // `field` yields "-" for an absent key; that sentinel must not become a
    // password or a database name.
    let f = |k: &str| match field(inspect, &format!("/{k}")) {
        v if v == "-" => String::new(),
        v => v,
    };
    match engine {
        Engine::Mysql | Engine::Mariadb => Creds {
            user: "root".into(),
            password: f("rootPassword"),
            database: f("databaseName"),
        },
        Engine::Postgres => Creds {
            user: match f("user") {
                u if u.is_empty() => "postgres".into(),
                u => u,
            },
            password: f("password"),
            database: f("databaseName"),
        },
        Engine::Mongo => Creds {
            user: f("user"),
            password: f("password"),
            database: f("databaseName"),
        },
    }
}

/// Wrap a value for `sh` as one single-quoted word: `'` becomes `'\''`.
///
/// LOAD-BEARING. Everything reaching a container is a shell line, so a password
/// with a quote in it, or a query the user typed containing `'` (which is most
/// queries — every string literal has two), must not be able to end the quoting
/// and become shell syntax. The same idiom the DB shell and the detached
/// launcher use.
pub fn sh_quote(s: &str) -> String {
    format!("'{}'", s.replace('\'', "'\\''"))
}

/// A SQL identifier, quoted for the engine: MySQL backticks (doubled inside),
/// everyone else double quotes (doubled inside).
fn ident(engine: Engine, name: &str) -> String {
    match engine {
        Engine::Mysql | Engine::Mariadb => format!("`{}`", name.replace('`', "``")),
        _ => format!("\"{}\"", name.replace('"', "\"\"")),
    }
}

/// A JavaScript string literal for a mongosh script.
fn js_string(s: &str) -> String {
    format!("\"{}\"", s.replace('\\', "\\\\").replace('"', "\\\""))
}

/// A postgres table as listed (`schema.table`) split into its two quoted parts.
///
/// Split at the FIRST dot, which is what `schemaname||'.'||tablename` produced —
/// a table name containing a dot would be mis-split, and no quoting can recover
/// the boundary from a joined string, so that (rare) case is a known limit
/// rather than a silent guess.
fn pg_qualified(name: &str) -> String {
    match name.split_once('.') {
        Some((schema, table)) => format!(
            "{}.{}",
            ident(Engine::Postgres, schema),
            ident(Engine::Postgres, table)
        ),
        None => ident(Engine::Postgres, name),
    }
}

/// The mysql client invocation, without the statement.
fn mysql_client(c: &Creds) -> String {
    format!(
        "MYSQL_PWD={} mysql -u {}",
        sh_quote(&c.password),
        sh_quote(&c.user)
    )
}

/// The psql invocation for a machine-readable table, without the statement.
/// The separator is a LITERAL tab inside single quotes rather than `$'\t'`,
/// which is bash-only syntax and these images are not guaranteed to run bash.
fn psql_client(c: &Creds, database: &str) -> String {
    let db = if database.is_empty() {
        String::new()
    } else {
        format!(" -d {}", sh_quote(database))
    };
    format!(
        "PGPASSWORD={} psql -U {}{db} -A -F '\t' -q -P footer=off -P null=NULL",
        sh_quote(&c.password),
        sh_quote(&c.user)
    )
}

/// The mongosh invocation, without the script. An empty user means the service
/// has no credentials recorded, so no auth flags are sent at all.
fn mongo_client(c: &Creds) -> String {
    if c.user.is_empty() {
        "mongosh --quiet".to_string()
    } else {
        format!(
            "mongosh -u {} -p {} --authenticationDatabase admin --quiet",
            sh_quote(&c.user),
            sh_quote(&c.password)
        )
    }
}

/// The command that lists the databases this service holds.
///
/// Unlike the backup picker (`backup::list_databases_command`, which hides the
/// system schemas because backing one up is harmful), NOTHING is hidden here:
/// reading `information_schema` is a legitimate thing to want from a browser,
/// and a list that quietly omits rows is worse than a long one.
pub fn list_databases_cmd(engine: Engine, c: &Creds) -> String {
    match engine {
        Engine::Mysql | Engine::Mariadb => {
            format!("{} -N -B -e 'SHOW DATABASES'", mysql_client(c))
        }
        Engine::Postgres => format!(
            "PGPASSWORD={} psql -U {} -A -t -q -c {}",
            sh_quote(&c.password),
            sh_quote(&c.user),
            sh_quote("SELECT datname FROM pg_database WHERE NOT datistemplate ORDER BY datname")
        ),
        Engine::Mongo => format!(
            "{} --eval {}",
            mongo_client(c),
            sh_quote(
                "db.adminCommand({listDatabases:1}).databases.forEach(function(d){print(d.name)})"
            )
        ),
    }
}

/// The command that lists a database's tables (mongo: its collections).
pub fn list_tables_cmd(engine: Engine, c: &Creds, database: &str) -> String {
    match engine {
        Engine::Mysql | Engine::Mariadb => format!(
            "{} -N -B -e {}",
            mysql_client(c),
            sh_quote(&format!("SHOW TABLES FROM {}", ident(engine, database)))
        ),
        Engine::Postgres => format!(
            "PGPASSWORD={} psql -U {} -d {} -A -t -q -c {}",
            sh_quote(&c.password),
            sh_quote(&c.user),
            sh_quote(database),
            sh_quote(
                "SELECT schemaname || '.' || tablename FROM pg_tables \
                 WHERE schemaname NOT IN ('pg_catalog','information_schema') ORDER BY 1"
            )
        ),
        Engine::Mongo => format!(
            "{} --eval {}",
            mongo_client(c),
            sh_quote(&format!(
                "db.getSiblingDB({}).getCollectionNames().forEach(function(n){{print(n)}})",
                js_string(database)
            ))
        ),
    }
}

/// The command that reads the first `limit` rows of a table.
pub fn preview_cmd(engine: Engine, c: &Creds, database: &str, table: &str, limit: usize) -> String {
    match engine {
        Engine::Mysql | Engine::Mariadb => format!(
            "{} --batch --raw -e {}",
            mysql_client(c),
            sh_quote(&format!(
                "SELECT * FROM {}.{} LIMIT {limit}",
                ident(engine, database),
                ident(engine, table)
            ))
        ),
        Engine::Postgres => format!(
            "{} -c {}",
            psql_client(c, database),
            sh_quote(&format!(
                "SELECT * FROM {} LIMIT {limit}",
                pg_qualified(table)
            ))
        ),
        Engine::Mongo => format!(
            "{} --eval {}",
            mongo_client(c),
            sh_quote(&format!(
                "db.getSiblingDB({}).getCollection({}).find().limit({limit})\
                 .forEach(function(d){{print(JSON.stringify(d))}})",
                js_string(database),
                js_string(table)
            ))
        ),
    }
}

/// The command that runs what the user typed.
///
/// It is sent VERBATIM (only shell-quoted): this is the same privilege the DB
/// shell already grants, and rewriting someone's query — adding a LIMIT,
/// stripping a semicolon — would make the tool answer a question that was not
/// asked.
pub fn query_cmd(engine: Engine, c: &Creds, database: &str, query: &str) -> String {
    match engine {
        Engine::Mysql | Engine::Mariadb => {
            let db = if database.is_empty() {
                String::new()
            } else {
                format!(" -D {}", sh_quote(database))
            };
            format!(
                "{}{db} --batch --raw -e {}",
                mysql_client(c),
                sh_quote(query)
            )
        }
        Engine::Postgres => format!("{} -c {}", psql_client(c, database), sh_quote(query)),
        Engine::Mongo => {
            // The script runs with `db` already pointing at the chosen database,
            // so `db.users.find()` means what it looks like it means.
            let script = if database.is_empty() {
                query.to_string()
            } else {
                format!("db = db.getSiblingDB({});\n{query}", js_string(database))
            };
            format!("{} --eval {}", mongo_client(c), sh_quote(&script))
        }
    }
}

/// A result set: the column names and the rows, as strings ready to render.
#[derive(Debug, Default, PartialEq, Eq)]
pub struct Grid {
    pub columns: Vec<String>,
    pub rows: Vec<Vec<String>>,
}

impl Grid {
    /// One column of names — how a database/table listing renders in the same
    /// grid as everything else.
    pub fn from_names(header: &str, names: Vec<String>) -> Grid {
        Grid {
            columns: vec![header.to_string()],
            rows: names.into_iter().map(|n| vec![n]).collect(),
        }
    }
}

/// The names in a one-column listing.
///
/// Blank lines go, and so do lines that are recognisably the CLIENT talking
/// rather than the server answering — matched by the prefixes those clients
/// actually emit, the same way [`engine_error`] matches its own.
///
/// A bare colon is NOT a valid discriminator, though it was used as one here at
/// first: a colon is a legal identifier character in both MySQL (backtick-quoted)
/// and PostgreSQL (double-quoted), so a database or table called `tenant:42` or
/// `cache:v2` — a common naming style — simply vanished from the listing with
/// nothing to say it had ever been there. A browser that hides rows is worse than
/// one that shows a line it could not explain, so everything that is not a known
/// client prefix is kept VERBATIM, system schemas included.
pub fn parse_names(out: &str) -> Vec<String> {
    const NOISE: &[&str] = &[
        "mysql:",    // mysql: [Warning] Using a password on the command line …
        "mariadb:",  // the mariadb client's own name for the same warning
        "psql:",     // psql: warning / could not connect
        "mongosh:",  // mongosh startup complaints
        "[Warning]", // mysql's bracketed form, when it leads the line
        "[Note]",    //
        "Warning:",  // generic client warnings
        "WARNING:",  // postgres server-side warning echoed to the terminal
        "NOTICE:",   // postgres notices (e.g. from a DO block)
    ];
    out.lines()
        .map(str::trim)
        .filter(|l| !l.is_empty() && !NOISE.iter().any(|p| l.starts_with(p)))
        .map(String::from)
        .collect()
}

/// A captured result as columns and rows.
///
/// Tab-separated engines: the first line is the header, the rest are rows. A row
/// with FEWER fields than the header is padded and one with more has its surplus
/// joined back into the last cell — never dropped, because with mysql `--raw` a
/// value containing a tab or a newline produces exactly that, and losing the row
/// would hide data that is really there.
///
/// Mongo: one JSON document per line. The columns are the union of the
/// documents' top-level keys in first-seen order (the first document does not
/// get to decide the shape of a collection that has no fixed shape), and a
/// nested value is rendered as compact JSON. A line that is not a JSON object
/// falls back to a single `output` column — a mongosh script can print anything.
pub fn parse_grid(engine: Engine, out: &str) -> Grid {
    match engine {
        Engine::Mongo => parse_json_lines(out),
        _ => parse_tsv(out),
    }
}

fn parse_tsv(out: &str) -> Grid {
    let mut lines = out.lines().filter(|l| !l.trim().is_empty());
    let Some(header) = lines.next() else {
        return Grid::default();
    };
    let columns: Vec<String> = header.split('\t').map(String::from).collect();
    let rows = lines
        .map(|l| {
            let mut cells: Vec<String> = l.split('\t').map(String::from).collect();
            match cells.len().cmp(&columns.len()) {
                std::cmp::Ordering::Less => cells.resize(columns.len(), String::new()),
                std::cmp::Ordering::Greater => {
                    let surplus = cells.split_off(columns.len().saturating_sub(1));
                    cells.push(surplus.join("\t"));
                }
                std::cmp::Ordering::Equal => {}
            }
            cells
        })
        .collect();
    Grid { columns, rows }
}

/// One captured JSON value as a cell: a string as itself, null as `NULL` (the
/// spelling both SQL engines use here), anything nested as compact JSON.
fn json_cell(v: &Value) -> String {
    match v {
        Value::Null => "NULL".into(),
        Value::String(s) => s.clone(),
        Value::Bool(b) => b.to_string(),
        Value::Number(n) => n.to_string(),
        other => other.to_string(),
    }
}

fn parse_json_lines(out: &str) -> Grid {
    let lines: Vec<&str> = out
        .lines()
        .map(str::trim)
        .filter(|l| !l.is_empty())
        .collect();
    let docs: Vec<Value> = lines
        .iter()
        .filter_map(|l| serde_json::from_str::<Value>(l).ok())
        .collect();
    let all_objects =
        !docs.is_empty() && docs.len() == lines.len() && docs.iter().all(Value::is_object);
    if !all_objects {
        return Grid {
            columns: vec!["output".into()],
            rows: lines.into_iter().map(|l| vec![l.to_string()]).collect(),
        };
    }

    let mut columns: Vec<String> = Vec::new();
    for doc in &docs {
        for key in doc.as_object().into_iter().flat_map(|o| o.keys()) {
            if !columns.iter().any(|c| c == key) {
                columns.push(key.clone());
            }
        }
    }
    let rows = docs
        .iter()
        .map(|doc| {
            columns
                .iter()
                .map(|c| doc.get(c).map(json_cell).unwrap_or_default())
                .collect()
        })
        .collect();
    Grid { columns, rows }
}

/// The engine's own complaint, if the output carries one.
///
/// A client can print an error and still exit 0 (a warning on stderr shares the
/// PTY with the rows), so the exit code is not the only signal. The message is
/// returned VERBATIM: the engine says why a query failed far better than a
/// rewritten version of it would.
pub fn engine_error(out: &str) -> Option<String> {
    const PREFIXES: &[&str] = &[
        "ERROR ",            // mysql/mariadb: ERROR 1064 (42000) at line 1: …
        "ERROR:",            // postgres
        "FATAL:",            // postgres (auth, missing database)
        "psql:",             // psql itself (connection refused)
        "MongoServerError",  // mongosh
        "MongoNetworkError", // mongosh
        "MongoshInvalidInputError",
        "SyntaxError",    // mongosh, a broken script
        "ReferenceError", // mongosh
        "MongoParseError",
    ];
    out.lines()
        .map(str::trim)
        .find(|l| PREFIXES.iter().any(|p| l.starts_with(p)))
        .map(String::from)
}

/// What went wrong with a captured run, or `None` when it succeeded.
///
/// Three outcomes that must never look alike:
/// - `exit_code: None` — the command never reported back inside the time it was
///   given. That is "may still be running, output may be incomplete", NOT
///   "0 rows".
/// - a non-zero exit — the engine's own message when it printed one, otherwise
///   whatever it did print, otherwise the bare status.
/// - exit 0 with an error line — a client that complained and shrugged.
pub fn failure(out: &str, exit_code: Option<i32>) -> Option<String> {
    match exit_code {
        None => {
            let tail = engine_error(out).unwrap_or_default();
            let detail = if tail.is_empty() {
                String::new()
            } else {
                format!(" — {tail}")
            };
            Some(format!(
                "Timed out waiting for the engine{detail} (it may still be running; \
                 anything shown may be incomplete)"
            ))
        }
        Some(0) => engine_error(out),
        Some(code) => Some(engine_error(out).unwrap_or_else(|| {
            let trimmed = out.trim();
            if trimmed.is_empty() {
                format!("The client exited with status {code} and said nothing")
            } else {
                trimmed.lines().take(4).collect::<Vec<_>>().join(" ")
            }
        })),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn redis_is_excluded_and_says_where_to_go_instead() {
        assert_eq!(Engine::from_service_type("redis"), None);
        let why = unsupported_reason("redis").expect("redis cannot be browsed");
        assert!(
            why.contains("redis-cli") && why.contains('y'),
            "the exclusion must name the thing that DOES work: {why}"
        );
        // A non-database is refused too, but for its own reason.
        let app = unsupported_reason("app").expect("an app service has no database");
        assert!(app.contains("app"), "{app}");
        // The four supported engines are not refused.
        for t in ["mysql", "mariadb", "postgres", "mongo"] {
            assert!(unsupported_reason(t).is_none(), "{t} must be browsable");
        }
    }

    /// What `sh` actually receives as one word, so the escaping is judged by a
    /// shell rather than by our own idea of it.
    fn sh_word(quoted: &str) -> String {
        let out = std::process::Command::new("sh")
            .arg("-c")
            .arg(format!("printf %s {quoted}"))
            .output()
            .expect("sh must run");
        assert!(out.status.success(), "sh rejected: {quoted}");
        String::from_utf8(out.stdout).expect("utf8")
    }

    #[test]
    fn user_sql_with_quotes_survives_the_shell() {
        let c = Creds {
            user: "root".into(),
            password: "p'w".into(),
            database: "shop".into(),
        };
        // Every string literal in SQL carries two quotes, and an escaped one
        // carries four — the case that breaks naive quoting.
        let sql = "SELECT * FROM t WHERE name = 'O''Hara' AND x = 'y'; -- $(rm -rf /) `id`";
        assert_eq!(
            sh_word(&sh_quote(sql)),
            sql,
            "the engine must receive the query byte for byte"
        );
        assert_eq!(sh_word(&sh_quote("p'w")), "p'w", "and so must a password");

        let cmd = query_cmd(Engine::Mysql, &c, "shop", sql);
        assert!(cmd.contains(r"MYSQL_PWD='p'\''w'"), "{cmd}");
        assert!(cmd.contains("--batch --raw"), "machine-readable: {cmd}");

        // psql's own listing SQL contains quotes too, and goes through the same door.
        let tables = list_tables_cmd(Engine::Postgres, &c, "shop");
        assert!(tables.contains(r"'\''pg_catalog'\''"), "{tables}");
    }

    #[test]
    fn identifiers_are_quoted_for_their_engine() {
        let c = Creds {
            user: "root".into(),
            password: "pw".into(),
            database: "shop".into(),
        };
        let my = preview_cmd(Engine::Mysql, &c, "shop", "order`s", PREVIEW_LIMIT);
        assert!(my.contains("`shop`.`order``s`"), "{my}");
        assert!(
            my.contains("LIMIT 200"),
            "the cap is in the statement: {my}"
        );
        // Postgres lists tables as schema.table and must address both halves.
        let pg = preview_cmd(Engine::Postgres, &c, "shop", "public.orders", PREVIEW_LIMIT);
        assert!(pg.contains(r#""public"."orders""#), "{pg}");
        assert!(pg.contains("-P footer=off"), "no (N rows) footer: {pg}");
        assert!(pg.contains("-P null=NULL"), "NULL is spelled: {pg}");
    }

    #[test]
    fn credentials_come_from_the_fields_each_engine_actually_has() {
        let mysql = json!({ "rootPassword": "rp", "user": "app", "password": "pw", "databaseName": "shop" });
        assert_eq!(
            creds(Engine::Mysql, &mysql),
            Creds {
                user: "root".into(),
                password: "rp".into(),
                database: "shop".into()
            },
            "browsing needs root, not the app user"
        );
        // Postgres has no rootPassword at all: reading one yields the "-" sentinel,
        // which must never become a password.
        let pg = json!({ "user": "", "password": "pw", "databaseName": "shop" });
        assert_eq!(
            creds(Engine::Postgres, &pg),
            Creds {
                user: "postgres".into(),
                password: "pw".into(),
                database: "shop".into()
            }
        );
        let mongo = json!({ "user": "u", "password": "pw" });
        assert_eq!(creds(Engine::Mongo, &mongo).database, "");
    }

    #[test]
    fn tab_separated_output_keeps_empty_fields_and_null() {
        // mysql --batch --raw: header, then rows; an empty string is an empty
        // field, a null is the word NULL, and neither may collapse into the other.
        let out = "id\tname\tnote\n1\tAda\t\n2\t\tNULL\n";
        let grid = parse_grid(Engine::Mysql, out);
        assert_eq!(grid.columns, ["id", "name", "note"]);
        assert_eq!(
            grid.rows,
            vec![
                vec!["1".to_string(), "Ada".into(), String::new()],
                vec!["2".to_string(), String::new(), "NULL".into()],
            ]
        );

        // A value that itself contains a tab (possible with --raw) makes a row
        // too long. The surplus stays in the last column instead of vanishing.
        let ragged = "id\tname\n1\tAda\tLovelace\n";
        assert_eq!(
            parse_grid(Engine::Postgres, ragged).rows,
            vec![vec!["1".to_string(), "Ada\tLovelace".into()]]
        );
        // A short row is padded, so the grid stays rectangular.
        assert_eq!(
            parse_grid(Engine::Postgres, "a\tb\tc\n1\n").rows,
            vec![vec!["1".to_string(), String::new(), String::new()]]
        );
        // Nothing at all is an empty grid, not a header of one empty column.
        assert_eq!(parse_grid(Engine::Mysql, ""), Grid::default());
    }

    #[test]
    fn mongo_columns_are_the_union_of_the_documents_keys() {
        let out = concat!(
            "{\"_id\":\"a\",\"name\":\"Ada\"}\n",
            "{\"_id\":\"b\",\"tags\":[1,2],\"name\":null}\n"
        );
        let grid = parse_grid(Engine::Mongo, out);
        assert_eq!(
            grid.columns,
            ["_id", "name", "tags"],
            "first-seen order, and a key only the SECOND document has is still a column"
        );
        assert_eq!(
            grid.rows,
            vec![
                vec!["a".to_string(), "Ada".into(), String::new()],
                // Nested value as compact JSON; an explicit null is NULL, while a
                // field the document simply does not have stays empty.
                vec!["b".to_string(), "NULL".into(), "[1,2]".into()],
            ]
        );

        // A script that printed something that is not a document still shows what
        // it printed, rather than an empty grid.
        let prose = parse_grid(Engine::Mongo, "switched to db shop\nok\n");
        assert_eq!(prose.columns, ["output"]);
        assert_eq!(prose.rows.len(), 2);
    }

    #[test]
    fn names_drop_client_noise_but_keep_every_real_object() {
        // This warning is the exact line the filter exists for, and it must stay
        // filtered.
        let noisy = "mysql: [Warning] Using a password on the command line interface can be insecure.\ninformation_schema\nshop\n\n";
        assert_eq!(parse_names(noisy), ["information_schema", "shop"]);

        // A colon is a LEGAL identifier character (quoted) in both mysql and
        // postgres, so a name that contains one must survive — dropping it made
        // real databases and tables disappear with nothing said.
        let colons = "mysql: [Warning] Using a password on the command line interface can be insecure.\ntenant:42\ncache:v2\nsessions\npsql: warning: extra command-line argument ignored\nWARNING:  there is no transaction in progress\n";
        assert_eq!(
            parse_names(colons),
            ["tenant:42", "cache:v2", "sessions"],
            "only the client's own lines go"
        );
    }

    #[test]
    fn a_failure_names_the_engines_own_message_and_never_looks_like_no_rows() {
        // Non-zero exit: the engine's line wins over the raw dump.
        let mysql = "ERROR 1064 (42000) at line 1: You have an error in your SQL syntax";
        assert_eq!(failure(mysql, Some(1)).as_deref(), Some(mysql));
        // Exit 0 with a complaint printed anyway.
        assert!(failure("ERROR:  relation \"x\" does not exist", Some(0)).is_some());
        // A clean result is not a failure.
        assert_eq!(failure("id\tname\n1\tAda\n", Some(0)), None);
        // Nothing printed and a bad status still says something.
        assert!(failure("", Some(127)).unwrap().contains("127"));
        // A timeout is its own outcome, and says the result may be partial.
        let timeout = failure("", None).unwrap();
        assert!(
            timeout.contains("Timed out") && timeout.contains("incomplete"),
            "{timeout}"
        );
    }
}
