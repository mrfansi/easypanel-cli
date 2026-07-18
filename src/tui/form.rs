use ratatui::widgets::ListState;
use serde_json::{json, Value};

use crate::output::field;

pub(super) const SERVICE_TYPES: &[&str] = &[
    "app",
    "mysql",
    "mariadb",
    "postgres",
    "mongo",
    "redis",
    "wordpress",
    "compose",
];
pub(super) const DEST_KINDS: &[&str] = &["service", "custom"];
pub(super) const PROTOCOLS: &[&str] = &["http", "https"];
pub(super) const PORT_PROTOCOLS: &[&str] = &["tcp", "udp"];
pub(super) const SOURCE_TYPES: &[&str] = &["github", "git", "image", "dockerfile"];
pub(super) const BUILD_TYPES: &[&str] = &[
    "nixpacks",
    "railpack",
    "dockerfile",
    "buildpacks",
    "heroku-buildpacks",
    "paketo-buildpacks",
];

/// Field form source; `source` adalah objek `source` dari inspectService.
///
/// `repos` kosong (GitHub tak tersambung / gagal) membuat "Repo" jadi input teks.
pub(super) fn source_fields(source: Option<&Value>, repos: Vec<String>) -> Vec<Field> {
    let get = |ptr: &str, default: &str| match source.map(|s| field(s, ptr)) {
        Some(v) if v != "-" => v,
        _ => default.to_string(),
    };
    let stype = get("/type", "github");
    let (owner, repo) = (get("/owner", ""), get("/repo", ""));
    let current = if owner.is_empty() {
        String::new()
    } else {
        format!("{owner}/{repo}")
    };
    let branch = get("/ref", "");

    let mut repos = repos;
    if current.is_empty() {
        // Service baru belum punya source. Tanpa pilihan kosong, choice_owned
        // memilih repo pertama daftar — Enter tanpa sadar akan menunjuk source
        // ke repo acak, bukan gagal dengan jelas.
        repos.insert(0, String::new());
    } else if !repos.contains(&current) {
        // Repo yang sedang dipakai wajib ada di daftar. Kalau tidak, choice_owned
        // akan diam-diam memilih repo pertama — mengganti source service saat user
        // cuma bermaksud mengubah branch.
        repos.insert(0, current.clone());
    }
    let repo_field = if repos.is_empty() {
        Field::text("Repo", &current)
    } else {
        Field::choice_owned("Repo", repos, &current)
    };

    let auto_deploy = source
        .and_then(|s| s.get("autoDeploy"))
        .and_then(Value::as_bool)
        .unwrap_or(false);

    vec![
        Field::choice("Source", SOURCE_TYPES, &stype),
        repo_field.when("Source", "github"),
        // Diisi setelah branch repo tsb dimuat; nilai lama dipertahankan supaya
        // mode edit tak kehilangan pilihannya sebelum data tiba.
        Field::choice_owned("Branch", vec![branch.clone()], &branch).when("Source", "github"),
        Field::boolean("Auto deploy", auto_deploy).when("Source", "github"),
        Field::text("Git URL", if stype == "git" { &repo } else { "" }).when("Source", "git"),
        Field::text("Ref", &branch).when("Source", "git"),
        Field::text("Path", &get("/path", "/")).when("Source", "github,git"),
        Field::editor("Dockerfile", &get("/dockerfile", "")).when("Source", "dockerfile"),
        Field::text("Docker image", &get("/image", "")).when("Source", "image"),
        Field::text("Registry user", &get("/username", "")).when("Source", "image"),
        Field::secret_val("Registry password", &get("/password", "")).when("Source", "image"),
    ]
}

