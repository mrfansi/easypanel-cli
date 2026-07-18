//! Test TUI, sengaja dibiarkan dalam satu berkas saat pemecahan ini.
//!
//! Pemecahan modul harus TANPA perubahan perilaku, dan test yang tak disentuh
//! adalah buktinya: 83 test yang persis sama, lulus sebelum dan sesudah.
//! Menyebarnya ke tiap modul adalah pekerjaan tersendiri.

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
    // Service baru: Enter tanpa sadar tak boleh menunjuk source ke repo acak.
    // inspectService mengembalikan `source: null`, bukan field yang absen.
    let f = form(source_fields(
        Some(&Value::Null),
        vec!["caesario/Kuze".into(), "acme/web".into()],
    ));
    assert_eq!(f_val(&f, "Repo"), "");
    assert_eq!(source_body(&f).unwrap_err(), "Repo wajib dipilih");
}

#[test]
fn domain_edit_keeps_middlewares_and_extra_servers() {
    // Middleware belum bisa diedit dari TUI, jadi HARUS ikut utuh. Begitu juga
    // server custom kedua dst., yang tak dimodelkan form.
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
    // Server kedua tak boleh terpangkas diam-diam.
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
        .value = "ya".into();
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
    assert_eq!(service_row(&github, None, None)[4], "acme/web#dev");

    let image = json!({
        "projectName": "p", "name": "cache", "type": "redis", "enabled": false,
        "source": { "type": "image", "image": "redis:7" }
    });
    let row = service_row(&image, None, None);
    assert_eq!(row[4], "redis:7");
    assert_eq!(row[3], "mati");

    // Service tanpa source (baru dibuat) tak boleh bikin panik.
    assert_eq!(service_row(&svc("p", "kosong", "app"), None, None)[4], "-");
}

#[test]
fn resource_body_parses_numbers_defaults_zero_and_rejects_junk() {
    // Prefill dari resources yang ada.
    let res = json!({ "cpuLimit": 1, "cpuReservation": 0.25,
                      "memoryLimit": 512, "memoryReservation": 128 });
    let f = form(resource_fields(Some(&res)));
    assert_eq!(f.by_label("CPU limit (core)"), "1");
    assert_eq!(f.by_label("CPU reservation (core)"), "0.25");
    assert_eq!(f.by_label("Memory limit (MB)"), "512");

    // resources null (belum diatur) -> semua "0".
    let f0 = form(resource_fields(None));
    assert_eq!(f0.by_label("Memory limit (MB)"), "0");
    let body = resource_body(&f0).unwrap();
    // Number, bukan string (API menolak string). CPU desimal, memory apa adanya.
    assert_eq!(body["resources"]["cpuLimit"], json!(0.0));
    assert_eq!(body["resources"]["memoryReservation"], json!(0.0));

    // Isi campur: kosong -> 0, desimal CPU dipertahankan.
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

    // Non-angka -> error dengan pesan, bukan diam-diam 0.
    let mut bad = form(resource_fields(None));
    bad.fields
        .iter_mut()
        .find(|x| x.label == "CPU limit (core)")
        .unwrap()
        .value = "banyak".into();
    assert!(resource_body(&bad).is_err());

    // Negatif ditolak.
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
    assert_eq!(base64(b"sh"), "c2g="); // yang dipakai shell container
    assert_eq!(base64(b""), "");
    assert_eq!(base64(b"M"), "TQ==");
    assert_eq!(base64(b"Ma"), "TWE=");
    assert_eq!(base64(b"Man"), "TWFu");
}

#[test]
fn db_command_per_type_uses_stored_credentials() {
    use super::terminal::db_command;
    // Bentuk tiap perintah diverifikasi live ke server (mysql/postgres/mongo/redis).
    let mysql = json!({ "rootPassword": "rp", "databaseName": "app" });
    assert_eq!(
        db_command("mysql", &mysql).unwrap(),
        "MYSQL_PWD='rp' mysql -uroot app"
    );
    // mariadb pakai klien `mysql` yang sama.
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
    // Non-database -> None (key 'y' menolak).
    assert!(db_command("app", &json!({})).is_none());
    // Kutip single-quote aman: password ber-apostrof tak memecah perintah sh.
    assert!(db_command("redis", &json!({ "password": "a'b" }))
        .unwrap()
        .contains(r"'a'\''b'"));
    // postgres tanpa user -> fallback superuser "postgres".
    assert!(db_command("postgres", &json!({ "password": "x" }))
        .unwrap()
        .contains("-U postgres"));
}

#[test]
fn env_body_omits_dot_env_path_when_disabled() {
    // Aktif: dotEnvPath ikut. Server menolak null/kosong, jadi "mati" = field diomit.
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
    // Permanent & Enabled default ya (301, on).
    let body = redirect_body(&f).unwrap();
    assert_eq!(body["regex"], json!("^https://old.test/(.*)"));
    assert_eq!(body["replacement"], json!("https://new.test/${1}"));
    assert_eq!(body["permanent"], json!(true));
    assert_eq!(body["enabled"], json!(true));

    // Permanent = tidak -> 302.
    set(&mut f, "Permanent (301)", "tidak");
    assert_eq!(redirect_body(&f).unwrap()["permanent"], json!(false));

    // Regex/replacement kosong -> error.
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
    // Prefill dari kredensial pertama yang ada (bentuk terverifikasi live).
    let data = json!({ "basicAuth": [{ "username": "admin", "password": "s3cret" }] });
    let f = form(basic_auth_fields(Some(&data)));
    assert_eq!(f.by_label("Username"), "admin");
    assert_eq!(f.by_label("Password"), "s3cret");
    assert_eq!(
        basic_auth_body(&f).unwrap(),
        json!([{ "username": "admin", "password": "s3cret" }])
    );

    // Keduanya kosong -> array kosong (matikan proteksi).
    let f = form(basic_auth_fields(None));
    assert_eq!(basic_auth_body(&f).unwrap(), json!([]));

    // Setengah (username tanpa password) -> error, bukan kredensial cacat.
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

    // Mount path kosong -> error, apa pun tipenya.
    let mut f = form(mount_fields());
    set(&mut f, "Type", "volume");
    set(&mut f, "Name", "data");
    assert!(mount_body(&f).is_err());

    // volume tanpa name -> error (bukan diam-diam kirim name kosong).
    let mut f = form(mount_fields());
    set(&mut f, "Type", "volume");
    set(&mut f, "Mount path", "/data");
    assert!(mount_body(&f).is_err());
}

