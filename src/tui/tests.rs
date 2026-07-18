//! TUI tests, deliberately kept in one file during this split.
//!
//! The module split must be WITHOUT behavior changes, and untouched tests are the
//! proof: 83 identical tests passing before and after. Spreading them across each
//! module is a separate job.

use serde_json::{json, Value};

use crate::output::field;

use super::app::*;
use super::form::*;
use super::render::*;
use super::table::*;
use super::worker::*;
use super::*;

fn form(fields: Vec<Field>) -> Form {
    Form::new(FormKind::ProjectCreate, "t", fields)
}

fn f_val(f: &Form, label: &str) -> String {
    f.by_label(label)
}

#[test]
fn source_without_config_does_not_default_to_first_repo() {
    // A new service: Enter must not unknowingly point the source at a random repo.
    // inspectService returns `source: null`, not an absent field.
    let f = form(source_fields(
        Some(&Value::Null),
        vec!["caesario/Kuze".into(), "acme/web".into()],
    ));
    assert_eq!(f_val(&f, "Repo"), "");
    assert_eq!(source_body(&f).unwrap_err(), "Repo must be selected");
}

#[test]
fn domain_edit_keeps_middlewares_and_extra_servers() {
    // Middlewares can't be edited from the TUI yet, so they MUST stay intact. Same
    // for a second custom server etc., which the form doesn't model.
    let original = json!({
        "id": "d1", "host": "a.test", "path": "/", "https": true,
        "wildcard": false, "certificateResolver": "google",
        "middlewares": ["mw1", "mw2"],
        "destinationType": "custom",
        "customDestination": { "servers": [
            { "url": "http://a:1", "weight": 1 },
            { "url": "http://b:2", "weight": 5 }
        ]}
    });
    let mut f = Form::new(
        FormKind::DomainEdit { id: "d1".into() },
        "t",
        domain_fields(Some(&original), &[]),
    );
    f.original = Some(original);
    let body = domain_body(&f).unwrap();
    assert_eq!(body["middlewares"], json!(["mw1", "mw2"]));
    assert_eq!(body["certificateResolver"], json!("google"));
    // The second server must not be silently trimmed.
    assert_eq!(
        body["customDestination"]["servers"][1],
        json!({ "url": "http://b:2", "weight": 5 })
    );
}

#[test]
fn domain_ssl_resolver_and_wildcard_are_editable() {
    let original = json!({
        "id": "d1", "host": "a.test", "path": "/", "https": true,
        "wildcard": false, "certificateResolver": "", "middlewares": [],
        "destinationType": "service",
        "serviceDestination": { "projectName": "p", "serviceName": "s",
                                "port": 80, "protocol": "http", "path": "/" }
    });
    let mut f = Form::new(
        FormKind::DomainEdit { id: "d1".into() },
        "t",
        domain_fields(Some(&original), &["p".into()]),
    );
    f.original = Some(original);
    f.fields
        .iter_mut()
        .find(|x| x.label == "SSL resolver")
        .unwrap()
        .value = "letsencrypt".into();
    f.fields
        .iter_mut()
        .find(|x| x.label == "Wildcard")
        .unwrap()
        .value = "yes".into();
    let body = domain_body(&f).unwrap();
    assert_eq!(body["certificateResolver"], json!("letsencrypt"));
    assert_eq!(body["wildcard"], json!(true));
}

fn svc(project: &str, name: &str, t: &str) -> Value {
    json!({ "projectName": project, "name": name, "type": t, "enabled": true })
}

#[test]
fn service_row_summarises_source_without_opening_anything() {
    let github = json!({
        "projectName": "p", "name": "api", "type": "app", "enabled": true,
        "source": { "type": "github", "owner": "acme", "repo": "web", "ref": "dev" }
    });
    assert_eq!(service_row(&github, None, None)[5], "acme/web#dev");

    let image = json!({
        "projectName": "p", "name": "cache", "type": "redis", "enabled": false,
        "source": { "type": "image", "image": "redis:7" }
    });
    let row = service_row(&image, None, None);
    assert_eq!(row[5], "redis:7");
    assert_eq!(row[3], "disabled");

    // A service with no source (just created) must not panic.
    assert_eq!(service_row(&svc("p", "empty", "app"), None, None)[5], "-");
}

#[test]
fn resource_body_parses_numbers_defaults_zero_and_rejects_junk() {
    // Prefill from existing resources.
    let res = json!({ "cpuLimit": 1, "cpuReservation": 0.25,
                      "memoryLimit": 512, "memoryReservation": 128 });
    let f = form(resource_fields(Some(&res)));
    assert_eq!(f.by_label("CPU limit (core)"), "1");
    assert_eq!(f.by_label("CPU reservation (core)"), "0.25");
    assert_eq!(f.by_label("Memory limit (MB)"), "512");

    // resources null (never set) -> everything "0".
    let f0 = form(resource_fields(None));
    assert_eq!(f0.by_label("Memory limit (MB)"), "0");
    let body = resource_body(&f0).unwrap();
    // Numbers, not strings (the API rejects strings). CPU decimal, memory as-is.
    assert_eq!(body["resources"]["cpuLimit"], json!(0.0));
    assert_eq!(body["resources"]["memoryReservation"], json!(0.0));

    // A mixed fill: empty -> 0, CPU decimal preserved.
    let mut f = form(resource_fields(None));
    for (label, val) in [
        ("CPU limit (core)", "0.5"),
        ("Memory limit (MB)", "1024"),
        ("Memory reservation (MB)", ""),
    ] {
        f.fields
            .iter_mut()
            .find(|x| x.label == label)
            .unwrap()
            .value = val.into();
    }
    let body = resource_body(&f).unwrap();
    assert_eq!(body["resources"]["cpuLimit"], json!(0.5));
    assert_eq!(body["resources"]["memoryLimit"], json!(1024.0));
    assert_eq!(body["resources"]["memoryReservation"], json!(0.0));

    // Non-numeric -> error with a message, not a silent 0.
    let mut bad = form(resource_fields(None));
    bad.fields
        .iter_mut()
        .find(|x| x.label == "CPU limit (core)")
        .unwrap()
        .value = "lots".into();
    assert!(resource_body(&bad).is_err());

    // Negatives rejected.
    let mut neg = form(resource_fields(None));
    neg.fields
        .iter_mut()
        .find(|x| x.label == "Memory limit (MB)")
        .unwrap()
        .value = "-1".into();
    assert!(resource_body(&neg).is_err());
}

#[test]
fn base64_matches_known_values() {
    use super::terminal::base64;
    assert_eq!(base64(b"sh"), "c2g="); // the container shell we use
    assert_eq!(base64(b""), "");
    assert_eq!(base64(b"M"), "TQ==");
    assert_eq!(base64(b"Ma"), "TWE=");
    assert_eq!(base64(b"Man"), "TWFu");
}

#[test]
fn db_command_per_type_uses_stored_credentials() {
    use super::terminal::db_command;
    // Each command's shape is verified live against the server (mysql/postgres/mongo/redis).
    let mysql = json!({ "rootPassword": "rp", "databaseName": "app" });
    assert_eq!(
        db_command("mysql", &mysql).unwrap(),
        "MYSQL_PWD='rp' mysql -uroot app"
    );
    // mariadb uses the same `mysql` client.
    assert_eq!(
        db_command("mariadb", &mysql).unwrap(),
        "MYSQL_PWD='rp' mysql -uroot app"
    );
    let pg = json!({ "user": "u", "password": "pw", "databaseName": "db" });
    assert_eq!(
        db_command("postgres", &pg).unwrap(),
        "PGPASSWORD='pw' psql -U u -d db"
    );
    let mo = json!({ "user": "mu", "password": "mp" });
    assert_eq!(
        db_command("mongo", &mo).unwrap(),
        "mongosh -u 'mu' -p 'mp' --authenticationDatabase admin"
    );
    assert_eq!(
        db_command("redis", &json!({ "password": "rp" })).unwrap(),
        "REDISCLI_AUTH='rp' redis-cli"
    );
    // Non-database -> None (the 'y' key refuses).
    assert!(db_command("app", &json!({})).is_none());
    // Single-quote escaping is safe: a password with an apostrophe doesn't break the sh command.
    assert!(db_command("redis", &json!({ "password": "a'b" }))
        .unwrap()
        .contains(r"'a'\''b'"));
    // postgres with no user -> fall back to the superuser "postgres".
    assert!(db_command("postgres", &json!({ "password": "x" }))
        .unwrap()
        .contains("-U postgres"));
}

#[test]
fn env_body_omits_dot_env_path_when_disabled() {
    // On: dotEnvPath included. The server rejects null/empty, so "off" = field omitted.
    let on = env_body("proj", "svc", "FOO=bar", Some(".env"));
    assert_eq!(on["env"], json!("FOO=bar"));
    assert_eq!(on["dotEnvPath"], json!(".env"));

    let off = env_body("proj", "svc", "FOO=bar", None);
    assert_eq!(off["env"], json!("FOO=bar"));
    assert!(off.get("dotEnvPath").is_none());
}

#[test]
fn redirect_body_builds_shape_and_requires_regex_replacement() {
    let set = |f: &mut Form, label: &str, val: &str| {
        f.fields
            .iter_mut()
            .find(|x| x.label == label)
            .unwrap()
            .value = val.into();
    };
    let mut f = form(redirect_fields());
    set(&mut f, "Regex", "^https://old.test/(.*)");
    set(&mut f, "Replacement", "https://new.test/${1}");
    // Permanent & Enabled default to yes (301, on).
    let body = redirect_body(&f).unwrap();
    assert_eq!(body["regex"], json!("^https://old.test/(.*)"));
    assert_eq!(body["replacement"], json!("https://new.test/${1}"));
    assert_eq!(body["permanent"], json!(true));
    assert_eq!(body["enabled"], json!(true));

    // Permanent = no -> 302.
    set(&mut f, "Permanent (301)", "no");
    assert_eq!(redirect_body(&f).unwrap()["permanent"], json!(false));

    // Empty regex/replacement -> error.
    let mut empty = form(redirect_fields());
    set(&mut empty, "Replacement", "x");
    assert!(redirect_body(&empty).is_err());
}

#[test]
fn basic_auth_body_sets_clears_and_rejects_half() {
    let set = |f: &mut Form, label: &str, val: &str| {
        f.fields
            .iter_mut()
            .find(|x| x.label == label)
            .unwrap()
            .value = val.into();
    };
    // Prefill from the first existing credential (shape verified live).
    let data = json!({ "basicAuth": [{ "username": "admin", "password": "s3cret" }] });
    let f = form(basic_auth_fields(Some(&data)));
    assert_eq!(f.by_label("Username"), "admin");
    assert_eq!(f.by_label("Password"), "s3cret");
    assert_eq!(
        basic_auth_body(&f).unwrap(),
        json!([{ "username": "admin", "password": "s3cret" }])
    );

    // Both empty -> empty array (turn off protection).
    let f = form(basic_auth_fields(None));
    assert_eq!(basic_auth_body(&f).unwrap(), json!([]));

    // Half (username without password) -> error, not a broken credential.
    let mut f = form(basic_auth_fields(None));
    set(&mut f, "Username", "admin");
    assert!(basic_auth_body(&f).is_err());
}

#[test]
fn mount_body_builds_per_type_and_validates() {
    let set = |f: &mut Form, label: &str, val: &str| {
        f.fields
            .iter_mut()
            .find(|x| x.label == label)
            .unwrap()
            .value = val.into();
    };
    // volume: {type, name, mountPath}
    let mut f = form(mount_fields());
    set(&mut f, "Type", "volume");
    set(&mut f, "Name", "data");
    set(&mut f, "Mount path", "/var/lib/data");
    assert_eq!(
        mount_body(&f).unwrap(),
        json!({ "type": "volume", "name": "data", "mountPath": "/var/lib/data" })
    );

    // bind: {type, hostPath, mountPath}
    let mut f = form(mount_fields());
    set(&mut f, "Type", "bind");
    set(&mut f, "Host path", "/etc/host/cfg");
    set(&mut f, "Mount path", "/cfg");
    assert_eq!(
        mount_body(&f).unwrap(),
        json!({ "type": "bind", "hostPath": "/etc/host/cfg", "mountPath": "/cfg" })
    );

    // Empty mount path -> error, whatever the type.
    let mut f = form(mount_fields());
    set(&mut f, "Type", "volume");
    set(&mut f, "Name", "data");
    assert!(mount_body(&f).is_err());

    // volume with no name -> error (not a silently-sent empty name).
    let mut f = form(mount_fields());
    set(&mut f, "Type", "volume");
    set(&mut f, "Mount path", "/data");
    assert!(mount_body(&f).is_err());
}