/// Field form build; `build` adalah objek `build` dari inspectService.
///
/// nixpacks dan railpack berbagi label perintah yang sama, dan itu aman HANYA
/// karena label bersama punya SATU field ber-.when("nixpacks,railpack") — bukan
/// karena by_label() sadar visibilitas. Ia memakai find(): field pertama dengan
/// label itu, tampil atau tidak. Dua field berlabel sama = tipe yang satu menulis
/// nilai milik tipe lain.
pub(super) fn build_fields(build: Option<&Value>) -> Vec<Field> {
    let get = |ptr: &str, default: &str| match build.map(|b| field(b, ptr)) {
        Some(v) if v != "-" => v,
        _ => default.to_string(),
    };
    let version = match get("/type", "nixpacks").as_str() {
        "railpack" => get("/railpackVersion", "0.17.1"),
        _ => get("/nixpacksVersion", "1.41.0"),
    };
    vec![
        Field::choice("Build", BUILD_TYPES, &get("/type", "nixpacks")),
        // SATU field Version, bukan satu per tipe: by_label() memakai find(), jadi
        // ia mengambil field PERTAMA berlabel itu — bukan yang sedang tampil. Dua
        // field "Version" akan membuat railpack menulis versi milik nixpacks.
        // build_body() sudah memetakannya ke kunci yang benar per tipe.
        Field::text("Version", &version).when("Build", "nixpacks,railpack"),
        Field::text("Install command", &get("/installCommand", ""))
            .when("Build", "nixpacks,railpack"),
        Field::text("Build command", &get("/buildCommand", "")).when("Build", "nixpacks,railpack"),
        Field::text("Start command", &get("/startCommand", "")).when("Build", "nixpacks,railpack"),
        Field::text("Nix packages", &get("/nixPackages", "")).when("Build", "nixpacks"),
        Field::text("Apt packages", &get("/aptPackages", "")).when("Build", "nixpacks"),
        Field::text("Mise packages", &get("/misePackages", "")).when("Build", "railpack"),
        Field::text("Dockerfile path", &get("/file", "Dockerfile")).when("Build", "dockerfile"),
        Field::text("Builder", &get("/buildpacksBuilder", "heroku/builder:24"))
            .when("Build", "buildpacks"),
    ]
}

/// Field form domain; `existing` mengisi nilai awal saat mode edit.
///
/// Field service dan custom ditampilkan sekaligus; yang dipakai ditentukan
/// "Tujuan". Ini mengikuti dialog panel, yang juga punya Protocol dan destination
/// custom (URL + weight) — keduanya tak boleh hilang saat mengedit.
pub(super) fn domain_fields(existing: Option<&Value>, projects: &[String]) -> Vec<Field> {
    let get = |ptr: &str, default: &str| match existing.map(|d| field(d, ptr)) {
        Some(v) if v != "-" => v,
        _ => default.to_string(),
    };
    let https = existing
        .and_then(|d| d.get("https"))
        .and_then(Value::as_bool)
        .unwrap_or(true);
    let server = existing.and_then(|d| d.pointer("/customDestination/servers/0"));
    let service = get("/serviceDestination/serviceName", "");

    let wildcard = existing
        .and_then(|d| d.get("wildcard"))
        .and_then(Value::as_bool)
        .unwrap_or(false);

    vec![
        Field::text("Host", &get("/host", "")),
        Field::text("Path", &get("/path", "/")),
        Field::boolean("HTTPS", https),
        // Nama resolver Traefik ditentukan konfigurasi server (mis. "letsencrypt",
        // "google"); tak ada endpoint untuk mendaftarnya, jadi teks bebas —
        // menebak-nebak isi dropdown justru menyesatkan.
        Field::text("SSL resolver", &get("/certificateResolver", "")),
        Field::boolean("Wildcard", wildcard),
        Field::choice("Tujuan", DEST_KINDS, &get("/destinationType", "service")),
        Field::choice_owned(
            "Project",
            projects.to_vec(),
            &get("/serviceDestination/projectName", ""),
        )
        .when("Tujuan", "service"),
        // Diisi setelah service project tsb dimuat; nilai lama dipertahankan
        // supaya mode edit tidak kehilangan pilihannya sebelum data tiba.
        Field::choice_owned("Service", vec![service.clone()], &service).when("Tujuan", "service"),
        Field::choice(
            "Protocol",
            PROTOCOLS,
            &get("/serviceDestination/protocol", "http"),
        )
        .when("Tujuan", "service"),
        Field::text("Port", &get("/serviceDestination/port", "80")).when("Tujuan", "service"),
        Field::text("Path tujuan", &get("/serviceDestination/path", "/")).when("Tujuan", "service"),
        Field::text(
            "Server URL",
            &server.map(|s| field(s, "/url")).unwrap_or_default(),
        )
        .when("Tujuan", "custom"),
        Field::text(
            "Weight",
            &server.map(|s| field(s, "/weight")).unwrap_or("1".into()),
        )
        .when("Tujuan", "custom"),
    ]
}