#[test]
fn port_body_parses_numbers_and_rejects_junk() {
    let f = form(port_fields());
    // Kosong -> ditolak dengan pesan, bukan port 0.
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
    // published/target HARUS number (API menolak string), protocol apa adanya.
    assert_eq!(v["published"], json!(8080));
    assert_eq!(v["target"], json!(80));
    assert_eq!(v["protocol"], json!("udp"));

    // Published non-angka -> error, bukan diam-diam 0.
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
    // `enabled` cuma "tidak di-disable", bukan "hidup". Service crash tetap
    // enabled — dulu tabel bohong menampilkan "aktif". Sekarang metrik yang
    // menentukan jalan/mati.
    let on = json!({ "projectName": "p", "name": "a", "type": "app", "enabled": true });
    let off = json!({ "projectName": "p", "name": "b", "type": "app", "enabled": false });

    // Di-disable user -> "mati", apa pun metriknya.
    assert_eq!(service_status(&off, Some(true), None), "mati");
    assert_eq!(service_status(&off, None, None), "mati");
    // Enabled + ada metrik -> jalan.
    assert_eq!(service_status(&on, Some(true), None), "aktif");
    // Enabled TAPI tak ada metrik (crash/stop) -> "berhenti", bukan "aktif" palsu.
    assert_eq!(service_status(&on, Some(false), None), "berhenti");
    // Metrik belum dimuat (None) -> jangan menuduh mati; jatuh ke "aktif".
    assert_eq!(service_status(&on, None, None), "aktif");
}

#[test]
fn replica_stats_distinguish_down_from_stopped() {
    // Replika swarm (actual/desired) memisahkan yang DULU dilebur jadi "berhenti":
    // service crash-loop (desired>0, actual<desired) itu RUSAK, bukan sengaja stop.
    let on = json!({ "projectName": "p", "name": "a", "type": "app", "enabled": true });

    // desired>0 tapi belum ada replika yang naik -> "turun" (rusak sekarang).
    assert_eq!(service_status(&on, Some(false), Some((0, 1))), "turun");
    // Sebagian replika hilang -> tetap "turun".
    assert_eq!(service_status(&on, None, Some((1, 3))), "turun");
    // Sengaja di-scale ke 0 -> "berhenti", bukan "turun".
    assert_eq!(service_status(&on, None, Some((0, 0))), "berhenti");
    // Replika penuh -> "aktif", meski metrik bilang belum ada (replika menang).
    assert_eq!(service_status(&on, Some(false), Some((1, 1))), "aktif");
    // enabled=false selalu "mati", replika tak mengubahnya.
    let off = json!({ "projectName": "p", "name": "b", "type": "app", "enabled": false });
    assert_eq!(service_status(&off, None, Some((0, 1))), "mati");
}