#[test]
fn port_body_parses_numbers_and_rejects_junk() {
    let f = form(port_fields());
    // Empty -> rejected with a message, not port 0.
    assert!(port_body(&f).is_err());

    let mut f = form(port_fields());
    for (label, val) in [("Published", "8080"), ("Target", "80"), ("Protocol", "udp")] {
        f.fields
            .iter_mut()
            .find(|x| x.label == label)
            .unwrap()
            .value = val.into();
    }
    let v = port_body(&f).unwrap();
    // published/target MUST be numbers (the API rejects strings), protocol as-is.
    assert_eq!(v["published"], json!(8080));
    assert_eq!(v["target"], json!(80));
    assert_eq!(v["protocol"], json!("udp"));

    // Non-numeric Published -> error, not a silent 0.
    let mut f = form(port_fields());
    f.fields
        .iter_mut()
        .find(|x| x.label == "Published")
        .unwrap()
        .value = "abc".into();
    f.fields
        .iter_mut()
        .find(|x| x.label == "Target")
        .unwrap()
        .value = "80".into();
    assert!(port_body(&f).is_err());
}

#[test]
fn status_reflects_running_state_not_just_enabled() {
    // `enabled` only means "not disabled", not "alive". A crashed service stays
    // enabled — the table used to lie and show "active". Now the metrics decide
    // up/down.
    let on = json!({ "projectName": "p", "name": "a", "type": "app", "enabled": true });
    let off = json!({ "projectName": "p", "name": "b", "type": "app", "enabled": false });

    // Disabled by the user -> "disabled", whatever the metrics.
    assert_eq!(service_status(&off, Some(true), None), "disabled");
    assert_eq!(service_status(&off, None, None), "disabled");
    // Enabled + metrics present -> up.
    assert_eq!(service_status(&on, Some(true), None), "active");
    // Enabled BUT no metrics (crash/stop) -> "stopped", not a false "active".
    assert_eq!(service_status(&on, Some(false), None), "stopped");
    // Metrics not loaded yet (None) -> don't accuse it of being dead; fall back to "active".
    assert_eq!(service_status(&on, None, None), "active");
}

#[test]
fn replica_stats_distinguish_down_from_stopped() {
    // Swarm replicas (actual/desired) separate what used to be lumped into
    // "stopped": a crash-looping service (desired>0, actual<desired) is BROKEN, not
    // deliberately stopped.
    let on = json!({ "projectName": "p", "name": "a", "type": "app", "enabled": true });

    // desired>0 but no replica up yet -> "down" (broken right now).
    assert_eq!(service_status(&on, Some(false), Some((0, 1))), "down");
    // Some replicas missing -> still "down".
    assert_eq!(service_status(&on, None, Some((1, 3))), "down");
    // Deliberately scaled to 0 -> "stopped", not "down".
    assert_eq!(service_status(&on, None, Some((0, 0))), "stopped");
    // Full replicas -> "active", even if metrics say none yet (replicas win).
    assert_eq!(service_status(&on, Some(false), Some((1, 1))), "active");
    // enabled=false is always "disabled", replicas don't change it.
    let off = json!({ "projectName": "p", "name": "b", "type": "app", "enabled": false });
    assert_eq!(service_status(&off, None, Some((0, 1))), "disabled");
}

#[test]
fn is_deploying_tracks_running_deployment_actions_only() {
    let mut app = App::new("s".into(), vec![]);
    app.all_services = vec![
        json!({ "projectName": "p", "name": "web", "type": "app" }),
        json!({ "projectName": "p", "name": "db", "type": "mysql" }),
    ];
    // running = in progress; done = finished (not); a live-verified status
    // pending → running → done/error.
    app.actions = vec![
        json!({ "type": "deployment", "status": "running", "projectName": "p", "serviceName": "web" }),
        json!({ "type": "deployment", "status": "done", "projectName": "p", "serviceName": "db" }),
    ];
    assert!(app.is_deploying("p", "web"));
    assert!(!app.is_deploying("p", "db"));
    assert!(!app.is_deploying("p", "absent"));
    assert_eq!(app.deploying_count(), 1);

    // pending is also "in progress"; a non-deployment type (backup) is ignored.
    app.actions = vec![
        json!({ "type": "deployment", "status": "pending", "projectName": "p", "serviceName": "web" }),
        json!({ "type": "backup", "status": "running", "projectName": "p", "serviceName": "db" }),
    ];
    assert!(app.is_deploying("p", "web"));
    assert!(!app.is_deploying("p", "db"));
    assert_eq!(app.deploying_count(), 1);
}

#[test]
fn context_menu_items_match_screen_and_selection() {
    let mut app = App::new("t".into(), vec![]);
    // Domains screen with no row selected -> no menu.
    app.screen = Screen::Domains;
    assert!(app.context_items().is_empty());
    // A domain row selected -> domain actions (edit/primary/delete).
    app.domains_state.select(Some(0));
    let items = app.context_items();
    assert_eq!(items.len(), 3);
    assert!(items.iter().any(|i| i.label == "Delete"));
    // A screen with no row actions (Dashboard) -> always empty.
    app.screen = Screen::Dashboard;
    assert!(app.context_items().is_empty());
}

#[test]
fn service_menu_groups_actions_and_respects_type() {
    let mut app = App::new("s".into(), vec![]);
    app.projects = vec!["p".into()];
    app.all_services = vec![
        json!({ "projectName": "p", "name": "web", "type": "app" }),
        json!({ "projectName": "p", "name": "db", "type": "mysql" }),
    ];
    app.screen = Screen::Projects;
    // visible_rows is sorted by name: [header "p", "db"(mysql), "web"(app)].
    let has = |v: &[super::app::MenuItem], s: &str| v.iter().any(|i| i.label == s);

    app.services_table.select(Some(2)); // web (app)
    let top = app.service_menu();
    let top: Vec<&str> = top.iter().map(|i| i.label.as_str()).collect();
    assert!(top.contains(&"Env ▸") && top.contains(&"Networking ▸") && top.contains(&"Danger ▸"));
    // app: the .env file can be toggled; shell WITHOUT a DB shell.
    assert!(has(&app.env_menu(), "Toggle .env file"));
    assert!(!app
        .shell_menu()
        .iter()
        .any(|i| i.label.contains("DB shell")));

    app.services_table.select(Some(1)); // db (mysql)
                                        // db: NO .env toggle; shell HAS a DB shell (auto login).
    assert!(!has(&app.env_menu(), "Toggle .env file"));
    assert!(app
        .shell_menu()
        .iter()
        .any(|i| i.label.contains("DB shell")));
    // db has Config file (Advanced) in the Build menu.
    assert!(has(&app.build_menu(), "Config file (Advanced)"));

    app.services_table.select(Some(2)); // web (app)
                                        // app has NO Config file (Advanced) — configFile is database-only.
    assert!(!has(&app.build_menu(), "Config file (Advanced)"));
}

#[test]
fn menu_arrows_open_drill_and_go_back() {
    let (tx, _rx) = std::sync::mpsc::channel();
    let mut app = App::new("s".into(), vec![]);
    app.projects = vec!["p".into()];
    app.all_services = vec![json!({ "projectName": "p", "name": "web", "type": "app" })];
    app.screen = Screen::Projects;
    app.services_table.select(Some(1)); // web

    // A right click opens the service menu (top-level), then → enters the Env
    // submenu; ← returns to the service menu (parent), not close.
    app.on_key(KeyCode::Char('e'), &tx); // keyboard opener: straight to the Env menu (no parent)
    assert!(app.menu.is_some());
    app.on_key(KeyCode::Left, &tx); // top level → closes
    assert!(app.menu.is_none());

    // Drilling two levels via a ▸ item keeps the parent for ←.
    let top = app.service_menu();
    app.open_menu(top);
    // select "Env ▸" (index 1) then → to enter the submenu.
    app.menu.as_mut().unwrap().state.select(Some(1));
    app.on_key(KeyCode::Right, &tx);
    // now in the Env submenu; the first item is "View env".
    assert_eq!(app.menu.as_ref().unwrap().items[0].label, "View env");
    // ← returns to the service menu (parent), doesn't close.
    app.on_key(KeyCode::Left, &tx);
    assert_eq!(app.menu.as_ref().unwrap().items[1].label, "Env ▸");
}

#[test]
fn space_opens_the_row_action_menu() {
    let (tx, _rx) = std::sync::mpsc::channel();
    let mut app = App::new("s".into(), vec![]);
    app.projects = vec!["p".into()];
    app.all_services = vec![json!({ "projectName": "p", "name": "web", "type": "app" })];
    app.screen = Screen::Projects;
    app.services_table.select(Some(1)); // web
    app.on_key(KeyCode::Char(' '), &tx);
    // Space = keyboard version of a right click: the top-level service menu (has Env ▸).
    assert!(app
        .menu
        .as_ref()
        .is_some_and(|m| m.items.iter().any(|i| i.label == "Env ▸")));
}

#[test]
fn palette_filters_then_jumps_to_service() {
    let (tx, _rx) = std::sync::mpsc::channel();
    let mut app = App::new("s".into(), vec![]);
    app.projects = vec!["proj".into()];
    app.all_services = vec![
        json!({ "projectName": "proj", "name": "web", "type": "app" }),
        json!({ "projectName": "proj", "name": "db", "type": "mysql" }),
    ];
    let labels = |a: &App| -> Vec<String> {
        a.palette
            .as_ref()
            .unwrap()
            .items
            .iter()
            .map(|i| i.label.clone())
            .collect()
    };

    // With no service selected (not on Services): the palette is PURE navigation —
    // no action entries (preventing hundreds of them).
    app.screen = Screen::Dashboard;
    app.open_palette();
    let nav = labels(&app);
    assert!(nav.iter().any(|x| x == "Open  proj/web  ·  app"));
    assert!(!nav.iter().any(|x| x.starts_with("Deploy")));
    app.palette = None;

    // With the web (app) service selected on Services: only THAT service's actions appear.
    app.screen = Screen::Projects;
    // visible_rows is sorted by name: [header "proj", "db"(mysql), "web"(app)].
    app.services_table.select(Some(2)); // web
    app.open_palette();
    let l = labels(&app);
    // The FULL action list (not just lifecycle): lifecycle + env + networking + etc.
    assert!(l.iter().any(|x| x == "Deploy  ·  proj/web"));
    assert!(l.iter().any(|x| x == "View env  ·  proj/web"));
    assert!(l.iter().any(|x| x == "Domain  ·  proj/web"));
    assert!(l.iter().any(|x| x == "Basic auth  ·  proj/web"));
    assert!(l.iter().any(|x| x == "Delete service  ·  proj/web"));
    assert!(!l.iter().any(|x| x.starts_with("DB shell"))); // app isn't a db
    assert!(!l.iter().any(|x| x == "Deploy  ·  proj/db")); // only the selected service
    app.palette = None;

    // db (mysql) selected → its actions include DB shell + Config file (Advanced).
    app.services_table.select(Some(1)); // db
    app.open_palette();
    let ldb = labels(&app);
    assert!(ldb
        .iter()
        .any(|x| x.starts_with("DB shell") && x.contains("proj/db")));
    assert!(ldb
        .iter()
        .any(|x| x == "Config file (Advanced)  ·  proj/db"));
    app.palette = None;

    // Context also works for NON-service rows: Domains with a domain selected → the
    // domain context-menu actions (Edit/Primary/Delete) appear in the palette.
    app.screen = Screen::Domains;
    app.domains = vec![json!({ "host": "x.test", "id": "d1" })];
    app.domains_state.select(Some(0));
    app.open_palette();
    assert!(labels(&app).iter().any(|x| x == "Domains: Delete"));
    app.palette = None;

    // Back to Services to test running a service action.
    app.screen = Screen::Projects;

    // Back to web, test running an action.
    app.services_table.select(Some(2)); // web
    app.open_palette();

    // Multi-word search (token-AND): "deploy web" narrows to entries that contain
    // both words (e.g. "Deploy …/web" and "Auto deploy …/web").
    app.palette.as_mut().unwrap().query = "deploy web".into();
    let pal = app.palette.as_ref().unwrap();
    let m = pal.matches();
    assert!(m.iter().all(|&i| {
        let l = pal.items[i].label.to_lowercase();
        l.contains("deploy") && l.contains("web")
    }));
    // Take the Deploy action entry (not "Auto deploy…") and run it.
    let pos = m
        .iter()
        .position(|&i| pal.items[i].label.starts_with("Deploy  "))
        .unwrap();
    app.palette.as_mut().unwrap().state.select(Some(pos));
    app.palette_run(&tx);
    assert!(app.palette.is_none());
    assert!(matches!(app.screen, Screen::Projects));
    assert!(app.services_table.selected().is_some());
    assert!(
        app.confirm.as_ref().is_some_and(|c| c.action == "deploy"),
        "the Deploy quick action must raise a deploy confirmation"
    );
}