/// Field khusus tipe untuk createService, hanya yang benar-benar diisi user.
///
/// Semuanya opsional di API, dan kosong berarti server yang membuatkan: password
/// acak, nama database = nama project, image resmi terbaru — sama seperti dialog
/// panel. Mengirim "" bukan berarti "buatkan", melainkan "pakai string kosong",
/// jadi field kosong harus DIHILANGKAN dari body, bukan dikirim kosong.
/// Objek `source` untuk createService, atau None bila memang tak ada.
///
/// createService menerima source inline (`{type, owner, repo, ref, path,
/// autoDeploy}`), yaitu isi yang dibungkus endpoint updateSource*. Bentuknya
/// dipinjam dari source_body() supaya validasi dan pemetaan kuncinya cuma punya
/// SATU tempat — dua salinan akan berbeda pelan-pelan.
///
/// Kosong berarti user belum memilih, bukan error: service app boleh dibuat
/// tanpa source lalu diatur belakangan, dan createService cuma mewajibkan
/// projectName + serviceName.
/// Panggilan updateSource: (endpoint, body, auto_deploy). Dipakai lintas modul
/// (form membuatnya, worker menjalankannya), jadi satu alias.
pub(super) type SourceCall = (&'static str, Value, Option<bool>);

/// Panggilan updateSource untuk service baru: (op, body, auto), atau None bila
/// bukan app / source belum diisi.
///
/// Source DITERAPKAN TERPISAH setelah createService, bukan inline. Sebabnya
/// diukur di server: createService dengan source langsung memicu deploy (~100
/// detik + bisa error build), sedangkan updateSource cuma menyimpan konfigurasi
/// (~2 detik, tanpa deploy). Jadi service muncul seketika di tabel dan deploy
/// tetap aksi eksplisit `d` — persis alur dashboard EasyPanel.
pub(super) fn create_source(form: &Form) -> std::result::Result<Option<SourceCall>, String> {
    if form.by_label("Tipe") != "app" {
        return Ok(None);
    }
    let untouched = match form.by_label("Source").as_str() {
        "github" => form.by_label("Repo").is_empty(),
        "git" => form.by_label("Git URL").is_empty(),
        "dockerfile" => form.by_label("Dockerfile").is_empty(),
        _ => form.by_label("Docker image").is_empty(),
    };
    if untouched {
        return Ok(None);
    }
    source_body(form).map(Some)
}

/// Objek `build` untuk createService, atau None untuk service non-app.
///
/// createService menerima `build` inline sama seperti `source`. Hanya app yang
/// punya build; database dibuat server tanpa langkah build. build_body() sudah
/// memetakan tiap engine ke kuncinya (nixpacksVersion vs railpackVersion, dst),
/// jadi cukup panggil dan ambil isinya.
pub(super) fn create_build(form: &Form) -> Option<Value> {
    if form.by_label("Tipe") != "app" {
        return None;
    }
    build_body(form).ok().and_then(|b| b.get("build").cloned())
}

/// Isi `env` untuk createService, atau None bila kosong. String KEY=value
/// multi-baris, disunting di $EDITOR — sama seperti env service yang sudah ada.
pub(super) fn create_env(form: &Form) -> Option<String> {
    let env = form.by_label("Environment");
    (!env.is_empty()).then_some(env)
}

/// Array `domains` untuk createService (satu domain), atau None bila host kosong.
///
/// API cuma mewajibkan `host`. `port` adalah number, jadi diparse; kalau tak
/// valid, dihilangkan ketimbang mengirim 0 yang menunjuk port salah.
pub(super) fn create_domains(form: &Form) -> Option<Value> {
    let host = form.by_label("Domain host");
    if host.is_empty() {
        return None;
    }
    let mut d = serde_json::Map::new();
    d.insert("host".into(), json!(host));
    d.insert("https".into(), json!(form.is_on_label("Domain HTTPS")));
    let path = form.by_label("Domain path");
    d.insert(
        "path".into(),
        json!(if path.is_empty() { "/".into() } else { path }),
    );
    if let Ok(port) = form.by_label("Domain port").parse::<u32>() {
        d.insert("port".into(), json!(port));
    }
    Some(json!([Value::Object(d)]))
}

/// Field limit resource. Semua number; kosong = 0 = tak dibatasi (konvensi
/// EasyPanel: 0 berarti tanpa batas). Satuan mengikuti dashboard EasyPanel —
/// CPU dalam core (boleh desimal, mis. 0.5), memory dalam MB. inspectService
/// menyimpan dan mengembalikan angka apa adanya (terverifikasi round-trip live).
pub(super) fn resource_fields(resources: Option<&Value>) -> Vec<Field> {
    // resources bisa null (belum diatur) → semua default "0".
    let get = |ptr: &str| match resources.map(|r| field(r, ptr)) {
        Some(v) if v != "-" => v,
        _ => "0".to_string(),
    };
    vec![
        Field::text("CPU limit (core)", &get("/cpuLimit")),
        Field::text("CPU reservation (core)", &get("/cpuReservation")),
        Field::text("Memory limit (MB)", &get("/memoryLimit")),
        Field::text("Memory reservation (MB)", &get("/memoryReservation")),
    ]
}

/// Body updateResources dari form. Keempat wajib number (API menolak string);
/// kosong → 0 (tak dibatasi). Negatif ditolak; angka tak valid ditolak dengan
/// pesan, bukan diam-diam jadi 0 yang salah.
pub(super) fn resource_body(form: &Form) -> std::result::Result<Value, String> {
    let num = |label: &str| -> std::result::Result<f64, String> {
        let v = form.by_label(label);
        let v = v.trim();
        if v.is_empty() {
            return Ok(0.0);
        }
        match v.parse::<f64>() {
            Ok(n) if n < 0.0 => Err(format!("{label} tak boleh negatif")),
            Ok(n) => Ok(n),
            Err(_) => Err(format!("{label} harus angka")),
        }
    };
    Ok(json!({ "resources": {
        "cpuLimit": num("CPU limit (core)")?,
        "cpuReservation": num("CPU reservation (core)")?,
        "memoryLimit": num("Memory limit (MB)")?,
        "memoryReservation": num("Memory reservation (MB)")?,
    }}))
}

pub(super) fn service_extra(form: &Form) -> Value {
    let mut out = serde_json::Map::new();
    for (label, key) in [
        ("Database", "databaseName"),
        ("User", "user"),
        ("Password", "password"),
        ("Root password", "rootPassword"),
        ("Image", "image"),
    ] {
        // Hanya field yang TAMPIL untuk tipe ini: mengirim rootPassword ke redis
        // akan ditolak server, dan user tak pernah melihat field itu.
        let visible = form
            .visible()
            .iter()
            .any(|i| form.fields[*i].label == label);
        let value = form.by_label(label);
        if visible && !value.is_empty() {
            out.insert(key.to_string(), json!(value));
        }
    }
    Value::Object(out)
}

/// Endpoint + body updateSource* dari form.
///
/// Tiap tipe source punya endpoint sendiri dengan field yang persis ditentukan
/// skema, jadi body dibangun dari nol — tak ada field tak termodel yang perlu
/// dilestarikan seperti pada domain.
/// `auto_deploy` hanya relevan untuk source github (endpoint lain tak punya konsep ini).
pub(super) fn source_body(
    form: &Form,
) -> std::result::Result<(&'static str, Value, Option<bool>), String> {
    let path = match form.by_label("Path").as_str() {
        "" => "/".to_string(),
        p => p.to_string(),
    };
    if !path.starts_with('/') {
        return Err("Path harus diawali /".into());
    }

    match form.by_label("Source").as_str() {
        "github" => {
            let full = form.by_label("Repo");
            if full.is_empty() {
                return Err("Repo wajib dipilih".into());
            }
            let (owner, repo) = full
                .split_once('/')
                .ok_or("Repo harus berbentuk owner/repo")?;
            let branch = form.by_label("Branch");
            if owner.is_empty() || repo.is_empty() || branch.is_empty() {
                return Err("Repo dan Branch wajib diisi".into());
            }
            Ok((
                "updateSourceGithub",
                json!({ "owner": owner, "repo": repo, "ref": branch, "path": path }),
                Some(form.is_on_label("Auto deploy")),
            ))
        }
        "git" => {
            let (repo, git_ref) = (form.by_label("Git URL"), form.by_label("Ref"));
            if repo.is_empty() || git_ref.is_empty() {
                return Err("Git URL dan Ref wajib diisi".into());
            }
            Ok((
                "updateSourceGit",
                json!({ "repo": repo, "ref": git_ref, "path": path }),
                None,
            ))
        }
        "dockerfile" => {
            let content = form.by_label("Dockerfile");
            if content.is_empty() {
                return Err("Dockerfile masih kosong — Spasi untuk membukanya di $EDITOR".into());
            }
            Ok((
                "updateSourceDockerfile",
                json!({ "dockerfile": content }),
                None,
            ))
        }
        _ => {
            let image = form.by_label("Docker image");
            if image.is_empty() {
                return Err("Docker image wajib diisi".into());
            }
            let mut body = json!({ "image": image });
            // username/password opsional: kosong = tak dikirim, bukan dikirim "".
            for (label, key) in [
                ("Registry user", "username"),
                ("Registry password", "password"),
            ] {
                let v = form.by_label(label);
                if !v.is_empty() {
                    body[key] = json!(v);
                }
            }
            Ok(("updateSourceImage", body, None))
        }
    }
}

/// Body updateBuild dari form.
///
/// Berangkat dari build asli hanya bila tipenya tak berubah, supaya field yang
/// tak ada di form (nixpacksVersion, railpackVersion) tetap utuh. Saat tipe
/// diganti, field tipe lama justru tak boleh ikut terbawa.
pub(super) fn build_body(form: &Form) -> std::result::Result<Value, String> {
    let t = form.by_label("Build");
    let same_type =
        form.original.as_ref().map(|o| field(o, "/type")).as_deref() == Some(t.as_str());

    let mut build = match form.original.clone() {
        Some(o) if same_type && o.is_object() => o,
        _ => json!({}),
    };
    build["type"] = json!(t);

    let keys: &[(&str, &str)] = match t.as_str() {
        "nixpacks" => &[
            ("Version", "nixpacksVersion"),
            ("Install command", "installCommand"),
            ("Build command", "buildCommand"),
            ("Start command", "startCommand"),
            ("Nix packages", "nixPackages"),
            ("Apt packages", "aptPackages"),
        ],
        "railpack" => &[
            ("Version", "railpackVersion"),
            ("Install command", "installCommand"),
            ("Build command", "buildCommand"),
            ("Start command", "startCommand"),
            ("Mise packages", "misePackages"),
        ],
        "dockerfile" => &[("Dockerfile path", "file")],
        "buildpacks" => &[("Builder", "buildpacksBuilder")],
        // heroku-buildpacks / paketo-buildpacks cuma butuh `type`.
        _ => &[],
    };

    let obj = build.as_object_mut().ok_or("bentuk build tak dikenal")?;
    for (label, key) in keys {
        match form.by_label(label) {
            v if v.is_empty() => obj.remove(*key),
            v => obj.insert((*key).to_string(), json!(v)),
        };
    }
    Ok(json!({ "build": build }))
}

/// Body createDomain/updateDomain dari form.
///
/// Saat edit, berangkat dari JSON domain aslinya sehingga field yang tak ada
/// di form (middlewares) tetap utuh — bukan ditimpa nilai default.
pub(super) fn domain_body(form: &Form) -> std::result::Result<Value, String> {
    let host = form.by_label("Host");
    if host.is_empty() {
        return Err("Host wajib diisi".into());
    }

    let mut body = form.original.clone().unwrap_or_else(
        || json!({ "wildcard": false, "certificateResolver": "", "middlewares": [] }),
    );
    body["host"] = json!(host);
    body["path"] = json!(match form.by_label("Path").as_str() {
        "" => "/".to_string(),
        p => p.to_string(),
    });
    body["https"] = json!(form.is_on_label("HTTPS"));
    body["certificateResolver"] = json!(form.by_label("SSL resolver"));
    body["wildcard"] = json!(form.is_on_label("Wildcard"));

    let obj = body.as_object_mut().ok_or("bentuk domain tak dikenal")?;
    if form.by_label("Tujuan") == "custom" {
        let url = form.by_label("Server URL");
        if url.is_empty() {
            return Err("Server URL wajib diisi untuk tujuan custom".into());
        }
        let weight: u32 = form
            .by_label("Weight")
            .parse()
            .map_err(|_| "Weight harus angka")?;

        // Form hanya memodelkan server pertama. Server lain (kalau ada) harus
        // ikut utuh — memangkasnya diam-diam sama saja merusak konfigurasi.
        let mut servers = form
            .original
            .as_ref()
            .and_then(|o| o.pointer("/customDestination/servers"))
            .and_then(Value::as_array)
            .cloned()
            .unwrap_or_default();
        let first = json!({ "url": url, "weight": weight });
        if servers.is_empty() {
            servers.push(first);
        } else {
            servers[0] = first;
        }

        obj.remove("serviceDestination");
        obj.insert("destinationType".into(), json!("custom"));
        obj.insert("customDestination".into(), json!({ "servers": servers }));
    } else {
        let (project, service) = (form.by_label("Project"), form.by_label("Service"));
        if project.is_empty() || service.is_empty() {
            return Err("Project dan service wajib diisi".into());
        }
        let port: u32 = form
            .by_label("Port")
            .parse()
            .map_err(|_| "Port harus angka")?;
        obj.remove("customDestination");
        obj.insert("destinationType".into(), json!("service"));
        obj.insert(
            "serviceDestination".into(),
            json!({
                "projectName": project,
                "serviceName": service,
                "port": port,
                "protocol": form.by_label("Protocol"),
                "path": match form.by_label("Path tujuan").as_str() {
                    "" => "/".to_string(),
                    p => p.to_string(),
                }
            }),
        );
    }
    Ok(body)
}

/// Field form "Port baru": published (host) -> target (container) + protokol.
pub(super) fn port_fields() -> Vec<Field> {
    vec![
        Field::text("Published", ""),
        Field::text("Target", ""),
        Field::choice("Protocol", PORT_PROTOCOLS, "tcp"),
    ]
}

/// Objek `values` untuk createPort: `{published, target, protocol}`.
///
/// Keduanya number di API, jadi diparse; angka non-valid ditolak dengan pesan
/// jelas, bukan dikirim 0 yang membuka port salah.
pub(super) fn port_body(form: &Form) -> std::result::Result<Value, String> {
    let published: u32 = form
        .by_label("Published")
        .parse()
        .map_err(|_| "Published harus angka port (mis. 8080)")?;
    let target: u32 = form
        .by_label("Target")
        .parse()
        .map_err(|_| "Target harus angka port (mis. 80)")?;
    Ok(json!({
        "published": published,
        "target": target,
        "protocol": form.by_label("Protocol"),
    }))
}

// ---------- Form (ratatui tak punya widget input, jadi dibuat sendiri) ----------

#[derive(PartialEq, Clone)]
pub(super) enum FieldKind {
    Text,
    Secret,
    Bool,
    /// Pilihan dari data nyata (project/service/protocol), digilir dgn spasi/←/→.
    /// Dinamis supaya isinya bisa datang dari API, bukan diketik manual.
    Choice(Vec<String>),
    /// Isi multi-baris yang disunting di $EDITOR, seperti env.
    ///
    /// Sebuah Dockerfile tak pernah muat di field satu baris; memaksakannya ke
    /// FieldKind::Text berarti user mengetik "\n" harfiah dan mengirim satu baris
    /// panjang yang tak akan pernah di-build.
    Editor,
}

impl FieldKind {
    pub(super) fn is_typed(&self) -> bool {
        matches!(self, FieldKind::Text | FieldKind::Secret)
    }
}

pub(super) struct Field {
    pub(super) label: &'static str,
    pub(super) value: String,
    pub(super) kind: FieldKind,
    /// Syarat tampil, digabung dengan DAN: (label field penentu, tag dipisah
    /// koma mis. "github,git"). Kosong = selalu tampil.
    ///
    /// Panel juga begini: memilih Service/Custom mengganti field di bawahnya,
    /// bukan menampilkan keduanya sekaligus.
    pub(super) only_for: Vec<(&'static str, &'static str)>,
    /// Langkah wizard tempat field ini muncul. 0 = langkah pertama. Form yang
    /// semua field-nya di langkah 0 tetap satu halaman biasa; begitu ada field
    /// di langkah > 0, form itu jadi wizard bertahap. Nilai submit tetap dibaca
    /// dari SEMUA langkah sekaligus.
    pub(super) step: u8,
}

impl Field {
    /// Field yang isinya disunting di $EDITOR, bukan diketik di form.
    pub(super) fn editor(label: &'static str, value: &str) -> Self {
        Self {
            label,
            value: value.into(),
            kind: FieldKind::Editor,
            only_for: Vec::new(),
            step: 0,
        }
    }
    pub(super) fn text(label: &'static str, value: &str) -> Self {
        Self {
            label,
            value: value.into(),
            kind: FieldKind::Text,
            only_for: Vec::new(),
            step: 0,
        }
    }
    /// Tampilkan field ini hanya bila field `switch` bernilai salah satu `tags`
    /// (dipisah koma). Bisa dipanggil berkali-kali: syaratnya digabung dengan
    /// DAN, supaya satu form bisa punya lebih dari satu penentu — form "Service
    /// baru" perlu "tipe service = app" DAN "tipe source = github" sekaligus.
    pub(super) fn when(mut self, switch: &'static str, tags: &'static str) -> Self {
        self.only_for.push((switch, tags));
        self
    }

    /// Tempatkan field di langkah wizard `n` (0 = langkah pertama).
    pub(super) fn step(mut self, n: u8) -> Self {
        self.step = n;
        self
    }
    pub(super) fn secret(label: &'static str) -> Self {
        Self::secret_val(label, "")
    }
    pub(super) fn secret_val(label: &'static str, value: &str) -> Self {
        Self {
            label,
            value: value.into(),
            kind: FieldKind::Secret,
            only_for: Vec::new(),
            step: 0,
        }
    }
    pub(super) fn boolean(label: &'static str, on: bool) -> Self {
        Self {
            label,
            value: if on { "ya".into() } else { "tidak".into() },
            kind: FieldKind::Bool,
            only_for: Vec::new(),
            step: 0,
        }
    }
    pub(super) fn choice(label: &'static str, options: &[&str], value: &str) -> Self {
        Self::choice_owned(
            label,
            options.iter().map(|o| o.to_string()).collect(),
            value,
        )
    }
    pub(super) fn choice_owned(label: &'static str, options: Vec<String>, value: &str) -> Self {
        let value = if options.iter().any(|o| o == value) {
            value.to_string()
        } else {
            options.first().cloned().unwrap_or_default()
        };
        Self {
            label,
            value,
            kind: FieldKind::Choice(options),
            only_for: Vec::new(),
            step: 0,
        }
    }
    /// Ganti daftar pilihan (mis. service terisi setelah project dipilih).
    ///
    /// Nilai yang sedang dipakai selalu dipertahankan, meski tak ada di daftar
    /// baru: melompat diam-diam ke pilihan pertama akan mengubah konfigurasi yang
    /// tak diminta user — mis. `ref` yang berupa tag akan berganti jadi branch
    /// pertama sesuai abjad, lalu ikut ter-deploy.
    pub(super) fn set_options(&mut self, mut options: Vec<String>) {
        if !self.value.is_empty() && !options.contains(&self.value) {
            options.insert(0, self.value.clone());
        }
        if !options.contains(&self.value) {
            self.value = options.first().cloned().unwrap_or_default();
        }
        self.kind = FieldKind::Choice(options);
    }
    /// Gilir ke pilihan berikutnya (Bool diperlakukan sebagai ya/tidak).
    pub(super) fn cycle(&mut self) {
        match self.kind {
            FieldKind::Bool => {
                self.value = if self.is_on() {
                    "tidak".into()
                } else {
                    "ya".into()
                }
            }
            FieldKind::Choice(ref opts) => {
                if opts.is_empty() {
                    return;
                }
                let i = opts.iter().position(|o| *o == self.value).unwrap_or(0);
                self.value = opts[(i + 1) % opts.len()].clone();
            }
            _ => {}
        }
    }
    pub(super) fn is_on(&self) -> bool {
        self.value == "ya"
    }
    pub(super) fn shown(&self) -> String {
        match self.kind {
            FieldKind::Secret => "•".repeat(self.value.chars().count()),
            // Isinya bisa ratusan baris: yang berguna di sini adalah apakah ia
            // ada dan sebesar apa, bukan cuplikan baris pertamanya.
            FieldKind::Editor if self.value.trim().is_empty() => "(kosong)".into(),
            FieldKind::Editor => format!("{} baris", self.value.lines().count()),
            _ => self.value.clone(),
        }
    }
}

/// Apa yang dilakukan form saat disubmit.
pub(super) enum FormKind {
    ServerAdd,
    ServerEdit {
        name: String,
    },
    ProjectCreate,
    /// Project ikut jadi field form: daftar datar tak punya "project yang
    /// sedang dibuka" untuk diwarisi.
    ServiceCreate,
    DomainCreate,
    DomainEdit {
        id: String,
    },
    /// Tambah port yang di-expose ke sebuah service.
    PortCreate {
        project: String,
        service: String,
    },
    /// Cari kata kunci di log semua service sekaligus.
    LogSearch,
    SourceEdit {
        project: String,
        service: String,
    },
    BuildEdit {
        project: String,
        service: String,
    },
    /// Atur limit CPU/memory sebuah service (semua tipe). `stype` menentukan
    /// grup endpoint (services/{stype}/updateResources).
    ResourceEdit {
        project: String,
        service: String,
        stype: String,
    },
}

pub(super) struct Form {
    pub(super) kind: FormKind,
    pub(super) title: String,
    pub(super) fields: Vec<Field>,
    pub(super) focus: usize,
    /// JSON asli saat mode edit. Submit berangkat dari sini supaya field yang
    /// tak ada di form (middlewares pada domain, nixpacksVersion pada build)
    /// ikut utuh.
    pub(super) original: Option<Value>,
    /// Langkah wizard yang sedang tampil. 0 untuk form satu halaman biasa.
    pub(super) step: usize,
}

impl Form {
    pub(super) fn new(kind: FormKind, title: impl Into<String>, fields: Vec<Field>) -> Self {
        Self {
            kind,
            title: title.into(),
            fields,
            focus: 0,
            original: None,
            step: 0,
        }
    }
    pub(super) fn with_original(mut self, original: Value) -> Self {
        self.original = Some(original);
        self
    }
    /// Indeks field yang tampil sekarang.
    ///
    /// Setiap field membawa syaratnya sendiri, jadi satu form boleh punya lebih
    /// dari satu penentu. Tanpa itu, form "Service baru" tak bisa memuat source
    /// sekaligus: ia butuh "tipe service = app" DAN "tipe source = github".
    pub(super) fn visible(&self) -> Vec<usize> {
        self.fields
            .iter()
            .enumerate()
            .filter(|(_, f)| {
                f.only_for.iter().all(|(switch, tags)| {
                    let cur = self.by_label(switch);
                    tags.split(',').any(|t| t == cur)
                })
            })
            .map(|(i, _)| i)
            .collect()
    }

    /// Langkah-langkah yang benar-benar punya field tampil, terurut. Untuk
    /// service database ini cuma `[0]` (field source/build tergantung Tipe=app),
    /// jadi form-nya tetap satu halaman; untuk app `[0, 1, 2]` → wizard.
    pub(super) fn steps_present(&self) -> Vec<u8> {
        let mut steps: Vec<u8> = self
            .visible()
            .iter()
            .map(|&i| self.fields[i].step)
            .collect();
        steps.sort_unstable();
        steps.dedup();
        steps
    }

    /// Form ini bertahap (lebih dari satu langkah berisi field).
    pub(super) fn is_wizard(&self) -> bool {
        self.steps_present().len() > 1
    }

    /// Field tampil DI LANGKAH SEKARANG. Beda dari visible(), yang lintas-langkah
    /// dan dipakai saat submit untuk membaca semua nilai sekaligus.
    pub(super) fn visible_here(&self) -> Vec<usize> {
        let step = self.step as u8;
        self.visible()
            .into_iter()
            .filter(|&i| self.fields[i].step == step)
            .collect()
    }

    /// Langkah berisi berikutnya setelah yang sekarang, bila ada.
    pub(super) fn next_present_step(&self) -> Option<usize> {
        self.steps_present()
            .into_iter()
            .map(usize::from)
            .find(|&s| s > self.step)
    }

    /// Langkah berisi sebelum yang sekarang, bila ada.
    pub(super) fn prev_present_step(&self) -> Option<usize> {
        self.steps_present()
            .into_iter()
            .rev()
            .map(usize::from)
            .find(|&s| s < self.step)
    }

    /// Pindah ke langkah `step` dan letakkan fokus di field pertamanya.
    pub(super) fn goto_step(&mut self, step: usize) {
        self.step = step;
        self.focus = self.visible_here().first().copied().unwrap_or(0);
    }

    /// Pindah fokus `delta` langkah di antara field yang tampil di langkah ini.
    pub(super) fn move_focus(&mut self, delta: isize) {
        let vis = self.visible_here();
        if vis.is_empty() {
            return;
        }
        let at = vis.iter().position(|i| *i == self.focus).unwrap_or(0) as isize;
        let next = (at + delta).rem_euclid(vis.len() as isize) as usize;
        self.focus = vis[next];
    }

    /// Setelah Tujuan berganti, fokus bisa tertinggal di field yang kini tersembunyi.
    pub(super) fn clamp_focus(&mut self) {
        let vis = self.visible_here();
        if !vis.contains(&self.focus) {
            self.focus = vis.first().copied().unwrap_or(0);
        }
    }

    pub(super) fn val(&self, i: usize) -> String {
        self.fields[i].value.trim().to_string()
    }
    pub(super) fn by_label(&self, label: &str) -> String {
        self.fields
            .iter()
            .find(|f| f.label == label)
            .map(|f| f.value.trim().to_string())
            .unwrap_or_default()
    }
    pub(super) fn is_on_label(&self, label: &str) -> bool {
        self.fields
            .iter()
            .find(|f| f.label == label)
            .map(Field::is_on)
            .unwrap_or(false)
    }
}

/// Dropdown untuk sebuah field Choice: daftar pilihan + filter ketik.
///
/// Menggilir pilihan dengan spasi tidak terpakai untuk daftar panjang (11 service),
/// jadi field Choice membuka daftar sungguhan yang bisa dicari.
pub(super) struct Chooser {
    pub(super) field: usize,
    pub(super) label: &'static str,
    pub(super) options: Vec<String>,
    pub(super) filter: String,
    pub(super) state: ListState,
}

impl Chooser {
    pub(super) fn new(
        field: usize,
        label: &'static str,
        options: Vec<String>,
        current: &str,
    ) -> Self {
        let mut state = ListState::default();
        state.select(Some(options.iter().position(|o| o == current).unwrap_or(0)));
        Self {
            field,
            label,
            options,
            filter: String::new(),
            state,
        }
    }

    /// Pilihan yang lolos filter (case-insensitive, substring).
    pub(super) fn matches(&self) -> Vec<String> {
        let f = self.filter.to_lowercase();
        self.options
            .iter()
            .filter(|o| f.is_empty() || o.to_lowercase().contains(&f))
            .cloned()
            .collect()
    }

    pub(super) fn selected(&self) -> Option<String> {
        let m = self.matches();
        self.state.selected().and_then(|i| m.get(i).cloned())
    }

    /// Jaga agar indeks terpilih tetap valid setelah filter berubah.
    pub(super) fn clamp(&mut self) {
        let len = self.matches().len();
        let i = self.state.selected().unwrap_or(0);
        self.state
            .select(if len == 0 { None } else { Some(i.min(len - 1)) });
    }
}