#[test]
fn is_deploying_tracks_running_deployment_actions_only() {
    let mut app = App::new("s".into(), vec![]);
    app.all_services = vec![
        json!({ "projectName": "p", "name": "web", "type": "app" }),
        json!({ "projectName": "p", "name": "db", "type": "mysql" }),
    ];
    // running = sedang berjalan; done = selesai (tidak); status live-verified
    // pending → running → done/error.
    app.actions = vec![
        json!({ "type": "deployment", "status": "running", "projectName": "p", "serviceName": "web" }),
        json!({ "type": "deployment", "status": "done", "projectName": "p", "serviceName": "db" }),
    ];
    assert!(app.is_deploying("p", "web"));
    assert!(!app.is_deploying("p", "db"));
    assert!(!app.is_deploying("p", "absent"));
    assert_eq!(app.deploying_count(), 1);

    // pending juga "sedang berjalan"; type non-deployment (backup) diabaikan.
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
    // Layar Domains tanpa baris terpilih -> tak ada menu.
    app.screen = Screen::Domains;
    assert!(app.context_items().is_empty());
    // Baris domain terpilih -> aksi domain (edit/primary/hapus).
    app.domains_state.select(Some(0));
    let items = app.context_items();
    assert_eq!(items.len(), 3);
    assert!(items.iter().any(|i| i.label == "Hapus"));
    // Layar tanpa aksi baris (Dashboard) -> selalu kosong.
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
    // visible_rows terurut nama: [header "p", "db"(mysql), "web"(app)].
    let has = |v: &[super::app::MenuItem], s: &str| v.iter().any(|i| i.label == s);

    app.services_table.select(Some(2)); // web (app)
    let top = app.service_menu();
    let top: Vec<&str> = top.iter().map(|i| i.label.as_str()).collect();
    assert!(top.contains(&"Env ▸") && top.contains(&"Jaringan ▸") && top.contains(&"Bahaya ▸"));
    // app: file .env bisa di-toggle; shell TANPA DB shell.
    assert!(has(&app.env_menu(), "Toggle file .env"));
    assert!(!app
        .shell_menu()
        .iter()
        .any(|i| i.label.contains("DB shell")));

    app.services_table.select(Some(1)); // db (mysql)
                                        // db: TANPA toggle .env; shell PUNYA DB shell (login otomatis).
    assert!(!has(&app.env_menu(), "Toggle file .env"));
    assert!(app
        .shell_menu()
        .iter()
        .any(|i| i.label.contains("DB shell")));
    // db punya Config file (Advanced) di menu Build.
    assert!(has(&app.build_menu(), "Config file (Advanced)"));

    app.services_table.select(Some(2)); // web (app)
                                        // app TANPA Config file (Advanced) — configFile hanya untuk service database.
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

    // Klik kanan membuka menu service (top-level), lalu → masuk submenu Env; ←
    // mengembalikan ke menu service (induk), bukan menutup.
    app.on_key(KeyCode::Char('e'), &tx); // opener keyboard: langsung menu Env (tanpa induk)
    assert!(app.menu.is_some());
    app.on_key(KeyCode::Left, &tx); // teratas → menutup
    assert!(app.menu.is_none());

    // Drill dua tingkat lewat item ▸ mempertahankan induk untuk ←.
    let top = app.service_menu();
    app.open_menu(top);
    // pilih "Env ▸" (indeks 1) lalu → masuk submenu.
    app.menu.as_mut().unwrap().state.select(Some(1));
    app.on_key(KeyCode::Right, &tx);
    // sekarang di submenu Env; item pertama "Lihat env".
    assert_eq!(app.menu.as_ref().unwrap().items[0].label, "Lihat env");
    // ← kembali ke menu service (induk), tidak menutup.
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
    // Space = versi keyboard dari klik kanan: menu service top-level (punya Env ▸).
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

    // Tanpa service terpilih (bukan di Services): palette MURNI navigasi — tak ada
    // entri aksi (mencegah bloating ratusan entri).
    app.screen = Screen::Dashboard;
    app.open_palette();
    let nav = labels(&app);
    assert!(nav.iter().any(|x| x == "Buka  proj/web  ·  app"));
    assert!(!nav.iter().any(|x| x.starts_with("Deploy")));
    app.palette = None;

    // Dengan service web (app) terpilih di Services: muncul aksi service ITU saja.
    app.screen = Screen::Projects;
    // visible_rows terurut nama: [header "proj", "db"(mysql), "web"(app)].
    app.services_table.select(Some(2)); // web
    app.open_palette();
    let l = labels(&app);
    // Daftar aksi PENUH (bukan cuma lifecycle): lifecycle + env + jaringan + dst.
    assert!(l.iter().any(|x| x == "Deploy  ·  proj/web"));
    assert!(l.iter().any(|x| x == "Lihat env  ·  proj/web"));
    assert!(l.iter().any(|x| x == "Domain  ·  proj/web"));
    assert!(l.iter().any(|x| x == "Basic auth  ·  proj/web"));
    assert!(l.iter().any(|x| x == "Hapus service  ·  proj/web"));
    assert!(!l.iter().any(|x| x.starts_with("DB shell"))); // app bukan db
    assert!(!l.iter().any(|x| x == "Deploy  ·  proj/db")); // hanya service terpilih
    app.palette = None;

    // db (mysql) terpilih → aksinya menyertakan DB shell + Config file (Advanced).
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

    // Konteks juga untuk baris NON-service: Domains dengan domain terpilih →
    // aksi menu konteks domain (Edit/Primary/Hapus) muncul di palette.
    app.screen = Screen::Domains;
    app.domains = vec![json!({ "host": "x.test", "id": "d1" })];
    app.domains_state.select(Some(0));
    app.open_palette();
    assert!(labels(&app).iter().any(|x| x == "Domains: Hapus"));
    app.palette = None;

    // Balik ke Services untuk uji jalankan aksi service.
    app.screen = Screen::Projects;

    // Kembali ke web, uji jalankan aksi.
    app.services_table.select(Some(2)); // web
    app.open_palette();

    // Pencarian multi-kata (token-AND): "deploy web" mempersempit ke entri yang
    // memuat kedua kata (mis. "Deploy …/web" dan "Auto deploy …/web").
    app.palette.as_mut().unwrap().query = "deploy web".into();
    let pal = app.palette.as_ref().unwrap();
    let m = pal.matches();
    assert!(m.iter().all(|&i| {
        let l = pal.items[i].label.to_lowercase();
        l.contains("deploy") && l.contains("web")
    }));
    // Ambil entri aksi Deploy (bukan "Auto deploy…") dan jalankan.
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
        "quick action Deploy harus memunculkan konfirmasi deploy"
    );
}

#[test]
fn spinner_shows_only_while_loading() {
    // Spinner = umpan balik "sedang bekerja". Muncul saat status diakhiri "..." /
    // "…" (Memuat…/Mengirim…/Mencari…), diam saat status biasa.
    let mut app = App::new("t".into(), vec![]);
    app.status = "Siap".into();
    assert!(app.spinner().is_none());
    app.status = "Memuat...".into();
    assert!(app.spinner().is_some());
    app.status = "Mencari 'x' di semua service…".into();
    assert!(app.spinner().is_some());
    app.status = "Domain dihapus".into();
    assert!(app.spinner().is_none());
}

#[test]
fn clone_body_drops_identity_and_source_but_keeps_config() {
    // Bentuk field diverifikasi ke server hidup (mysql). Clone menyalin config,
    // membuang identitas/token, dan menerapkan source/configFile TERPISAH — jadi
    // keduanya harus absen dari body createService.
    let inspect = json!({
        "name": "mysql", "serviceName": "mysql", "projectName": "db", "type": "mysql",
        "enabled": true, "token": "secret", "primaryDomainId": "d1",
        "image": "mysql:8.0", "env": "TZ=Asia/Jakarta", "password": "pw",
        "rootPassword": "rpw", "user": "u", "databaseName": "app",
        "source": { "type": "github" }, "configFile": "[mysqld]\nserver-id=2"
    });
    let body = clone_body(&inspect, "db", "mysql-replica");
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
            "'{k}' harusnya dibuang dari body clone"
        );
    }
    // Config sungguhan (termasuk kredensial) ikut tersalin.
    assert_eq!(body["image"], json!("mysql:8.0"));
    assert_eq!(body["env"], json!("TZ=Asia/Jakarta"));
    assert_eq!(body["rootPassword"], json!("rpw"));
    assert_eq!(body["databaseName"], json!("app"));
    // Diarahkan ke target.
    assert_eq!(body["projectName"], json!("db"));
    assert_eq!(body["serviceName"], json!("mysql-replica"));
}

#[test]
fn task_stats_parse_matches_server_shape() {
    // Bentuk terverifikasi ke server hidup: objek { "{proj}_{svc}": {actual,desired} }.
    let v = json!({
        "edukasistudio-db_mysql": { "actual": 1, "desired": 1 },
        "harisenin-com_webapp":   { "actual": 0, "desired": 1 },
        "rusak":                  { "actual": 2 }
    });
    let m = parse_task_stats(&v);
    assert_eq!(m.get("edukasistudio-db_mysql"), Some(&(1, 1)));
    assert_eq!(m.get("harisenin-com_webapp"), Some(&(0, 1)));
    // Entri tanpa `desired` diabaikan, tidak bikin panik atau nilai palsu.
    assert_eq!(m.get("rusak"), None);
}