#[test]
fn spinner_shows_only_while_loading() {
    // The spinner = "working" feedback, driven by REAL in-flight work rather than
    // by the message ending in "...". The text was only ever a guess: it kept
    // spinning after the reply arrived, and went quiet the moment an unrelated
    // message replaced it.
    use std::sync::atomic::Ordering;
    let mut app = App::new("t".into(), vec![]);

    app.status = "Ready".into();
    assert!(app.spinner().is_none(), "idle lane, no spinner");

    // The wording no longer decides anything — the lane does.
    app.status = "Loading...".into();
    assert!(app.spinner().is_none(), "nothing is actually running");

    app.busy.store(1, Ordering::Relaxed);
    assert!(app.spinner().is_some(), "a request is in flight");
    app.status = "Domain deleted".into();
    assert!(
        app.spinner().is_some(),
        "still working, whatever the message says"
    );

    app.busy.store(0, Ordering::Relaxed);
    assert!(app.spinner().is_none(), "the work finished");
}

#[test]
fn clone_body_drops_identity_and_source_but_keeps_config() {
    // Field shapes verified against a live server (mysql). Clone copies the config,
    // drops identity/token, and applies source/configFile SEPARATELY — so both must
    // be absent from the createService body.
    let inspect = json!({
        "name": "mysql", "serviceName": "mysql", "projectName": "db", "type": "mysql",
        "enabled": true, "token": "secret", "primaryDomainId": "d1",
        "image": "mysql:8.0", "env": "TZ=Asia/Jakarta", "password": "pw",
        "rootPassword": "rpw", "user": "u", "databaseName": "app",
        "source": { "type": "github" }, "configFile": "[mysqld]\nserver-id=2"
    });
    let body = crate::migrate::service_body(&inspect, "db", "mysql-replica");
    for k in [
        "name",
        "type",
        "enabled",
        "token",
        "primaryDomainId",
        "source",
        "configFile",
    ] {
        assert!(
            body.get(k).is_none(),
            "'{k}' should be dropped from the clone body"
        );
    }
    // The real config (including credentials) is copied along.
    assert_eq!(body["image"], json!("mysql:8.0"));
    assert_eq!(body["env"], json!("TZ=Asia/Jakarta"));
    assert_eq!(body["rootPassword"], json!("rpw"));
    assert_eq!(body["databaseName"], json!("app"));
    // Pointed at the target.
    assert_eq!(body["projectName"], json!("db"));
    assert_eq!(body["serviceName"], json!("mysql-replica"));
}

#[test]
fn task_stats_parse_matches_server_shape() {
    // Shape verified against a live server: an object { "{proj}_{svc}": {actual,desired} }.
    let v = json!({
        "edukasistudio-db_mysql": { "actual": 1, "desired": 1 },
        "harisenin-com_webapp":   { "actual": 0, "desired": 1 },
        "broken":                 { "actual": 2 }
    });
    let m = parse_task_stats(&v);
    assert_eq!(m.get("edukasistudio-db_mysql"), Some(&(1, 1)));
    assert_eq!(m.get("harisenin-com_webapp"), Some(&(0, 1)));
    // An entry without `desired` is ignored, not a panic or a fake value.
    assert_eq!(m.get("broken"), None);
}

#[test]
fn auto_deploy_column_separates_off_from_not_applicable() {
    // This shape is confirmed against the real API, not invented: a github source
    // always carries autoDeploy (15/16 apps), an image source never does.
    let on = json!({ "projectName": "p", "name": "a", "type": "app",
            "source": { "type": "github", "owner": "acme", "repo": "web",
                        "ref": "dev", "autoDeploy": true } });
    let off = json!({ "projectName": "p", "name": "b", "type": "app",
            "source": { "type": "github", "owner": "acme", "repo": "web",
                        "ref": "dev", "autoDeploy": false } });
    let image = json!({ "projectName": "p", "name": "c", "type": "app",
            "source": { "type": "image", "image": "nginx:1" } });
    let db = json!({ "projectName": "p", "name": "d", "type": "mysql" });

    assert_eq!(auto_deploy_cell(&on), "✓");
    assert_eq!(auto_deploy_cell(&off), "✗");
    // Not "✗": MySQL and an image source have no auto deploy to turn on, so "not
    // yet" would be a wrong claim.
    assert_eq!(auto_deploy_cell(&image), "-");
    assert_eq!(auto_deploy_cell(&db), "-");

    // service_row still keeps project and name separate; render folds them, so the
    // index shifts by one against the header.
    assert_eq!(service_row(&on, None, None)[6], "✓");
    assert_eq!(SERVICE_HEADERS[5], "Auto");
}

#[test]
fn auto_deploy_toggle_flips_the_value_and_refuses_where_it_cannot_apply() {
    let (tx, rx) = std::sync::mpsc::channel();
    let mut app = App::new("s".into(), vec![]);
    app.projects = vec!["p".into()];
    app.all_services = vec![
        json!({ "projectName": "p", "name": "a", "type": "app",
                    "source": { "type": "github", "owner": "acme", "repo": "web",
                                "ref": "dev", "autoDeploy": true } }),
        json!({ "projectName": "p", "name": "b", "type": "mysql" }),
    ];

    // Row 0 = project header, 1 = service "a", 2 = service "b".
    app.services_table.select(Some(1));
    app.toggle_auto_deploy(&tx);
    assert!(
        matches!(rx.try_recv(), Ok(Req::AutoDeploy { ref service, on, .. })
                     if service == "a" && !on),
        "true must be sent as on:false, not resending the old value"
    );

    // MySQL: no auto deploy to flip. Silently sending disableGithubDeploy there
    // would only produce a server error.
    app.services_table.select(Some(2));
    app.toggle_auto_deploy(&tx);
    assert!(rx.try_recv().is_err(), "there must be no request for MySQL");
    assert!(app.status.contains("GitHub"), "status: {}", app.status);

    // A project header isn't a service.
    app.services_table.select(Some(0));
    app.toggle_auto_deploy(&tx);
    assert!(rx.try_recv().is_err());
}

#[test]
fn metric_cols_render_bytes_and_rates() {
    let m = json!({ "cpu": 0.257, "memory": 573857792.0,
                        "networkIn": 12540.9, "networkOut": 32653.2 });
    assert_eq!(
        metric_cols(Some(&m)),
        vec!["0.3 %", "547.3 MB", "12.2 KB/s", "31.9 KB/s"]
    );
    // A service with no metrics must not panic or show a fake 0.
    assert_eq!(metric_cols(None), vec!["-", "-", "-", "-"]);
}

#[test]
fn metrics_join_by_project_and_service() {
    // getAllServicesStats carries more entries than the service list (system
    // services, compose sub-services) — and the same service name can exist in
    // different projects, so the key must be the pair, not the name alone.
    let mut app = App::new("s".into(), vec![]);
    app.all_services = vec![svc("proj-a", "mysql", "mysql")];
    app.monitor = vec![
        json!({ "projectName": "proj-b", "serviceName": "mysql",
                    "cpu": 9.0, "memory": 1.0, "networkIn": 0.0, "networkOut": 0.0 }),
        json!({ "projectName": "proj-a", "serviceName": "mysql",
                    "cpu": 1.0, "memory": 2048.0, "networkIn": 0.0, "networkOut": 0.0 }),
    ];
    let m = app.metric_for("proj-a", "mysql").unwrap();
    // Must pick proj-a, not proj-b with the same name.
    assert_eq!(metric_cols(Some(m))[0], "1.0 %");
    assert_eq!(metric_cols(Some(m))[1], "2.0 KB");

    // A service with no metrics: its columns are "-", not a fake 0.
    assert!(app.metric_for("proj-c", "ghost").is_none());
    assert_eq!(metric_cols(app.metric_for("proj-c", "ghost"))[0], "-");
}

#[test]
fn flat_list_filters_across_projects() {
    // The point of a flat list: searching "mysql" finds it in any project, with no
    // need to know which project it's in.
    let mut app = App::new("s".into(), vec![]);
    app.all_services = vec![
        svc("harisenin-net", "api", "app"),
        svc("harisenin-net-db", "mysql", "mysql"),
        svc("edukasistudio-db", "mysql-r1", "mysql"),
        svc("edukasistudio", "web", "app"),
    ];
    assert_eq!(app.visible_services().len(), 4);

    app.filter = "mysql".into();
    let vis = app.visible_services();
    assert_eq!(vis.len(), 2);
    assert_eq!(field(vis[0], "/projectName"), "harisenin-net-db");
    assert_eq!(field(vis[1], "/projectName"), "edukasistudio-db");

    // A project name is matched too, not just a service name.
    app.filter = "edukasistudio".into();
    assert_eq!(app.visible_services().len(), 2);
}

/// The "New service" form as the user sees it, with Kind/Source set.
fn create_form(pairs: &[(&str, &str)]) -> Form {
    let (tx, _rx) = std::sync::mpsc::channel();
    let mut app = App::new("s".into(), vec![]);
    app.projects = vec!["p".into()];
    app.new_service_form(&tx);
    let mut form = app.form.take().unwrap();
    for (label, val) in pairs {
        form.fields
            .iter_mut()
            .find(|f| &f.label == label)
            .unwrap_or_else(|| panic!("field '{label}' isn't in the form"))
            .value = (*val).into();
    }
    form
}

#[test]
fn a_new_app_carries_its_source_in_the_same_request() {
    // The point here: createService accepts an inline source, so an app service
    // doesn't need to be created first and then edited. This form used to be able to
    // send only project/name/type.
    let f = create_form(&[
        ("Name", "web"),
        ("Kind", "app"),
        ("Source", "github"),
        ("Repo", "acme/web"),
        ("Branch", "dev"),
    ]);
    // The source is applied via updateSourceGithub SEPARATELY (not inline in
    // createService, which triggers a deploy). create_source gives (op, body, auto).
    let (op, body, auto) = create_source(&f).unwrap().unwrap();
    assert_eq!(op, "updateSourceGithub");
    assert_eq!(
        body,
        json!({ "owner": "acme", "repo": "web", "ref": "dev", "path": "/" })
    );
    assert_eq!(auto, Some(false));

    let f = create_form(&[
        ("Name", "web"),
        ("Repo", "acme/web"),
        ("Branch", "dev"),
        ("Auto deploy", "yes"),
    ]);
    assert_eq!(create_source(&f).unwrap().unwrap().2, Some(true));
}

#[test]
fn a_new_service_without_a_source_sends_none_at_all() {
    // An empty repo = the user hasn't chosen. createService only requires
    // projectName + serviceName, so the source is OMITTED — not sent half-formed,
    // and not an error that blocks creation.
    let f = create_form(&[("Name", "web"), ("Kind", "app")]);
    assert_eq!(create_source(&f).unwrap(), None);

    // A database has no source at all.
    let f = create_form(&[("Name", "db"), ("Kind", "mysql")]);
    assert_eq!(create_source(&f).unwrap(), None);

    // But a half-formed source is still rejected: repo chosen, branch not.
    let f = create_form(&[("Name", "web"), ("Repo", "acme/web"), ("Branch", "")]);
    assert!(create_source(&f).is_err());
}

