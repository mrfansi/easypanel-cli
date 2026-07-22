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
    use crate::container::base64;
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
    let mut f = form(mount_fields(None));
    set(&mut f, "Type", "volume");
    set(&mut f, "Name", "data");
    set(&mut f, "Mount path", "/var/lib/data");
    assert_eq!(
        mount_body(&f).unwrap(),
        json!({ "type": "volume", "name": "data", "mountPath": "/var/lib/data" })
    );

    // bind: {type, hostPath, mountPath}
    let mut f = form(mount_fields(None));
    set(&mut f, "Type", "bind");
    set(&mut f, "Host path", "/etc/host/cfg");
    set(&mut f, "Mount path", "/cfg");
    assert_eq!(
        mount_body(&f).unwrap(),
        json!({ "type": "bind", "hostPath": "/etc/host/cfg", "mountPath": "/cfg" })
    );

    // Empty mount path -> error, whatever the type.
    let mut f = form(mount_fields(None));
    set(&mut f, "Type", "volume");
    set(&mut f, "Name", "data");
    assert!(mount_body(&f).is_err());

    // volume with no name -> error (not a silently-sent empty name).
    let mut f = form(mount_fields(None));
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

/// An app with two projects, the cursor on the first row (a project header).
fn marking_app() -> App {
    let mut app = App::new("s".into(), vec![]);
    app.projects = vec!["alpha".into(), "beta".into()];
    app.all_services = vec![
        svc("alpha", "api", "app"),
        svc("alpha", "web", "app"),
        svc("beta", "db", "mysql"),
    ];
    app.services_table.select(Some(0));
    app
}

#[test]
fn marking_a_project_header_marks_everything_under_it() {
    let mut app = marking_app();
    // Row 0 is the "alpha" header, not a service.
    assert!(app.selected_row().is_none());
    app.toggle_mark();
    assert_eq!(app.marked.len(), 2, "both of alpha's services");
    assert!(app.is_marked("alpha", "api") && app.is_marked("alpha", "web"));
    assert!(!app.is_marked("beta", "db"));

    // Pressing it again on a fully marked project clears it, rather than being
    // a key that visibly does nothing.
    app.toggle_mark();
    assert!(app.marked.is_empty());
}

#[test]
fn marking_follows_the_filter_and_forgets_services_that_vanish() {
    let mut app = marking_app();
    app.filter = "alpha".into();
    app.mark_all_visible();
    assert_eq!(app.marked.len(), 2, "only what the filter shows");

    // The set a bulk action would hit carries each service's CURRENT type,
    // resolved at dispatch, so the API group can't go stale.
    let targets = app.bulk_targets();
    assert_eq!(
        targets,
        vec![
            ("alpha".to_string(), "api".to_string(), "app".to_string()),
            ("alpha".to_string(), "web".to_string(), "app".to_string()),
        ]
    );

    // A service destroyed elsewhere drops out of the targets instead of being
    // sent as a call for something that no longer exists.
    app.all_services.retain(|s| field(s, "/name") != "api");
    assert_eq!(app.bulk_targets().len(), 1);
}