#[test]
fn auto_deploy_column_separates_off_from_not_applicable() {
    // Bentuk ini dipastikan ke API sungguhan, bukan dikarang: source github
    // selalu membawa autoDeploy (15/16 app), source image tak pernah.
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
    // Bukan "✗": MySQL dan source image tak punya auto deploy untuk
    // dinyalakan, jadi "belum" akan jadi klaim yang salah.
    assert_eq!(auto_deploy_cell(&image), "-");
    assert_eq!(auto_deploy_cell(&db), "-");

    // service_row masih memisah project dan nama; render melebur keduanya,
    // jadi indeksnya bergeser satu terhadap header.
    assert_eq!(service_row(&on, None, None)[5], "✓");
    assert_eq!(SERVICE_HEADERS[4], "Auto");
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

    // Baris 0 = header project, 1 = service "a", 2 = service "b".
    app.services_table.select(Some(1));
    app.toggle_auto_deploy(&tx);
    assert!(
        matches!(rx.try_recv(), Ok(Req::AutoDeploy { ref service, on, .. })
                     if service == "a" && !on),
        "true harus dikirim sebagai on:false, bukan mengirim ulang nilai lama"
    );

    // MySQL: tak ada auto deploy untuk dibalik. Diam-diam mengirim
    // disableGithubDeploy ke sana hanya menghasilkan error dari server.
    app.services_table.select(Some(2));
    app.toggle_auto_deploy(&tx);
    assert!(rx.try_recv().is_err(), "tak boleh ada request untuk MySQL");
    assert!(app.status.contains("GitHub"), "status: {}", app.status);

    // Header project bukan service.
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
    // Service tanpa metrik tak boleh bikin panik atau menampilkan 0 palsu.
    assert_eq!(metric_cols(None), vec!["-", "-", "-", "-"]);
}

#[test]
fn metrics_join_by_project_and_service() {
    // getAllServicesStats memuat lebih banyak entri daripada daftar service
    // (service sistem, sub-service compose) — dan nama service yang sama bisa
    // ada di project berbeda, jadi kuncinya harus pasangan, bukan nama saja.
    let mut app = App::new("s".into(), vec![]);
    app.all_services = vec![svc("proj-a", "mysql", "mysql")];
    app.monitor = vec![
        json!({ "projectName": "proj-b", "serviceName": "mysql",
                    "cpu": 9.0, "memory": 1.0, "networkIn": 0.0, "networkOut": 0.0 }),
        json!({ "projectName": "proj-a", "serviceName": "mysql",
                    "cpu": 1.0, "memory": 2048.0, "networkIn": 0.0, "networkOut": 0.0 }),
    ];
    let m = app.metric_for("proj-a", "mysql").unwrap();
    // Harus mengambil proj-a, bukan proj-b yang namanya sama.
    assert_eq!(metric_cols(Some(m))[0], "1.0 %");
    assert_eq!(metric_cols(Some(m))[1], "2.0 KB");

    // Service yang tak punya metrik: kolomnya "-", bukan 0 palsu.
    assert!(app.metric_for("proj-c", "hantu").is_none());
    assert_eq!(metric_cols(app.metric_for("proj-c", "hantu"))[0], "-");
}

#[test]
fn flat_list_filters_across_projects() {
    // Inti daftar datar: cari "mysql" menemukannya di project mana pun,
    // tanpa perlu tahu ia ada di project yang mana.
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

    // Nama project juga ikut dicocokkan, bukan cuma nama service.
    app.filter = "edukasistudio".into();
    assert_eq!(app.visible_services().len(), 2);
}

/// Form "Service baru" seperti yang dilihat user, dengan Tipe/Source diatur.
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
            .unwrap_or_else(|| panic!("field '{label}' tak ada di form"))
            .value = (*val).into();
    }
    form
}

#[test]
fn a_new_app_carries_its_source_in_the_same_request() {
    // Inti item ini: createService menerima source inline, jadi service app
    // tak perlu dibuat dulu lalu diedit. Sebelumnya form ini hanya bisa
    // mengirim project/nama/tipe.
    let f = create_form(&[
        ("Nama", "web"),
        ("Tipe", "app"),
        ("Source", "github"),
        ("Repo", "acme/web"),
        ("Branch", "dev"),
    ]);
    // Source diterapkan lewat updateSourceGithub TERPISAH (bukan inline di
    // createService, yang memicu deploy). create_source memberi (op, body, auto).
    let (op, body, auto) = create_source(&f).unwrap().unwrap();
    assert_eq!(op, "updateSourceGithub");
    assert_eq!(
        body,
        json!({ "owner": "acme", "repo": "web", "ref": "dev", "path": "/" })
    );
    assert_eq!(auto, Some(false));

    let f = create_form(&[
        ("Nama", "web"),
        ("Repo", "acme/web"),
        ("Branch", "dev"),
        ("Auto deploy", "ya"),
    ]);
    assert_eq!(create_source(&f).unwrap().unwrap().2, Some(true));
}

#[test]
fn a_new_service_without_a_source_sends_none_at_all() {
    // Repo kosong = user belum memilih. createService hanya mewajibkan
    // projectName + serviceName, jadi source DIHILANGKAN — bukan dikirim
    // setengah jadi, dan bukan pula error yang memblokir pembuatan.
    let f = create_form(&[("Nama", "web"), ("Tipe", "app")]);
    assert_eq!(create_source(&f).unwrap(), None);

    // Database tak punya source sama sekali.
    let f = create_form(&[("Nama", "db"), ("Tipe", "mysql")]);
    assert_eq!(create_source(&f).unwrap(), None);

    // Tapi source yang setengah jadi tetap ditolak: repo dipilih, branch tidak.
    let f = create_form(&[("Nama", "web"), ("Repo", "acme/web"), ("Branch", "")]);
    assert!(create_source(&f).is_err());
}