#[test]
fn source_fields_hide_unless_the_service_type_is_app() {
    // Two deciders at once: the service type AND the source type. This is what used
    // to be impossible — a form had only one switch.
    let shown =
        |f: &Form| -> Vec<&str> { f.visible().iter().map(|i| f.fields[*i].label).collect() };

    let f = create_form(&[("Kind", "app"), ("Source", "github")]);
    assert!(shown(&f).contains(&"Repo"));
    assert!(shown(&f).contains(&"Branch"));
    assert!(!shown(&f).contains(&"Docker image"));
    assert!(!shown(&f).contains(&"Database"), "app isn't a database");

    // Kind mysql: the whole source block vanishes, database fields appear.
    let f = create_form(&[("Kind", "mysql"), ("Source", "github")]);
    assert!(!shown(&f).contains(&"Repo"));
    assert!(!shown(&f).contains(&"Source"));
    assert!(shown(&f).contains(&"Database"));
    assert!(shown(&f).contains(&"Root password"));
}

#[test]
fn the_merged_create_form_has_no_duplicate_labels() {
    // This form merges the database fields and the source fields. Both used to have
    // a "Type", "Image", and "Password" — and by_label() uses find(), so the
    // database field would hijack the source's value. That's why the source labels
    // were renamed to Source/Docker image/Registry *.
    let f = create_form(&[]);
    let mut seen = std::collections::HashSet::new();
    for field in &f.fields {
        assert!(
            seen.insert(field.label),
            "label '{}' is duplicated",
            field.label
        );
    }
}

#[test]
fn service_extra_omits_empty_and_hidden_fields() {
    let (tx, _rx) = std::sync::mpsc::channel();
    let mut app = App::new("s".into(), vec![]);
    app.projects = vec!["p".into()];
    app.services_table.select(None);
    app.new_service_form(&tx);
    let form = app.form.as_mut().unwrap();

    // Kind app: no database fields at all.
    assert_eq!(service_extra(form), json!({}));

    // Kind redis: only password + image show. Filling Root password (hidden for
    // redis) must not be sent — the server rejects it.
    for (label, val) in [
        ("Kind", "redis"),
        ("Password", "s3cret"),
        ("Root password", "leaked"),
    ] {
        form.fields
            .iter_mut()
            .find(|f| f.label == label)
            .unwrap()
            .value = val.into();
    }
    assert_eq!(service_extra(form), json!({ "password": "s3cret" }));

    // Kind mysql: Root password now shows, so it's included. Database/User/Image
    // left empty -> OMITTED, not sent as "": the server must create them, and "" means
    // "use an empty string".
    form.fields
        .iter_mut()
        .find(|f| f.label == "Kind")
        .unwrap()
        .value = "mysql".into();
    assert_eq!(
        service_extra(form),
        json!({ "password": "s3cret", "rootPassword": "leaked" })
    );
}

#[test]
fn empty_project_shows_no_metrics_not_negative_zero() {
    // The old version of this test exercised `vec![].sum()` and `metric_cols(None)`
    // — Rust float semantics and a function the project header row doesn't call. It
    // passed while the real screen showed "-0.0 %". Now it calls the actual row
    // builder that render uses.
    let row = project_row("empty", 0, &[]);
    assert_eq!(row[0], "empty (0)");
    assert_eq!(&row[6..], ["-", "-", "-", "-"], "nothing measured");
    assert!(
        !row.iter().any(|c| c.contains("-0.0")),
        "the Sum f64 identity is -0.0; it must not leak to the screen: {row:?}"
    );

    // With metrics -> actually summed, not "-".
    let m = json!({ "cpu": 1.5, "memory": 2048.0, "networkIn": 0.0, "networkOut": 0.0 });
    let row = project_row("filled", 1, &[&m]);
    assert_eq!(row[6], "1.5 %");

    assert_eq!(metric_cols(None), vec!["-", "-", "-", "-"]);
}

#[test]
fn rows_are_project_headers_followed_by_their_services() {
    let mut app = App::new("s".into(), vec![]);
    app.projects = vec!["p1".into(), "p2".into()];
    app.all_services = vec![svc("p1", "b", "app"), svc("p1", "a", "app")];

    let rows = app.visible_rows();
    // p1 header + 2 services (sorted by name) + p2 header (empty, still shown).
    assert_eq!(rows.len(), 4);
    assert!(
        matches!(&rows[0], Line2::Project { name, services } if *name == "p1" && services.len() == 2)
    );
    assert!(matches!(&rows[1], Line2::Service(s) if field(s, "/name") == "a"));
    assert!(matches!(&rows[2], Line2::Service(s) if field(s, "/name") == "b"));
    // A project with no services MUST have a row: otherwise it vanishes entirely —
    // invisible, unselectable, undeletable.
    assert!(
        matches!(&rows[3], Line2::Project { name, services } if *name == "p2" && services.is_empty())
    );
}

#[test]
fn header_row_is_never_mistaken_for_a_service() {
    // A service action (logs/deploy/delete) on a project header would touch a
    // nonexistent service. selected_row() must be None on a header.
    let mut app = App::new("s".into(), vec![]);
    app.projects = vec!["p1".into()];
    app.all_services = vec![svc("p1", "api", "app")];

    app.services_table.select(Some(0)); // header
    assert_eq!(app.selected_row(), None);
    assert_eq!(app.selected_project(), Some("p1".into()));

    app.services_table.select(Some(1)); // service
    assert_eq!(
        app.selected_row(),
        Some(("p1".into(), "api".into(), "app".into()))
    );
    assert_eq!(app.selected_project(), Some("p1".into()));
}

#[test]
fn filtering_a_project_name_keeps_its_services() {
    let mut app = App::new("s".into(), vec![]);
    app.projects = vec!["harisenin-net".into(), "edukasistudio".into()];
    app.all_services = vec![
        svc("harisenin-net", "api", "app"),
        svc("edukasistudio", "web", "app"),
    ];

    // Project name matches -> its contents show too, not an empty header.
    app.filter = "harisenin".into();
    let rows = app.visible_rows();
    assert_eq!(rows.len(), 2);
    assert!(matches!(&rows[1], Line2::Service(s) if field(s, "/name") == "api"));

    // A service name matches -> its project header comes along, so the context is clear.
    app.filter = "web".into();
    let rows = app.visible_rows();
    assert_eq!(rows.len(), 2);
    assert!(matches!(&rows[0], Line2::Project { name, .. } if *name == "edukasistudio"));
}

#[test]
fn every_interactive_screen_documents_its_keys() {
    for sc in [
        Screen::Hosts,
        Screen::Maintenance,
        Screen::Actions,
        Screen::Monitor,
        Screen::Domains,
        Screen::Projects,
        Screen::Viewer,
    ] {
        assert!(
            !screen_keys(sc).is_empty(),
            "{:?} has no registered keybindings",
            TABS[sc.index()]
        );
    }
}

#[test]
fn help_lists_the_destructive_keys_that_exist() {
    // Destructive keys most need finding before they're pressed. Their actions now
    // live in group menus, so the opener must be documented AND its description must
    // name the destructive/lifecycle actions (delete/deploy/restart/stop/start).
    let projects = screen_keys(Screen::Projects);
    let keys: Vec<&str> = projects.iter().map(|k| k.0).collect();
    assert!(
        keys.contains(&"x"),
        "the Danger menu opener isn't documented"
    );
    assert!(
        keys.contains(&"d"),
        "the Lifecycle menu opener isn't documented"
    );
    let danger = projects.iter().find(|k| k.0 == "x").unwrap().1;
    assert!(
        danger.contains("delete"),
        "the Danger menu must mention delete"
    );
    let lifecycle = projects.iter().find(|k| k.0 == "d").unwrap().1;
    for word in ["deploy", "restart", "stop", "start"] {
        assert!(
            lifecycle.contains(word),
            "the Lifecycle menu must mention '{word}'"
        );
    }
    let maint: Vec<&str> = screen_keys(Screen::Maintenance)
        .iter()
        .map(|k| k.0)
        .collect();
    for k in ["p", "i", "c"] {
        assert!(maint.contains(&k), "'{k}' isn't documented on Maintenance");
    }
}

#[test]
fn help_key_and_quit_key_are_documented_globally() {
    let g: Vec<&str> = GLOBAL_KEYS.iter().map(|k| k.0).collect();
    assert!(g.contains(&"?"));
    assert!(g.contains(&"q / Ctrl-C"));
    // Esc cancels, and that must be written down: Esc used to close the TUI.
    assert!(g.contains(&"Esc"));
}

#[test]
fn keep_matches_any_column_case_insensitively() {
    let row = vec![
        "https://Rezabelle.com/".to_string(),
        "http://proxy:80/".into(),
    ];
    assert!(keep(&row, ""));
    assert!(keep(&row, "rezabelle"));
    assert!(keep(&row, "PROXY"));
    assert!(!keep(&row, "nothinghere"));
}

#[test]
fn enter_saves_the_form_from_a_choice_field() {
    // Regression: Enter used to open the dropdown on a Choice field, so a form whose
    // last field is a Choice — "New service", app type, exactly the reported case —
    // was impossible to save. Enter only opened and closed the dropdown.
    let (tx, rx) = std::sync::mpsc::channel();
    let mut app = App::new("s".into(), vec![]);
    app.projects = vec!["p".into()];
    app.new_service_form(&tx);
    // This form loads the repo list for its source dropdown; that's the first
    // request in the queue, not the one under test.
    assert!(matches!(rx.try_recv(), Ok(Req::Repos)));

    let form = app.form.as_mut().unwrap();
    form.fields
        .iter_mut()
        .find(|f| f.label == "Name")
        .unwrap()
        .value = "webapp".into();
    // A database Kind = a one-step wizard, so Enter saves right away (rather than
    // advancing to the source/build steps that don't exist for a database). The
    // guarded regression: Enter on a Choice field does NOT open the dropdown — it
    // used to, so the form could never be saved.
    form.fields
        .iter_mut()
        .find(|f| f.label == "Kind")
        .unwrap()
        .value = "redis".into();
    form.focus = form.fields.iter().position(|f| f.label == "Kind").unwrap();
    assert!(matches!(form.fields[form.focus].kind, FieldKind::Choice(_)));
    assert!(!app.form.as_ref().unwrap().is_wizard(), "redis = one step");

    app.form_key(KeyCode::Enter, &tx);

    assert!(app.chooser.is_none(), "Enter must not open the dropdown");
    assert!(matches!(
        rx.try_recv(),
        Ok(Req::ServiceCreate { ref service, ref stype, .. })
            if service == "webapp" && stype == "redis"
    ));
}

#[test]
fn env_and_domain_are_attached_only_when_filled() {
    // Both steps optional: empty = not sent (not env "" or a domain with no host).
    // The port is parsed into a number; path defaults to "/".
    let f = create_form(&[("Name", "web"), ("Kind", "app")]);
    assert_eq!(create_env(&f), None, "an empty env isn't sent");
    assert_eq!(create_domains(&f), None, "no host = no domain");

    let f = create_form(&[
        ("Name", "web"),
        ("Environment", "FOO=bar\nBAZ=qux"),
        ("Domain host", "web.test"),
        ("Domain port", "8080"),
    ]);
    assert_eq!(create_env(&f).as_deref(), Some("FOO=bar\nBAZ=qux"));
    let d = create_domains(&f).unwrap();
    assert_eq!(d[0]["host"], json!("web.test"));
    assert_eq!(
        d[0]["port"],
        json!(8080),
        "the port must be a number, not a string"
    );
    assert_eq!(d[0]["https"], json!(true));
    assert_eq!(d[0]["path"], json!("/"));

    // A non-numeric port is dropped, not sent as 0 (a wrong port is worse).
    let f = create_form(&[("Domain host", "web.test"), ("Domain port", "notaport")]);
    assert!(create_domains(&f).unwrap()[0].get("port").is_none());
}