#[test]
fn a_bulk_action_needs_a_confirmation_that_names_its_targets() {
    let mut app = marking_app();
    // Nothing marked: no confirmation, and a hint on how to mark.
    app.open_bulk_confirm("restart", false);
    assert!(app.confirm.is_none());

    app.toggle_mark();
    app.open_bulk_confirm("restart", false);
    let c = app.confirm.as_ref().expect("must ask first");
    assert_eq!(c.action, "bulk-restart");
    assert!(c.label.contains('2'), "the count: {}", c.label);
    assert!(c.label.contains("alpha/api"), "the names: {}", c.label);
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
    let has = |v: &[super::actions::MenuItem], s: &str| v.iter().any(|i| i.label == s);

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
    assert_eq!(app.menu.as_ref().unwrap().items[0].label, "Env");
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
    // Each row is JUST the action — the service was repeated on all thirty of them,
    // which is what made the palette a wall of the same text.
    for action in ["Deploy", "Env", "Domain", "Basic auth", "Delete service"] {
        assert!(l.iter().any(|x| x == action), "missing {action} in {l:?}");
    }
    // It appears ONCE — as the row that jumps to it. Not on all thirty actions.
    assert_eq!(
        l.iter().filter(|x| x.contains("proj/web")).count(),
        1,
        "the service must not be repeated down the action rows: {l:?}"
    );
    // It is named once, in the title.
    assert_eq!(
        app.palette.as_ref().and_then(|p| p.context.clone()),
        Some("proj/web".into())
    );
    assert!(!l.iter().any(|x| x.starts_with("DB shell"))); // app isn't a db

    // Dropping it from the LABEL must not drop it from the SEARCH: "deploy web"
    // still has to find the deploy action on web.
    if let Some(p) = app.palette.as_mut() {
        p.query = "deploy web".into();
        let hits: Vec<String> = p
            .matches()
            .into_iter()
            .map(|i| p.items[i].label.clone())
            .collect();
        assert!(
            hits.iter().any(|x| x == "Deploy"),
            "multi-word search across the context is lost: {hits:?}"
        );
    }
    app.palette = None;

    // db (mysql) selected → its actions include DB shell + Config file (Advanced).
    app.services_table.select(Some(1)); // db
    app.open_palette();
    let ldb = labels(&app);
    assert!(ldb.iter().any(|x| x.starts_with("DB shell")));
    assert!(ldb.iter().any(|x| x == "Config file (Advanced)"));
    assert_eq!(
        app.palette.as_ref().and_then(|p| p.context.clone()),
        Some("proj/db".into())
    );
    app.palette = None;

    // Context also works for NON-service rows: Domains with a domain selected → the
    // domain context-menu actions (Edit/Primary/Delete) appear in the palette.
    app.screen = Screen::Domains;
    app.domains = vec![json!({ "host": "x.test", "id": "d1" })];
    app.domains_state.select(Some(0));
    app.open_palette();
    assert!(labels(&app).iter().any(|x| x == "Delete"));
    // The screen names the context once, rather than prefixing every row with it.
    assert_eq!(
        app.palette.as_ref().and_then(|p| p.context.clone()),
        Some("Domains".into())
    );
    // And it is still searchable that way.
    if let Some(p) = app.palette.as_mut() {
        p.query = "domains delete".into();
        assert!(
            !p.matches().is_empty(),
            "searching by screen name must work"
        );
    }
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
    // Matching is on the SEARCH text, which still carries the context the label
    // no longer repeats.
    assert!(m.iter().all(|&i| {
        let l = pal.items[i].search.to_lowercase();
        l.contains("deploy") && l.contains("web")
    }));
    // Take the Deploy action entry (not "Auto deploy…") and run it.
    let pos = m
        .iter()
        .position(|&i| pal.items[i].label == "Deploy")
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
        vec!["0.3%", "547.3 MB", "12.2 KB/s", "31.9 KB/s"]
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
    // Through the index the renderer actually uses — a per-row scan would be
    // O(services²) on a path that runs every frame.
    let idx = app.metric_index();
    let m = *idx.get(&("proj-a", "mysql")).unwrap();
    // Must pick proj-a, not proj-b with the same name.
    assert_eq!(metric_cols(Some(m))[0], "1.0%");
    assert_eq!(metric_cols(Some(m))[1], "2.0 KB");

    // A service with no metrics: its columns are "-", not a fake 0.
    assert!(!idx.contains_key(&("proj-c", "ghost")));
    assert_eq!(metric_cols(idx.get(&("proj-c", "ghost")).copied())[0], "-");
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
fn the_wizard_source_step_advances_with_no_source_and_blocks_a_half_formed_one() {
    // The Source step (step 1) must let an app advance with NO source chosen — a
    // shell to wire up later, which submit and the panel both allow. The gate used
    // to validate through `source_body` (stricter than submit) and dead-ended it.
    let mut f = create_form(&[("Name", "web"), ("Kind", "app")]);
    f.step = 1;
    assert!(
        validate_step(&f).is_ok(),
        "an untouched source must not block advancing past the Source step"
    );

    // …but a half-formed source is still blocked at the gate, same as submit.
    let mut f = create_form(&[("Name", "web"), ("Repo", "acme/web"), ("Branch", "")]);
    f.step = 1;
    assert!(
        validate_step(&f).is_err(),
        "a repo with no branch must still be refused"
    );
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
    // passed while the real screen showed "-0.0%". Now it calls the actual row
    // builder that render uses.
    let row = project_row("empty", 0, &[]);
    assert_eq!(row[0], "empty (0)");
    // Type / Status / Repl / Source / Auto are blank on a header, not "-": a
    // project has no single per-service state, and the dashes were pure noise.
    assert_eq!(
        &row[1..6],
        ["", "", "", "", ""],
        "per-service columns are blank"
    );
    assert_eq!(&row[6..], ["-", "-", "-", "-"], "nothing measured");
    assert!(
        !row.iter().any(|c| c.contains("-0.0")),
        "the Sum f64 identity is -0.0; it must not leak to the screen: {row:?}"
    );

    // With metrics -> actually summed, not "-".
    let m = json!({ "cpu": 1.5, "memory": 2048.0, "networkIn": 0.0, "networkOut": 0.0 });
    let row = project_row("filled", 1, &[&m]);
    assert_eq!(row[6], "1.5%");

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
fn the_tab_switch_hint_names_every_numbered_tab() {
    // The number keys jump straight to a tab: 1..=N, one per TAB_SCREENS entry
    // (keys.rs maps '1'..'8'). When Uptime was added as the 8th tab, this hint
    // still read "1-7", hiding the '8' jump from anyone reading the help. Derive
    // the upper bound from TAB_SCREENS so a future tab can't outrun the hint again.
    let n = TAB_SCREENS.len();
    let hint = GLOBAL_KEYS
        .iter()
        .find(|k| k.1 == "switch tab")
        .expect("the tab-switch keybinding must exist")
        .0;
    assert!(
        hint.contains(&format!("1-{n}")),
        "tab-switch hint {hint:?} must cover all {n} numbered tabs"
    );
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
    // The Source step now refuses to be left without a repo — as it must, since
    // the request it builds would be rejected anyway.
    for (label, value) in [("Repo", "owner/repo"), ("Branch", "main")] {
        form.fields
            .iter_mut()
            .find(|f| f.label == label)
            .unwrap()
            .value = value.into();
    }
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
    let url = crate::container::ws_url(&client, "zzz-emb", "zzz-redis", "sh").expect("ws_url");

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
fn a_long_action_description_is_cut_with_an_ellipsis_not_silently() {
    // Found by driving the TUI at 80 columns: a commit message stopped dead —
    // "Deploy service: feat: u" — and read as the whole description. The row was
    // trimmed at 200 characters, a limit a ~23-column cell never reaches, so
    // ratatui clipped the rest at the edge with nothing to show for it.
    use ratatui::backend::TestBackend;
    use ratatui::Terminal;

    let mut app = App::new("s".into(), vec![]);
    app.screen = Screen::Actions;
    app.actions = vec![json!({
        "projectName": "harisenin-com", "serviceName": "strapi", "status": "done",
        "type": "deployment",
        "description": "Deploy service: feat: unify the checkout flow across every storefront",
        "createdAt": "2026-07-20 05:55:15", "updatedAt": "2026-07-20 06:03:14"
    })];

    let mut term = Terminal::new(TestBackend::new(80, 10)).unwrap();
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
        screen.contains('…'),
        "a cut description must say that it was cut:\n{screen}"
    );
}

#[test]
fn a_failed_domain_load_says_so_instead_of_no_domains_yet() {
    use ratatui::backend::TestBackend;
    use ratatui::Terminal;

    let render = |app: &mut App| -> String {
        let mut term = Terminal::new(TestBackend::new(100, 12)).unwrap();
        term.draw(|f| super::render::ui(f, app)).unwrap();
        term.backend()
            .buffer()
            .content()
            .chunks(100)
            .map(|r| r.iter().map(|c| c.symbol()).collect::<String>())
            .collect::<Vec<_>>()
            .join("\n")
    };

    // A genuinely empty host: the invitation to add one is right.
    let mut empty = App::new("s".into(), vec![]);
    empty.screen = Screen::Domains;
    let s = render(&mut empty);
    assert!(s.contains("No domains yet"), "empty host:\n{s}");

    // A FAILED fetch leaves the list empty too — but it must NOT read as "no
    // domains yet" on a host that may have hundreds; it must name the failure.
    let mut failed = App::new("s".into(), vec![]);
    failed.screen = Screen::Domains;
    failed.domains_error = Some("[502] Bad Gateway".into());
    let s = render(&mut failed);
    assert!(
        !s.contains("No domains yet"),
        "a 502 must not read as an empty host:\n{s}"
    );
    assert!(
        s.contains("Couldn't load") && s.contains("502"),
        "the failure must be named:\n{s}"
    );
}

#[test]
fn the_monitor_table_indents_a_service_under_its_project_header() {
    use ratatui::backend::TestBackend;
    use ratatui::Terminal;

    let mut app = App::new("s".into(), vec![]);
    app.screen = Screen::Monitor;
    app.monitor = vec![
        json!({"projectName":"shop","serviceName":"webby","cpu":1.0,"memory":1048576.0,"networkIn":0.0,"networkOut":0.0}),
        json!({"projectName":"shop","serviceName":"dbase","cpu":2.0,"memory":2097152.0,"networkIn":0.0,"networkOut":0.0}),
    ];
    let mut term = Terminal::new(TestBackend::new(100, 24)).unwrap();
    term.draw(|f| super::render::ui(f, &mut app)).unwrap();
    let lines: Vec<String> = term
        .backend()
        .buffer()
        .content()
        .chunks(100)
        .map(|r| r.iter().map(|c| c.symbol()).collect::<String>())
        .collect();

    // The project header is flush-left; its services are INDENTED under it — the
    // tree the old `first_line` .trim() used to flatten (services rendered
    // flush, indistinguishable from their header).
    let header = lines
        .iter()
        .find(|l| l.contains("shop (2)"))
        .expect("a project header row");
    let svc = lines
        .iter()
        .find(|l| l.contains("webby"))
        .expect("a service row");
    assert!(
        !header.contains("  shop (2)"),
        "the project header must sit flush, not indented:\n{header}"
    );
    assert!(
        svc.contains("  webby"),
        "a service must be indented under its project:\n{svc}"
    );
}

#[test]
fn a_failed_services_or_stats_load_says_so_instead_of_reading_empty_or_idle() {
    use ratatui::backend::TestBackend;
    use ratatui::Terminal;

    let render = |app: &mut App, w: u16, h: u16| -> String {
        let mut term = Terminal::new(TestBackend::new(w, h)).unwrap();
        term.draw(|f| super::render::ui(f, app)).unwrap();
        term.backend()
            .buffer()
            .content()
            .chunks(w as usize)
            .map(|r| r.iter().map(|c| c.symbol()).collect::<String>())
            .collect::<Vec<_>>()
            .join("\n")
    };

    // Services: a failed fetch (empty list + error) must not read as "this host
    // has nothing" on a host that may have hundreds.
    let mut svc = App::new("s".into(), vec![]);
    svc.screen = Screen::Projects;
    svc.services_error = Some("[502] Bad Gateway".into());
    let s = render(&mut svc, 100, 12);
    assert!(
        s.contains("Couldn't load services") && s.contains("502"),
        "a failed service load must name the failure:\n{s}"
    );

    // Dashboard: a failed stats load must NOT draw a fabricated 0.0% gauge; it
    // must say the load failed.
    let mut dash = App::new("s".into(), vec![]);
    dash.screen = Screen::Dashboard;
    dash.stats_error = Some("[502] Bad Gateway".into());
    let s = render(&mut dash, 100, 16);
    assert!(
        s.contains("Couldn't load stats"),
        "a failed stats load must say so, not draw 0.0% gauges:\n{s}"
    );
    assert!(
        !s.contains("0.0%"),
        "a failed stats load must not fabricate a 0.0% gauge:\n{s}"
    );
}

#[test]
fn replicas_must_be_a_number_and_zero_is_refused() {
    let f = |replicas: &str| {
        form(vec![
            Field::text("Replicas", replicas),
            Field::text("Command", ""),
            Field::boolean("Zero downtime", true),
        ])
    };
    // The happy path, and the shape that matters: a flat {replicas} is answered
    // 200 by the server and changes NOTHING, so these keys are the payload.
    let body = deploy_body(&f("3")).expect("3 is fine");
    assert_eq!(body["replicas"], json!(3));
    assert_eq!(body["command"], json!(null), "empty means the image's own");
    assert_eq!(body["zeroDowntime"], json!(true));

    // Typos must not reach the server as something plausible.
    assert!(deploy_body(&f("two")).is_err());
    assert!(deploy_body(&f("")).is_err());
    // 0 stops the service; Lifecycle ▸ Stop is the honest door for that.
    let zero = deploy_body(&f("0")).unwrap_err();
    assert!(zero.contains("Stop"), "{zero}");
}

#[test]
fn a_wide_terminal_stops_cutting_the_destination() {
    // Reported from real use: at 186 columns the destination read
    // "http://harisenin-com-miniapp-gopa…" while a 25-character cuid sat beside
    // it at full width. Destination was pinned at 34 and Source absorbed every
    // spare column.
    use ratatui::backend::TestBackend;
    use ratatui::Terminal;

    let dest = "http://harisenin-com_miniapp-gopay:3000/";
    let mut app = App::new("s".into(), vec![]);
    app.screen = Screen::Domains;
    app.domains = vec![json!({
        "id": "cmrs19yie001007mc7gtd8398",
        "host": "miniapp-gopay.harisenin.com", "path": "/", "https": true,
        "destinationType": "service",
        "serviceDestination": {"projectName": "harisenin-com", "serviceName": "miniapp-gopay",
                               "protocol": "http", "port": 3000, "path": "/"}
    })];

    let mut term = Terminal::new(TestBackend::new(160, 12)).unwrap();
    term.draw(|f| super::render::ui(f, &mut app)).unwrap();
    let screen: String = term
        .backend()
        .buffer()
        .content()
        .chunks(160)
        .map(|r| r.iter().map(|c| c.symbol()).collect::<String>())
        .collect::<Vec<_>>()
        .join("\n");

    assert!(
        screen.contains(dest),
        "the destination must fit when there is room:\n{screen}"
    );
}

#[test]
fn a_confirmation_names_the_server_it_is_about_to_change() {
    // With several hosts configured, the only answer to "which machine?" was the
    // frame's title — behind the very dialog asking to destroy something.
    use ratatui::backend::TestBackend;
    use ratatui::Terminal;

    let mut app = App::new("angelia-machine".into(), vec![]);
    app.confirm = Some(Confirm {
        action: "destroy".into(),
        project: "shop".into(),
        service: "db".into(),
        stype: String::new(),
        label: "Destroy service 'db'?".into(),
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
        screen.contains("on angelia-machine"),
        "the host must be named where it is read:\n{screen}"
    );
    assert!(
        screen.contains("shop/db"),
        "and still the target:\n{screen}"
    );
    assert!(screen.contains("[y] Yes"), "keys still fit:\n{screen}");
}

#[test]
fn a_server_keeps_its_colour_and_differs_from_its_neighbours() {
    use super::render::server_colour;
    // Stable: the same name gives the same colour every run, or the signal is
    // worse than useless — it would teach the wrong association.
    assert_eq!(
        server_colour("aurel-machine"),
        server_colour("aurel-machine")
    );
    // And the hosts actually in play must not collide, or the whole point goes.
    assert_ne!(
        server_colour("aurel-machine"),
        server_colour("angelia-machine")
    );
}

#[test]
fn editing_a_mount_prefills_it_and_keeps_e_meaning_one_thing() {
    // Mounts could be added and deleted but not CHANGED, so moving one path
    // meant deleting a volume mount and rebuilding it. The values are fetched
    // fresh and prefilled rather than parsed back out of the row's text.
    let (tx, _rx) = std::sync::mpsc::channel();
    let mut app = App::new("s".into(), vec![]);
    app.handle(
        Resp::MountForm {
            project: "proj".into(),
            service: "web".into(),
            index: 2,
            values: json!({"type": "bind", "hostPath": "/srv/data", "mountPath": "/data"}),
        },
        &tx,
    );
    let form = app.form.as_ref().expect("the edit form opens");
    assert_eq!(form.by_label("Type"), "bind");
    assert_eq!(form.by_label("Host path"), "/srv/data");
    assert_eq!(form.by_label("Mount path"), "/data");
    assert!(
        matches!(form.kind, FormKind::MountEdit { index: 2, .. }),
        "it edits the mount it was opened on, by index"
    );

    // A mount that vanished between listing and editing is refused, not guessed.
    app.handle(Resp::Err("Mount [2] is no longer there".into()), &tx);
    assert!(app.status.contains("no longer there"), "{}", app.status);
}

#[test]
fn a_form_is_wide_enough_for_the_note_that_explains_it() {
    // The form was a fixed 64 columns, so its own explanation was cut mid-word:
    // "lists that server's backups; only ones on shared remote stora". A
    // sentence that stops without warning is worse than none, because the reader
    // cannot tell what was withheld.
    use ratatui::backend::TestBackend;
    use ratatui::Terminal;

    let note = "lists that server's backups; only ones on shared remote storage";
    let mut app = App::new("s".into(), vec![]);
    app.form = Some(
        Form::new(
            FormKind::ProjectCreate,
            " Restore from another server ",
            vec![Field::text("Server", "angelia-machine")],
        )
        .with_note(note.to_string()),
    );

    let mut term = Terminal::new(TestBackend::new(100, 24)).unwrap();
    term.draw(|f| super::render::ui(f, &mut app)).unwrap();
    let screen: String = term
        .backend()
        .buffer()
        .content()
        .chunks(100)
        .map(|r| r.iter().map(|c| c.symbol()).collect::<String>())
        .collect::<Vec<_>>()
        .join("\n");

    assert!(screen.contains(note), "the whole note must fit:\n{screen}");
}

#[test]
fn ticked_databases_are_what_gets_backed_up() {
    // Reported from real use: the picker offered "all or exactly one", so
    // backing up three of five meant running the whole flow three times.
    let (tx, _rx) = std::sync::mpsc::channel();
    let mut app = App::new("s".into(), vec![]);
    app.backups.providers = vec![("p1".into(), "R2".into(), "s3".into())];
    app.backups.provider = Some(("p1".into(), "R2".into()));
    app.handle(
        Resp::DatabasesIn {
            project: "proj".into(),
            service: "db".into(),
            names: vec!["one".into(), "two".into(), "three".into()],
        },
        &tx,
    );

    // Cursor starts on "[0] All"; move onto a database and tick two of them.
    app.on_key(KeyCode::Down, &tx);
    app.on_key(KeyCode::Char('v'), &tx);
    app.on_key(KeyCode::Down, &tx);
    app.on_key(KeyCode::Down, &tx);
    app.on_key(KeyCode::Char('v'), &tx);
    assert_eq!(app.backups.marked.len(), 2, "two ticked");
    assert!(app.viewer.lines.iter().any(|l| l.contains("✓ one")));
    assert!(app.viewer.lines.iter().any(|l| l.contains("✓ three")));

    // Enter acts on the TICKS, not on whatever row the cursor rests on.
    app.on_key(KeyCode::Enter, &tx);
    let c = app.confirm.as_ref().expect("must confirm first");
    assert!(
        c.label.contains("one") && c.label.contains("three"),
        "{}",
        c.label
    );
    assert!(
        !c.label.contains("two"),
        "an unticked one must not go: {}",
        c.label
    );
    assert_eq!(
        app.backups.pending,
        vec!["one".to_string(), "three".to_string()]
    );
}

#[test]
fn the_arrows_still_move_the_list_while_a_filter_is_being_typed() {
    // Reported from real use: type a filter, watch the table narrow, reach for ↓
    // — and nothing happens, with no hint that Enter is needed first. Every
    // navigation key was swallowed while the filter had focus.
    let (tx, _rx) = std::sync::mpsc::channel();
    let mut app = App::new("s".into(), vec![]);
    app.projects = vec!["p".into()];
    app.all_services = vec![svc("p", "api", "app"), svc("p", "web", "app")];
    app.screen = Screen::Projects;

    app.on_key(KeyCode::Char('/'), &tx);
    assert!(app.filter_input, "typing a filter");
    for c in "ap".chars() {
        app.on_key(KeyCode::Char(c), &tx);
    }
    assert_eq!(app.filter, "ap", "letters are still text, not commands");

    let before = app.services_table.selected();
    app.on_key(KeyCode::Down, &tx);
    assert_ne!(
        app.services_table.selected(),
        before,
        "↓ must move the selection while the filter is still open"
    );
    // …and the filter is untouched by it.
    assert_eq!(app.filter, "ap");
    assert!(app.filter_input, "moving does not close the filter");
}

#[test]
fn a_bulk_confirmation_does_not_claim_to_affect_the_whole_host() {
    // The blast-radius line was INFERRED from an empty project, which until bulk
    // existed only ever meant a maintenance action. A bulk run has no single
    // target either, so restarting three marked services announced itself as
    // affecting the ENTIRE host — seen on screen against a live server.
    use ratatui::backend::TestBackend;
    use ratatui::Terminal;

    let mut app = marking_app();
    app.toggle_mark();
    app.open_bulk_confirm("restart", false);

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
        !screen.contains("ENTIRE host"),
        "a bulk action is not host-wide:\n{screen}"
    );
    assert!(
        screen.contains("marked services"),
        "it must still state its blast radius:\n{screen}"
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
    // Disk's presence now depends on how wide the Server column had to be — a
    // short name leaves room a long one does not. What must hold either way is
    // that whatever IS shown is whole.
    if narrow.contains("Disk") {
        assert!(
            narrow.contains("93.1 GB / 745.1 GB"),
            "a Disk column that is shown must be whole:\n{narrow}"
        );
    }
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
    for l in &app.viewer.lines {
        assert!(
            l.chars().count() <= 78,
            "line overflows the pane and would be cut: {l:?}"
        );
    }
    // And nothing was LOST in the wrapping — the whole reason is recoverable.
    let rejoined = app.viewer.lines.join(" ");
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
    assert!(matches!(app.viewer.from, Screen::Hosts));
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
    app.viewer.from = Screen::Projects;
    app.viewer.title = "Logs".into();
    app.viewer.lines = vec![format!("{}THE-END", "x".repeat(70))];

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
    assert_eq!(app.viewer.hscroll, 0);
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
    app.viewer.from = Screen::Projects;
    app.viewer.title = "Logs".into();
    app.viewer.lines = (1..=12).map(|i| format!("line-{i}")).collect();
    app.viewer.follow = false;

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

#[test]
fn a_narrow_tile_shortens_its_figures_rather_than_cutting_them() {
    use super::render::{compact_pair, fit_sub};

    // Five tiles across 80 columns leave 14 usable each. Cut to that,
    // "199.9 GB / 784.9 GB" reads "199.9 GB / 784" — a total with no unit, wrong
    // by three orders of magnitude.
    let full = "199.9 GB / 784.9 GB".to_string();
    let compact = compact_pair("199.9 GB", "784.9 GB");
    assert_eq!(compact, "199.9/784.9 GB");
    assert!(compact.chars().count() <= 14, "the compact form must fit");

    let forms = vec![full.clone(), compact.clone()];
    assert_eq!(fit_sub(&forms, 40), full, "room for the full form");
    assert_eq!(fit_sub(&forms, 14), compact, "narrow: shorten, don't cut");
    // Narrower than BOTH: nothing beats a wrong number.
    assert_eq!(fit_sub(&forms, 8), "");
    // Whatever comes back is never wider than the tile.
    for w in 0..40 {
        assert!(fit_sub(&forms, w).chars().count() <= w);
    }

    // Both numbers survive the shortening — a lone "199.9 GB" would read as a
    // complete figure while hiding that it is a half.
    assert!(compact.contains("199.9") && compact.contains("784.9"));

    // Mismatched units can't share a suffix, so both are kept.
    assert_eq!(compact_pair("900.0 MB", "2.0 GB"), "900.0 MB/2.0 GB");
}

#[test]
fn monitor_tiles_never_show_a_half_number_at_80_columns() {
    use ratatui::backend::TestBackend;
    use ratatui::Terminal;

    let mut app = App::new("s".into(), vec![]);
    app.screen = Screen::Monitor;
    app.stats = Some(json!({
        "cpu": [[0, 8.6]],
        "memory": [[0, 52.7]],
        "disk": [[0, 25.5]],
        "cpuCores": 16,
        "loadAvg": ["1.20", "1.30", "1.40"],
        "memoryUsedBytes": 33_300_000_000i64,
        "memoryTotalBytes": 63_400_000_000i64,
        "diskUsedBytes": 214_600_000_000i64,
        "diskTotalBytes": 842_700_000_000i64,
    }));
    let mut t = Terminal::new(TestBackend::new(80, 20)).unwrap();
    t.draw(|f| ui(f, &mut app)).unwrap();
    let buf = t.backend().buffer().clone();
    let shown: String = buf
        .content()
        .chunks(80)
        .map(|r| r.iter().map(|c| c.symbol()).collect::<String>())
        .collect::<Vec<_>>()
        .join("\n");

    // The exact string the bug produced: a total cut away from its unit.
    assert!(
        !shown.contains("/ 784") && !shown.contains("/ 78 "),
        "a total must never appear without its unit:\n{shown}"
    );
    // Both figures are present, in the shortened form.
    assert!(
        shown.contains("31.0/59.0 GB"),
        "memory must keep both halves:\n{shown}"
    );
    assert!(
        shown.contains("199.9/784.8 GB"),
        "disk must keep both halves:\n{shown}"
    );
}

#[test]
fn a_narrow_actions_table_keeps_when_and_drops_how_long() {
    // The full set needs 88 columns. Below that "Target" was squeezed from 28 to
    // 20 and the service an action happened to stopped being identifiable —
    // "harisenin-net-db/php" for phpmyadmin.
    use ratatui::backend::TestBackend;
    use ratatui::Terminal;

    let render_at = |w: u16| {
        let (tx, _rx) = std::sync::mpsc::channel();
        let mut app = App::new("s".into(), vec![]);
        app.screen = Screen::Actions;
        app.handle(
            Resp::Actions(vec![json!({
                "status": "done",
                "projectName": "harisenin-net-db",
                "serviceName": "phpmyadmin",
                "description": "Deploy service",
                "createdAt": "2026-07-19 03:00:00",
                "updatedAt": "2026-07-19 03:00:18",
            })]),
            &tx,
        );
        let mut t = Terminal::new(TestBackend::new(w, 8)).unwrap();
        t.draw(|f| ui(f, &mut app)).unwrap();
        let buf = t.backend().buffer().clone();
        buf.content()
            .chunks(w as usize)
            .map(|r| r.iter().map(|c| c.symbol()).collect::<String>())
            .collect::<Vec<_>>()
            .join("\n")
    };

    let narrow = render_at(80);
    // The whole target, not a name cut mid-word.
    assert!(
        narrow.contains("harisenin-net-db/phpmyadmin"),
        "the service acted on must stay identifiable:\n{narrow}"
    );
    // "When" survives; "how long" is what makes room for it.
    assert!(
        narrow.contains("Age"),
        "a history must keep when:\n{narrow}"
    );
    assert!(
        !narrow.contains("Duration"),
        "Duration is the one to give up:\n{narrow}"
    );

    // Wide enough for the lot: everything is back.
    let wide = render_at(100);
    for col in ["Status", "Target", "Description", "Duration", "Age"] {
        assert!(wide.contains(col), "{col} missing at 100 cols:\n{wide}");
    }
    assert!(wide.contains("harisenin-net-db/phpmyadmin"));
}

#[test]
fn a_forms_own_guidance_outlives_the_fading_status_line() {
    use ratatui::backend::TestBackend;
    use ratatui::Terminal;

    let (tx, _rx) = std::sync::mpsc::channel();
    let mut app = App::new("s".into(), vec![]);
    app.screen = Screen::Projects;
    app.handle(
        Resp::AllServices {
            projects: vec!["proj".into()],
            services: vec![json!({"projectName": "proj", "name": "web", "type": "app"})],
        },
        &tx,
    );
    app.services_table.select(Some(1));
    app.open_clone_form();

    // The warning belongs to the form, not to a status line that erases itself
    // after six seconds while the user is still typing in that very form.
    let note = app
        .form
        .as_ref()
        .and_then(|f| f.note.clone())
        .expect("the clone form must carry its own note");
    assert!(note.contains("NOT the data"), "got: {note}");
    assert!(
        !app.status.contains("not data"),
        "the guidance must not sit in the fading status line: {}",
        app.status
    );
    // Stronger than "it fades": opening a form no longer commandeers the status
    // line at all, so nothing there can go stale under the user.
    assert_eq!(
        app.status, "Ready",
        "the form must not hijack the status line"
    );

    // And it is actually drawn.
    let mut t = Terminal::new(TestBackend::new(90, 16)).unwrap();
    t.draw(|f| ui(f, &mut app)).unwrap();
    let buf = t.backend().buffer().clone();
    let shown: String = buf
        .content()
        .chunks(90)
        .map(|r| r.iter().map(|c| c.symbol()).collect::<String>())
        .collect::<Vec<_>>()
        .join("\n");
    assert!(
        shown.contains("NOT the data"),
        "the note must be on the form itself:\n{shown}"
    );
}

#[test]
#[ignore]
fn bench_render_cost() {
    use ratatui::backend::TestBackend;
    use ratatui::Terminal;
    use std::time::Instant;

    let mk_services = |n: usize| -> Vec<Value> {
        (0..n)
            .map(|i| {
                json!({
                    "projectName": format!("proj-{}", i / 10),
                    "name": format!("svc-{i}"),
                    "type": "app",
                    "memory": 1_000_000 + i,
                    "cpu": 0.5,
                    "networkIn": 100, "networkOut": 200,
                })
            })
            .collect()
    };

    for n in [50usize, 200, 500] {
        let (tx, _rx) = std::sync::mpsc::channel();
        let mut app = App::new("s".into(), vec![]);
        app.handle(
            Resp::AllServices {
                projects: (0..n / 10).map(|i| format!("proj-{i}")).collect(),
                services: mk_services(n),
            },
            &tx,
        );
        app.monitor = mk_services(n);
        let mut t = Terminal::new(TestBackend::new(120, 40)).unwrap();

        for (name, screen) in [("Services", Screen::Projects), ("Monitor", Screen::Monitor)] {
            app.screen = screen;
            // warm
            t.draw(|f| ui(f, &mut app)).unwrap();
            let start = Instant::now();
            const FRAMES: u32 = 100;
            for _ in 0..FRAMES {
                t.draw(|f| ui(f, &mut app)).unwrap();
            }
            let per = start.elapsed().as_secs_f64() * 1000.0 / FRAMES as f64;
            println!("n={n:4} {name:9} {per:7.3} ms/frame");
        }
    }
}

#[test]
fn a_gauge_label_stays_readable_where_the_bar_covers_it() {
    // ratatui swaps fg/bg for the part of the label sitting ON the filled bar.
    // With no bg set, that half rendered as the terminal's DEFAULT foreground on
    // green — light on light in a dark theme, unreadable at exactly the moment
    // the number is worth reading. Reported from a screenshot at 54.4%.
    use ratatui::backend::TestBackend;
    use ratatui::Terminal;

    for pct in [10.6_f64, 54.4, 95.0] {
        let mut t = Terminal::new(TestBackend::new(40, 3)).unwrap();
        t.draw(|f| {
            super::render::render_gauge(f, f.area(), "Memory", pct, ratatui::style::Color::Reset)
        })
        .unwrap();
        let buf = t.backend().buffer().clone();
        let row: Vec<_> = buf.content().chunks(40).nth(1).unwrap().to_vec();
        for c in row
            .iter()
            .filter(|c| c.symbol().chars().any(|ch| ch.is_ascii_digit()))
        {
            assert_ne!(
                c.fg,
                ratatui::style::Color::Reset,
                "at {pct}% a label cell falls back to the terminal default, which \
                 has no guaranteed contrast against the bar"
            );
            assert_ne!(c.fg, c.bg, "at {pct}% the label is invisible");
        }
    }
}

#[test]
fn the_wizard_refuses_to_leave_a_step_it_cannot_satisfy() {
    // It used to walk you through all five steps with an empty Name and an empty
    // Repo without a word, then refuse on the Domains step with a complaint about
    // a field two steps back and off screen — blaming the character set of a name
    // that was simply missing.
    let (tx, _rx) = std::sync::mpsc::channel();
    let mut app = App::new("s".into(), vec![]);
    app.projects = vec!["p".into()];
    app.new_service_form(&tx);

    // Step 1 with no name: refused, and we stay where the field is.
    app.form_key(KeyCode::Enter, &tx);
    let form = app.form.as_ref().unwrap();
    assert_eq!(form.step, 0, "must not advance past an unnamed service");
    let err = form.error.clone().expect("the refusal must be shown");
    assert!(err.to_lowercase().contains("name"), "got: {err}");
    // On the form itself, not in a status line that fades while you look for it.
    assert_eq!(app.status, "Ready");

    // A name with illegal characters is refused too, and says so precisely.
    let set = |app: &mut App, label: &str, v: &str| {
        app.form
            .as_mut()
            .unwrap()
            .fields
            .iter_mut()
            .find(|f| f.label == label)
            .unwrap()
            .value = v.into();
    };
    set(&mut app, "Name", "Web Server!");
    app.form_key(KeyCode::Enter, &tx);
    assert_eq!(app.form.as_ref().unwrap().step, 0);
    assert!(app
        .form
        .as_ref()
        .unwrap()
        .error
        .as_ref()
        .unwrap()
        .contains("a-z"));

    // Valid name: through to Source, and the refusal is cleared.
    set(&mut app, "Name", "web");
    app.form_key(KeyCode::Enter, &tx);
    assert_eq!(app.form.as_ref().unwrap().step, 1);
    assert!(app.form.as_ref().unwrap().error.is_none());

    // A HALF-formed source is caught HERE, on its own step — never deferred two
    // steps on. (An UNtouched source is fine: an app may be created as a shell and
    // wired up later — form.rs documents this and the panel allows it. A repo with
    // no branch, though, is a broken source and must be refused at its own step.)
    set(&mut app, "Repo", "owner/repo");
    app.form_key(KeyCode::Enter, &tx);
    assert_eq!(
        app.form.as_ref().unwrap().step,
        1,
        "a half-formed source must not advance"
    );
    assert!(app.form.as_ref().unwrap().error.is_some());

    // Completed, it advances — and the failure can never surface two steps later.
    set(&mut app, "Branch", "main");
    app.form_key(KeyCode::Enter, &tx);
    assert_eq!(app.form.as_ref().unwrap().step, 2);
    assert!(app.form.as_ref().unwrap().error.is_none());
}

#[test]
fn a_terminal_keeps_history_you_can_scroll_back_to() {
    // The parser was created with a scrollback length of ZERO, so output that
    // left the screen was DISCARDED, not merely out of reach — no key could have
    // brought it back. Reported after a `SHOW REPLICA STATUS` scrolled away.
    let mut app = App::new("s".into(), vec![]);
    let mut parser = vt100::Parser::new(4, 40, super::TERM_SCROLLBACK);
    for i in 1..=40 {
        parser.process(format!("line-{i}\r\n").as_bytes());
    }
    app.term.parser = Some(parser);

    // There IS history behind the four visible rows.
    app.term_scroll(10);
    assert_eq!(
        app.term.parser.as_ref().unwrap().screen().scrollback(),
        10,
        "must be able to move back into the session's output"
    );

    // Scrolling further back stops at the oldest line rather than running off.
    app.term_scroll(10_000);
    let deepest = app.term.parser.as_ref().unwrap().screen().scrollback();
    assert!(
        deepest > 0 && deepest < 10_000,
        "clamped to real history: {deepest}"
    );

    // And the near end is guarded too: back to live, never past it.
    app.term_scroll(isize::MIN / 2);
    assert_eq!(app.term.parser.as_ref().unwrap().screen().scrollback(), 0);

    // With no session open it is simply a no-op, not a panic.
    app.term.parser = None;
    app.term_scroll(5);
}

#[test]
fn monitor_navigation_and_filter_agree_with_what_is_drawn() {
    // Three call sites worked the row count out independently and disagreed.
    // Navigation counted raw metric entries, which excludes the project header
    // rows the table inserts: with 60 metrics across 11 projects the table drew
    // 71 rows and the cursor stopped at 60 — the last eleven unreachable, with no
    // filter involved at all.
    let (tx, _rx) = std::sync::mpsc::channel();
    let mut app = App::new("s".into(), vec![]);
    app.screen = Screen::Monitor;
    app.monitor = (0..6)
        .map(|i| {
            json!({
                "projectName": format!("proj-{}", i / 3),
                "serviceName": format!("svc-{i}"),
                "cpu": 1.0, "memory": 1000.0, "networkIn": 0.0, "networkOut": 0.0
            })
        })
        .collect();

    // 6 services in 2 projects = 6 rows + 2 headers.
    assert_eq!(app.visible_monitor_rows().len(), 8);
    assert_eq!(
        app.monitor_rows_shown(),
        8,
        "navigation must be bounded by the DRAWN rows, headers included"
    );

    // End reaches the true last row, not the raw-metric count.
    app.monitor_key(KeyCode::End, &tx);
    assert_eq!(app.monitor_state.selected(), Some(7));

    // The filter applies to the Services view...
    app.filter = "svc-4".into();
    assert!(app.monitor_rows_shown() < 8);

    // ...and to Storage, where `/` used to do nothing whatsoever.
    app.monitor_view = MonitorView::Storage;
    app.storage = vec![
        json!({ "projectName": "p", "serviceName": "keep", "size": 10, "path": "/a" }),
        json!({ "projectName": "p", "serviceName": "other", "size": 20, "path": "/b" }),
    ];
    app.filter = "keep".into();
    assert_eq!(app.visible_storage_rows().len(), 1, "storage must filter");
    assert_eq!(app.monitor_rows_shown(), 1, "and navigation must follow it");

    // A filter matching nothing leaves nothing to move onto.
    app.filter = "zzzz-nothing".into();
    assert_eq!(app.monitor_rows_shown(), 0);
}

#[test]
fn filtering_the_monitor_keeps_each_row_attached_to_its_project() {
    // Filtering the flat row list dropped the project headers — they rarely
    // contain what you typed — leaving orphaned service rows. Two services named
    // "webapp" in different projects became two identical, indistinguishable
    // lines on a screen where you act on the row you picked.
    let mut app = App::new("s".into(), vec![]);
    app.screen = Screen::Monitor;
    app.monitor = vec![
        json!({"projectName": "alpha", "serviceName": "webapp",
               "cpu": 1.0, "memory": 900.0, "networkIn": 0.0, "networkOut": 0.0}),
        json!({"projectName": "alpha", "serviceName": "db",
               "cpu": 1.0, "memory": 800.0, "networkIn": 0.0, "networkOut": 0.0}),
        json!({"projectName": "beta", "serviceName": "webapp",
               "cpu": 1.0, "memory": 700.0, "networkIn": 0.0, "networkOut": 0.0}),
    ];

    app.filter = "webapp".into();
    let rows = app.visible_monitor_rows();
    let col0: Vec<&str> = rows.iter().map(|r| r[0].as_str()).collect();

    // Both matches are present AND each is under its own project.
    assert_eq!(
        col0,
        vec!["alpha (2)", "  webapp", "beta (1)", "  webapp"],
        "each match must stay under the project that owns it"
    );
    // The project that matched nothing is gone entirely, header included.
    assert!(!col0.iter().any(|c| c.starts_with("gamma")));
    // A service that did NOT match is not dragged along by its sibling.
    assert!(!col0.iter().any(|c| c.trim() == "db"));

    // Matching a PROJECT keeps everything inside it, as the Services table does.
    app.filter = "alpha".into();
    let rows = app.visible_monitor_rows();
    let col0: Vec<&str> = rows.iter().map(|r| r[0].as_str()).collect();
    assert_eq!(col0, vec!["alpha (2)", "  webapp", "  db"]);

    // No filter: untouched.
    app.filter.clear();
    assert_eq!(app.visible_monitor_rows().len(), 5);
}

#[test]
fn a_dropdown_that_matches_nothing_does_not_close_as_if_it_had_picked() {
    // Enter used to close the dropdown, leave the field on its old value, and say
    // nothing — indistinguishable from a successful pick, so a typo left the user
    // believing they had changed it. The same silent close was fixed in the
    // palette; this is its sibling caller.
    use ratatui::backend::TestBackend;
    use ratatui::Terminal;

    let (tx, _rx) = std::sync::mpsc::channel();
    let mut app = App::new("s".into(), vec![]);
    app.screen = Screen::Projects;
    app.handle(
        Resp::AllServices {
            projects: vec!["alpha".into(), "beta".into()],
            services: vec![json!({"projectName": "alpha", "name": "web", "type": "app"})],
        },
        &tx,
    );
    app.services_table.select(Some(1));
    app.open_clone_form();

    // Open the Project dropdown and narrow it to nothing.
    app.on_key(KeyCode::Char(' '), &tx);
    assert!(app.chooser.is_some(), "the dropdown must open");
    for c in "zzzz".chars() {
        app.on_key(KeyCode::Char(c), &tx);
    }
    assert!(app.chooser.as_ref().unwrap().matches().is_empty());

    let before = app.form.as_ref().unwrap().by_label("Project");
    app.on_key(KeyCode::Enter, &tx);

    // It stays open rather than pretending to have chosen.
    assert!(
        app.chooser.is_some(),
        "Enter with no match must not close the dropdown"
    );
    assert_eq!(
        app.form.as_ref().unwrap().by_label("Project"),
        before,
        "and the field must be untouched"
    );

    // The box says so, instead of being an unexplained blank.
    let mut t = Terminal::new(TestBackend::new(90, 20)).unwrap();
    t.draw(|f| ui(f, &mut app)).unwrap();
    let buf = t.backend().buffer().clone();
    let shown: String = buf
        .content()
        .chunks(90)
        .map(|r| r.iter().map(|c| c.symbol()).collect::<String>())
        .collect::<Vec<_>>()
        .join("\n");
    assert!(
        shown.contains("nothing matches"),
        "must explain itself:\n{shown}"
    );
    assert!(
        shown.contains("Esc cancel"),
        "and offer the way out:\n{shown}"
    );

    // Widening the search brings it back, and Enter then really picks.
    for _ in 0..4 {
        app.on_key(KeyCode::Backspace, &tx);
    }
    app.on_key(KeyCode::Enter, &tx);
    assert!(app.chooser.is_none(), "a real match closes it");
}

#[test]
fn each_collection_has_one_door_not_two() {
    // "View X" and "Add X" were separate menu entries for the same screen, and
    // env had THREE — view, "edit (partial)" and "replace entire" — for what is
    // one operation. Saving sends the whole env string either way, so the only
    // difference between the last two was whether $EDITOR opened pre-filled.
    let (tx, _rx) = std::sync::mpsc::channel();
    let mut app = App::new("s".into(), vec![]);
    app.screen = Screen::Projects;
    app.handle(
        Resp::AllServices {
            projects: vec!["proj".into()],
            services: vec![json!({"projectName": "proj", "name": "web", "type": "app"})],
        },
        &tx,
    );
    app.services_table.select(Some(1));

    let labels = |v: Vec<super::actions::MenuItem>| -> Vec<String> {
        v.iter().map(|i| i.label.clone()).collect()
    };
    let env = labels(app.env_menu());
    assert!(env.contains(&"Env".to_string()), "one entry: {env:?}");
    assert!(
        !env.iter()
            .any(|l| l.contains("Replace") || l.contains("partial")),
        "the duplicate doors are gone: {env:?}"
    );
    for (menu, thing) in [
        (labels(app.net_menu()), "Ports"),
        (labels(app.net_menu()), "Redirects"),
        (labels(app.store_menu()), "Mounts"),
    ] {
        assert!(menu.contains(&thing.to_string()), "{thing} in {menu:?}");
        assert!(
            !menu
                .iter()
                .any(|l| l.starts_with(&format!("Add {}", &thing[..thing.len() - 1]))),
            "no separate Add door for {thing}: {menu:?}"
        );
    }

    // The doors that closed still open — from inside the screen that owns them.
    app.open_view(View::Ports, &tx);
    app.viewer.ctx = Some((View::Ports, "proj".into(), "web".into(), "app".into()));
    app.screen = Screen::Viewer;
    app.on_key(KeyCode::Char('n'), &tx);
    assert!(
        app.form.is_some(),
        "`n` in the Ports viewer must add a port"
    );

    // And `e` in the Env viewer starts the editor hand-off.
    let mut app2 = App::new("s".into(), vec![]);
    app2.handle(
        Resp::AllServices {
            projects: vec!["proj".into()],
            services: vec![json!({"projectName": "proj", "name": "web", "type": "app"})],
        },
        &tx,
    );
    app2.services_table.select(Some(1));
    app2.screen = Screen::Viewer;
    app2.viewer.ctx = Some((View::Env, "proj".into(), "web".into(), "app".into()));
    app2.on_key(KeyCode::Char('e'), &tx);
    assert!(
        app2.edit_env.is_some(),
        "`e` in the Env viewer must edit it"
    );
}

#[test]
fn a_maintenance_row_that_failed_does_not_read_like_a_value() {
    // It rendered in the terminal's ordinary text colour — identical to a real
    // Docker version directly above it. This is the screen that offers three
    // irreversible host-wide actions, so a reading that was never fetched must
    // not look like one that was.
    use ratatui::backend::TestBackend;
    use ratatui::Terminal;

    let mut app = App::new("s".into(), vec![]);
    app.screen = Screen::Maintenance;
    app.maint = vec![
        ("Docker".into(), Ok("29.6.1".into())),
        (
            "Daily cleanup".into(),
            Err("error sending request for url (https://10.255.255.1/api/rpc/settings/x)".into()),
        ),
    ];

    let mut t = Terminal::new(TestBackend::new(80, 16)).unwrap();
    t.draw(|f| ui(f, &mut app)).unwrap();
    let buf = t.backend().buffer().clone();
    let row_of = |needle: &str| -> usize {
        buf.content()
            .chunks(80)
            .position(|r| {
                r.iter()
                    .map(|c| c.symbol())
                    .collect::<String>()
                    .contains(needle)
            })
            .unwrap_or_else(|| panic!("{needle} not drawn"))
    };
    let cell = |row: usize, needle: &str| {
        let line: String = buf
            .content()
            .chunks(80)
            .nth(row)
            .unwrap()
            .iter()
            .map(|c| c.symbol())
            .collect();
        let col = line.find(needle).unwrap();
        buf.content()[row * 80 + col].clone()
    };

    let ok = cell(row_of("29.6.1"), "29.6.1");
    let bad = cell(row_of("could not load"), "could not load");
    assert_ne!(
        bad.fg, ok.fg,
        "a failed row must not be drawn in the same ink as a real value"
    );
    assert_ne!(
        bad.fg,
        ratatui::style::Color::Reset,
        "and not in the terminal default, which has no meaning"
    );

    // The reason survives instead of being cut at the pane edge...
    let all: String = buf
        .content()
        .chunks(80)
        .map(|r| r.iter().map(|c| c.symbol()).collect::<String>())
        .collect::<Vec<_>>()
        .join("");
    assert!(all.contains("10.255.255.1"), "the reason must be readable");
    // ...and so does the consequence of the destructive key next to it.
    assert!(
        all.contains("build cache"),
        "the [p] consequence must not be cut"
    );
}

#[test]
fn a_collection_row_is_selected_and_deleted_without_a_ten_row_ceiling() {
    // Deleting used to be "press the digit printed on the line": capped at [9] by
    // construction, and 1-7 were swallowed by the global tab keys. A collection
    // now has a SELECTED row, moved with the same helper every other table uses,
    // and `x` deletes it — the verb Domains and the server picker already use.
    let (tx, _rx) = std::sync::mpsc::channel();
    let mut app = App::new("s".into(), vec![]);
    app.screen = Screen::Viewer;
    app.viewer.from = Screen::Projects;
    app.viewer.ctx = Some((View::Ports, "proj".into(), "web".into(), "app".into()));
    // Fourteen rows: past the old ceiling on purpose.
    app.viewer.lines = (0..14).map(|i| format!("[{i}] 80{i:02}:80/tcp")).collect();
    app.viewer.row.select(Some(0));

    app.on_key(KeyCode::Down, &tx);
    app.on_key(KeyCode::Down, &tx);
    assert_eq!(app.viewer.row.selected(), Some(2));
    app.on_key(KeyCode::Char('x'), &tx);
    let c = app.confirm.take().expect("x must ask before deleting");
    assert!(c.label.contains("[2]"), "got: {}", c.label);
    assert_eq!(c.stype, "2", "the index travels to the delete request");

    // End reaches row 13 — past [9], which the digit scheme could never address.
    app.on_key(KeyCode::End, &tx);
    assert_eq!(app.viewer.row.selected(), Some(13));
    app.on_key(KeyCode::Char('x'), &tx);
    let c = app
        .confirm
        .take()
        .expect("the fourteenth row is deletable too");
    assert!(c.label.contains("[13]"), "got: {}", c.label);

    // Digits belong to the tabs again, on every screen including this one.
    app.on_key(KeyCode::Char('2'), &tx);
    assert!(matches!(app.screen, Screen::Hosts));

    // `x` where nothing is deletable says so rather than doing nothing.
    let mut env = App::new("s".into(), vec![]);
    env.screen = Screen::Viewer;
    env.viewer.ctx = Some((View::Env, "proj".into(), "web".into(), "app".into()));
    env.on_key(KeyCode::Char('x'), &tx);
    assert!(env.confirm.is_none());
    assert!(env.status.contains("Nothing here"), "got: {}", env.status);
}
#[test]
fn a_key_the_viewer_does_not_take_says_what_it_does() {
    // `a`/`e`/`b` are per-view: `a` adds a port but means nothing in Env. Doing
    // nothing at all read as a stuck app rather than a wrong key.
    let (tx, _rx) = std::sync::mpsc::channel();
    let mut app = App::new("s".into(), vec![]);
    app.screen = Screen::Viewer;
    app.viewer.from = Screen::Projects;
    app.viewer.ctx = Some((View::Env, "proj".into(), "web".into(), "app".into()));

    app.on_key(KeyCode::Char('b'), &tx);
    assert!(
        app.status.contains("e edit"),
        "it must name what this screen accepts, got: {}",
        app.status
    );
}

#[test]
fn the_server_picker_never_cuts_the_url_that_tells_hosts_apart() {
    // At 46% of an 80-column terminal this box was 36 wide: its own title lost
    // "x delete", and every URL was cut with no ellipsis —
    // "https://panel.internal.exa" reads as a complete, different host. The URL
    // is shown precisely so you can be sure which server you are about to edit
    // or DELETE.
    use ratatui::backend::TestBackend;
    use ratatui::Terminal;

    let mut app = App::new("aurel".into(), vec![]);
    app.all_servers = vec![
        ("aurel".into(), "https://aurel.kkbahagia.com".into()),
        ("prod".into(), "https://panel.internal.example.com".into()),
        (
            "staging".into(),
            "https://panel-staging.internal.example.com".into(),
        ),
    ];
    app.picker = Some(ratatui::widgets::ListState::default());

    let mut t = Terminal::new(TestBackend::new(80, 20)).unwrap();
    t.draw(|f| ui(f, &mut app)).unwrap();
    let buf = t.backend().buffer().clone();
    let shown: String = buf
        .content()
        .chunks(80)
        .map(|r| r.iter().map(|c| c.symbol()).collect::<String>())
        .collect::<Vec<_>>()
        .join("\n");

    // The two look-alike hosts stay distinguishable.
    assert!(
        shown.contains("panel.internal.example.com"),
        "the URL must survive:\n{shown}"
    );
    assert!(
        shown.contains("panel-staging.internal"),
        "and so must the one it could be confused with:\n{shown}"
    );
    // If anything is ever cut, it says so.
    for line in shown.lines().filter(|l| l.contains("https://")) {
        let url_part = line.split("https://").nth(1).unwrap_or("");
        let cut = url_part.trim_end_matches(['│', ' ']);
        assert!(
            !cut.ends_with("exa") && !cut.ends_with(".i"),
            "a cut URL must end in an ellipsis, got: {line}"
        );
    }
    // The destructive key is not the one that falls off the end.
    assert!(shown.contains("x delete"), "keys must fit:\n{shown}");
}

#[test]
fn an_empty_screen_says_what_to_do_instead_of_showing_a_blank_box() {
    use ratatui::backend::TestBackend;
    use ratatui::Terminal;

    let draw = |app: &mut App, w: u16| -> String {
        let mut t = Terminal::new(TestBackend::new(w, 12)).unwrap();
        t.draw(|f| ui(f, app)).unwrap();
        let buf = t.backend().buffer().clone();
        buf.content()
            .chunks(w as usize)
            .map(|r| r.iter().map(|c| c.symbol()).collect::<String>())
            .collect::<Vec<_>>()
            .join("\n")
    };

    // An empty collection: the placeholder is a MESSAGE, not a row you can
    // select — highlighting it made "No ports yet" look like something picked
    // and deletable.
    let mut app = App::new("s".into(), vec![]);
    app.screen = Screen::Viewer;
    app.viewer.from = Screen::Projects;
    app.viewer.title = "Ports · proj/web".into();
    app.viewer.ctx = Some((View::Ports, "proj".into(), "web".into(), "app".into()));
    app.viewer.lines = vec!["No ports yet — press n to add one".into()];
    let shown = draw(&mut app, 70);
    assert!(
        shown.contains("press n to add one"),
        "must say what to do:\n{shown}"
    );
    assert!(
        !shown.contains("› No ports"),
        "the message must not be highlighted like a row:\n{shown}"
    );

    // Domains with nothing to show says WHY — an excluded filter and an empty
    // list need different actions.
    let mut d = App::new("s".into(), vec![]);
    d.screen = Screen::Domains;
    let empty = draw(&mut d, 70);
    assert!(empty.contains("No domains yet"), "got:\n{empty}");

    d.domains = vec![json!({ "host": "a.test", "id": "d1" })];
    d.filter = "zzzz".into();
    let filtered = draw(&mut d, 70);
    assert!(
        filtered.contains("Nothing matches") && filtered.contains("Esc"),
        "a filter that excludes everything must say so:\n{filtered}"
    );
}

#[test]
fn a_freshly_opened_collection_does_not_arm_the_previous_selection() {
    // viewer.row survived across viewer loads, and render only seeded it when it
    // was None — true exactly once per process. So opening a collection inherited
    // whatever index the last one was left on: a different service, a different
    // resource, a row nobody chose, sitting armed under `x delete`.
    let (tx, _rx) = std::sync::mpsc::channel();
    let mut app = App::new("s".into(), vec![]);
    app.screen = Screen::Viewer;
    app.viewer.ctx = Some((View::Ports, "p".into(), "first".into(), "app".into()));
    app.handle(
        Resp::Viewer(
            "Ports · p/first".into(),
            (0..6).map(|i| format!("[{i}] 800{i}:80/tcp")).collect(),
        ),
        &tx,
    );
    for _ in 0..4 {
        app.on_key(KeyCode::Down, &tx);
    }
    assert_eq!(app.viewer.row.selected(), Some(4));

    // A different service's mounts arrive: the selection must start over.
    app.viewer.ctx = Some((View::Mounts, "p".into(), "other".into(), "app".into()));
    app.handle(
        Resp::Viewer(
            "Mounts · p/other".into(),
            (0..3)
                .map(|i| format!("[{i}] volume v{i} -> /d{i}"))
                .collect(),
        ),
        &tx,
    );
    assert_eq!(
        app.viewer.row.selected(),
        None,
        "a fresh list must not arrive with a row already chosen"
    );
}

#[test]
fn the_wheel_and_vim_keys_move_a_collections_selection() {
    // The wheel and j/k wrote to viewer.scroll, which a collection view does not
    // read — so both did nothing at all, on the one screen where every other
    // table answers them.
    let (tx, _rx) = std::sync::mpsc::channel();
    let mut app = App::new("s".into(), vec![]);
    app.screen = Screen::Viewer;
    app.viewer.ctx = Some((View::Ports, "p".into(), "web".into(), "app".into()));
    app.viewer.lines = (0..30).map(|i| format!("[{i}] 8{i:03}:80/tcp")).collect();
    app.viewer.row.select(Some(0));

    app.on_key(KeyCode::Char('j'), &tx);
    app.on_key(KeyCode::Char('j'), &tx);
    assert_eq!(
        app.viewer.row.selected(),
        Some(2),
        "j must move the selection"
    );
    app.on_key(KeyCode::Char('k'), &tx);
    assert_eq!(
        app.viewer.row.selected(),
        Some(1),
        "and k must move it back"
    );

    use ratatui::crossterm::event::{MouseEvent, MouseEventKind};
    app.on_mouse(
        MouseEvent {
            kind: MouseEventKind::ScrollDown,
            column: 5,
            row: 5,
            modifiers: ratatui::crossterm::event::KeyModifiers::NONE,
        },
        &tx,
    );
    assert!(
        app.viewer.row.selected().is_some_and(|i| i > 1),
        "the wheel must move the selection, not a scroll offset nothing reads"
    );
}

#[test]
fn a_menu_does_not_offer_what_the_service_type_cannot_have() {
    // Redirects and basic auth are web-only; source & build is app-only. The
    // handlers already refused ("only for web services"), but one keystroke too
    // late and in a status line that fades. Worse, Redirects OPENED on a redis
    // service and offered `n add` for something structurally impossible.
    let (tx, _rx) = std::sync::mpsc::channel();
    let mut app = App::new("s".into(), vec![]);
    app.screen = Screen::Projects;
    app.handle(
        Resp::AllServices {
            projects: vec!["p".into()],
            services: vec![
                json!({"projectName": "p", "name": "cache", "type": "redis"}),
                json!({"projectName": "p", "name": "web", "type": "app"}),
            ],
        },
        &tx,
    );
    let labels = |v: Vec<super::actions::MenuItem>| -> Vec<String> {
        v.iter().map(|i| i.label.clone()).collect()
    };

    // Rows: [0] project header, [1] cache (redis), [2] web (app).
    app.services_table.select(Some(1));
    let net = labels(app.net_menu());
    let build = labels(app.build_menu());
    // This used to assert redis HAS ports — an assumption never checked against
    // a server. It does not: `ports` and `mounts` answer "Invalid service type"
    // for every database, for compose and for wordpress. A test that encodes a
    // wrong assumption is worse than no test, because it turns a bug into
    // evidence of correctness.
    assert!(
        !net.contains(&"Ports".to_string()),
        "redis has no ports: {net:?}"
    );
    assert!(
        !labels(app.store_menu()).contains(&"Mounts".to_string()),
        "nor mounts"
    );
    for gone in ["Redirects", "Basic auth"] {
        assert!(!net.contains(&gone.to_string()), "{gone} on redis: {net:?}");
    }
    assert!(
        !build.contains(&"Source & build".to_string()),
        "redis has no source: {build:?}"
    );
    assert!(
        build.contains(&"Config file (Advanced)".to_string()),
        "but it does have the advanced config: {build:?}"
    );

    // The app service keeps all of them.
    app.services_table.select(Some(2));
    let net = labels(app.net_menu());
    let build = labels(app.build_menu());
    for kept in ["Redirects", "Basic auth"] {
        assert!(net.contains(&kept.to_string()), "{kept} on app: {net:?}");
    }
    assert!(build.contains(&"Source & build".to_string()));
}

#[test]
fn a_database_service_does_not_inherit_env_typed_for_an_app() {
    // `by_label` is visibility-blind: it returns a hidden field's value exactly
    // as if it were on screen. Fill Environment while Kind is "app", step back,
    // switch to postgres — and the env went with it. Verified against a live
    // server: createService on services/postgres ACCEPTS and STORES that env, so
    // this was a silently misconfigured database, not a rejected request.
    let (tx, _rx) = std::sync::mpsc::channel();
    let mut app = App::new("s".into(), vec![]);
    app.projects = vec!["p".into()];
    app.new_service_form(&tx);

    let set = |app: &mut App, label: &str, v: &str| {
        app.form
            .as_mut()
            .unwrap()
            .fields
            .iter_mut()
            .find(|f| f.label == label)
            .unwrap()
            .value = v.into();
    };
    set(&mut app, "Name", "db");
    set(&mut app, "Environment", "LEAKED=yes");
    set(&mut app, "Domain host", "leaked.example.com");

    // As an app, both belong.
    let form = app.form.as_ref().unwrap();
    assert_eq!(create_env(form), Some("LEAKED=yes".to_string()));
    assert!(create_domains(form).is_some());

    // Switched to a database, neither does — and the wizard collapses to one
    // page, so the user never sees those fields again to clear them.
    set(&mut app, "Kind", "postgres");
    let form = app.form.as_ref().unwrap();
    assert_eq!(form.steps_present(), vec![0], "a database is one page");
    assert_eq!(create_env(form), None, "env must not follow the switch");
    assert_eq!(create_domains(form), None, "nor the domain");
}

#[test]
fn a_validation_message_does_not_outlive_the_field_it_names() {
    // The refusal used to stay on the border until the next SUCCESSFUL Enter, so
    // it survived the very edit that answered it — and could end up naming a
    // field that a source switch had taken off screen entirely.
    let (tx, _rx) = std::sync::mpsc::channel();
    let mut app = App::new("s".into(), vec![]);
    app.projects = vec!["p".into()];
    app.new_service_form(&tx);

    app.form_key(KeyCode::Enter, &tx);
    assert!(app.form.as_ref().unwrap().error.is_some(), "no name yet");

    // Typing the answer dismisses the complaint about it.
    app.form_key(KeyCode::Char('w'), &tx);
    assert_eq!(app.form.as_ref().unwrap().error, None);
}

#[test]
fn an_image_source_has_no_build_step() {
    // A prebuilt image is pulled, not built. Verified against a live panel:
    // createService STORES the build, then updateSourceImage nulls it — so the
    // wizard was asking for nixpacks settings it knew would be discarded.
    let (tx, _rx) = std::sync::mpsc::channel();
    let mut app = App::new("s".into(), vec![]);
    app.projects = vec!["p".into()];
    app.new_service_form(&tx);

    let set = |app: &mut App, label: &str, v: &str| {
        app.form
            .as_mut()
            .unwrap()
            .fields
            .iter_mut()
            .find(|f| f.label == label)
            .unwrap()
            .value = v.into();
    };
    set(&mut app, "Name", "web");
    set(&mut app, "Install command", "npm ci");

    // Built from a repo, the build step is there and is sent.
    let form = app.form.as_ref().unwrap();
    assert!(form.steps_present().contains(&2), "repo source builds");
    assert!(create_build(form).is_some());

    // From an image there is nothing to build: the page is gone, and the
    // settings typed before the switch do not follow it.
    set(&mut app, "Source", "image");
    let form = app.form.as_ref().unwrap();
    assert!(
        !form.steps_present().contains(&2),
        "no build page for image"
    );
    assert_eq!(create_build(form), None, "nor a build in the payload");
}

#[test]
fn r_refetches_an_action_detail_instead_of_only_claiming_to() {
    // An action detail has no viewer.ctx, so refresh had nothing to re-send: `r`
    // reported "Refreshing..." and left a RUNNING deploy's log frozen where it
    // was first fetched — on the screen you open to watch one.
    let (tx, rx) = std::sync::mpsc::channel();
    let mut app = App::new("s".into(), vec![]);
    app.screen = Screen::Actions;
    app.actions = vec![json!({
        "id": "act-1", "type": "deployment", "status": "running",
        "projectName": "p", "serviceName": "web", "description": "Deploy service",
        "createdAt": "2026-07-19 10:00:00", "updatedAt": "2026-07-19 10:00:00",
    })];
    app.actions_state.select(Some(0));
    app.on_key(KeyCode::Enter, &tx);
    assert!(
        matches!(rx.try_recv(), Ok(Req::ActionDetail(ref id)) if id == "act-1"),
        "Enter opens the detail"
    );

    app.screen = Screen::Viewer;
    app.viewer.ctx = None; // as actions_key leaves it
    app.refresh(&tx);
    let sent: Vec<Req> = std::iter::from_fn(|| rx.try_recv().ok()).collect();
    assert!(
        sent.iter()
            .any(|r| matches!(r, Req::ActionDetail(id) if id == "act-1")),
        "`r` must actually re-fetch the detail it is showing"
    );

    // Opening a service view clears it, so `r` there does not resurrect the action.
    app.handle(
        Resp::AllServices {
            projects: vec!["p".into()],
            services: vec![json!({"projectName": "p", "name": "web", "type": "app"})],
        },
        &tx,
    );
    app.services_table.select(Some(1));
    app.open_view(View::Env, &tx);
    assert!(app.viewer.action_detail.is_none());
}

/// The Domains tab holding one domain per host, with a filter already applied.
fn domain_app() -> App {
    let mut app = App::new("s".into(), vec![]);
    app.screen = Screen::Domains;
    app.domains = vec![
        json!({ "id": "d1", "host": "one.old.com", "path": "/",
                "destinationType": "service",
                "serviceDestination": { "projectName": "p", "serviceName": "api",
                                        "port": 80, "protocol": "http", "path": "/" } }),
        json!({ "id": "d2", "host": "two.old.com", "path": "/",
                "destinationType": "service",
                "serviceDestination": { "projectName": "p", "serviceName": "api",
                                        "port": 80, "protocol": "http", "path": "/" } }),
        json!({ "id": "d3", "host": "keep.other.com", "path": "/",
                "destinationType": "service",
                "serviceDestination": { "projectName": "p", "serviceName": "api",
                                        "port": 80, "protocol": "http", "path": "/" } }),
    ];
    app.domains_state.select(Some(0));
    app
}

#[test]
fn a_bulk_domain_rewrite_only_touches_what_the_filter_shows() {
    use ratatui::crossterm::event::KeyCode;
    let (tx, _rx) = std::sync::mpsc::channel();
    let mut app = domain_app();
    // The filter IS the selection: with it on, "keep.other.com" is off screen and
    // must stay untouched even though a rewrite of "old.com" would never match it
    // anyway — what matters is that the plan is built from the visible set.
    app.filter = "one.old".into();
    app.on_key(KeyCode::Char('E'), &tx);
    assert!(app.form.is_some(), "E opens the bulk edit form");
    app.preview_domain_edits("host", "old.com", "new.com");

    // Nothing is sent yet: the preview is the confirmation, and it names each
    // domain rather than only counting them.
    assert_eq!(app.domain_edits.len(), 1);
    assert_eq!(app.domain_edits[0].id, "d1");
    assert!(app.screen == Screen::Viewer);
    assert!(
        app.viewer
            .lines
            .iter()
            .any(|l| l.contains("one.old.com") && l.contains("one.new.com")),
        "{:?}",
        app.viewer.lines
    );

    // Walking away disarms it, so a later Enter cannot fire the rewrite.
    app.on_key(KeyCode::Esc, &tx);
    assert!(app.domain_edits.is_empty());
}

#[test]
fn a_destination_rewrite_names_the_domain_each_line_belongs_to() {
    // Every line of a destination rewrite reads "p/api → p/api2", identically.
    // Without the host, a preview of five domains is five copies of one sentence
    // and the user cannot tell which five they are approving.
    let mut app = domain_app();
    app.preview_domain_edits("destination service", "p/api", "p/api2");
    assert_eq!(app.domain_edits.len(), 3);
    assert!(
        app.viewer
            .lines
            .iter()
            .any(|l| l.starts_with("one.old.com:")),
        "{:?}",
        app.viewer.lines
    );
}

#[test]
fn a_bulk_rewrite_that_matches_nothing_says_so_instead_of_going_quiet() {
    let mut app = domain_app();
    app.preview_domain_edits("host", "absent.example", "x");
    assert!(app.domain_edits.is_empty());
    assert!(app.status.contains("No domain on screen"), "{}", app.status);
    // A rewrite that would break a domain keeps the form open to be corrected.
    app.form = Some(Form::new(
        FormKind::DomainBulkEdit,
        "t",
        domain_bulk_fields(),
    ));
    app.preview_domain_edits("host", "one.old.com", "");
    assert!(
        app.form.is_some(),
        "the form stays open on a rejected rewrite"
    );
    assert!(status_is_error(&app.status), "{}", app.status);
}

#[test]
fn a_project_env_change_only_offers_to_deploy_what_can_be_deployed() {
    let mut app = App::new("s".into(), vec![]);
    app.all_services = vec![
        svc("alpha", "api", "app"),
        svc("alpha", "db", "mysql"),
        svc("alpha", "cms", "wordpress"),
        svc("beta", "other", "app"),
    ];
    // A database is pulled and a wordpress has no build step, so deploying them
    // is a route that does not exist — offering it would 404 on the user's behalf.
    // And another project's services are not affected at all.
    let stale = app.deployable_in("alpha");
    assert_eq!(stale.len(), 1);
    assert_eq!(stale[0].1, "api");

    // The count in the offer IS the count that gets deployed: same function.
    let (tx, _rx) = std::sync::mpsc::channel();
    app.handle(Resp::ProjectEnvSaved("alpha".into()), &tx);
    let c = app.confirm.as_ref().expect("an offer to deploy");
    assert!(c.label.contains("1 service(s)"), "{}", c.label);

    // A project with nothing deployable must not raise an offer that would send
    // an empty deploy — it just reports the save.
    app.confirm = None;
    app.all_services = vec![svc("gamma", "db", "mysql")];
    app.handle(Resp::ProjectEnvSaved("gamma".into()), &tx);
    assert!(app.confirm.is_none());
    assert!(app.status.contains("saved"), "{}", app.status);
}

#[test]
fn a_server_name_is_editable_and_the_edit_form_knows_it_is_a_rename() {
    use ratatui::crossterm::event::KeyCode;
    use ratatui::widgets::ListState;
    let (tx, _rx) = std::sync::mpsc::channel();
    let mut app = App::new("prod".into(), vec![]);
    app.all_servers = vec![("prod".into(), "https://p".into())];
    app.picker = Some(ListState::default().with_selected(Some(0)));
    app.picker_key(KeyCode::Char('e'), &tx);

    let f = app.form.as_ref().expect("the edit form");
    // The name used to be missing entirely, so a typo in it was permanent: the
    // only way to fix one was to delete the server, taking its token with it.
    assert_eq!(f.val(0), "prod");
    assert_eq!(f.val(1), "https://p");

    // Saving the same name is an ordinary save, not a rename.
    app.submit_form(&tx);
    assert!(matches!(
        app.server_action,
        Some(ServerAction::Save {
            rename_from: None,
            ..
        })
    ));

    // Changing it makes the save carry where it is coming FROM, which is the
    // only way the token and the default flag can follow it.
    app.picker = Some(ListState::default().with_selected(Some(0)));
    app.picker_key(KeyCode::Char('e'), &tx);
    if let Some(f) = app.form.as_mut() {
        f.fields[0].value = "prod-eu".into();
    }
    app.submit_form(&tx);
    match app.server_action {
        Some(ServerAction::Save {
            rename_from: Some(ref old),
            ref name,
            ..
        }) => {
            assert_eq!(old, "prod");
            assert_eq!(name, "prod-eu");
        }
        _ => panic!("a changed name must travel as a rename"),
    }
}

#[test]
fn the_hosts_table_never_cuts_the_name_that_tells_hosts_apart() {
    use ratatui::backend::TestBackend;
    use ratatui::Terminal;
    let mut app = App::new("aurel-machine".into(), vec![]);
    // Fifteen characters — one more than the old fixed width, which rendered it
    // as "angelia-machin" and broke the one column that identifies the machine.
    app.hosts = vec![
        HostRow {
            name: "aurel-machine".into(),
            url: "https://a".into(),
            state: HostState::Loading,
        },
        HostRow {
            name: "angelia-machine".into(),
            url: "https://b".into(),
            state: HostState::Loading,
        },
    ];
    app.screen = Screen::Hosts;
    let mut t = Terminal::new(TestBackend::new(120, 12)).unwrap();
    t.draw(|f| ui(f, &mut app)).unwrap();
    let buf = t.backend().buffer().clone();
    let text: String = buf
        .content()
        .chunks(120)
        .map(|row| {
            row.iter()
                .map(|c| c.symbol())
                .collect::<String>()
                .trim_end()
                .to_string()
                + "\n"
        })
        .collect();
    assert!(text.contains("angelia-machine"), "{text}");
}

#[test]
fn the_services_table_never_cuts_a_repo_name_silently() {
    use ratatui::backend::TestBackend;
    use ratatui::Terminal;
    let mut app = App::new("s".into(), vec![]);
    app.projects = vec!["alpha".into()];
    let mut svc = svc("alpha", "dashboard", "app");
    // The real case: two services of one owner whose names differ only after the
    // point a 16-column cell can show. Cut silently they read as one repo.
    svc["source"] = json!({
        "type": "github", "owner": "harisenincom", "repo": "edukasistudio", "ref": "dev"
    });
    app.all_services = vec![svc];
    app.screen = Screen::Projects;

    for width in [100u16, 120, 200] {
        let mut t = Terminal::new(TestBackend::new(width, 12)).unwrap();
        t.draw(|f| ui(f, &mut app)).unwrap();
        let text: String = t
            .backend()
            .buffer()
            .content()
            .chunks(width as usize)
            .map(|r| r.iter().map(|c| c.symbol()).collect::<String>() + "\n")
            .collect();
        let row = text
            .lines()
            .find(|l| l.contains("dashboard"))
            .unwrap_or_else(|| panic!("no service row at width {width}:\n{text}"))
            .to_string();
        // Either the whole repo is there, or what is shown says it was cut.
        assert!(
            row.contains("harisenincom/edukasistudio") || row.contains('…'),
            "width {width} cut the source with no ellipsis: {row}"
        );
    }
}

#[test]
fn a_long_host_name_never_squeezes_the_failure_reason() {
    use ratatui::backend::TestBackend;
    use ratatui::Terminal;
    // "angelia-machine" is fifteen characters, one more than the width the drop
    // thresholds were written for. At 133 columns the seventh column was kept at
    // a width that no longer fit, and Status — the only flexible column — paid
    // for it, on the one column that carries WHY a host is unreachable.
    let mut app = App::new("angelia-machine".into(), vec![]);
    app.hosts = vec![HostRow {
        name: "angelia-machine".into(),
        url: "https://angelia.example.com".into(),
        state: HostState::Err("error sending request for url (https://x)".into()),
    }];
    app.screen = Screen::Hosts;
    for w in [100u16, 133, 160] {
        let mut t = Terminal::new(TestBackend::new(w, 8)).unwrap();
        t.draw(|f| ui(f, &mut app)).unwrap();
        let text: String = t
            .backend()
            .buffer()
            .content()
            .chunks(w as usize)
            .map(|r| r.iter().map(|c| c.symbol()).collect::<String>() + "\n")
            .collect();
        let row = text
            .lines()
            .find(|l| l.contains("DOWN"))
            .unwrap_or_else(|| panic!("no host row at {w}:\n{text}"))
            .to_string();
        // The name itself survives, and the reason either fits or says it was cut.
        assert!(row.contains("angelia-machine"), "{w}: {row}");
        assert!(
            row.contains("error sending request for url (https://x)") || row.contains('…'),
            "{w}: the reason was cut with no ellipsis: {row}"
        );
    }
}

#[test]
fn a_swarm_node_that_left_the_cluster_is_not_painted_like_a_healthy_one() {
    use ratatui::backend::TestBackend;
    use ratatui::style::Color;
    use ratatui::Terminal;
    let mut app = App::new("s".into(), vec![]);
    app.nodes = vec![
        json!({"Description":{"Hostname":"node-ok"},"Spec":{"Role":"manager","Availability":"active"},
               "Status":{"State":"ready","Addr":"10.0.0.1"}}),
        json!({"Description":{"Hostname":"node-gone"},"Spec":{"Role":"worker","Availability":"active"},
               "Status":{"State":"down","Addr":"10.0.0.2"}}),
    ];
    app.screen = Screen::Dashboard;
    let mut t = Terminal::new(TestBackend::new(100, 24)).unwrap();
    t.draw(|f| ui(f, &mut app)).unwrap();
    let buf = t.backend().buffer().clone();

    // The colour of the first cell of each node's row. A node leaving the cluster
    // is why services vanish, and the Dashboard is the first screen on launch —
    // it must not read as ordinary text there.
    let colour_of = |name: &str| {
        let w = 100usize;
        let (i, _) = buf
            .content()
            .chunks(w)
            .enumerate()
            .find(|(_, r)| {
                r.iter()
                    .map(|c| c.symbol())
                    .collect::<String>()
                    .contains(name)
            })
            .unwrap_or_else(|| panic!("no row for {name}"));
        let row: Vec<_> = buf.content().chunks(w).nth(i).unwrap().to_vec();
        let col = row
            .iter()
            .position(|c| c.symbol() != " " && c.symbol() != "│")
            .unwrap();
        row[col].fg
    };
    assert_ne!(
        colour_of("node-gone"),
        colour_of("node-ok"),
        "a down node must not look like a ready one"
    );
    assert_eq!(colour_of("node-gone"), Color::Indexed(196));
}

#[test]
fn filtering_from_the_bottom_of_a_long_list_shows_the_matches_not_one_row() {
    use ratatui::backend::TestBackend;
    use ratatui::crossterm::event::KeyCode;
    use ratatui::Terminal;
    let (tx, _rx) = std::sync::mpsc::channel();
    let mut app = App::new("s".into(), vec![]);
    // Long enough that the list scrolls: a real host had 713 domains.
    app.domains = (0..60)
        .map(|i| {
            json!({ "id": format!("d{i}"), "host": format!("host{i}.example.com"),
                    "path": "/", "https": true, "destinationType": "service",
                    "serviceDestination": { "projectName": "p", "serviceName": "s",
                                            "port": 80, "protocol": "http", "path": "/" } })
        })
        .collect();
    app.domains_state.select(Some(0));
    app.screen = Screen::Domains;

    let mut t = Terminal::new(TestBackend::new(100, 20)).unwrap();
    let rows_shown = |app: &mut App, t: &mut Terminal<TestBackend>| {
        t.draw(|f| ui(f, app)).unwrap();
        t.backend()
            .buffer()
            .content()
            .chunks(100)
            .filter(|r| {
                r.iter()
                    .map(|c| c.symbol())
                    .collect::<String>()
                    .contains("example.com")
            })
            .count()
    };

    let full = rows_shown(&mut app, &mut t);
    assert!(full > 5, "the table should be full to start with: {full}");

    // Walk to the bottom, then narrow. The old code clamped the SELECTED index to
    // the shorter list but left the scroll offset where it was, and ratatui only
    // scrolls up when the selection is above the offset — so the screen rendered
    // from row ~59 of a 40-row list: ONE row under a title claiming 40 matches.
    app.on_key(KeyCode::End, &tx);
    rows_shown(&mut app, &mut t);
    app.filter_input = true;
    for c in "host1".chars() {
        app.filter_key(KeyCode::Char(c));
    }
    let after = rows_shown(&mut app, &mut t);
    assert_eq!(
        app.visible_domains().len(),
        11,
        "host1, host1x — the filter should match 11"
    );
    assert!(
        after > 5,
        "filtering left the view stuck at the bottom: {after} row(s) on screen for 11 matches"
    );
}

#[test]
fn enrolling_a_domain_goes_through_the_form_not_a_silent_default() {
    use ratatui::crossterm::event::KeyCode;
    let (tx, _rx) = std::sync::mpsc::channel();
    let mut app = App::new("s".into(), vec![]);
    app.screen = Screen::Domains;
    app.domains = vec![json!({ "id": "d1", "host": "one.test", "path": "/", "https": true })];
    app.domains_state.select(Some(0));

    // Nothing is watched until the operator says so — never the whole list.
    assert!(app.watch.is_empty());
    app.on_key(KeyCode::Char('w'), &tx);

    // `w` does NOT enrol on its own. Enrolling is a deliberate act, so what the
    // domain is checked WITH is chosen here rather than defaulted silently and
    // configured on another screen afterwards.
    assert!(
        app.watch.is_empty(),
        "nothing is stored before the form is saved"
    );
    let form = app.form.as_ref().expect("the check form");
    assert!(form.title.contains("one.test"), "{}", form.title);
    assert_eq!(form.val(0), "GET", "prefilled with the obvious check");

    // Backing out enrols nothing at all.
    app.form = None;
    assert!(app.watch.is_empty() && app.watch_action.is_none());

    // Saving it does.
    app.on_key(KeyCode::Char('w'), &tx);
    if let Some(f) = app.form.as_mut() {
        f.fields[0].value = "POST".into();
    }
    app.submit_form(&tx);
    assert_eq!(app.watch.len(), 1);
    assert_eq!(app.watch[0].url, "https://one.test/");
    assert_eq!(app.watch[0].method, "POST", "the choice made in the form");
    // The file write is left to the event loop, which owns every path on disk.
    assert!(matches!(app.watch_action, Some(WatchAction::Put(_))));

    // A second `w` on an already-watched domain edits it rather than adding a
    // duplicate: one door to a check, whichever screen you came from.
    app.on_key(KeyCode::Char('w'), &tx);
    assert_eq!(app.form.as_ref().unwrap().val(0), "POST");
    app.submit_form(&tx);
    assert_eq!(app.watch.len(), 1);
}

#[test]
fn editing_a_check_drops_the_answer_that_described_the_old_request() {
    let (tx, _rx) = std::sync::mpsc::channel();
    let mut app = App::new("s".into(), vec![]);
    let url = "https://a.test/";
    app.watch = vec![crate::uptime::Check::get(url)];
    app.probes = vec![crate::uptime::Probe {
        url: url.into(),
        outcome: crate::uptime::Outcome::Answered {
            status: 200,
            head: std::time::Duration::from_millis(10),
            total: std::time::Duration::from_millis(10),
        },
    }];
    app.form = Some(Form::new(
        FormKind::CheckEdit { url: url.into() },
        "t",
        check_fields(&crate::uptime::Check::get(url)),
    ));
    if let Some(f) = app.form.as_mut() {
        f.fields[0].value = "POST".into();
    }
    app.submit_form(&tx);

    assert_eq!(app.watch[0].method, "POST");
    // A 200 from a GET says nothing about what the POST will do; leaving it on
    // screen next to the new check would be a stale claim.
    assert!(app.probes.is_empty());
    assert!(app.form.is_none());
}

#[test]
fn a_check_that_cannot_be_sent_keeps_the_form_open_to_be_fixed() {
    let (tx, _rx) = std::sync::mpsc::channel();
    let mut app = App::new("s".into(), vec![]);
    let url = "https://a.test/";
    app.watch = vec![crate::uptime::Check::get(url)];
    app.form = Some(Form::new(
        FormKind::CheckEdit { url: url.into() },
        "t",
        check_fields(&crate::uptime::Check::get(url)),
    ));
    if let Some(f) = app.form.as_mut() {
        f.fields[2].value = "0".into();
    }
    app.submit_form(&tx);
    assert!(app.form.is_some(), "the fix is one character in this box");
    assert!(status_is_error(&app.status), "{}", app.status);
    assert_eq!(app.watch[0].timeout_secs, 10, "nothing was saved");
}

#[test]
fn the_tab_bar_never_loses_a_tab_at_eighty_columns() {
    use ratatui::backend::TestBackend;
    use ratatui::Terminal;
    // Adding the eighth tab pushed the full labels past 80 columns and the strip
    // was clipped at the frame — the newest tab vanished from the one bar whose
    // job is saying where you are and where you can go.
    for w in [80u16, 100, 120, 200] {
        let mut app = App::new("s".into(), vec![]);
        let mut t = Terminal::new(TestBackend::new(w, 8)).unwrap();
        t.draw(|f| ui(f, &mut app)).unwrap();
        let bar: String = t
            .backend()
            .buffer()
            .content()
            .chunks(w as usize)
            .map(|r| r.iter().map(|c| c.symbol()).collect::<String>())
            .find(|l| l.contains("Dash"))
            .unwrap_or_default();
        // Every tab is reachable, so every tab must be readable — whether by its
        // full name or its shortened one.
        for (full, short) in super::app::TABS.iter().zip(super::app::SHORT_TABS.iter()) {
            assert!(
                bar.contains(full) || bar.contains(short),
                "at {w} columns the bar lost '{full}':\n{bar}"
            );
        }
    }
}

#[test]
fn a_dialog_is_as_wide_as_it_measured_itself_to_need() {
    use ratatui::backend::TestBackend;
    use ratatui::Terminal;
    // Both dialogs compute an absolute width in COLUMNS and used to hand it to a
    // helper whose parameter is a PERCENTAGE. At 80 columns a form that measured
    // itself at 68 was drawn 54 wide — cutting exactly the text the measurement
    // existed to protect — and the confirmation wrapped its label using a width
    // it never got, so the line naming the keys could fall out of the box.
    let long = "https://harisenin-net-db-phpmyadmin.aurel.kkbahagia.com/";

    let mut app = App::new("s".into(), vec![]);
    app.form = Some(Form::new(
        FormKind::CheckEdit { url: long.into() },
        format!(" Watch {long} "),
        check_fields(&crate::uptime::Check::get(long)),
    ));
    let mut t = Terminal::new(TestBackend::new(80, 20)).unwrap();
    t.draw(|f| ui(f, &mut app)).unwrap();
    let screen: String = t
        .backend()
        .buffer()
        .content()
        .chunks(80)
        .map(|r| r.iter().map(|c| c.symbol()).collect::<String>() + "\n")
        .collect();
    assert!(
        screen.contains("kkbahagia.com"),
        "the form cut the URL it is about:\n{screen}"
    );

    // The confirmation must always show how to answer it.
    let mut app = App::new("s".into(), vec![]);
    app.confirm = Some(Confirm {
        action: "destroy".into(),
        project: "p".into(),
        service: "s".into(),
        stype: "app".into(),
        label: "Destroy 'harisenin-net-db-phpmyadmin'? This removes the service and \
                everything in it, and cannot be undone."
            .into(),
    });
    let mut t = Terminal::new(TestBackend::new(80, 24)).unwrap();
    t.draw(|f| ui(f, &mut app)).unwrap();
    let screen: String = t
        .backend()
        .buffer()
        .content()
        .chunks(80)
        .map(|r| r.iter().map(|c| c.symbol()).collect::<String>() + "\n")
        .collect();
    assert!(
        screen.contains("[y] Yes"),
        "an irreversible action was asked without showing how to answer:\n{screen}"
    );
}

#[test]
fn the_palette_can_find_a_service_before_you_have_visited_the_services_tab() {
    // `:` is advertised as "jump to any service from anywhere", but the service
    // list was only fetched when the Services tab was opened. On a fresh launch
    // the palette therefore answered "0 results" for a service that plainly
    // exists — a confidently wrong answer, not an empty one.
    let (tx, rx) = std::sync::mpsc::channel();
    super::send_initial(&tx);
    drop(tx);
    let asked: Vec<Req> = rx.into_iter().collect();
    assert!(
        asked.iter().any(|r| matches!(r, Req::AllServices)),
        "start-up must ask for the services the palette searches"
    );

    // And with them loaded, it finds one.
    let mut app = App::new("s".into(), vec![]);
    app.all_services = vec![svc("harisenin-com-db", "mysql", "mysql")];
    app.open_palette();
    let pal = app.palette.as_mut().expect("the palette");
    pal.query = "mysql".into();
    assert!(!pal.matches().is_empty(), "the service is in the index");
}

#[test]
fn the_source_view_never_prints_a_credential() {
    use super::viewer::{is_secret, source_lines};
    // Found on a live host: a private registry's `password` — a real GitHub
    // token — printed in full on the Source & build screen. The view already
    // skipped the `token` and `env` KEYS of the service, which was the right
    // instinct applied to a list that missed one. The edit form for this very
    // field has always masked it, so one fact was rendered two ways.
    let svc = json!({
        "source": {
            "type": "image",
            "image": "ghcr.io/acme/app:latest",
            "username": "acme-bot",
            "password": "ghp_notarealtokenbutlookslikeone1234567890"
        },
        "build": {}, "deploy": { "replicas": 1 }, "resources": {}
    });
    let text = source_lines(&svc).join("\n");
    assert!(
        !text.contains("ghp_notarealtokenbutlookslikeone1234567890"),
        "a credential reached the screen:\n{text}"
    );
    // Masked, not dropped: the operator still needs to know a password IS set.
    assert!(text.contains("password: ••••••••"), "{text}");
    // Everything that is not a secret still shows.
    assert!(text.contains("username: acme-bot") && text.contains("ghcr.io/acme/app"));

    // Matched by NAME, so a secret field added by a future EasyPanel arrives
    // hidden rather than exposed.
    for k in [
        "password",
        "apiToken",
        "REGISTRY_SECRET",
        "privateKey",
        "apiKey",
    ] {
        assert!(is_secret(k), "{k} should be treated as a credential");
    }
    for k in ["image", "username", "replicas", "command"] {
        assert!(!is_secret(k), "{k} is not a credential");
    }
}

#[test]
fn a_viewer_says_what_is_there_and_what_is_not() {
    // These formatters used to live inside the function that fetched their input,
    // so none of this could be checked without an HTTP server.
    use super::viewer::{backups_lines, mounts_lines, ports_lines, redirects_lines};
    // An empty collection is a sentence, never a blank pane: the reader must be
    // able to tell "nothing here" from "it failed to load".
    assert_eq!(
        ports_lines(&json!([]))[0],
        "No ports yet — press n to add one"
    );
    assert_eq!(redirects_lines(&json!({}))[0], "No redirects");
    assert_eq!(backups_lines(&json!([]))[0], "No database backups");

    let ports = ports_lines(&json!([{ "protocol": "tcp", "published": 8080, "target": 80 }]));
    assert!(ports[0].contains("tcp 8080->80"), "{:?}", ports);
    // A bind mount reads host -> container; a volume reads name -> container.
    let mounts = mounts_lines(&json!([
        { "type": "bind", "hostPath": "/srv/a", "mountPath": "/app/a" },
        { "type": "volume", "name": "data", "mountPath": "/var/lib/x" }
    ]));
    assert!(mounts[0].contains("/srv/a -> /app/a"), "{:?}", mounts);
    assert!(mounts[1].contains("data -> /var/lib/x"), "{:?}", mounts);
    // The backups header only earns its line when there is something under it.
    let backups = backups_lines(
        &json!([{ "databaseName": "app", "schedule": "0 3 * * *", "enabled": true }]),
    );
    assert!(backups[0].starts_with("Database"), "{:?}", backups);
    assert!(
        backups[1].contains("app") && backups[1].ends_with("on"),
        "{:?}",
        backups
    );
}

#[test]
fn a_failed_terminal_connection_never_prints_the_url_it_tried() {
    use crate::container::connect_failure;
    use tungstenite::error::UrlError;
    // The WebSocket URL carries ?token={api token}, and for a database shell the
    // base64 of a command containing the root password. tungstenite renders this
    // variant as "Unable to connect to {the whole URI}", and that went straight
    // to the status line — so a firewalled port or a panel outage printed the API
    // token on screen and left it in the scrollback.
    let secret_url = "wss://panel.example.com/ws/containerShell?container=abc\
                      &command=TVlTUUxfUFdEPSdodW50ZXIyJw==&token=ep_realtokenvalue";
    let e = tungstenite::Error::Url(UrlError::UnableToConnect(secret_url.into()));
    let shown = connect_failure(&e);
    assert!(
        !shown.contains("ep_realtokenvalue"),
        "token leaked: {shown}"
    );
    assert!(!shown.contains("TVlTUUxfUFdE"), "command leaked: {shown}");
    assert!(!shown.contains("wss://"), "the URL leaked: {shown}");
    assert!(shown.contains("could not reach the panel"), "{shown}");

    // The variants that do NOT carry the URI keep their message: those are the
    // useful ones, and blanket-suppressing them would trade one problem for
    // another.
    let io = tungstenite::Error::Io(std::io::Error::new(
        std::io::ErrorKind::ConnectionRefused,
        "connection refused",
    ));
    assert!(connect_failure(&io).contains("connection refused"));
}

#[test]
fn an_editor_temp_file_is_readable_only_by_its_owner() {
    // On Linux temp_dir() is the shared /tmp and the default mode would let every
    // account on the box read a service's whole environment. The config files
    // next door already chmod 0600; this writer did not.
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("easypanel-p-s.env");
    super::write_private(&path, "SECRET=hunter2\n").unwrap();
    assert_eq!(std::fs::read_to_string(&path).unwrap(), "SECRET=hunter2\n");
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let mode = std::fs::metadata(&path).unwrap().permissions().mode() & 0o777;
        assert_eq!(mode, 0o600, "world-readable secrets: {mode:o}");
    }
    // A stale file from an earlier session is replaced, not written through.
    super::write_private(&path, "SECRET=changed\n").unwrap();
    assert_eq!(std::fs::read_to_string(&path).unwrap(), "SECRET=changed\n");
}

#[test]
fn a_table_cuts_its_flexible_column_with_an_ellipsis() {
    use super::render::{flex_width, render_table};
    use ratatui::backend::TestBackend;
    use ratatui::layout::Constraint;
    use ratatui::widgets::TableState;
    use ratatui::Terminal;
    // Found on the Monitor tab: two storage paths differing only in their last
    // character both rendered as ".../mysql-r", so the column that identifies the
    // row said nothing while looking complete. Every table drawn through
    // render_table now cuts its one flexible column here rather than letting
    // ratatui clip it at the pane edge.
    let widths = [
        Constraint::Length(20),
        Constraint::Length(18),
        Constraint::Length(11),
        Constraint::Min(20),
    ];
    let rows = vec![
        vec![
            "viding-co-db".into(),
            "mysql-r1".into(),
            "78.1 GB".into(),
            "/etc/easypanel/projects/viding-co-db/mysql-r1".into(),
        ],
        vec![
            "viding-co-db".into(),
            "mysql-r2".into(),
            "78.2 GB".into(),
            "/etc/easypanel/projects/viding-co-db/mysql-r2".into(),
        ],
    ];
    // A selected row, as every real table has: the highlight symbol takes two
    // columns, and the first version of this test forgot that — the path fitted
    // exactly, so the test passed whether or not anything was cut.
    let mut state = TableState::default().with_selected(Some(0));
    let mut t = Terminal::new(TestBackend::new(100, 8)).unwrap();
    t.draw(|f| {
        let area = f.area();
        render_table(
            f,
            area,
            " Storage ".into(),
            &["Project", "Service", "Size", "Path"],
            &widths,
            rows,
            &mut state,
            ratatui::style::Color::Blue,
            |_, _| None,
        )
    })
    .unwrap();
    let screen: String = t
        .backend()
        .buffer()
        .content()
        .chunks(100)
        .map(|r| r.iter().map(|c| c.symbol()).collect::<String>() + "\n")
        .collect();
    println!(
        "RENDERED:
{screen}"
    );
    for line in screen.lines().filter(|l| l.contains("/etc/easypanel")) {
        // The WHOLE path, not just its tail: "mysql-r1" also appears in the
        // Service column, so checking for that alone passed even while the path
        // was being cut — the first two versions of this test proved nothing.
        assert!(
            line.contains("/etc/easypanel/projects/viding-co-db/mysql-r1")
                || line.contains("/etc/easypanel/projects/viding-co-db/mysql-r2")
                || line.contains('…'),
            "a path was cut with no ellipsis: {line}"
        );
    }

    // Two flexible columns: ratatui shares the slack and no single width exists,
    // so nothing is cut rather than something being cut at a guessed width.
    assert_eq!(
        flex_width(&[Constraint::Min(10), Constraint::Min(10)], 80, true),
        None
    );
    // The highlight symbol only takes room when there is a selection.
    let one = [Constraint::Length(10), Constraint::Min(10)];
    assert_eq!(
        flex_width(&one, 80, false).unwrap() - flex_width(&one, 80, true).unwrap(),
        2
    );
}

#[test]
fn a_failed_action_does_not_look_like_a_successful_one() {
    use ratatui::backend::TestBackend;
    use ratatui::style::Color;
    use ratatui::Terminal;
    // Ground truth from a live panel: of the last 200 actions, 181 were `done`,
    // 16 `killed` and 3 `error` — a tenth of the screen was findings, drawn in
    // exactly the same grey as the successes. Colour carries state on every other
    // screen in this app; the history screen was the one that said nothing.
    let mut app = App::new("s".into(), vec![]);
    app.screen = Screen::Actions;
    app.actions = ["done", "killed", "error"]
        .iter()
        .map(|st| {
            json!({ "id": format!("a-{st}"), "status": st, "type": "deploy",
                    "projectName": "p", "serviceName": "svc",
                    "description": "Deploy service", "createdAt": "2026-07-21T00:00:00.000Z" })
        })
        .collect();
    app.actions_state.select(Some(0));

    let mut t = Terminal::new(TestBackend::new(120, 10)).unwrap();
    t.draw(|f| ui(f, &mut app)).unwrap();
    let buf = t.backend().buffer().clone();

    // The colour of the first letter of each status, read off the screen.
    let colour_of = |word: &str| {
        let w = 120usize;
        let (row_i, _) = buf
            .content()
            .chunks(w)
            .enumerate()
            .find(|(_, r)| {
                r.iter()
                    .map(|c| c.symbol())
                    .collect::<String>()
                    .contains(word)
            })
            .unwrap_or_else(|| panic!("no row for {word}"));
        let row: Vec<_> = buf.content().chunks(w).nth(row_i).unwrap().to_vec();
        let col = row
            .iter()
            .map(|c| c.symbol())
            .collect::<String>()
            .find(word)
            .unwrap();
        row[col].fg
    };

    let (done, killed, error) = (colour_of("done"), colour_of("killed"), colour_of("error"));
    assert_ne!(
        done, killed,
        "a killed action must not look like a done one"
    );
    assert_ne!(done, error, "a failed action must not look like a done one");
    // The same palette the rest of the app uses: green succeeded, yellow was
    // stopped on purpose, red failed on its own.
    assert_eq!(done, Color::Indexed(2));
    assert_eq!(killed, Color::Indexed(3));
    assert_eq!(error, Color::Indexed(196));
}

#[test]
fn a_domain_whose_service_is_gone_is_marked_and_counted() {
    use ratatui::backend::TestBackend;
    use ratatui::Terminal;
    let mut app = App::new("s".into(), vec![]);
    app.screen = Screen::Domains;
    let dom = |svc: &str| {
        json!({ "id": format!("d-{svc}"), "host": format!("{svc}.test"), "path": "/",
                "https": true, "destinationType": "service",
                "serviceDestination": { "projectName": "shop", "serviceName": svc,
                                        "port": 80, "protocol": "http", "path": "/" } })
    };
    app.domains = vec![dom("api"), dom("retired")];
    app.domains_state.select(Some(0));

    let draw = |app: &mut App| {
        let mut t = Terminal::new(TestBackend::new(110, 10)).unwrap();
        t.draw(|f| ui(f, app)).unwrap();
        let s: String = t
            .backend()
            .buffer()
            .content()
            .chunks(110)
            .map(|r| r.iter().map(|c| c.symbol()).collect::<String>() + "\n")
            .collect();
        s
    };

    // Before the service list arrives NOTHING is judged: an empty list would
    // condemn every domain on the panel at once, which on a real host is 713
    // confident wrong answers.
    let loading = draw(&mut app);
    assert!(
        !loading.contains('✗'),
        "judged before the services loaded:\n{loading}"
    );

    app.all_services = vec![svc("shop", "api", "app")];
    let loaded = draw(&mut app);
    // The dead one is marked and counted; the live one is left alone.
    assert!(
        loaded.contains("1 pointing at a service that is gone"),
        "{loaded}"
    );
    let dead_row = loaded
        .lines()
        .find(|l| l.contains("retired.test"))
        .unwrap_or_default();
    assert!(
        dead_row.contains('✗'),
        "the dead route is not marked: {dead_row}"
    );
    let live_row = loaded
        .lines()
        .find(|l| l.contains("api.test"))
        .unwrap_or_default();
    assert!(
        !live_row.contains('✗'),
        "a live route was marked: {live_row}"
    );
}

#[test]
fn failures_only_hides_the_clean_successes_and_says_so() {
    use ratatui::crossterm::event::KeyCode;
    let (tx, _rx) = std::sync::mpsc::channel();
    let mut app = App::new("s".into(), vec![]);
    app.screen = Screen::Actions;
    let act = |id: &str, status: &str| {
        json!({ "id": id, "status": status, "type": "deploy",
                "projectName": "p", "serviceName": "svc",
                "description": "Deploy service", "createdAt": "2026-07-21T00:00:00.000Z" })
    };
    app.actions = vec![
        act("a", "done"),
        act("b", "killed"),
        act("c", "done"),
        act("d", "error"),
        act("e", "running"),
    ];
    app.actions_state.select(Some(3));

    // Off by default: everything shows.
    assert_eq!(app.visible_actions().len(), 5);

    // `f` keeps only what did not finish cleanly — killed, error, running — and
    // drops both `done` rows. A text search could not do this: "error" typed into
    // the filter also matches a commit message containing the word.
    app.on_key(KeyCode::Char('f'), &tx);
    let shown: Vec<String> = app
        .visible_actions()
        .iter()
        .map(|a| field(a, "/status"))
        .collect();
    assert_eq!(shown, vec!["killed", "error", "running"]);
    assert!(app.actions_failures_only);

    // The title must show the SHOWN count, not the raw total: "(5)" above three
    // rows would be the missing-data lie the announcement exists to prevent.
    {
        use ratatui::backend::TestBackend;
        use ratatui::Terminal;
        let mut t = Terminal::new(TestBackend::new(100, 12)).unwrap();
        t.draw(|f| ui(f, &mut app)).unwrap();
        let top: String = t
            .backend()
            .buffer()
            .content()
            .chunks(100)
            .map(|r| r.iter().map(|c| c.symbol()).collect::<String>())
            .find(|l| l.contains("failures only"))
            .unwrap_or_default();
        assert!(
            top.contains("(3/5)") && top.contains("failures only"),
            "{top}"
        );
    }
    // The selection was index 3 of the old list; it must not still point past the
    // shorter one.
    assert!(app.selected_action_id().is_some());

    // `f` again restores the full list.
    app.on_key(KeyCode::Char('f'), &tx);
    assert_eq!(app.visible_actions().len(), 5);
    assert!(!app.actions_failures_only);
}

#[test]
fn comparing_two_services_clears_the_marks_so_esc_is_not_a_dead_end() {
    use ratatui::crossterm::event::KeyCode;
    let (tx, rx) = std::sync::mpsc::channel();
    let mut app = App::new("s".into(), vec![]);
    app.all_services = vec![svc("prod", "api", "app"), svc("staging", "api", "app")];
    app.marked.insert(("prod".into(), "api".into()));
    app.marked.insert(("staging".into(), "api".into()));

    app.diff_marked(&tx);
    // The request carries both, ordered as bulk_targets sorts them.
    match rx.try_recv().unwrap() {
        Req::DiffServices { a, b } => {
            assert_eq!((a.0.as_str(), a.1.as_str()), ("prod", "api"));
            assert_eq!((b.0.as_str(), b.1.as_str()), ("staging", "api"));
        }
        _ => panic!("expected a diff request"),
    }
    // The marks are consumed. Left set, the global "Esc clears marks" handler
    // shadows the viewer's Esc, so leaving the diff would take two presses and
    // read as a dead end.
    assert!(
        app.marked.is_empty(),
        "marks must not outlive the comparison"
    );

    // The result opens a viewer, and one Esc from it returns to Services.
    app.handle(
        Resp::Viewer(" Diff ".into(), vec!["prod/api   vs   staging/api".into()]),
        &tx,
    );
    assert!(app.screen == Screen::Viewer);
    app.on_key(KeyCode::Esc, &tx);
    assert!(app.screen != Screen::Viewer, "one Esc must leave the diff");
}

#[test]
fn comparing_across_hosts_asks_the_event_loop_to_resolve_the_target_token() {
    let (tx, _rx) = std::sync::mpsc::channel();
    let mut app = App::new(
        "prod".into(),
        vec![
            ("prod".into(), "https://prod".into()),
            ("staging".into(), "https://staging".into()),
        ],
    );
    app.projects = vec!["shop".into()];
    app.all_services = vec![svc("shop", "api", "app")];
    app.screen = Screen::Projects;
    app.services_table.select(Some(1)); // the service row, under its project header

    app.open_diff_across_form();
    let form = app.form.as_ref().expect("the compare form");
    // Only OTHER hosts are offered — comparing prod with prod is not a thing.
    assert!(form.title.contains("shop/api"));

    // Pick the other host and submit.
    if let Some(f) = app.form.as_mut() {
        f.fields[0].value = "staging".into();
    }
    app.submit_form(&tx);
    // The token lives in the ServerConfig, which only the event loop can read, so
    // the App hands off a request naming the server rather than fetching itself.
    let req = app.diff_across_req.as_ref().expect("a cross-host request");
    assert_eq!(req.local, ("shop".into(), "api".into(), "app".into()));
    assert_eq!(req.target_server, "staging");

    // With the server field left empty it refuses rather than comparing prod
    // with itself. (A lone other host is auto-selected, which is fine — this
    // forces the empty case the guard exists for.)
    app.diff_across_req = None;
    app.open_diff_across_form();
    if let Some(f) = app.form.as_mut() {
        f.fields[0].value = String::new();
    }
    app.submit_form(&tx);
    assert!(app.diff_across_req.is_none());
    assert!(app.status.contains("Choose"), "{}", app.status);
}

#[test]
fn a_viewer_advertises_horizontal_scroll_only_when_a_line_overflows() {
    use super::render::viewer_overflows;
    let mut app = App::new("s".into(), vec![]);
    app.viewer.lines = vec![
        "short".into(),
        "a line that is definitely wider than a narrow pane will ever be".into(),
    ];
    // Inner width = area_width - 2. At 30 cols the long line overflows.
    assert!(viewer_overflows(&app, 30));
    // At a width that fits every line, nothing to advertise.
    assert!(!viewer_overflows(&app, 200));

    // The offset is added back: once scrolled far enough that the reach clears
    // the longest line, there is no longer more to show.
    let longest = app
        .viewer
        .lines
        .iter()
        .map(|l| l.chars().count())
        .max()
        .unwrap();
    app.viewer.hscroll = 0;
    assert!(viewer_overflows(&app, 30)); // inner 28 < longest
    app.viewer.hscroll = longest as u16; // reach 28 + longest > longest
    assert!(!viewer_overflows(&app, 30));
}

#[test]
fn the_diff_footer_shows_the_scroll_hint_at_a_narrow_width() {
    use ratatui::backend::TestBackend;
    use ratatui::Terminal;
    let mut app = App::new("s".into(), vec![]);
    app.screen = Screen::Viewer;
    app.viewer.title = "Diff".into();
    app.viewer.lines =
        vec!["source.image   ghcr.io/acme/a-very-long-image-name:latest  →  other/thing:v2".into()];
    let footer = |app: &mut App, w: u16| {
        let mut t = Terminal::new(TestBackend::new(w, 8)).unwrap();
        t.draw(|f| ui(f, app)).unwrap();
        t.backend()
            .buffer()
            .content()
            .chunks(w as usize)
            .map(|r| r.iter().map(|c| c.symbol()).collect::<String>())
            .collect::<Vec<_>>()
            .join("\n")
    };
    // Narrow: the line is cut, so the hint is shown.
    assert!(
        footer(&mut app, 50).contains("scroll"),
        "no scroll hint when cut"
    );
    // Wide: it fits, so no hint clutters the footer.
    assert!(
        !footer(&mut app, 120).contains("scroll"),
        "hint shown when it fits"
    );
}

#[test]
fn comparing_a_whole_project_across_hosts_resolves_the_target_token() {
    let (tx, _rx) = std::sync::mpsc::channel();
    let mut app = App::new(
        "prod".into(),
        vec![
            ("prod".into(), "https://prod".into()),
            ("staging".into(), "https://staging".into()),
        ],
    );
    app.projects = vec!["shop".into()];
    app.all_services = vec![svc("shop", "api", "app")];
    app.screen = Screen::Projects;
    app.services_table.select(Some(0)); // the project header

    app.open_diff_project_across_form();
    let form = app.form.as_ref().expect("the project compare form");
    assert!(form.title.contains("project shop"), "{}", form.title);

    if let Some(f) = app.form.as_mut() {
        f.fields[0].value = "staging".into();
    }
    app.submit_form(&tx);
    // The token lives only in the ServerConfig, so the App hands off a request
    // naming the server rather than fetching itself.
    let req = app.diff_project_across_req.as_ref().expect("a request");
    assert_eq!(req.project, "shop");
    assert_eq!(req.target_server, "staging");

    // Empty server refuses rather than comparing prod with itself.
    app.diff_project_across_req = None;
    app.open_diff_project_across_form();
    if let Some(f) = app.form.as_mut() {
        f.fields[0].value = String::new();
    }
    app.submit_form(&tx);
    assert!(app.diff_project_across_req.is_none());
    assert!(app.status.contains("Choose"), "{}", app.status);
}

// ---------- Cloudflare workspace ----------

#[test]
fn the_w_switch_moves_between_workspaces_and_esc_returns() {
    // `W` opens a switch menu (the shared action-menu machinery); choosing
    // Cloudflare enters that workspace, and Esc on its account screen returns to
    // EasyPanel. The Screen underneath is untouched throughout.
    let (tx, _rx) = std::sync::mpsc::channel();
    let mut app = App::new("s".into(), vec![]);
    app.screen = Screen::Projects;
    assert!(app.workspace == Workspace::Easypanel);

    app.on_key(KeyCode::Char('W'), &tx);
    let menu = app.menu.as_ref().expect("W opens the workspace menu");
    assert_eq!(menu.items.len(), 2, "EasyPanel + Cloudflare");
    // Highlight Cloudflare (second item) and run it.
    app.on_key(KeyCode::Down, &tx);
    app.on_key(KeyCode::Enter, &tx);
    assert!(app.workspace == Workspace::Cloudflare);
    assert!(app.menu.is_none());

    // Esc on the Cloudflare root returns to EasyPanel — the Screen is still Projects.
    app.on_key(KeyCode::Esc, &tx);
    assert!(app.workspace == Workspace::Easypanel);
    assert!(app.screen == Screen::Projects);
}

#[test]
fn a_digit_key_is_inert_in_the_cloudflare_workspace() {
    // ISOLATION: while in the Cloudflare workspace, the EasyPanel digit tab-jumps
    // (and Tab, and ←/→) must NOT act — they belong to the other workspace.
    let (tx, _rx) = std::sync::mpsc::channel();
    let mut app = App::new("s".into(), vec![]);
    app.screen = Screen::Projects;
    app.set_workspace(Workspace::Cloudflare);

    app.on_key(KeyCode::Char('3'), &tx); // Maintenance in EasyPanel
    app.on_key(KeyCode::Tab, &tx);
    app.on_key(KeyCode::Right, &tx);
    assert!(
        app.screen == Screen::Projects,
        "EasyPanel tab keys must not act in the Cloudflare workspace"
    );
    assert!(app.workspace == Workspace::Cloudflare);
}

#[test]
fn help_opens_in_the_cloudflare_workspace_and_documents_cf_keys() {
    // `?` was swallowed by the isolation gate — in the CF workspace it did nothing,
    // unlike EasyPanel. It now opens the help there too, and the "this screen" section
    // is sourced from cf_screen_keys (per CfScreen), not the stale EasyPanel Screen.
    let (tx, _rx) = std::sync::mpsc::channel();
    let mut app = App::new("s".into(), vec![]);
    app.set_workspace(Workspace::Cloudflare);
    assert!(!app.help);

    app.on_key(KeyCode::Char('?'), &tx);
    assert!(app.help, "? opens help in the Cloudflare workspace");

    let zones: Vec<&str> = cf_screen_keys(CfScreen::Zones)
        .iter()
        .map(|k| k.0)
        .collect();
    assert!(
        zones.contains(&"a") && zones.contains(&"Enter"),
        "zones help lists the account picker and records keys"
    );
    let records: Vec<&str> = cf_screen_keys(CfScreen::Records)
        .iter()
        .map(|k| k.0)
        .collect();
    assert!(
        records.contains(&"e") && records.contains(&"Space"),
        "records help lists the edit and bulk keys"
    );
}

#[test]
fn cf_help_anywhere_and_mouse_omit_keys_that_are_inert_in_the_workspace() {
    // The CF workspace has no EasyPanel tabs / `:` palette / `s` server picker and no
    // tab-click — so the help's "Anywhere" and "Mouse" sections must not advertise
    // them (help that lies is worse than no help). Right-click IS live now: it opens
    // the Zones row menu, so the Mouse section documents it.
    let g: Vec<&str> = CF_GLOBAL_KEYS.iter().map(|k| k.0).collect();
    assert!(g.contains(&"W") && g.contains(&"?") && g.contains(&"r"));
    for stale in ["1-8 / Tab / ←→", "s", ":"] {
        assert!(
            !g.contains(&stale),
            "CF 'Anywhere' must not advertise `{stale}`"
        );
    }
    let m: Vec<&str> = CF_MOUSE_KEYS.iter().map(|k| k.0).collect();
    assert!(m.contains(&"Click row") && m.contains(&"Scroll"));
    assert!(
        m.contains(&"Right click"),
        "right-click opens the zone action menu, so the Mouse help lists it"
    );
    assert!(
        !m.contains(&"Click tab"),
        "the one product tab isn't click-switchable yet"
    );
}

#[test]
fn the_cloudflare_screen_reports_the_empty_state() {
    use ratatui::backend::TestBackend;
    use ratatui::Terminal;

    let (tx, _rx) = std::sync::mpsc::channel();
    let mut app = App::new("s".into(), vec![]);
    app.set_workspace(Workspace::Cloudflare);
    assert!(app.cf_empty(), "no accounts seeded");

    let mut term = Terminal::new(TestBackend::new(90, 12)).unwrap();
    term.draw(|f| super::render::ui(f, &mut app)).unwrap();
    let screen = term
        .backend()
        .buffer()
        .content()
        .chunks(90)
        .map(|r| r.iter().map(|c| c.symbol()).collect::<String>())
        .collect::<Vec<_>>()
        .join("\n");
    assert!(
        screen.contains(CF_EMPTY_HINT),
        "the empty Cloudflare Zones home must invite adding an account:\n{screen}"
    );
    // `a` opens the account picker even when empty; `n` there opens the add form —
    // never a dead end.
    app.on_key(KeyCode::Char('a'), &tx);
    assert!(app.cf_picker.is_some(), "a opens the account picker");
    app.on_key(KeyCode::Char('n'), &tx);
    assert!(
        app.form.is_some(),
        "n opens the add-account form from the picker"
    );
}

#[test]
fn adding_a_cloudflare_account_via_the_app_path_updates_the_config() {
    use crate::cloudflare::CloudflareAccount;
    use crate::config::CloudflareConfig;

    let (tx, _rx) = std::sync::mpsc::channel();
    let mut app = App::new("s".into(), vec![]);
    app.set_workspace(Workspace::Cloudflare);

    // The add form is a LOCAL submit: it stages a CfAction rather than sending a
    // network request. Reached via the account picker (`a` then `n`).
    app.on_key(KeyCode::Char('a'), &tx);
    app.on_key(KeyCode::Char('n'), &tx);
    {
        let f = app.form.as_mut().expect("the add form");
        f.fields[0].value = "prod".into();
        f.fields[1].value = "tok-secret".into();
        f.fields[2].value = "acc-123".into();
    }
    app.submit_form(&tx);
    assert!(app.form.is_none(), "submit closes the form");
    assert!(
        app.cf_action.is_some(),
        "the submit stages a local account add, not a worker request"
    );

    // Resolve it against a throwaway config — the path the event loop takes.
    let dir = std::env::temp_dir().join(format!("epcf-{}", std::process::id()));
    let _ = std::fs::create_dir_all(&dir);
    let cfg = CloudflareConfig::new(dir.join("cloudflare.json"));
    let msg = super::apply_cf_action(&cfg, app.cf_action.take().unwrap()).expect("saved");
    assert!(msg.contains("prod"), "{msg}");

    let stored = cfg.list();
    assert_eq!(stored.len(), 1);
    assert_eq!(stored[0].name, "prod");
    assert_eq!(stored[0].api_token, "tok-secret");
    assert_eq!(stored[0].account_id.as_deref(), Some("acc-123"));
    assert!(stored[0].default, "the first account becomes the default");

    // Re-seeding is what the event loop does next; the screen then shows it.
    app.cf.accounts = stored;
    assert!(!app.cf_empty());
    let _ = std::fs::remove_dir_all(&dir);

    // Guard the CloudflareAccount shape the form fills.
    let _ = CloudflareAccount {
        name: "x".into(),
        api_token: "y".into(),
        account_id: None,
        default: false,
    };
}

#[test]
fn adding_the_first_account_auto_activates_it() {
    // After a CfAction::Add is resolved with no account active yet, the event loop
    // auto-activates the just-added account and loads its zones — otherwise the
    // Zones home would sit on "— zones" as if nothing happened. This exercises the
    // reused entry path (enter_cloudflare) with the newly-seeded account.
    let (tx, rx) = std::sync::mpsc::channel();
    let mut app = App::new("s".into(), vec![]);
    app.set_workspace(Workspace::Cloudflare);
    assert!(app.cf.active.is_none(), "no account active yet");

    // The event loop re-seeds accounts from the saved config after an Add; the
    // first account is the default.
    app.cf.accounts = vec![cf_account()];
    app.enter_cloudflare(&tx);

    assert!(
        app.cf.active.is_some(),
        "the just-added first account becomes active"
    );
    assert_eq!(app.cf.screen, CfScreen::Zones);
    assert!(
        matches!(rx.try_recv(), Ok(Req::Cf(CfReq::Zones { .. }))),
        "its zones are loaded so the user lands on them"
    );
}

// ---------- Cloudflare zones & records ----------

fn cf_account() -> crate::cloudflare::CloudflareAccount {
    crate::cloudflare::CloudflareAccount {
        name: "prod".into(),
        api_token: "tok".into(),
        account_id: Some("acc-1".into()),
        default: true,
    }
}

fn cf_zone(id: &str, name: &str) -> crate::cloudflare::Zone {
    crate::cloudflare::Zone {
        id: id.into(),
        name: name.into(),
        status: "active".into(),
    }
}

fn cf_record(id: &str, kind: &str, name: &str, content: &str) -> crate::cloudflare::Record {
    crate::cloudflare::Record {
        id: id.into(),
        kind: kind.into(),
        name: name.into(),
        content: content.into(),
        ttl: 1,
        proxied: false,
        priority: None,
    }
}

#[test]
fn cf_home_is_zones_and_the_account_picker_switches_accounts() {
    // Entering the workspace lands on the Zones home of the active (default)
    // account; `a` opens the account picker; Enter there activates the account.
    let (tx, _rx) = std::sync::mpsc::channel();
    let mut app = App::new("s".into(), vec![]);
    app.cf.accounts = vec![cf_account()];
    app.set_workspace(Workspace::Cloudflare);
    assert_eq!(app.cf.screen, CfScreen::Zones, "home is the Zones list");
    assert!(
        app.cf.active.is_some(),
        "the default account is active on entry"
    );

    // `a` opens the picker (mirrors the server `s` picker); Enter activates.
    app.on_key(KeyCode::Char('a'), &tx);
    assert!(app.cf_picker.is_some(), "a opens the account picker");
    app.on_key(KeyCode::Enter, &tx);
    assert!(app.cf_picker.is_none(), "Enter closes the picker");
    assert!(app.cf_action.is_some(), "selecting stages 'make default'");
    assert_eq!(app.cf.screen, CfScreen::Zones);
}

#[test]
fn cf_navigation_drills_in_and_esc_walks_back() {
    // Enter on a zone opens its Records; Esc walks back one level each time:
    // Records → Zones home → EasyPanel. State transitions only — no network.
    let (tx, _rx) = std::sync::mpsc::channel();
    let mut app = App::new("s".into(), vec![]);
    app.cf.accounts = vec![cf_account()];
    app.set_workspace(Workspace::Cloudflare);
    assert_eq!(app.cf.screen, CfScreen::Zones);

    // Seed a zone (the worker would) and drill into its Records.
    app.cf.zones = vec![cf_zone("z1", "example.com")];
    app.cf.zones_row.select(Some(0));
    app.on_key(KeyCode::Enter, &tx);
    assert_eq!(app.cf.screen, CfScreen::Records);
    assert_eq!(
        app.cf.current_zone.as_ref().map(|z| z.name.as_str()),
        Some("example.com")
    );

    // Esc: Records → Zones home → EasyPanel.
    app.on_key(KeyCode::Esc, &tx);
    assert_eq!(app.cf.screen, CfScreen::Zones);
    app.on_key(KeyCode::Esc, &tx);
    assert!(app.workspace == Workspace::Easypanel);
}

#[test]
fn cf_wheel_scrolls_the_zones_list_not_the_hidden_easypanel_table() {
    // In the CF workspace the mouse must drive the CF list under the cursor. The
    // wheel used to move the hidden EasyPanel table (self.screen) while the zones
    // list sat still. State transitions only — no paint, so seed table_area.
    use ratatui::crossterm::event::{MouseEvent, MouseEventKind};
    let (tx, _rx) = std::sync::mpsc::channel();
    let mut app = App::new("s".into(), vec![]);
    // A live EasyPanel table behind the workspace — it must NOT move.
    app.screen = Screen::Projects;
    app.services_table.select(Some(0));

    app.workspace = Workspace::Cloudflare;
    app.cf.screen = CfScreen::Zones;
    app.cf.zones = (0..40)
        .map(|i| cf_zone(&format!("z{i}"), &format!("d{i}.com")))
        .collect();
    app.cf.zones_row.select(Some(0));
    // The pane the CF table paints into (set by render_cf_zones).
    app.table_area = ratatui::layout::Rect::new(0, 0, 80, 20);

    app.on_mouse(
        MouseEvent {
            kind: MouseEventKind::ScrollDown,
            column: 5,
            row: 5,
            modifiers: ratatui::crossterm::event::KeyModifiers::NONE,
        },
        &tx,
    );

    assert!(
        app.cf.zones_row.offset() > 0,
        "the wheel must move the CF zones viewport"
    );
    assert!(
        app.cf.zones_row.selected().is_some_and(|i| i > 0),
        "and carry the selection with it"
    );
    assert_eq!(
        app.services_table.selected(),
        Some(0),
        "the hidden EasyPanel table must not move"
    );
    assert_eq!(app.services_table.offset(), 0);
}

#[test]
fn cf_hover_and_click_select_the_row_under_the_cursor() {
    // Hover (Moved) and left-click both select the CF row under the cursor, exactly
    // as they do on the EasyPanel tables. Row math: table_area.y + 2 is the first
    // data row (top border + header).
    use ratatui::crossterm::event::{MouseButton, MouseEvent, MouseEventKind};
    let (tx, _rx) = std::sync::mpsc::channel();
    let mut app = App::new("s".into(), vec![]);
    app.workspace = Workspace::Cloudflare;
    app.cf.screen = CfScreen::Zones;
    app.cf.zones = (0..10)
        .map(|i| cf_zone(&format!("z{i}"), &format!("d{i}.com")))
        .collect();
    app.cf.zones_row.select(Some(0));
    app.table_area = ratatui::layout::Rect::new(0, 0, 80, 20);

    // Hover over the 3rd data row (idx 2).
    app.on_mouse(
        MouseEvent {
            kind: MouseEventKind::Moved,
            column: 5,
            row: 4,
            modifiers: ratatui::crossterm::event::KeyModifiers::NONE,
        },
        &tx,
    );
    assert_eq!(
        app.cf.zones_row.selected(),
        Some(2),
        "hover selects the CF row under the cursor"
    );

    // Left-click the 2nd data row (idx 1).
    app.on_mouse(
        MouseEvent {
            kind: MouseEventKind::Down(MouseButton::Left),
            column: 5,
            row: 3,
            modifiers: ratatui::crossterm::event::KeyModifiers::NONE,
        },
        &tx,
    );
    assert_eq!(
        app.cf.zones_row.selected(),
        Some(1),
        "left-click selects the CF row under the cursor"
    );
    // Click selects only — it must not drill into Records (that's Enter).
    assert_eq!(app.cf.screen, CfScreen::Zones);
}

#[test]
fn cf_space_opens_the_zone_action_menu_and_records_route_drills_in() {
    // Space on the Zones screen opens the row action menu (the CF mirror of
    // EasyPanel's row menu), and "Open DNS records" reaches Records — the same
    // flow as Enter. State transitions only — no paint.
    let (tx, _rx) = std::sync::mpsc::channel();
    let mut app = App::new("s".into(), vec![]);
    app.cf.active = Some(cf_account());
    app.set_workspace(Workspace::Cloudflare);
    app.cf.zones = vec![cf_zone("z1", "example.com")];
    app.cf.zones_row.select(Some(0));

    app.on_key(KeyCode::Char(' '), &tx);
    let labels: Vec<String> = app
        .menu
        .as_ref()
        .expect("Space opens the zone action menu")
        .items
        .iter()
        .map(|it| it.label.clone())
        .collect();
    assert!(
        labels.iter().any(|l| l == "Open DNS records"),
        "the menu offers the records route: {labels:?}"
    );
    assert!(
        labels.iter().any(|l| l == "Delete zone…"),
        "the menu offers the delete route: {labels:?}"
    );

    // Selecting "Open DNS records" (index 0) drills into the zone's Records.
    app.menu.as_mut().unwrap().state.select(Some(0));
    app.on_key(KeyCode::Enter, &tx);
    assert!(app.menu.is_none(), "activating a leaf item closes the menu");
    assert_eq!(app.cf.screen, CfScreen::Records);
    assert_eq!(
        app.cf.current_zone.as_ref().map(|z| z.name.as_str()),
        Some("example.com"),
        "the records route carries the selected zone"
    );
}

#[test]
fn cf_zone_menu_delete_opens_the_form_and_right_click_opens_the_menu() {
    // "Delete zone…" opens the typed-name delete form (the existing flow), and a
    // right click on a Zones row selects it and opens the SAME menu.
    use ratatui::crossterm::event::{MouseButton, MouseEvent, MouseEventKind};
    let (tx, _rx) = std::sync::mpsc::channel();
    let mut app = App::new("s".into(), vec![]);
    app.cf.active = Some(cf_account());
    app.set_workspace(Workspace::Cloudflare);
    app.cf.zones = vec![cf_zone("z1", "example.com"), cf_zone("z2", "other.com")];
    app.cf.zones_row.select(Some(0));
    app.table_area = ratatui::layout::Rect::new(0, 0, 80, 20);

    // Space → pick "Delete zone…" → the typed-name delete form opens.
    app.on_key(KeyCode::Char(' '), &tx);
    let del = app
        .menu
        .as_ref()
        .unwrap()
        .items
        .iter()
        .position(|it| it.label == "Delete zone…")
        .expect("the menu offers Delete zone…");
    app.menu.as_mut().unwrap().state.select(Some(del));
    app.on_key(KeyCode::Enter, &tx);
    assert!(
        matches!(
            app.form.as_ref().map(|f| &f.kind),
            Some(FormKind::CfZoneDelete { .. })
        ),
        "Delete zone… opens the zone-delete form"
    );
    app.form = None; // clear the form so the next menu isn't swallowed by it

    // Right-click on the 2nd data row (idx 1 = table_area.y + 2 + 1) selects it,
    // then opens the same action menu.
    app.on_mouse(
        MouseEvent {
            kind: MouseEventKind::Down(MouseButton::Right),
            column: 5,
            row: 3,
            modifiers: ratatui::crossterm::event::KeyModifiers::NONE,
        },
        &tx,
    );
    assert_eq!(
        app.cf.zones_row.selected(),
        Some(1),
        "right-click selects the row under the cursor"
    );
    let labels: Vec<String> = app
        .menu
        .as_ref()
        .expect("right-click opens the zone action menu")
        .items
        .iter()
        .map(|it| it.label.clone())
        .collect();
    assert!(
        labels.iter().any(|l| l == "Open DNS records")
            && labels.iter().any(|l| l == "Delete zone…"),
        "right-click opens the same menu: {labels:?}"
    );
}

#[test]
fn cf_filter_narrows_the_loaded_records() {
    // The CF-local filter narrows the already-loaded list client-side (a zone can
    // hold thousands), matching type/name/content, and never touches the EasyPanel
    // filter.
    let records = vec![
        cf_record("r1", "A", "api.example.com", "1.1.1.1"),
        cf_record("r2", "CNAME", "www.example.com", "example.com"),
        cf_record("r3", "TXT", "example.com", "v=spf1"),
    ];
    assert_eq!(filter_records(&records, "").len(), 3, "empty keeps all");
    assert_eq!(filter_records(&records, "api").len(), 1, "by name");
    assert_eq!(filter_records(&records, "cname").len(), 1, "by type");
    assert_eq!(filter_records(&records, "spf1").len(), 1, "by content");

    let (tx, _rx) = std::sync::mpsc::channel();
    let mut app = App::new("s".into(), vec![]);
    app.cf.active = Some(cf_account());
    app.cf.current_zone = Some(cf_zone("z1", "example.com"));
    // set_workspace lands on Zones; switch to the Records drill-in afterwards.
    app.set_workspace(Workspace::Cloudflare);
    app.cf.screen = CfScreen::Records;
    app.cf.records = records;
    app.cf.records_row.select(Some(0));

    // Type "/api" into the CF filter; only the one record remains, and the
    // EasyPanel filter is untouched.
    app.on_key(KeyCode::Char('/'), &tx);
    for c in "api".chars() {
        app.on_key(KeyCode::Char(c), &tx);
    }
    assert_eq!(app.cf_records_shown().len(), 1);
    assert!(app.filter.is_empty(), "EasyPanel filter stays isolated");
}

#[test]
fn the_cf_product_tab_bar_renders_with_dns_active() {
    use ratatui::backend::TestBackend;
    use ratatui::Terminal;

    let mut app = App::new("s".into(), vec![]);
    app.cf.accounts = vec![cf_account()];
    app.set_workspace(Workspace::Cloudflare);
    assert_eq!(app.cf.screen, CfScreen::Zones);

    let mut term = Terminal::new(TestBackend::new(90, 12)).unwrap();
    term.draw(|f| super::render::ui(f, &mut app)).unwrap();
    let rows: Vec<String> = term
        .backend()
        .buffer()
        .content()
        .chunks(90)
        .map(|r| r.iter().map(|c| c.symbol()).collect::<String>())
        .collect();

    // The border title still names the active account; the header's SECOND line
    // (inside the border) is the product tab bar, with the DNS tab present.
    assert!(
        rows[0].contains("Cloudflare — prod"),
        "header title names the account:\n{}",
        rows[0]
    );
    assert!(
        rows[1].contains("DNS"),
        "the product tab bar sits on the header's second line:\n{}",
        rows[1]
    );

    // The per-screen key hints moved OUT of the header into the STATUS BAR (last
    // row) — the header's second line is ONLY the tab bar now.
    let status = rows.last().unwrap();
    assert!(
        status.contains("Enter records"),
        "the Zones key hints render in the status bar:\n{status}"
    );
    assert!(
        !rows[1].contains("Enter records"),
        "the header no longer carries the key hints:\n{}",
        rows[1]
    );
}

#[test]
fn cf_product_switch_keys_toggle_dns_and_r2_and_load_buckets() {
    // CF_PRODUCTS is the single source the tab bar and the switch keys share: DNS is
    // tab 1, R2 tab 2. A future product is one more row here (plus its enum variant).
    assert_eq!(CF_PRODUCTS.len(), 2);
    assert_eq!(CF_PRODUCTS[0], ("DNS", CfProduct::Dns));
    assert_eq!(CF_PRODUCTS[1], ("R2", CfProduct::R2));
    assert_eq!(CfProduct::Dns.index(), 0);
    assert_eq!(CfProduct::R2.index(), 1);
    // Cycling wraps between the two products.
    assert_eq!(CfProduct::Dns.next(), CfProduct::R2);
    assert_eq!(CfProduct::R2.next(), CfProduct::Dns);
    assert_eq!(CfProduct::Dns.prev(), CfProduct::R2);

    // In the workspace the tab keys act on the product, never on the EasyPanel
    // Screen or the workspace. Selecting R2 loads the active account's buckets.
    let (tx, rx) = std::sync::mpsc::channel();
    let mut app = App::new("s".into(), vec![]);
    app.cf.accounts = vec![cf_account()];
    app.set_workspace(Workspace::Cloudflare);
    assert_eq!(app.cf.product, CfProduct::Dns);

    // Tab → R2: the product flips and a buckets request is sent (account-scoped).
    app.on_key(KeyCode::Tab, &tx);
    assert_eq!(app.cf.product, CfProduct::R2);
    assert!(app.workspace == Workspace::Cloudflare);
    assert!(
        matches!(rx.try_recv(), Ok(Req::Cf(CfReq::R2Buckets { .. }))),
        "switching to R2 loads the account's buckets"
    );

    // `1` jumps back to DNS; `2` selects R2 again.
    app.on_key(KeyCode::Char('1'), &tx);
    assert_eq!(app.cf.product, CfProduct::Dns);
    app.on_key(KeyCode::Char('2'), &tx);
    assert_eq!(app.cf.product, CfProduct::R2);
}

fn cf_bucket(name: &str) -> crate::cloudflare::R2Bucket {
    crate::cloudflare::R2Bucket {
        name: name.into(),
        creation_date: "2024-01-02T03:04:05.000Z".into(),
        location: Some("weur".into()),
        storage_class: "Standard".into(),
        jurisdiction: "default".into(),
    }
}

#[test]
fn cf_buckets_key_handles_add_delete_and_filter() {
    // With R2 active and buckets loaded: `n` opens the add form, `x` opens the
    // typed-name delete form for the selected bucket, `/` starts the filter input.
    let (tx, _rx) = std::sync::mpsc::channel();
    let mut app = App::new("s".into(), vec![]);
    app.cf.accounts = vec![cf_account()];
    app.set_workspace(Workspace::Cloudflare);
    app.on_key(KeyCode::Tab, &tx); // → R2
    assert_eq!(app.cf.product, CfProduct::R2);

    app.cf.r2_buckets = vec![cf_bucket("assets"), cf_bucket("backups")];
    app.cf.r2_row.select(Some(1));

    // `/` opens the CF-local filter input.
    app.on_key(KeyCode::Char('/'), &tx);
    assert!(app.cf.filter_input, "/ starts the bucket filter");
    app.on_key(KeyCode::Esc, &tx);
    assert!(!app.cf.filter_input);

    // `n` opens the add-bucket form (the account has an account-id).
    app.on_key(KeyCode::Char('n'), &tx);
    assert!(app.form.is_some(), "n opens the add-bucket form");
    app.form = None;

    // `x` opens the typed-name delete form naming the SELECTED bucket.
    app.cf.r2_row.select(Some(1));
    app.on_key(KeyCode::Char('x'), &tx);
    let form = app.form.as_ref().expect("x opens the delete form");
    assert!(
        form.note.as_deref().unwrap_or("").contains("backups"),
        "the delete confirm names the selected bucket"
    );
}

#[test]
fn cf_status_hints_name_each_screens_keys() {
    // The header carries the tab bar; the per-screen keys live in the status bar,
    // one source so the render and this test can't drift.
    assert!(cf_status_hints(CfScreen::Zones).contains("Enter records"));
    assert!(cf_status_hints(CfScreen::Zones).contains("Esc EasyPanel"));
    assert!(cf_status_hints(CfScreen::Records).contains("Space bulk"));
    assert!(cf_status_hints(CfScreen::Records).contains("Esc zones"));
}

#[test]
fn cf_list_state_tells_loading_from_empty_from_failed() {
    // busy + nothing yet = Loading; a fetch error = Error; a clean empty result =
    // Empty; anything present = Ready. The empty-vs-failed distinction the render
    // depends on to never draw "No records" over a failed fetch.
    assert_eq!(cf_list_state(true, false, true), CfListState::Loading);
    assert_eq!(cf_list_state(false, true, true), CfListState::Error);
    assert_eq!(cf_list_state(false, false, true), CfListState::Empty);
    assert_eq!(cf_list_state(false, false, false), CfListState::Ready);
    // A retry (busy again) reads as Loading even with a prior error still set.
    assert_eq!(cf_list_state(true, true, true), CfListState::Loading);
    // A non-empty list is Ready regardless of a stale error flag.
    assert_eq!(cf_list_state(false, true, false), CfListState::Ready);
}

#[test]
fn cf_record_patch_gates_proxied_and_priority_by_type() {
    // An A record carries content/ttl/proxied but no priority; MX carries priority
    // but never proxied; TXT carries neither.
    let a = cf_record_patch("A", "5.6.7.8", "1", true, "");
    assert_eq!(a.content.as_deref(), Some("5.6.7.8"));
    assert_eq!(a.ttl, Some(1));
    assert_eq!(a.proxied, Some(true));
    assert_eq!(a.priority, None);

    let mx = cf_record_patch("MX", "mail.example.com", "3600", false, "10");
    assert_eq!(mx.priority, Some(10));
    assert_eq!(mx.proxied, None, "MX is not proxyable");

    let txt = cf_record_patch("TXT", "v=spf1", "1", true, "5");
    assert_eq!(txt.proxied, None);
    assert_eq!(txt.priority, None);
}

#[test]
fn cf_record_loading_empty_and_error_states_render_without_panic() {
    use ratatui::backend::TestBackend;
    use ratatui::Terminal;

    let render = |app: &mut App| {
        let mut term = Terminal::new(TestBackend::new(100, 12)).unwrap();
        term.draw(|f| super::render::ui(f, app)).unwrap();
        term.backend()
            .buffer()
            .content()
            .chunks(100)
            .map(|r| r.iter().map(|c| c.symbol()).collect::<String>())
            .collect::<Vec<_>>()
            .join("\n")
    };

    let mut app = App::new("s".into(), vec![]);
    app.cf.active = Some(cf_account());
    app.cf.current_zone = Some(cf_zone("z1", "example.com"));
    app.set_workspace(Workspace::Cloudflare);
    app.cf.screen = CfScreen::Records;

    // Error: a failed fetch shows the message, never the empty state.
    app.cf.error = Some("Cloudflare: bad token".into());
    let screen = render(&mut app);
    assert!(screen.contains("bad token"), "error state:\n{screen}");

    // Empty: a clean empty result says so.
    app.cf.error = None;
    let screen = render(&mut app);
    assert!(screen.contains("No DNS records"), "empty state:\n{screen}");

    // Ready: the record's columns render (TTL 1 → "auto").
    app.cf.records = vec![cf_record("r1", "A", "api.example.com", "1.1.1.1")];
    app.cf.records_row.select(Some(0));
    let screen = render(&mut app);
    assert!(screen.contains("api.example.com"), "ready state:\n{screen}");
    assert!(screen.contains("auto"), "ttl 1 renders as auto:\n{screen}");
}