#[test]
fn source_fields_hide_unless_the_service_type_is_app() {
    // Dua penentu sekaligus: tipe service DAN tipe source. Ini yang dulu
    // mustahil — satu form hanya punya satu switch.
    let shown =
        |f: &Form| -> Vec<&str> { f.visible().iter().map(|i| f.fields[*i].label).collect() };

    let f = create_form(&[("Tipe", "app"), ("Source", "github")]);
    assert!(shown(&f).contains(&"Repo"));
    assert!(shown(&f).contains(&"Branch"));
    assert!(!shown(&f).contains(&"Docker image"));
    assert!(!shown(&f).contains(&"Database"), "app bukan database");

    // Tipe mysql: seluruh blok source lenyap, field database muncul.
    let f = create_form(&[("Tipe", "mysql"), ("Source", "github")]);
    assert!(!shown(&f).contains(&"Repo"));
    assert!(!shown(&f).contains(&"Source"));
    assert!(shown(&f).contains(&"Database"));
    assert!(shown(&f).contains(&"Root password"));
}

#[test]
fn the_merged_create_form_has_no_duplicate_labels() {
    // Form ini menggabungkan field database dan field source. Keduanya dulu
    // sama-sama punya "Tipe", "Image", dan "Password" — dan by_label() memakai
    // find(), jadi field database akan membajak nilai milik source. Inilah
    // alasan label source diganti jadi Source/Docker image/Registry *.
    let f = create_form(&[]);
    let mut seen = std::collections::HashSet::new();
    for field in &f.fields {
        assert!(seen.insert(field.label), "label '{}' ganda", field.label);
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

    // Tipe app: tak ada field database sama sekali.
    assert_eq!(service_extra(form), json!({}));

    // Tipe redis: hanya password + image yang tampil. Mengisi Root password
    // (tersembunyi untuk redis) tak boleh ikut terkirim — server menolaknya.
    for (label, val) in [
        ("Tipe", "redis"),
        ("Password", "s3cret"),
        ("Root password", "bocor"),
    ] {
        form.fields
            .iter_mut()
            .find(|f| f.label == label)
            .unwrap()
            .value = val.into();
    }
    assert_eq!(service_extra(form), json!({ "password": "s3cret" }));

    // Tipe mysql: Root password kini tampil, jadi ikut. Database/User/Image
    // dibiarkan kosong -> DIHILANGKAN, bukan dikirim "": server harus
    // membuatkan sendiri, dan "" berarti "pakai string kosong".
    form.fields
        .iter_mut()
        .find(|f| f.label == "Tipe")
        .unwrap()
        .value = "mysql".into();
    assert_eq!(
        service_extra(form),
        json!({ "password": "s3cret", "rootPassword": "bocor" })
    );
}

#[test]
fn empty_project_shows_no_metrics_not_negative_zero() {
    // Versi lama test ini menguji `vec![].sum()` dan `metric_cols(None)` —
    // semantik float Rust dan sebuah fungsi yang tak dipanggil baris header
    // project. Ia lulus sementara layar sungguhan menampilkan "-0.0 %".
    // Sekarang ia memanggil pembangun baris yang sebenarnya dipakai render.
    let row = project_row("kosong", 0, &[]);
    assert_eq!(row[0], "kosong (0)");
    assert_eq!(&row[5..], ["-", "-", "-", "-"], "tak ada yang diukur");
    assert!(
        !row.iter().any(|c| c.contains("-0.0")),
        "identitas Sum f64 adalah -0.0; ia tak boleh bocor ke layar: {row:?}"
    );

    // Ada metrik -> dijumlahkan sungguhan, bukan "-".
    let m = json!({ "cpu": 1.5, "memory": 2048.0, "networkIn": 0.0, "networkOut": 0.0 });
    let row = project_row("isi", 1, &[&m]);
    assert_eq!(row[5], "1.5 %");

    assert_eq!(metric_cols(None), vec!["-", "-", "-", "-"]);
}

#[test]
fn rows_are_project_headers_followed_by_their_services() {
    let mut app = App::new("s".into(), vec![]);
    app.projects = vec!["p1".into(), "p2".into()];
    app.all_services = vec![svc("p1", "b", "app"), svc("p1", "a", "app")];

    let rows = app.visible_rows();
    // p1 header + 2 service (urut nama) + p2 header (kosong, tetap tampil).
    assert_eq!(rows.len(), 4);
    assert!(
        matches!(&rows[0], Line2::Project { name, services } if *name == "p1" && services.len() == 2)
    );
    assert!(matches!(&rows[1], Line2::Service(s) if field(s, "/name") == "a"));
    assert!(matches!(&rows[2], Line2::Service(s) if field(s, "/name") == "b"));
    // Project tanpa service HARUS punya baris: kalau tidak ia hilang sama
    // sekali — tak terlihat, tak bisa dipilih, tak bisa dihapus.
    assert!(
        matches!(&rows[3], Line2::Project { name, services } if *name == "p2" && services.is_empty())
    );
}

#[test]
fn header_row_is_never_mistaken_for_a_service() {
    // Aksi service (logs/deploy/hapus) pada header project akan menyentuh
    // service yang tak ada. selected_row() harus None di header.
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

    // Nama project cocok -> isinya ikut tampil, bukan header kosong.
    app.filter = "harisenin".into();
    let rows = app.visible_rows();
    assert_eq!(rows.len(), 2);
    assert!(matches!(&rows[1], Line2::Service(s) if field(s, "/name") == "api"));

    // Nama service cocok -> header project-nya ikut, supaya konteksnya jelas.
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
            "{:?} tak punya keybinding terdaftar",
            TABS[sc.index()]
        );
    }
}

#[test]
fn help_lists_the_destructive_keys_that_exist() {
    // Tombol destruktif paling perlu ditemukan sebelum ditekan. Kini aksinya ada
    // di menu grup, jadi opener-nya harus terdokumentasi DAN deskripsinya menyebut
    // aksi destruktif/lifecycle-nya (hapus/deploy/restart/stop/start).
    let projects = screen_keys(Screen::Projects);
    let keys: Vec<&str> = projects.iter().map(|k| k.0).collect();
    assert!(keys.contains(&"x"), "opener menu Bahaya tak terdokumentasi");
    assert!(keys.contains(&"d"), "opener menu Siklus tak terdokumentasi");
    let bahaya = projects.iter().find(|k| k.0 == "x").unwrap().1;
    assert!(bahaya.contains("hapus"), "menu Bahaya harus menyebut hapus");
    let siklus = projects.iter().find(|k| k.0 == "d").unwrap().1;
    for word in ["deploy", "restart", "stop", "start"] {
        assert!(siklus.contains(word), "menu Siklus harus menyebut '{word}'");
    }
    let maint: Vec<&str> = screen_keys(Screen::Maintenance)
        .iter()
        .map(|k| k.0)
        .collect();
    for k in ["p", "i", "c"] {
        assert!(
            maint.contains(&k),
            "'{k}' tak terdokumentasi di Maintenance"
        );
    }
}