#[test]
fn create_keeps_source_separate_and_never_deploys_inline() {
    // The owner's insight: createService with an inline source deploys right away
    // (~100 seconds, can error). The source must be SEPARATE so the service appears
    // in the table first, then deploys manually. So the Req carries its own
    // `source`, and `extra` (which goes into createService) does NOT contain "source".
    let (tx, rx) = std::sync::mpsc::channel();
    let mut app = App::new("s".into(), vec![]);
    app.projects = vec!["p".into()];
    app.new_service_form(&tx);
    assert!(matches!(rx.try_recv(), Ok(Req::Repos)));
    let form = app.form.as_mut().unwrap();
    for (label, val) in [
        ("Name", "web"),
        ("Repo", "acme/web"),
        ("Branch", "dev"),
        ("Environment", "A=1"),
        ("Create .env file", "yes"),
    ] {
        form.fields
            .iter_mut()
            .find(|f| f.label == label)
            .unwrap()
            .value = val.into();
    }
    app.submit_form(&tx);

    match rx.try_recv() {
        Ok(Req::ServiceCreate { extra, source, .. }) => {
            assert!(
                extra.get("source").is_none(),
                "the source must not be inline"
            );
            assert!(matches!(source, Some(("updateSourceGithub", _, _))));
            // env file: toggle on -> dotEnvPath, and env goes inline too (safe).
            assert_eq!(extra["env"], json!("A=1"));
            assert_eq!(extra["dotEnvPath"], json!(".env"));
        }
        _ => panic!("must send ServiceCreate"),
    }
}

#[test]
fn app_creation_is_a_five_step_wizard_ending_in_one_request() {
    // The EasyPanel dashboard flow: Basics → Source → Build → Environment → Domains.
    // Enter advances each step then SAVES on the last; Esc steps back. It all
    // becomes ONE createService, not create-then-edit over and over.
    let (tx, rx) = std::sync::mpsc::channel();
    let mut app = App::new("s".into(), vec![]);
    app.projects = vec!["p".into()];
    app.new_service_form(&tx);
    assert!(matches!(rx.try_recv(), Ok(Req::Repos)));

    let form = app.form.as_mut().unwrap();
    form.fields
        .iter_mut()
        .find(|f| f.label == "Name")
        .unwrap()
        .value = "web".into();
    // Fill one domain so it can be confirmed to reach the request.
    form.fields
        .iter_mut()
        .find(|f| f.label == "Domain host")
        .unwrap()
        .value = "web.test".into();
    // app (default) → five steps.
    assert_eq!(form.steps_present(), vec![0, 1, 2, 3, 4]);
    assert_eq!(form.step, 0);

    for expected in [1, 2, 3, 4] {
        app.form_key(KeyCode::Enter, &tx);
        assert_eq!(app.form.as_ref().unwrap().step, expected);
    }
    // Step back one then forward again: two-way navigation.
    app.form_key(KeyCode::Esc, &tx);
    assert_eq!(app.form.as_ref().unwrap().step, 3);
    app.form_key(KeyCode::Enter, &tx); // back to Domains (last)
    app.form_key(KeyCode::Enter, &tx); // Domains = last → save

    assert!(
        matches!(rx.try_recv(), Ok(Req::ServiceCreate { ref service, ref stype, ref extra, .. })
            if service == "web" && stype == "app"
                && extra.get("build").is_some()
                && extra["domains"][0]["host"] == json!("web.test")),
        "the last step must send createService with build + domain inline"
    );
}

#[test]
fn space_opens_the_dropdown_on_a_choice_field() {
    // A consequence of the test above: if Enter saves, some other key must open the
    // dropdown — otherwise Kind couldn't be changed.
    let (tx, _rx) = std::sync::mpsc::channel();
    let mut app = App::new("s".into(), vec![]);
    app.projects = vec!["p".into()];
    app.new_service_form(&tx);
    let form = app.form.as_mut().unwrap();
    form.focus = form.fields.iter().position(|f| f.label == "Kind").unwrap();

    app.form_key(KeyCode::Char(' '), &tx);
    assert!(app.chooser.is_some(), "Space must open the dropdown");
}

#[test]
fn filter_narrows_domains_and_actions_use_the_same_list() {
    // If render is filtered but actions use full-list indices, `x` would delete the
    // wrong domain. Both must go through visible_domains().
    let mut app = App::new("s".into(), vec![]);
    app.domains = vec![
        json!({ "id": "a", "host": "one.com", "https": true, "path": "/",
                    "destinationType": "service",
                    "serviceDestination": { "projectName": "p", "serviceName": "x",
                                            "port": 80, "protocol": "http", "path": "/" } }),
        json!({ "id": "b", "host": "two.com", "https": true, "path": "/",
                    "destinationType": "service",
                    "serviceDestination": { "projectName": "p", "serviceName": "y",
                                            "port": 80, "protocol": "http", "path": "/" } }),
    ];
    assert_eq!(app.visible_domains().len(), 2);

    app.filter = "two".into();
    let vis = app.visible_domains();
    assert_eq!(vis.len(), 1);
    // Index 0 of the filtered list must be "two.com" — not "one.com".
    assert_eq!(vis[0]["id"], json!("b"));
}

#[test]
fn clamp_keeps_selection_inside_filtered_list() {
    let mut app = App::new("s".into(), vec![]);
    app.screen = Screen::Domains;
    app.domains = vec![
        json!({ "id": "a", "host": "one.com", "https": true, "path": "/" }),
        json!({ "id": "b", "host": "two.com", "https": true, "path": "/" }),
    ];
    app.domains_state.select(Some(1));
    app.filter = "one".into();
    app.clamp_filtered();
    // Only 1 row left; row 1 is gone.
    assert_eq!(app.domains_state.selected(), Some(0));

    app.filter = "nothingmatches".into();
    app.clamp_filtered();
    assert_eq!(app.domains_state.selected(), None);
}

#[test]
fn branch_falls_back_to_text_when_its_list_cannot_load() {
    // The Branch field is a dropdown filled from GitHub. If its list fails to load
    // (e.g. the GitHub token in EasyPanel was revoked — users hit this for real),
    // its options hold only the current value, so the branch CAN'T be changed at
    // all. That single-option dropdown is a locked door, not graceful degradation.
    let f = source_fields(
        Some(&json!({ "type": "github", "owner": "acme", "repo": "web", "ref": "dev" })),
        vec!["acme/web".into()],
    );
    let branch = f.iter().find(|x| x.label == "Branch").unwrap();
    assert!(matches!(branch.kind, FieldKind::Choice(_)));
    match &branch.kind {
        FieldKind::Choice(o) => assert_eq!(o, &vec!["dev".to_string()]),
        _ => unreachable!(),
    }
}

#[test]
fn auto_deploy_error_names_the_cause_and_never_swallows_the_rest() {
    // This message is copied as-is from a real server while trying to turn on auto
    // deploy for a service sourced from a third-party repo.
    let real = "[400] Request failed with status code 404 Not Found: \
                    GET https://api.github.com/repos/benborla/mcp-server-mysql/hooks";
    let msg = auto_deploy_error("mysql-mcp", real);
    assert!(msg.contains("webhook"), "{msg}");
    assert!(
        !msg.contains("404"),
        "a stack of status codes doesn't help: {msg}"
    );

    // The unrecognized must not be dropped — dropping a server message is a bug
    // that has already happened here.
    let msg = auto_deploy_error("api", "connection reset");
    assert!(msg.contains("connection reset"), "{msg}");
}

#[test]
fn short_reason_names_the_cause_not_the_status_stack() {
    // EasyPanel wraps upstream errors, so a dead GitHub token shows up as "[400]
    // Request failed with status code 403 Forbidden" — two status codes and zero
    // hint about what to fix.
    assert_eq!(
        short_reason("[400] Request failed with status code 403 Forbidden"),
        "GitHub rejected: 403"
    );
    assert_eq!(
        short_reason("[400] Request failed with status code 401 Unauthorized"),
        "GitHub rejected: invalid token"
    );
    assert_eq!(short_reason("connection reset"), "failed");
}

#[test]
fn source_github_sends_owner_and_repo_split() {
    let f = form(source_fields(
        Some(&json!({
            "type": "github", "owner": "acme", "repo": "web", "ref": "dev", "path": "/",
            "autoDeploy": true
        })),
        vec!["acme/web".into()],
    ));
    let (op, body, auto) = source_body(&f).unwrap();
    assert_eq!(op, "updateSourceGithub");
    assert_eq!(
        body,
        json!({ "owner": "acme", "repo": "web", "ref": "dev", "path": "/" })
    );
    // updateSourceGithub resets autoDeploy to false on the server; its value must
    // come along so it can be reapplied afterwards.
    assert_eq!(auto, Some(true));
}

#[test]
fn dockerfile_source_sends_its_contents_inline() {
    // updateSourceDockerfile accepts the Dockerfile CONTENTS, not a path — so it's
    // multi-line, and that's why it goes through $EDITOR.
    let body = json!({ "type": "dockerfile", "dockerfile": "FROM alpine\nRUN echo hi" });
    let f = form(source_fields(Some(&body), vec![]));
    let (op, sent, auto) = source_body(&f).unwrap();
    assert_eq!(op, "updateSourceDockerfile");
    assert_eq!(sent, json!({ "dockerfile": "FROM alpine\nRUN echo hi" }));
    assert_eq!(auto, None, "a dockerfile has no auto deploy");

    // Empty is rejected by naming how to fill it, not with "required".
    let f = form(source_fields(
        Some(&json!({ "type": "dockerfile" })),
        vec![],
    ));
    let err = source_body(&f).unwrap_err();
    assert!(err.contains("$EDITOR"), "{err}");
}

#[test]
fn dockerfile_source_is_not_mislabelled_as_an_image() {
    // create_source used to map the type with a catch-all `_ => "image"`. A
    // dockerfile source would slip through as an image: the shape is valid, but the
    // service gets built from an image nobody ever named.
    let f = create_form(&[
        ("Name", "web"),
        ("Kind", "app"),
        ("Source", "dockerfile"),
        ("Dockerfile", "FROM alpine"),
    ]);
    // The Dockerfile is applied via updateSourceDockerfile, not mislabelled as an
    // image. create_source gives the right op + the Dockerfile contents.
    let (op, body, _) = create_source(&f).unwrap().unwrap();
    assert_eq!(op, "updateSourceDockerfile");
    assert_eq!(body["dockerfile"], json!("FROM alpine"));
    assert!(body.get("image").is_none());

    // An empty Dockerfile = not filled -> the source is omitted.
    let f = create_form(&[("Name", "web"), ("Source", "dockerfile")]);
    assert_eq!(create_source(&f).unwrap(), None);
}

#[test]
fn the_editor_field_shows_its_size_not_its_first_line() {
    // Hundreds of lines in a single-line field is just mush; what's useful is
    // whether it's been filled.
    let f = Field::editor("Dockerfile", "FROM alpine\nRUN echo hi\nCMD sh");
    assert_eq!(f.shown(), "3 lines");
    assert_eq!(Field::editor("Dockerfile", "").shown(), "(empty)");
    // Not a typed field: Space opens $EDITOR, doesn't insert a space.
    assert!(!f.kind.is_typed());
}

#[test]
fn source_git_and_image_have_no_auto_deploy() {
    // Only a github source has the concept of auto deploy.
    let f = form(source_fields(
        Some(&json!({ "type": "image", "image": "nginx" })),
        vec![],
    ));
    assert_eq!(source_body(&f).unwrap().2, None);
}

#[test]
fn source_rejects_path_without_leading_slash() {
    let mut f = form(source_fields(Some(&json!({ "type": "github" })), vec![]));
    f.fields
        .iter_mut()
        .find(|x| x.label == "Path")
        .unwrap()
        .value = "sub".into();
    assert!(source_body(&f).is_err());
}

#[test]
fn source_image_omits_empty_credentials() {
    let f = form(source_fields(
        Some(&json!({ "type": "image", "image": "nginx:latest" })),
        vec![],
    ));
    let (op, body, _) = source_body(&f).unwrap();
    assert_eq!(op, "updateSourceImage");
    // Sending "" would overwrite the registry credentials with blanks.
    assert_eq!(body, json!({ "image": "nginx:latest" }));
}

#[test]
fn version_field_maps_to_the_right_key_per_builder() {
    // A single "Version" field serves nixpacks and railpack. If it were two fields
    // with the same label, by_label() — which uses find(), not visibility — would
    // take the first, so railpack would write nixpacks's version.
    let original = json!({ "type": "railpack", "railpackVersion": "0.17.1" });
    let mut f = form(build_fields(Some(&original)));
    f.original = Some(original);
    assert_eq!(f.by_label("Version"), "0.17.1");

    let body = build_body(&f).unwrap();
    assert_eq!(body["build"]["railpackVersion"], json!("0.17.1"));
    assert!(
        body["build"].get("nixpacksVersion").is_none(),
        "railpack must not write nixpacks's key"
    );

    // nixpacks reads its own version key.
    let original = json!({ "type": "nixpacks", "nixpacksVersion": "1.41.0" });
    let mut f = form(build_fields(Some(&original)));
    f.original = Some(original);
    assert_eq!(f.by_label("Version"), "1.41.0");
    assert_eq!(
        build_body(&f).unwrap()["build"]["nixpacksVersion"],
        json!("1.41.0")
    );
}

#[test]
fn changing_the_builder_version_actually_reaches_the_body() {
    // The version used to only be preserved from the original build: the user was
    // locked to whatever version happened to be in use when the service was created.
    let original = json!({ "type": "nixpacks", "nixpacksVersion": "1.41.0" });
    let mut f = form(build_fields(Some(&original)));
    f.original = Some(original);
    f.fields
        .iter_mut()
        .find(|x| x.label == "Version")
        .unwrap()
        .value = "1.42.0".into();
    assert_eq!(
        build_body(&f).unwrap()["build"]["nixpacksVersion"],
        json!("1.42.0")
    );
}

#[test]
fn no_two_fields_share_a_label_in_any_form() {
    // by_label() uses find(): a duplicate label means a hidden field could hijack a
    // shown one's value, silently.
    let forms = vec![
        ("build", build_fields(Some(&json!({ "type": "nixpacks" })))),
        (
            "build/railpack",
            build_fields(Some(&json!({ "type": "railpack" }))),
        ),
        (
            "source",
            source_fields(Some(&json!({ "type": "github" })), vec![]),
        ),
        ("domain", domain_fields(None, &["p".into()])),
    ];
    for (name, fields) in forms {
        let mut seen = std::collections::HashSet::new();
        for f in &fields {
            assert!(
                seen.insert(f.label),
                "form {name}: label '{}' is duplicated",
                f.label
            );
        }
    }
}

#[test]
fn build_keeps_unmodelled_version_on_same_type() {
    let original = json!({
        "type": "nixpacks", "installCommand": "npm ci", "nixpacksVersion": "1.41.0"
    });
    let mut f = form(build_fields(Some(&original)));
    f.original = Some(original);
    let body = build_body(&f).unwrap();
    // nixpacksVersion isn't in the form; losing it = the build changes silently.
    assert_eq!(body["build"]["nixpacksVersion"], json!("1.41.0"));
    assert_eq!(body["build"]["installCommand"], json!("npm ci"));
}

#[test]
fn build_drops_old_fields_when_type_changes() {
    let original = json!({
        "type": "nixpacks", "installCommand": "npm ci", "nixpacksVersion": "1.41.0"
    });
    let mut f = form(build_fields(Some(&original)));
    f.original = Some(original);
    f.fields
        .iter_mut()
        .find(|x| x.label == "Build")
        .unwrap()
        .value = "dockerfile".into();
    let body = build_body(&f).unwrap();
    assert_eq!(body["build"]["type"], json!("dockerfile"));
    assert_eq!(body["build"]["file"], json!("Dockerfile"));
    assert!(body["build"].get("nixpacksVersion").is_none());
    assert!(body["build"].get("installCommand").is_none());
}

#[test]
fn build_removes_field_emptied_by_user() {
    let original = json!({ "type": "nixpacks", "installCommand": "npm ci" });
    let mut f = form(build_fields(Some(&original)));
    f.original = Some(original);
    f.fields
        .iter_mut()
        .find(|x| x.label == "Install command")
        .unwrap()
        .value
        .clear();
    let body = build_body(&f).unwrap();
    assert!(body["build"].get("installCommand").is_none());
}

#[test]
fn set_options_keeps_current_value_missing_from_list() {
    // `ref` can be a tag; searchBranches doesn't include it. Jumping to the first
    // branch would change what gets deployed.
    let mut f = Field::choice_owned("Branch", vec!["v1.2.0".into()], "v1.2.0");
    f.set_options(vec!["main".into(), "dev".into()]);
    assert_eq!(f.value, "v1.2.0");
    match &f.kind {
        FieldKind::Choice(o) => assert_eq!(o[0], "v1.2.0"),
        _ => panic!("must stay a Choice"),
    }
}

#[test]
fn source_fields_keep_repo_absent_from_list() {
    // The repo in use isn't in searchRepos (e.g. lost access) -> don't silently
    // switch to the first repo.
    let f = source_fields(
        Some(&json!({ "type": "github", "owner": "acme", "repo": "old", "ref": "dev" })),
        vec!["other/new".into()],
    );
    assert_eq!(
        f.iter().find(|x| x.label == "Repo").unwrap().value,
        "acme/old"
    );
}

#[test]
fn visible_follows_switch_and_multi_tag() {
    let f = form(source_fields(Some(&json!({ "type": "github" })), vec![]));
    let shown =
        |f: &Form| -> Vec<&str> { f.visible().iter().map(|i| f.fields[*i].label).collect() };
    assert!(shown(&f).contains(&"Branch"));
    assert!(shown(&f).contains(&"Path")); // when("Source", "github,git")
    assert!(!shown(&f).contains(&"Docker image"));

    let mut f = f;
    f.fields
        .iter_mut()
        .find(|x| x.label == "Source")
        .unwrap()
        .value = "image".into();
    assert!(shown(&f).contains(&"Docker image"));
    assert!(!shown(&f).contains(&"Path"));
    assert!(!shown(&f).contains(&"Branch"));
}

#[test]
fn encode_key_matches_xterm() {
    use ratatui::crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
    let k = |code, m| super::terminal::encode_key(KeyEvent::new(code, m));
    // An ordinary char, Enter, Backspace, an arrow, and Ctrl-C (0x03).
    assert_eq!(k(KeyCode::Char('a'), KeyModifiers::NONE), Some(vec![b'a']));
    assert_eq!(k(KeyCode::Enter, KeyModifiers::NONE), Some(vec![b'\r']));
    assert_eq!(k(KeyCode::Backspace, KeyModifiers::NONE), Some(vec![0x7f]));
    assert_eq!(
        k(KeyCode::Up, KeyModifiers::NONE),
        Some(vec![0x1b, b'[', b'A'])
    );
    assert_eq!(
        k(KeyCode::Char('c'), KeyModifiers::CONTROL),
        Some(vec![0x03])
    );
}

/// A WebSocket terminal round-trip THROUGH THE RUST CODE (not python): ws_url +
/// spawn_session + vt100. Needs a live server and the containers zzz-emb/zzz-redis
/// running, hence #[ignore] — run it manually with `--ignored`.
#[test]
#[ignore = "live: needs zzz-emb/zzz-redis running on the server"]
fn terminal_ws_roundtrip_live() {
    use super::worker::Resp;
    use std::sync::mpsc::channel;
    use std::time::{Duration, Instant};

    let cfg = crate::config::ServerConfig::new(crate::config::ServerConfig::default_path());
    let srv = cfg.default().expect("a default server exists");
    let client = crate::client::EasypanelClient::new(&srv.url, &srv.token);
    let url = super::terminal::ws_url(&client, "zzz-emb", "zzz-redis", "sh").expect("ws_url");

    let (out_tx, out_rx) = channel::<Resp>();
    let (in_tx, in_rx) = channel::<super::terminal::TermMsg>();
    super::terminal::spawn_session(url, out_tx, in_rx, 80, 24);
    std::thread::sleep(Duration::from_millis(900));
    in_tx
        .send(super::terminal::TermMsg::Input(
            "echo RUSTPROOF_331\n".into(),
        ))
        .unwrap();

    let mut parser = vt100::Parser::new(24, 80, 0);
    let deadline = Instant::now() + Duration::from_secs(8);
    loop {
        if let Ok(Resp::TermOutput(b)) = out_rx.recv_timeout(Duration::from_millis(400)) {
            parser.process(&b);
        }
        if parser.screen().contents().contains("RUSTPROOF_331") {
            break; // the shell executed and its output came back through the Rust code
        }
        assert!(Instant::now() < deadline, "no output containing the proof");
    }
    drop(in_tx); // close the session
}

#[test]
fn first_load_lands_on_a_service_not_a_project_header() {
    // Row 0 of visible_rows is a PROJECT HEADER. Landing there makes every service
    // action a silent no-op (selected_row() is None for a header), so the grouped
    // menus look broken on first contact.
    let (tx, _rx) = std::sync::mpsc::channel();
    let mut app = App::new("s".into(), vec![]);
    app.screen = Screen::Projects;
    app.handle(
        Resp::AllServices {
            projects: vec!["proj".into()],
            services: vec![json!({ "projectName": "proj", "name": "web", "type": "app" })],
        },
        &tx,
    );
    assert!(
        app.selected_row().is_some(),
        "after the first load a SERVICE must be selected, not the project header"
    );
}

#[test]
fn row_actions_say_something_when_no_service_is_selected() {
    // A header row is selected: opening a group menu would build a menu whose every
    // item silently fails. Say so instead.
    let (tx, _rx) = std::sync::mpsc::channel();
    let mut app = App::new("s".into(), vec![]);
    app.screen = Screen::Projects;
    app.projects = vec!["proj".into()];
    app.all_services = vec![json!({ "projectName": "proj", "name": "web", "type": "app" })];
    app.services_table.select(Some(0)); // the project header
    app.status = "Ready".into();
    app.on_key(KeyCode::Char('d'), &tx); // Lifecycle group key
    assert!(app.menu.is_none(), "no menu should open without a service");
    assert_ne!(
        app.status, "Ready",
        "the rejection must be visible in the status"
    );
}

#[test]
fn services_screen_highlights_a_service_row_on_first_paint() {
    // Renders the real screen and inspects what is drawn: the highlight symbol must
    // sit on a service, not on the project header. A unit test on the selection
    // index can't see that the marker landed on the wrong line.
    use ratatui::backend::TestBackend;
    use ratatui::Terminal;

    let (tx, _rx) = std::sync::mpsc::channel();
    let mut app = App::new("s".into(), vec![]);
    app.screen = Screen::Projects;
    app.handle(
        Resp::AllServices {
            projects: vec!["proj".into()],
            services: vec![
                json!({ "projectName": "proj", "name": "web", "type": "app" }),
                json!({ "projectName": "proj", "name": "db", "type": "mysql" }),
            ],
        },
        &tx,
    );

    let mut term = Terminal::new(TestBackend::new(120, 20)).unwrap();
    term.draw(|f| super::render::ui(f, &mut app)).unwrap();

    let screen: Vec<String> = term
        .backend()
        .buffer()
        .content()
        .chunks(120)
        .map(|row| row.iter().map(|c| c.symbol()).collect())
        .collect();
    let marked = screen
        .iter()
        .find(|l| l.contains('›'))
        .expect("a row must be highlighted");
    assert!(
        marked.contains("db") || marked.contains("web"),
        "the highlight must be on a service, got: {marked:?}"
    );
    assert!(
        !marked.contains("proj ("),
        "the highlight must not sit on the project header, got: {marked:?}"
    );
}