#[test]
fn help_key_and_quit_key_are_documented_globally() {
    let g: Vec<&str> = GLOBAL_KEYS.iter().map(|k| k.0).collect();
    assert!(g.contains(&"?"));
    assert!(g.contains(&"q / Ctrl-C"));
    // Esc membatalkan, dan itu harus tertulis: sebelumnya Esc menutup TUI.
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
    assert!(!keep(&row, "tidakada"));
}

#[test]
fn enter_saves_the_form_from_a_choice_field() {
    // Regresi: Enter dulu membuka dropdown pada field Choice, jadi form yang
    // field terakhirnya Choice — "Service baru" tipe app, persis kasus yang
    // dilaporkan — mustahil disimpan. Enter cuma buka-tutup dropdown.
    let (tx, rx) = std::sync::mpsc::channel();
    let mut app = App::new("s".into(), vec![]);
    app.projects = vec!["p".into()];
    app.new_service_form(&tx);
    // Form ini memuat daftar repo untuk dropdown source-nya; itu request
    // pertama di antrean, bukan yang sedang diuji.
    assert!(matches!(rx.try_recv(), Ok(Req::Repos)));

    let form = app.form.as_mut().unwrap();
    form.fields
        .iter_mut()
        .find(|f| f.label == "Nama")
        .unwrap()
        .value = "webapp".into();
    // Tipe database = wizard satu langkah, jadi Enter langsung menyimpan (bukan
    // maju ke langkah source/build yang tak ada untuk database). Regresi yang
    // dijaga: Enter pada field Choice TAK membuka dropdown — dulu ia begitu,
    // sehingga form tak pernah bisa disimpan.
    form.fields
        .iter_mut()
        .find(|f| f.label == "Tipe")
        .unwrap()
        .value = "redis".into();
    form.focus = form.fields.iter().position(|f| f.label == "Tipe").unwrap();
    assert!(matches!(form.fields[form.focus].kind, FieldKind::Choice(_)));
    assert!(
        !app.form.as_ref().unwrap().is_wizard(),
        "redis = satu langkah"
    );

    app.form_key(KeyCode::Enter, &tx);

    assert!(app.chooser.is_none(), "Enter tak boleh membuka dropdown");
    assert!(matches!(
        rx.try_recv(),
        Ok(Req::ServiceCreate { ref service, ref stype, .. })
            if service == "webapp" && stype == "redis"
    ));
}

#[test]
fn env_and_domain_are_attached_only_when_filled() {
    // Kedua langkah opsional: kosong = tak dikirim (bukan env "" atau domain
    // tanpa host). Port diparse jadi number; path default "/".
    let f = create_form(&[("Nama", "web"), ("Tipe", "app")]);
    assert_eq!(create_env(&f), None, "env kosong tak dikirim");
    assert_eq!(create_domains(&f), None, "tanpa host tak ada domain");

    let f = create_form(&[
        ("Nama", "web"),
        ("Environment", "FOO=bar\nBAZ=qux"),
        ("Domain host", "web.test"),
        ("Domain port", "8080"),
    ]);
    assert_eq!(create_env(&f).as_deref(), Some("FOO=bar\nBAZ=qux"));
    let d = create_domains(&f).unwrap();
    assert_eq!(d[0]["host"], json!("web.test"));
    assert_eq!(d[0]["port"], json!(8080), "port harus number, bukan string");
    assert_eq!(d[0]["https"], json!(true));
    assert_eq!(d[0]["path"], json!("/"));

    // Port non-numerik dihilangkan, bukan dikirim 0 (port salah lebih buruk).
    let f = create_form(&[("Domain host", "web.test"), ("Domain port", "bukan")]);
    assert!(create_domains(&f).unwrap()[0].get("port").is_none());
}

#[test]
fn create_keeps_source_separate_and_never_deploys_inline() {
    // Insight pemilik proyek: createService dengan source inline langsung
    // men-deploy (~100 detik, bisa error). Source harus TERPISAH supaya service
    // muncul dulu di tabel, lalu deploy manual. Jadi Req membawa `source`
    // sendiri, dan `extra` (yang masuk createService) TAK memuat "source".
    let (tx, rx) = std::sync::mpsc::channel();
    let mut app = App::new("s".into(), vec![]);
    app.projects = vec!["p".into()];
    app.new_service_form(&tx);
    assert!(matches!(rx.try_recv(), Ok(Req::Repos)));
    let form = app.form.as_mut().unwrap();
    for (label, val) in [
        ("Nama", "web"),
        ("Repo", "acme/web"),
        ("Branch", "dev"),
        ("Environment", "A=1"),
        ("Buat file .env", "ya"),
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
            assert!(extra.get("source").is_none(), "source tak boleh inline");
            assert!(matches!(source, Some(("updateSourceGithub", _, _))));
            // env file: toggle nyala -> dotEnvPath, dan env ikut inline (aman).
            assert_eq!(extra["env"], json!("A=1"));
            assert_eq!(extra["dotEnvPath"], json!(".env"));
        }
        _ => panic!("harus mengirim ServiceCreate"),
    }
}