#[test]
fn repl_column_shows_the_live_count_and_flags_a_shortfall() {
    let app = json!({ "projectName": "p", "name": "web", "type": "app",
                      "deploy": { "replicas": 3 } });
    // Swarm agrees with the target: just the number.
    assert_eq!(replicas_cell(&app, Some((3, 3))), "3");
    // Short of target: show both — this is the moment the number matters.
    assert_eq!(replicas_cell(&app, Some((1, 3))), "1/3");
    assert_eq!(replicas_cell(&app, Some((0, 1))), "0/1");
    // Swarm not loaded yet: fall back to the configured count, never a guess.
    assert_eq!(replicas_cell(&app, None), "3");
    // A database has no deploy block at all.
    let db = json!({ "projectName": "p", "name": "mysql", "type": "mysql" });
    assert_eq!(replicas_cell(&db, None), "-");

    // The column sits between Status and Source, and the header matches.
    assert_eq!(SERVICE_HEADERS[3], "Repl");
    assert_eq!(service_row(&app, None, Some((1, 3)))[4], "1/3");
}

#[test]
fn narrow_terminals_drop_the_metric_columns_not_the_identity_ones() {
    // At 80 columns the ten-column table squeezed "Status" into "Statu" and the
    // metrics into slivers like "0." and "77". Identity and state must survive; the
    // numbers already have a home on the Monitor tab.
    use ratatui::backend::TestBackend;
    use ratatui::Terminal;

    let (tx, _rx) = std::sync::mpsc::channel();
    let mut app = App::new("s".into(), vec![]);
    app.screen = Screen::Projects;
    app.handle(
        Resp::AllServices {
            projects: vec!["proj".into()],
            services: vec![json!({ "projectName": "proj", "name": "web", "type": "app" })],
        },
        &tx,
    );

    let paint = |app: &mut App, w: u16| -> String {
        let mut term = Terminal::new(TestBackend::new(w, 12)).unwrap();
        term.draw(|f| super::render::ui(f, app)).unwrap();
        term.backend()
            .buffer()
            .content()
            .chunks(w as usize)
            .map(|r| r.iter().map(|c| c.symbol()).collect::<String>())
            .collect::<Vec<_>>()
            .join("\n")
    };

    let narrow = paint(&mut app, 80);
    assert!(narrow.contains("Status"), "state must survive: {narrow}");
    assert!(narrow.contains("Repl"), "replicas must survive: {narrow}");
    assert!(
        !narrow.contains("Net Out"),
        "metrics must be dropped: {narrow}"
    );

    let wide = paint(&mut app, 140);
    assert!(
        wide.contains("Net Out"),
        "a wide terminal keeps the metrics: {wide}"
    );
}

#[test]
fn a_confirmation_never_hides_the_question_or_the_keys() {
    // At 80x24 the percentage-sized box was 41x5 for six lines: the question was cut
    // mid-word and "[y] Yes [n] Cancel" fell off the bottom, so an irreversible
    // host-wide action was confirmed blind.
    use ratatui::backend::TestBackend;
    use ratatui::Terminal;

    let mut app = App::new("s".into(), vec![]);
    app.confirm = Some(Confirm {
        action: "maint:systemPrune".into(),
        project: String::new(),
        service: String::new(),
        stype: String::new(),
        label: "Prune the Docker system? Unused containers, networks, images, \
                and build cache will be removed."
            .into(),
    });

    let mut term = Terminal::new(TestBackend::new(80, 24)).unwrap();
    term.draw(|f| super::render::ui(f, &mut app)).unwrap();
    let screen: String = term
        .backend()
        .buffer()
        .content()
        .chunks(80)
        .map(|r| r.iter().map(|c| c.symbol()).collect::<String>())
        .collect::<Vec<_>>()
        .join("\n");

    assert!(
        screen.contains("[y] Yes"),
        "the confirming key must be visible:\n{screen}"
    );
    assert!(
        screen.contains("removed."),
        "the question must wrap, not truncate:\n{screen}"
    );
    assert!(
        screen.contains("ENTIRE host"),
        "the blast radius must stay on screen:\n{screen}"
    );
}

#[test]
fn help_shows_everything_it_has_even_on_a_short_terminal() {
    // At 80x24 the help used to stop at the bottom border: the Anywhere, form and
    // Mouse sections were invisible with nothing on screen to say so, and every
    // description was cut mid-word. Help that lies by omission is worse than none.
    use ratatui::backend::TestBackend;
    use ratatui::Terminal;

    let mut app = App::new("s".into(), vec![]);
    app.screen = Screen::Projects;
    app.help = true;

    let paint = |app: &mut App| -> String {
        let mut term = Terminal::new(TestBackend::new(80, 24)).unwrap();
        term.draw(|f| super::render::ui(f, app)).unwrap();
        term.backend()
            .buffer()
            .content()
            .chunks(80)
            .map(|r| r.iter().map(|c| c.symbol()).collect::<String>())
            .collect::<Vec<_>>()
            .join("\n")
    };

    let top = paint(&mut app);
    assert!(
        top.contains("↑↓ scroll"),
        "overflow must be signalled, not hidden:\n{top}"
    );
    // Long descriptions wrap instead of truncating: the Env menu entry spills onto a
    // continuation line rather than ending mid-word.
    assert!(
        top.contains(".env file"),
        "descriptions must wrap, not truncate:\n{top}"
    );

    // Scrolling to the end reaches the sections that used to be unreachable.
    app.help_scroll = u16::MAX;
    let bottom = paint(&mut app);
    assert!(
        bottom.contains("Mouse") && bottom.contains("Right click"),
        "the last section must be reachable:\n{bottom}"
    );
}

#[test]
fn palette_says_so_when_nothing_matches() {
    // Enter on a query with no results used to take the palette away in silence,
    // which looks exactly like having run something.
    let (tx, _rx) = std::sync::mpsc::channel();
    let mut app = App::new("s".into(), vec![]);
    app.all_services = vec![json!({ "projectName": "p", "name": "web", "type": "app" })];
    app.open_palette();
    app.palette.as_mut().unwrap().query = "nosuchthing".into();
    assert!(app.palette.as_ref().unwrap().matches().is_empty());

    app.status = "Ready".into();
    app.palette_run(&tx);
    assert!(app.palette.is_none(), "the palette still closes");
    assert!(
        app.status.contains("Nothing matches"),
        "the dead end must be visible, got: {}",
        app.status
    );
}

#[test]
fn form_hints_are_dropped_whole_never_truncated() {
    use super::render::fit_hints;
    let parts: Vec<String> = ["[Enter] save", "[Esc] cancel", "[Tab] move field"]
        .iter()
        .map(|s| s.to_string())
        .collect();
    // Wide enough for everything.
    assert_eq!(
        fit_hints(&parts, 80),
        "[Enter] save  [Esc] cancel  [Tab] move field"
    );
    // Room for two: the third is dropped WHOLE, not cut into "[Tab] move fi".
    let narrow = fit_hints(&parts, 30);
    assert_eq!(narrow, "[Enter] save  [Esc] cancel");
    assert!(narrow.chars().count() <= 30);
    // The escape hatch survives before the nice-to-haves.
    assert!(fit_hints(&parts, 26).contains("[Esc] cancel"));
    // Multi-byte arrows are measured in chars, so they don't over-count and
    // wrongly drop a hint that fits.
    let arrows = vec!["[Enter] next →".to_string(), "[Esc] ← back".to_string()];
    assert_eq!(fit_hints(&arrows, 28), "[Enter] next →  [Esc] ← back");
}

#[test]
fn a_project_row_offers_project_actions_not_a_dead_end() {
    let (tx, _rx) = std::sync::mpsc::channel();
    let mut app = App::new("s".into(), vec![("s".into(), "u".into())]);
    app.screen = Screen::Projects;
    app.handle(
        Resp::AllServices {
            projects: vec!["proj".into()],
            services: vec![json!({"projectName": "proj", "name": "web", "type": "app"})],
        },
        &tx,
    );
    // Row 0 is the project header: no service selected, but the project itself
    // still has actions — this used to open nothing at all.
    app.services_table.select(Some(0));
    assert!(app.selected_row().is_none(), "row 0 must be the header");
    let items = app.context_items();
    assert!(
        items
            .iter()
            .any(|i| i.label.contains("Migrate WHOLE project")),
        "a project row must offer migrating the project, got: {:?}",
        items.iter().map(|i| i.label.clone()).collect::<Vec<_>>()
    );
}

#[test]
fn migrating_needs_somewhere_to_migrate_to() {
    // A single-host setup can't migrate anywhere. Saying so beats opening a form
    // with an empty dropdown the user can't satisfy.
    let (tx, _rx) = std::sync::mpsc::channel();
    let mut app = App::new("only".into(), vec![("only".into(), "u".into())]);
    app.screen = Screen::Projects;
    app.handle(
        Resp::AllServices {
            projects: vec!["p".into()],
            services: vec![json!({"projectName": "p", "name": "web", "type": "app"})],
        },
        &tx,
    );
    app.open_migrate_form(false);
    assert!(app.form.is_none(), "no form without a destination");
    assert!(
        app.status.contains("No other server"),
        "got: {}",
        app.status
    );
}

#[test]
fn migrating_a_project_collects_every_service_in_it() {
    let mut app = App::new(
        "a".into(),
        vec![("a".into(), "u".into()), ("b".into(), "u".into())],
    );
    app.screen = Screen::Projects;
    app.all_services = vec![
        json!({"projectName": "keep", "name": "web", "type": "app"}),
        json!({"projectName": "keep", "name": "db", "type": "mysql"}),
        json!({"projectName": "other", "name": "web", "type": "app"}),
    ];
    let got = app.project_services("keep");
    assert_eq!(got.len(), 2, "only the project's own services");
    assert!(got.iter().any(|(_, s, t)| s == "db" && t == "mysql"));
    assert!(
        !got.iter().any(|(p, _, _)| p == "other"),
        "another project's service must not be swept in"
    );
}

#[test]
fn gui_editors_are_made_to_wait_so_the_edit_is_not_lost() {
    use super::with_editor_wait;
    let cmd = |s: &str| s.split_whitespace().map(String::from).collect::<Vec<_>>();

    // Without the flag VS Code hands off and exits at once; the temp file would be
    // read back unchanged and deleted, throwing the user's edit away.
    let (got, gui) = with_editor_wait(&cmd("code"));
    assert_eq!(got, cmd("code --wait"));
    assert!(gui);

    // A full path and a Windows launcher resolve the same as a bare name.
    let (got, _) = with_editor_wait(&cmd("/usr/local/bin/code"));
    assert_eq!(got, cmd("/usr/local/bin/code --wait"));
    let (got, _) = with_editor_wait(&cmd("code.cmd"));
    assert_eq!(got, cmd("code.cmd --wait"));

    // Already correct: don't pass it twice.
    for already in ["code -w", "code --wait", "gvim -f"] {
        let (got, gui) = with_editor_wait(&cmd(already));
        assert_eq!(got, cmd(already), "{already} must be left alone");
        assert!(gui);
    }

    // Terminal editors block on their own — adding a flag would break them.
    for term in ["vi", "nano", "nvim", "vim", "emacs", "helix", "micro"] {
        let (got, gui) = with_editor_wait(&cmd(term));
        assert_eq!(got, cmd(term), "{term} must be untouched");
        assert!(!gui, "{term} is not a GUI editor");
    }

    // Existing user arguments survive; the flag goes after them.
    let (got, _) = with_editor_wait(&cmd("code --new-window"));
    assert_eq!(got, cmd("code --new-window --wait"));
}

#[test]
fn a_failure_is_never_faded_away() {
    use super::{status_should_fade, STATUS_IDLE};
    let long = STATUS_IDLE + std::time::Duration::from_secs(5);
    let short = std::time::Duration::from_secs(1);

    // The status line is the only place a failure is ever shown — no log, no
    // history. Fading it both destroys the message and claims all is well.
    for err in [
        "Error: [400] Project already exists.",
        "Error: connection refused",
        "Migrated 0/2 · failed: db: couldn't create",
    ] {
        assert!(
            !status_should_fade(err, long, 0),
            "a failure must survive: {err}"
        );
    }

    // Routine notices still fade, so they don't linger forever.
    for notice in ["Deploy started", "Env saved", "Refreshing..."] {
        assert!(status_should_fade(notice, long, 0), "should fade: {notice}");
        assert!(!status_should_fade(notice, short, 0), "not before its time");
    }

    // "Ready" is already the resting state; there is nothing to revert to.
    assert!(!status_should_fade("Ready", long, 0));
}

#[test]
fn render_and_the_fade_agree_on_what_counts_as_a_failure() {
    // These two used to carry separate copies of the rule, so a message could be
    // painted red as an error and then quietly erased as a routine notice.
    use super::app::status_is_error;
    use super::{status_should_fade, STATUS_IDLE};
    let long = STATUS_IDLE + std::time::Duration::from_secs(5);
    for s in [
        "Error: boom",
        "clone: failed",
        "Ready",
        "Env saved",
        "Deploy started",
    ] {
        assert_eq!(
            status_is_error(s),
            !status_should_fade(s, long, 0) && s != "Ready",
            "the colour rule and the fade rule disagree about: {s}"
        );
    }
}

#[test]
fn force_rebuild_is_offered_and_actually_turns_the_cache_off() {
    let (tx, rx) = std::sync::mpsc::channel();
    let mut app = App::new("s".into(), vec![("s".into(), "u".into())]);
    app.screen = Screen::Projects;
    app.handle(
        Resp::AllServices {
            projects: vec!["proj".into()],
            services: vec![json!({"projectName": "proj", "name": "web", "type": "app"})],
        },
        &tx,
    );
    let _ = rx.try_recv(); // drain anything the load queued

    // Offered as its own entry, separate from Deploy: skipping the cache is far
    // slower and must be a deliberate choice.
    let life = app.life_menu();
    let labels: Vec<_> = life.iter().map(|i| i.label.clone()).collect();
    assert!(
        labels.iter().any(|l| l.contains("Force rebuild")),
        "Lifecycle must offer a force rebuild, got: {labels:?}"
    );
    assert!(labels.iter().any(|l| l == "Deploy"), "plain Deploy stays");

    // The confirmation says what it will do, not "Deploy-force service".
    app.services_table.select(Some(1));
    app.ask_action("deploy-force");
    let label = app.confirm.as_ref().expect("a confirmation").label.clone();
    assert!(
        label.contains("cache") && !label.contains("Deploy-force"),
        "confirmation must read naturally, got: {label}"
    );

    // The point of the whole feature: forceRebuild reaches the request.
    app.confirm_key(KeyCode::Char('y'), &tx);
    let sent = rx.try_recv().expect("an action was dispatched");
    match sent {
        Req::Action {
            action,
            force,
            service,
            ..
        } => {
            assert_eq!(action, "deploy", "it is still the deploy endpoint");
            assert!(force, "the cache must be turned OFF");
            assert_eq!(service, "web");
        }
        _ => panic!("expected Req::Action"),
    }

    // A plain deploy must NOT have become a force rebuild.
    app.ask_action("deploy");
    app.confirm_key(KeyCode::Char('y'), &tx);
    match rx.try_recv().expect("dispatched") {
        Req::Action { action, force, .. } => {
            assert_eq!(action, "deploy");
            assert!(!force, "a plain deploy still uses the cache");
        }
        _ => panic!("expected Req::Action"),
    }
}

#[test]
fn nothing_reports_ready_while_a_request_is_still_running() {
    use super::{status_should_fade, STATUS_IDLE};
    let long = STATUS_IDLE + std::time::Duration::from_secs(60);

    // The case that made this worth fixing: `systemPrune` is a host-wide,
    // irreversible Docker prune whose only feedback is "Sending...". Fading that
    // to "Ready" after six seconds claims a destructive action finished when it
    // has not — and re-running it is the obvious next move.
    assert!(
        !status_should_fade("Sending...", long, 1),
        "a running request must never be reported as finished"
    );
    // Not about the wording: a message with no "..." is equally protected.
    assert!(!status_should_fade("Pruning the host", long, 1));

    // Once the lane goes idle the same notice is free to fade.
    assert!(status_should_fade("Sending...", long, 0));

    // A screen that refreshes WITHOUT sending anything (Dashboard, Terminal) must
    // still fade — the naive fix for this bug (never fade a "..." message) left a
    // spinner running forever there, which is why it was rejected.
    assert!(
        status_should_fade("Refreshing...", long, 0),
        "a message with no request behind it must not stick"
    );
}

#[test]
fn a_narrow_hosts_table_drops_columns_instead_of_halving_the_numbers() {
    // Squeezed, ratatui shrinks every column proportionally, and
    // "29.8 GB / 59.0 GB" became "29.8 GB" — a figure that reads as complete and
    // is not. That is the "confidently wrong number" class, not a cosmetic cut.
    use ratatui::backend::TestBackend;
    use ratatui::Terminal;

    let render_at = |w: u16| {
        let mut app = App::new("s".into(), vec![]);
        app.screen = Screen::Hosts;
        app.hosts = vec![HostRow {
            name: "prod".into(),
            url: "https://panel.example.com".into(),
            state: HostState::Ok(Box::new(json!({
                "cpu": [[0, 12.0]],
                "memoryUsedBytes": 32_000_000_000i64,
                "memoryTotalBytes": 64_000_000_000i64,
                "diskUsedBytes": 100_000_000_000i64,
                "diskTotalBytes": 800_000_000_000i64,
                "loadAvg": ["2.00", "3.02", "3.10"],
            }))),
        }];
        let mut t = Terminal::new(TestBackend::new(w, 12)).unwrap();
        t.draw(|f| ui(f, &mut app)).unwrap();
        let buf = t.backend().buffer().clone();
        buf.content()
            .chunks(w as usize)
            .map(|r| r.iter().map(|c| c.symbol()).collect::<String>())
            .collect::<Vec<_>>()
            .join("\n")
    };

    // Narrow: the memory figure must be WHOLE or absent — never a half that
    // reads as a total.
    let narrow = render_at(80);
    assert!(
        narrow.contains("29.8 GB / 59.6 GB"),
        "memory must keep both halves at 80 cols:\n{narrow}"
    );
    // The columns that made room are gone, not squeezed into slivers.
    assert!(!narrow.contains("URL"), "URL is dropped first:\n{narrow}");
    assert!(!narrow.contains("Load"), "Load goes next:\n{narrow}");
    assert!(
        !narrow.contains("Disk"),
        "Disk goes rather than render half a figure:\n{narrow}"
    );
    // A width that fits Disk WHOLE must show it whole.
    let mid = render_at(90);
    assert!(
        mid.contains("93.1 GB / 745.1 GB"),
        "disk must keep both halves where it fits:\n{mid}"
    );
    // What matters most is still there.
    assert!(narrow.contains("Status") && narrow.contains("prod"));

    // Wide enough for the lot — 133 columns, counting spacing, the highlight
    // symbol and the borders.
    let wide = render_at(140);
    for col in ["Server", "Status", "CPU", "Memory", "Disk", "Load", "URL"] {
        assert!(wide.contains(col), "{col} missing at 140 cols:\n{wide}");
    }
    assert!(wide.contains("29.8 GB / 59.6 GB"));
}

#[test]
fn an_unreachable_host_can_be_asked_why() {
    // Hosts is the screen you are on when something is broken, and its Status cell
    // cuts the reason to a few words ("DOWN — error sen"). It used to have no row
    // action at all, so that truncated fragment was all you could ever see.
    let (tx, _rx) = std::sync::mpsc::channel();
    let mut app = App::new("s".into(), vec![]);
    app.screen = Screen::Hosts;
    let long =
        "error sending request for url (https://10.255.255.1/api/rpc/metrics/getSystemStats): \
                connection timed out";
    app.hosts = vec![HostRow {
        name: "blackhole".into(),
        url: "https://10.255.255.1".into(),
        state: HostState::Err(long.into()),
    }];
    app.hosts_state.select(Some(0));
    // The pane the detail will be drawn into (set by the last Hosts paint).
    app.table_area = ratatui::layout::Rect::new(0, 0, 80, 20);

    app.on_key(KeyCode::Enter, &tx);

    assert!(
        matches!(app.screen, Screen::Viewer),
        "Enter on a host must open its detail"
    );
    // Wrapped, not cut: the viewer neither wraps nor scrolls sideways, so every
    // line has to fit the pane on its own.
    for l in &app.viewer_lines {
        assert!(
            l.chars().count() <= 78,
            "line overflows the pane and would be cut: {l:?}"
        );
    }
    // And nothing was LOST in the wrapping — the whole reason is recoverable.
    let rejoined = app.viewer_lines.join(" ");
    let flat: String = rejoined.split_whitespace().collect::<Vec<_>>().join(" ");
    assert!(
        flat.contains("connection timed out"),
        "the WHOLE reason must survive, got:\n{flat}"
    );
    assert!(
        flat.contains("getSystemStats"),
        "including the failing endpoint"
    );
    assert!(flat.contains("blackhole") && flat.contains("10.255.255.1"));
    // Esc goes back where you came from, not to some default screen.
    assert!(matches!(app.viewer_from, Screen::Hosts));
}

#[test]
fn the_viewer_can_reach_the_end_of_a_long_line() {
    // Logs open here and lines are neither wrapped nor reflowed, so anything past
    // the pane used to be unreachable. Worse, → was swallowed by the global tab
    // handler: reaching for the rest of a line threw you off the screen entirely.
    use ratatui::backend::TestBackend;
    use ratatui::Terminal;

    let (tx, _rx) = std::sync::mpsc::channel();
    let mut app = App::new("s".into(), vec![]);
    app.screen = Screen::Viewer;
    app.viewer_from = Screen::Projects;
    app.viewer_title = "Logs".into();
    app.viewer_lines = vec![format!("{}THE-END", "x".repeat(70))];

    let draw = |app: &mut App| {
        let mut t = Terminal::new(TestBackend::new(40, 10)).unwrap();
        t.draw(|f| ui(f, app)).unwrap();
        let buf = t.backend().buffer().clone();
        buf.content()
            .chunks(40)
            .map(|r| r.iter().map(|c| c.symbol()).collect::<String>())
            .collect::<Vec<_>>()
            .join("\n")
    };

    assert!(
        !draw(&mut app).contains("THE-END"),
        "the tail starts out beyond the pane"
    );

    // → scrolls sideways and must NOT leave the viewer.
    for _ in 0..8 {
        app.on_key(KeyCode::Right, &tx);
    }
    assert!(
        matches!(app.screen, Screen::Viewer),
        "→ must scroll, not switch tab"
    );
    let scrolled = draw(&mut app);
    assert!(
        scrolled.contains("THE-END"),
        "the end of the line must be reachable:\n{scrolled}"
    );
    // And you can tell you are no longer at the left edge.
    assert!(
        scrolled.contains("col"),
        "the offset must be shown:\n{scrolled}"
    );

    // Home returns to the left edge, not just the first line.
    app.on_key(KeyCode::Home, &tx);
    assert_eq!(app.viewer_hscroll, 0);
    assert!(!draw(&mut app).contains("THE-END"));
}

#[test]
fn the_viewer_cannot_scroll_past_its_last_line() {
    // Down/PageDown add without an upper bound, so holding either used to scroll
    // into a blank bordered box that reads as an empty log.
    use ratatui::backend::TestBackend;
    use ratatui::Terminal;

    let (tx, _rx) = std::sync::mpsc::channel();
    let mut app = App::new("s".into(), vec![]);
    app.screen = Screen::Viewer;
    app.viewer_from = Screen::Projects;
    app.viewer_title = "Logs".into();
    app.viewer_lines = (1..=12).map(|i| format!("line-{i}")).collect();
    app.viewer_follow = false;

    for _ in 0..40 {
        app.on_key(KeyCode::PageDown, &tx);
    }
    let mut t = Terminal::new(TestBackend::new(40, 10)).unwrap();
    t.draw(|f| ui(f, &mut app)).unwrap();
    let buf = t.backend().buffer().clone();
    let shown: String = buf
        .content()
        .chunks(40)
        .map(|r| r.iter().map(|c| c.symbol()).collect::<String>())
        .collect::<Vec<_>>()
        .join("\n");
    assert!(
        shown.contains("line-12"),
        "the last line must stay on screen however hard you scroll:\n{shown}"
    );
}