#[test]
fn app_creation_is_a_five_step_wizard_ending_in_one_request() {
    // Alur dashboard EasyPanel: Dasar → Source → Build → Environment → Domains.
    // Enter maju tiap langkah lalu MENYIMPAN di langkah terakhir; Esc mundur.
    // Semuanya jadi SATU createService, bukan create-lalu-edit berkali-kali.
    let (tx, rx) = std::sync::mpsc::channel();
    let mut app = App::new("s".into(), vec![]);
    app.projects = vec!["p".into()];
    app.new_service_form(&tx);
    assert!(matches!(rx.try_recv(), Ok(Req::Repos)));

    let form = app.form.as_mut().unwrap();
    form.fields
        .iter_mut()
        .find(|f| f.label == "Nama")
        .unwrap()
        .value = "web".into();
    // Isi satu domain supaya bisa dipastikan ikut ke request.
    form.fields
        .iter_mut()
        .find(|f| f.label == "Domain host")
        .unwrap()
        .value = "web.test".into();
    // app (default) → lima langkah.
    assert_eq!(form.steps_present(), vec![0, 1, 2, 3, 4]);
    assert_eq!(form.step, 0);

    for expected in [1, 2, 3, 4] {
        app.form_key(KeyCode::Enter, &tx);
        assert_eq!(app.form.as_ref().unwrap().step, expected);
    }
    // Mundur satu lalu maju lagi: navigasi dua arah.
    app.form_key(KeyCode::Esc, &tx);
    assert_eq!(app.form.as_ref().unwrap().step, 3);
    app.form_key(KeyCode::Enter, &tx); // kembali ke Domains (terakhir)
    app.form_key(KeyCode::Enter, &tx); // Domains = terakhir → simpan

    assert!(
        matches!(rx.try_recv(), Ok(Req::ServiceCreate { ref service, ref stype, ref extra, .. })
            if service == "web" && stype == "app"
                && extra.get("build").is_some()
                && extra["domains"][0]["host"] == json!("web.test")),
        "langkah terakhir harus mengirim createService dengan build + domain inline"
    );
}

#[test]
fn space_opens_the_dropdown_on_a_choice_field() {
    // Konsekuensi dari test di atas: kalau Enter menyimpan, harus ada tombol
    // lain yang membuka dropdown — kalau tidak, Tipe jadi tak bisa diubah.
    let (tx, _rx) = std::sync::mpsc::channel();
    let mut app = App::new("s".into(), vec![]);
    app.projects = vec!["p".into()];
    app.new_service_form(&tx);
    let form = app.form.as_mut().unwrap();
    form.focus = form.fields.iter().position(|f| f.label == "Tipe").unwrap();

    app.form_key(KeyCode::Char(' '), &tx);
    assert!(app.chooser.is_some(), "Spasi harus membuka dropdown");
}

#[test]
fn filter_narrows_domains_and_actions_use_the_same_list() {
    // Kalau render difilter tapi aksi memakai indeks daftar penuh, `x` akan
    // menghapus domain yang salah. Keduanya wajib lewat visible_domains().
    let mut app = App::new("s".into(), vec![]);
    app.domains = vec![
        json!({ "id": "a", "host": "satu.com", "https": true, "path": "/",
                    "destinationType": "service",
                    "serviceDestination": { "projectName": "p", "serviceName": "x",
                                            "port": 80, "protocol": "http", "path": "/" } }),
        json!({ "id": "b", "host": "dua.com", "https": true, "path": "/",
                    "destinationType": "service",
                    "serviceDestination": { "projectName": "p", "serviceName": "y",
                                            "port": 80, "protocol": "http", "path": "/" } }),
    ];
    assert_eq!(app.visible_domains().len(), 2);

    app.filter = "dua".into();
    let vis = app.visible_domains();
    assert_eq!(vis.len(), 1);
    // Indeks 0 dari daftar terfilter harus "dua.com" — bukan "satu.com".
    assert_eq!(vis[0]["id"], json!("b"));
}

#[test]
fn clamp_keeps_selection_inside_filtered_list() {
    let mut app = App::new("s".into(), vec![]);
    app.screen = Screen::Domains;
    app.domains = vec![
        json!({ "id": "a", "host": "satu.com", "https": true, "path": "/" }),
        json!({ "id": "b", "host": "dua.com", "https": true, "path": "/" }),
    ];
    app.domains_state.select(Some(1));
    app.filter = "satu".into();
    app.clamp_filtered();
    // Hanya 1 baris tersisa; baris ke-1 sudah tak ada.
    assert_eq!(app.domains_state.selected(), Some(0));

    app.filter = "tidakadayangcocok".into();
    app.clamp_filtered();
    assert_eq!(app.domains_state.selected(), None);
}

#[test]
fn branch_falls_back_to_text_when_its_list_cannot_load() {
    // Field Branch adalah dropdown yang diisi dari GitHub. Kalau daftarnya
    // gagal (mis. token GitHub di EasyPanel ter-revoke — user benar-benar
    // mengalaminya), opsinya cuma berisi nilai sekarang, jadi branch TAK BISA
    // diganti sama sekali. Dropdown satu-opsi itu pintu terkunci, bukan
    // degradasi anggun.
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
    // Pesan ini disalin apa adanya dari server sungguhan saat mencoba
    // menyalakan auto deploy untuk service yang sumbernya repo pihak ketiga.
    let real = "[400] Request failed with status code 404 Not Found: \
                    GET https://api.github.com/repos/benborla/mcp-server-mysql/hooks";
    let msg = auto_deploy_error("mysql-mcp", real);
    assert!(msg.contains("webhook"), "{msg}");
    assert!(
        !msg.contains("404"),
        "tumpukan kode status tak menolong: {msg}"
    );

    // Yang tak dikenali tak boleh dibuang — membuang pesan server adalah bug
    // yang sudah pernah terjadi di sini.
    let msg = auto_deploy_error("api", "connection reset");
    assert!(msg.contains("connection reset"), "{msg}");
}

#[test]
fn short_reason_names_the_cause_not_the_status_stack() {
    // EasyPanel membungkus error upstream, jadi token GitHub mati muncul
    // sebagai "[400] Request failed with status code 403 Forbidden" — dua kode
    // status dan nol petunjuk tentang apa yang harus diperbaiki.
    assert_eq!(
        short_reason("[400] Request failed with status code 403 Forbidden"),
        "GitHub menolak: 403"
    );
    assert_eq!(
        short_reason("[400] Request failed with status code 401 Unauthorized"),
        "GitHub menolak: token tidak valid"
    );
    assert_eq!(short_reason("connection reset"), "gagal");
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
    // updateSourceGithub mereset autoDeploy jadi false di server; nilainya
    // harus ikut supaya bisa dipasang ulang setelahnya.
    assert_eq!(auto, Some(true));
}

#[test]
fn dockerfile_source_sends_its_contents_inline() {
    // updateSourceDockerfile menerima ISI Dockerfile, bukan path — jadi ia
    // multi-baris, dan itulah kenapa ia lewat $EDITOR.
    let body = json!({ "type": "dockerfile", "dockerfile": "FROM alpine\nRUN echo hai" });
    let f = form(source_fields(Some(&body), vec![]));
    let (op, sent, auto) = source_body(&f).unwrap();
    assert_eq!(op, "updateSourceDockerfile");
    assert_eq!(sent, json!({ "dockerfile": "FROM alpine\nRUN echo hai" }));
    assert_eq!(auto, None, "dockerfile tak punya auto deploy");

    // Kosong ditolak dengan menyebut cara mengisinya, bukan "wajib diisi".
    let f = form(source_fields(
        Some(&json!({ "type": "dockerfile" })),
        vec![],
    ));
    let err = source_body(&f).unwrap_err();
    assert!(err.contains("$EDITOR"), "{err}");
}

#[test]
fn dockerfile_source_is_not_mislabelled_as_an_image() {
    // create_source dulu memetakan tipe dengan catch-all `_ => "image"`. Sebuah
    // source dockerfile akan lolos sebagai image: bentuknya sah, tapi
    // service-nya di-build dari image yang tak pernah disebut siapa pun.
    let f = create_form(&[
        ("Nama", "web"),
        ("Tipe", "app"),
        ("Source", "dockerfile"),
        ("Dockerfile", "FROM alpine"),
    ]);
    // Dockerfile diterapkan lewat updateSourceDockerfile, bukan disalahlabeli
    // jadi image. create_source memberi op yang benar + isi Dockerfile.
    let (op, body, _) = create_source(&f).unwrap().unwrap();
    assert_eq!(op, "updateSourceDockerfile");
    assert_eq!(body["dockerfile"], json!("FROM alpine"));
    assert!(body.get("image").is_none());

    // Dockerfile kosong = belum diisi -> source dihilangkan.
    let f = create_form(&[("Nama", "web"), ("Source", "dockerfile")]);
    assert_eq!(create_source(&f).unwrap(), None);
}

#[test]
fn the_editor_field_shows_its_size_not_its_first_line() {
    // Isi ratusan baris di kolom satu baris cuma jadi bubur; yang berguna adalah
    // apakah ia sudah diisi.
    let f = Field::editor("Dockerfile", "FROM alpine\nRUN echo hai\nCMD sh");
    assert_eq!(f.shown(), "3 baris");
    assert_eq!(Field::editor("Dockerfile", "").shown(), "(kosong)");
    // Bukan field ketik: Spasi membuka $EDITOR, bukan menyisipkan spasi.
    assert!(!f.kind.is_typed());
}

#[test]
fn source_git_and_image_have_no_auto_deploy() {
    // Hanya source github yang punya konsep auto deploy.
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
    // Kirim "" akan menimpa kredensial registry jadi kosong.
    assert_eq!(body, json!({ "image": "nginx:latest" }));
}

#[test]
fn version_field_maps_to_the_right_key_per_builder() {
    // Satu field "Version" melayani nixpacks dan railpack. Kalau dibuat dua
    // field berlabel sama, by_label() — yang memakai find(), bukan visibilitas —
    // akan mengambil yang pertama, jadi railpack menulis versi milik nixpacks.
    let original = json!({ "type": "railpack", "railpackVersion": "0.17.1" });
    let mut f = form(build_fields(Some(&original)));
    f.original = Some(original);
    assert_eq!(f.by_label("Version"), "0.17.1");

    let body = build_body(&f).unwrap();
    assert_eq!(body["build"]["railpackVersion"], json!("0.17.1"));
    assert!(
        body["build"].get("nixpacksVersion").is_none(),
        "railpack tak boleh menulis kunci milik nixpacks"
    );

    // nixpacks membaca kunci versinya sendiri.
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
    // Sebelumnya versi hanya dilestarikan dari build asli: user terkunci di
    // versi yang kebetulan dipakai saat service dibuat.
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
    // by_label() memakai find(): label ganda berarti field yang tersembunyi bisa
    // membajak nilai milik yang tampil, diam-diam.
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
                "form {name}: label '{}' ganda",
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
    // nixpacksVersion tak ada di form; hilang = build berubah diam-diam.
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
    // `ref` bisa berupa tag; searchBranches tak memuatnya. Melompat ke branch
    // pertama akan mengganti apa yang ter-deploy.
    let mut f = Field::choice_owned("Branch", vec!["v1.2.0".into()], "v1.2.0");
    f.set_options(vec!["main".into(), "dev".into()]);
    assert_eq!(f.value, "v1.2.0");
    match &f.kind {
        FieldKind::Choice(o) => assert_eq!(o[0], "v1.2.0"),
        _ => panic!("harus tetap Choice"),
    }
}

#[test]
fn source_fields_keep_repo_absent_from_list() {
    // Repo yang dipakai tak ada di searchRepos (mis. hilang akses) -> jangan
    // diam-diam pindah ke repo pertama.
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
    // Char biasa, Enter, Backspace, arrow, dan Ctrl-C (0x03).
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

/// Round-trip WebSocket terminal LEWAT KODE RUST (bukan python): ws_url +
/// spawn_session + vt100. Butuh server hidup dan container zzz-emb/zzz-redis
/// yang berjalan, jadi #[ignore] — jalankan manual dengan `--ignored`.
#[test]
#[ignore = "live: butuh zzz-emb/zzz-redis berjalan di server"]
fn terminal_ws_roundtrip_live() {
    use super::worker::Resp;
    use std::sync::mpsc::channel;
    use std::time::{Duration, Instant};

    let cfg = crate::config::ServerConfig::new(crate::config::ServerConfig::default_path());
    let srv = cfg.default().expect("ada server default");
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
            break; // shell mengeksekusi dan output kembali lewat kode Rust
        }
        assert!(Instant::now() < deadline, "tak ada output berisi bukti");
    }
    drop(in_tx); // tutup sesi
}
