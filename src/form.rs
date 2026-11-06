use crate::tunnel::SshTunnel;
use serde_json::{json, Value};

use crate::output::field;

pub(crate) const SERVICE_TYPES: &[&str] = &[
    "app",
    "box",
    "mysql",
    "mariadb",
    "postgres",
    "mongo",
    "redis",
    "wordpress",
    "compose",
];
pub(crate) const DEST_KINDS: &[&str] = &["service", "custom"];
pub(crate) const PROTOCOLS: &[&str] = &["http", "https"];
pub(crate) const PORT_PROTOCOLS: &[&str] = &["tcp", "udp"];
pub(crate) const MOUNT_TYPES: &[&str] = &["volume", "bind", "file"];
pub(crate) const SOURCE_TYPES: &[&str] = &["github", "git", "image", "dockerfile"];
pub(crate) const BUILD_TYPES: &[&str] = &[
    "nixpacks",
    "railpack",
    "dockerfile",
    "buildpacks",
    "heroku-buildpacks",
    "paketo-buildpacks",
];

/// Fields for the source form; `source` is the `source` object from inspectService.
///
/// Empty `repos` (GitHub not connected / failed) makes "Repo" a text input.
pub(crate) fn source_fields(source: Option<&Value>, repos: Vec<String>) -> Vec<Field> {
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
        // A new service has no source yet. Without an empty choice, choice_owned
        // picks the first repo in the list — Enter would unknowingly point the
        // source at a random repo instead of failing clearly.
        repos.insert(0, String::new());
    } else if !repos.contains(&current) {
        // The repo currently in use must be in the list. Otherwise choice_owned
        // would silently pick the first one — changing the service's source when
        // the user only meant to change the branch.
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
        // Filled in once that repo's branches load; the old value is kept so edit
        // mode doesn't lose its choice before the data arrives.
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

/// Fields for the build form; `build` is the `build` object from inspectService.
///
/// nixpacks and railpack share the same command labels, and that's safe ONLY
/// because the shared label has a SINGLE field with .when("nixpacks,railpack") —
/// not because by_label() is visibility-aware. It uses find(): the first field
/// with that label, shown or not. Two fields with the same label = one type
/// writing the other type's value.
pub(crate) fn build_fields(build: Option<&Value>) -> Vec<Field> {
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
        // A SINGLE Version field, not one per type: by_label() uses find(), so it
        // takes the FIRST field with that label — not the one currently shown. Two
        // "Version" fields would make railpack write nixpacks's version.
        // build_body() already maps it to the right key per type.
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

/// Fields for the domain form; `existing` fills the initial values in edit mode.
///
/// The service and custom fields are all present; which ones apply is decided by
/// "Destination". This mirrors the panel dialog, which also has a Protocol and a
/// custom destination (URL + weight) — neither should be lost while editing.
pub(crate) fn domain_fields(existing: Option<&Value>, projects: &[String]) -> Vec<Field> {
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
        // The Traefik resolver name is set by the server config (e.g.
        // "letsencrypt", "google"); there's no endpoint to list them, so it's free
        // text — guessing dropdown contents would only mislead.
        Field::text("SSL resolver", &get("/certificateResolver", "")),
        Field::boolean("Wildcard", wildcard),
        Field::choice(
            "Destination",
            DEST_KINDS,
            &get("/destinationType", "service"),
        ),
        Field::choice_owned(
            "Project",
            projects.to_vec(),
            &get("/serviceDestination/projectName", ""),
        )
        .when("Destination", "service"),
        // Filled in once that project's services load; the old value is kept so
        // edit mode doesn't lose its choice before the data arrives.
        Field::choice_owned("Service", vec![service.clone()], &service)
            .when("Destination", "service"),
        Field::choice(
            "Protocol",
            PROTOCOLS,
            &get("/serviceDestination/protocol", "http"),
        )
        .when("Destination", "service"),
        Field::text("Port", &get("/serviceDestination/port", "80")).when("Destination", "service"),
        Field::text("Destination path", &get("/serviceDestination/path", "/"))
            .when("Destination", "service"),
        Field::text(
            "Server URL",
            &server.map(|s| field(s, "/url")).unwrap_or_default(),
        )
        .when("Destination", "custom"),
        Field::text(
            "Weight",
            &server.map(|s| field(s, "/weight")).unwrap_or("1".into()),
        )
        .when("Destination", "custom"),
    ]
}

/// Fields for editing one uptime check.
///
/// Headers and body are `$EDITOR` fields rather than single-line inputs: a JSON
/// payload or three headers do not fit on one line, and typing them into a box
/// that scrolls sideways is how a stray character goes unnoticed.
pub(crate) fn check_fields(check: &crate::uptime::Check) -> Vec<Field> {
    vec![
        Field::choice("Method", crate::uptime::METHODS, &check.method),
        // Empty means "any 2xx or 3xx", which is right for a page but wrong for
        // an endpoint that correctly answers 401 — so it is editable, not fixed.
        Field::text(
            "Expect status",
            &check.expect.map(|s| s.to_string()).unwrap_or_default(),
        ),
        Field::text("Timeout (s)", &check.timeout_secs.to_string()),
        Field::editor("Headers", &crate::uptime::headers_to_text(&check.headers)),
        Field::editor("Body", check.body.as_deref().unwrap_or(""))
            .when("Method", "POST,PUT,PATCH,DELETE"),
    ]
}

/// The edited check, or why it cannot be sent.
pub(crate) fn check_body(
    url: &str,
    form: &Form,
) -> std::result::Result<crate::uptime::Check, String> {
    let expect = match form.by_label("Expect status").trim() {
        "" => None,
        n => Some(
            n.parse::<u16>()
                .map_err(|_| "Expect status must be a number like 200".to_string())?,
        ),
    };
    let timeout_secs = match form.by_label("Timeout (s)").trim() {
        "" => 10,
        n => n
            .parse::<u64>()
            .map_err(|_| "Timeout must be a whole number of seconds".to_string())?,
    };
    if timeout_secs == 0 {
        return Err("A timeout of 0 would fail every check instantly".into());
    }
    let body = match form.by_label("Body") {
        b if b.trim().is_empty() => None,
        b => Some(b),
    };
    let check = crate::uptime::Check {
        url: url.to_string(),
        method: form.by_label("Method"),
        headers: crate::uptime::headers_from_text(&form.by_label("Headers")),
        body,
        expect,
        timeout_secs,
    };
    match check.problem() {
        Some(why) => Err(why),
        None => Ok(check),
    }
}

/// Fields for the bulk domain rewrite: which part, and the plain find/replace.
///
/// No regex. These values are hostnames and service names, where a `.` typed as
/// itself would quietly match any character and rewrite a domain the user never
/// meant to touch.
pub(crate) fn domain_bulk_fields() -> Vec<Field> {
    vec![
        Field::choice("Replace in", crate::domains::TARGETS, "host"),
        Field::text("Find", ""),
        Field::text("Replace with", ""),
    ]
}

/// Type-specific fields for createService, only the ones the user actually fills.
///
/// All are optional in the API, and empty means the server creates them: a random
/// password, a database named after the project, the latest official image — just
/// like the panel dialog. Sending "" doesn't mean "create one for me", it means
/// "use an empty string", so an empty field must be OMITTED from the body, not
/// sent empty.
/// The `source` object for createService, or None when there genuinely isn't one.
///
/// createService accepts an inline source (`{type, owner, repo, ref, path,
/// autoDeploy}`), the same payload the updateSource* endpoints wrap. Its shape is
/// borrowed from source_body() so validation and key mapping have a SINGLE home —
/// two copies would drift apart.
///
/// Empty means the user hasn't chosen yet, not an error: an app service can be
/// created without a source and configured later, and createService only requires
/// projectName + serviceName.
/// An updateSource call: (endpoint, body, auto_deploy). Used across modules (the
/// form builds it, the worker runs it), hence one alias.
pub(crate) type SourceCall = (&'static str, Value, Option<bool>);

/// The updateSource call for a new service: (op, body, auto), or None when it's
/// not an app / the source hasn't been filled.
///
/// The source is APPLIED SEPARATELY after createService, not inline. The reason
/// was measured on the server: createService with an inline source triggers a
/// deploy (~100 seconds + can error out during the build), whereas updateSource
/// only stores the config (~2 seconds, no deploy). So the service appears in the
/// table instantly and deploy stays an explicit `d` action — exactly the EasyPanel
/// dashboard flow.
pub(crate) fn create_source(form: &Form) -> std::result::Result<Option<SourceCall>, String> {
    if form.by_label("Kind") != "app" {
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

/// The `build` object for createService, or None for a non-app service.
///
/// createService accepts an inline `build` just like `source`. Only apps have a
/// build; databases are created by the server with no build step. build_body()
/// already maps each engine to its key (nixpacksVersion vs railpackVersion, etc.),
/// so just call it and take the contents.
/// An image source has nothing to build: the server itself nulls the build the
/// moment `updateSourceImage` runs (verified against a live panel — the build is
/// stored by createService, then wiped). Sending it writes a value that is
/// guaranteed to be thrown away.
pub(crate) fn create_build(form: &Form) -> Option<Value> {
    if form.by_label("Kind") != "app" || form.by_label("Source") == "image" {
        return None;
    }
    build_body(form).ok().and_then(|b| b.get("build").cloned())
}

/// The `env` contents for createService, or None if empty. A multi-line KEY=value
/// string, edited in $EDITOR — same as an existing service's env.
pub(crate) fn create_env(form: &Form) -> Option<String> {
    // Only for an app, like its two siblings above. `by_label` is
    // visibility-blind — it returns a hidden field's value exactly as if it were
    // on screen — so a user who filled Environment while Kind was "app", stepped
    // back and switched to postgres, silently shipped that env to the database.
    // Verified against a live server: createService on services/postgres ACCEPTS
    // and STORES the env. Not a confusing error; a wrong service, quietly.
    if form.by_label("Kind") != "app" {
        return None;
    }
    let env = form.by_label("Environment");
    (!env.is_empty()).then_some(env)
}

/// The `domains` array for createService (a single domain), or None if the host
/// is empty.
///
/// The API only requires `host`. `port` is a number, so it's parsed; if invalid,
/// it's dropped rather than sending a 0 that points at the wrong port.
pub(crate) fn create_domains(form: &Form) -> Option<Value> {
    // Same guard, same reason. The server ignores `domains` on a database rather
    // than storing it (checked live — no domain was created), so this one was
    // harmless; it is guarded anyway because "harmless today" is not a rule, and
    // the four sibling builders now all read the same way.
    if form.by_label("Kind") != "app" {
        return None;
    }
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

/// Fields for the resource limit form. All numbers; empty = 0 = unlimited
/// (EasyPanel's convention: 0 means no limit). Units follow the EasyPanel
/// dashboard — CPU in cores (decimals allowed, e.g. 0.5), memory in MB.
/// inspectService stores and returns the numbers as-is (verified round-trip live).
/// Fields for the deploy form; `deploy` is the `deploy` object from inspectService.
pub(crate) fn deploy_fields(deploy: Option<&Value>) -> Vec<Field> {
    let get = |ptr: &str, default: &str| match deploy.map(|d| field(d, ptr)) {
        Some(v) if v != "-" => v,
        _ => default.to_string(),
    };
    let zero = deploy
        .and_then(|d| d.get("zeroDowntime"))
        .and_then(Value::as_bool)
        .unwrap_or(true);
    vec![
        Field::text("Replicas", &get("/replicas", "1")),
        Field::text("Command", &get("/command", "")),
        Field::boolean("Zero downtime", zero),
    ]
}

/// The `deploy` object for updateDeploy.
///
/// It goes NESTED under "deploy" — a flat `{replicas: 3}` is answered with 200
/// and silently changes nothing, so the shape is not optional and not guessable.
pub(crate) fn deploy_body(form: &Form) -> std::result::Result<Value, String> {
    let raw = form.by_label("Replicas");
    let replicas: u32 = raw
        .trim()
        .parse()
        .map_err(|_| format!("Replicas must be a whole number, not '{raw}'"))?;
    // Swarm accepts 0 (scale to nothing), but from a form it is almost always a
    // typo, and the honest way to stop a service is Lifecycle ▸ Stop.
    if replicas == 0 {
        return Err("0 replicas stops the service — use Lifecycle ▸ Stop for that".into());
    }
    let command = form.by_label("Command");
    Ok(json!({
        "replicas": replicas,
        // An empty command means "the image's own", which the server stores as null.
        "command": if command.trim().is_empty() { Value::Null } else { json!(command) },
        "zeroDowntime": form.is_on_label("Zero downtime"),
    }))
}

pub(crate) fn resource_fields(resources: Option<&Value>) -> Vec<Field> {
    // resources may be null (never set) → everything defaults to "0".
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

/// updateResources body from the form. All four must be numbers (the API rejects
/// strings); empty → 0 (unlimited). Negatives are rejected; invalid numbers are
/// rejected with a message rather than silently becoming a wrong 0.
pub(crate) fn resource_body(form: &Form) -> std::result::Result<Value, String> {
    let num = |label: &str| -> std::result::Result<f64, String> {
        let v = form.by_label(label);
        let v = v.trim();
        if v.is_empty() {
            return Ok(0.0);
        }
        match v.parse::<f64>() {
            Ok(n) if n < 0.0 => Err(format!("{label} can't be negative")),
            Ok(n) => Ok(n),
            Err(_) => Err(format!("{label} must be a number")),
        }
    };
    Ok(json!({ "resources": {
        "cpuLimit": num("CPU limit (core)")?,
        "cpuReservation": num("CPU reservation (core)")?,
        "memoryLimit": num("Memory limit (MB)")?,
        "memoryReservation": num("Memory reservation (MB)")?,
    }}))
}

pub(crate) fn service_extra(form: &Form) -> Value {
    let mut out = serde_json::Map::new();
    for (label, key) in [
        ("Database", "databaseName"),
        ("User", "user"),
        ("Password", "password"),
        ("Root password", "rootPassword"),
        ("Image", "image"),
    ] {
        // Only fields that are SHOWN for this type: sending rootPassword to redis
        // would be rejected by the server, and the user never saw that field.
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

/// Endpoint + body for updateSource* from the form.
///
/// Each source type has its own endpoint with fields defined exactly by the
/// schema, so the body is built from scratch — there are no unmodelled fields to
/// preserve like there are on a domain.
/// `auto_deploy` is only relevant for a github source (the other endpoints have no
/// such concept).
/// What must hold before leaving `step` of the create wizard.
///
/// Checked on the way OUT of each step rather than at the end. The wizard used to
/// walk you through all five steps with an empty Name and an empty Repo without a
/// word, then refuse on the Domains step with a message about a field two steps
/// back and off screen — and the message blamed the character set of a name that
/// was simply missing.
pub(crate) fn validate_step(form: &Form) -> Result<(), String> {
    match form.step {
        0 => {
            if form.by_label("Project").is_empty() {
                return Err("Choose a project first".into());
            }
            let name = form.by_label("Name");
            if name.is_empty() {
                return Err("Give the service a name first".into());
            }
            if !crate::operations::valid_name(&name) {
                return Err("A service name may only contain a-z, 0-9, - and _".into());
            }
            Ok(())
        }
        // The source step validates through the SAME function the submit path uses
        // (`create_source`), so the gate and the request can't disagree. It permits
        // an untouched source (an app created as a shell, its repo/image wired up
        // later — what the panel allows and `a_new_service_without_a_source…`
        // asserts), while still rejecting a half-filled one. `source_body` alone
        // was stricter than submit and dead-ended that valid workflow.
        1 => create_source(form).map(|_| ()),
        _ => Ok(()),
    }
}

pub(crate) fn source_body(
    form: &Form,
) -> std::result::Result<(&'static str, Value, Option<bool>), String> {
    let path = match form.by_label("Path").as_str() {
        "" => "/".to_string(),
        p => p.to_string(),
    };
    if !path.starts_with('/') {
        return Err("Path must start with /".into());
    }

    // Validate what the user typed, then hand the shaping to `source::source_call`
    // so the form and the clone path can't disagree about which keys an endpoint
    // takes. Only auto deploy stays here — it is a form toggle, not payload shape.
    let kind = form.by_label("Source");
    let src = match kind.as_str() {
        "github" => {
            let full = form.by_label("Repo");
            if full.is_empty() {
                return Err("Repo must be selected".into());
            }
            let (owner, repo) = full
                .split_once('/')
                .ok_or("Repo must be in owner/repo form")?;
            let branch = form.by_label("Branch");
            if owner.is_empty() || repo.is_empty() || branch.is_empty() {
                return Err("Repo and Branch are required".into());
            }
            json!({ "type": "github", "owner": owner, "repo": repo, "ref": branch, "path": path })
        }
        "git" => {
            let (repo, git_ref) = (form.by_label("Git URL"), form.by_label("Ref"));
            if repo.is_empty() || git_ref.is_empty() {
                return Err("Git URL and Ref are required".into());
            }
            json!({ "type": "git", "repo": repo, "ref": git_ref, "path": path })
        }
        "dockerfile" => {
            let content = form.by_label("Dockerfile");
            if content.is_empty() {
                return Err("Dockerfile is still empty — Space to open it in $EDITOR".into());
            }
            json!({ "type": "dockerfile", "dockerfile": content })
        }
        _ => {
            let image = form.by_label("Docker image");
            if image.is_empty() {
                return Err("Docker image is required".into());
            }
            let mut src = json!({ "type": "image", "image": image });
            for (label, key) in [
                ("Registry user", "username"),
                ("Registry password", "password"),
            ] {
                let v = form.by_label(label);
                if !v.is_empty() {
                    src[key] = json!(v);
                }
            }
            src
        }
    };
    let (op, body) = crate::source::source_call(&src).ok_or("Unknown source type")?;
    let auto = (kind == "github").then(|| form.is_on_label("Auto deploy"));
    Ok((op, body, auto))
}

/// updateBuild body from the form.
///
/// Starts from the original build only when the type is unchanged, so fields not
/// in the form (nixpacksVersion, railpackVersion) stay intact. When the type
/// changes, the old type's fields must NOT be carried along.
pub(crate) fn build_body(form: &Form) -> std::result::Result<Value, String> {
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
        // heroku-buildpacks / paketo-buildpacks only need `type`.
        _ => &[],
    };

    let obj = build.as_object_mut().ok_or("unrecognized build shape")?;
    for (label, key) in keys {
        match form.by_label(label) {
            v if v.is_empty() => obj.remove(*key),
            v => obj.insert((*key).to_string(), json!(v)),
        };
    }
    Ok(json!({ "build": build }))
}

/// createDomain/updateDomain body from the form.
///
/// On edit, starts from the domain's original JSON so fields not in the form
/// (middlewares) stay intact — rather than being overwritten with defaults.
pub(crate) fn domain_body(form: &Form) -> std::result::Result<Value, String> {
    let host = form.by_label("Host");
    if host.is_empty() {
        return Err("Host is required".into());
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

    let obj = body.as_object_mut().ok_or("unrecognized domain shape")?;
    if form.by_label("Destination") == "custom" {
        let url = form.by_label("Server URL");
        if url.is_empty() {
            return Err("Server URL is required for a custom destination".into());
        }
        let weight: u32 = form
            .by_label("Weight")
            .parse()
            .map_err(|_| "Weight must be a number")?;

        // The form only models the first server. Other servers (if any) must stay
        // intact — silently trimming them would corrupt the config.
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
            return Err("Project and service are required".into());
        }
        let port: u32 = form
            .by_label("Port")
            .parse()
            .map_err(|_| "Port must be a number")?;
        obj.remove("customDestination");
        obj.insert("destinationType".into(), json!("service"));
        obj.insert(
            "serviceDestination".into(),
            json!({
                "projectName": project,
                "serviceName": service,
                "port": port,
                "protocol": form.by_label("Protocol"),
                "path": match form.by_label("Destination path").as_str() {
                    "" => "/".to_string(),
                    p => p.to_string(),
                }
            }),
        );
    }
    Ok(body)
}

/// Fields for the "New port" form: published (host) -> target (container) + protocol.
pub(crate) fn port_fields() -> Vec<Field> {
    vec![
        Field::text("Published", ""),
        Field::text("Target", ""),
        Field::choice("Protocol", PORT_PROTOCOLS, "tcp"),
    ]
}

/// Fields for the add-redirect form. `Regex` matches the source URL, `Replacement`
/// its target (may use groups `${1}`). Permanent = 301, otherwise 302.
pub(crate) fn redirect_fields() -> Vec<Field> {
    vec![
        Field::text("Regex", ""),
        Field::text("Replacement", ""),
        Field::boolean("Permanent (301)", true),
        Field::boolean("Enabled", true),
    ]
}

/// A redirect object for updateRedirects: `{enabled, permanent, regex, replacement}`
/// (all four required by the schema). Regex & replacement must not be empty.
pub(crate) fn redirect_body(form: &Form) -> std::result::Result<Value, String> {
    let regex = form.by_label("Regex");
    let replacement = form.by_label("Replacement");
    if regex.trim().is_empty() || replacement.trim().is_empty() {
        return Err("Regex and Replacement are required".into());
    }
    Ok(json!({
        "enabled": form.is_on_label("Enabled"),
        "permanent": form.is_on_label("Permanent (301)"),
        "regex": regex,
        "replacement": replacement,
    }))
}

/// Fields for the basic auth form, prefilled with the first existing credential
/// (this form manages a SINGLE user — the common "protect this service" case). The
/// password is prefilled too so changing just the username doesn't clear it.
pub(crate) fn basic_auth_fields(data: Option<&Value>) -> Vec<Field> {
    let first = data.and_then(|d| d.pointer("/basicAuth/0"));
    let user = first.map(|c| field(c, "/username")).unwrap_or_default();
    let pass = first.map(|c| field(c, "/password")).unwrap_or_default();
    vec![
        Field::text("Username", &user),
        Field::secret_val("Password", &pass),
    ]
}

/// The `basicAuth` array for updateBasicAuth. Both empty = remove protection ([]);
/// one empty = error (half a credential is useless).
pub(crate) fn basic_auth_body(form: &Form) -> std::result::Result<Value, String> {
    let user = form.by_label("Username");
    let pass = form.by_label("Password");
    let (u, p) = (user.trim(), pass.as_str());
    if u.is_empty() && p.is_empty() {
        return Ok(json!([])); // clear both = turn off protection
    }
    if u.is_empty() || p.is_empty() {
        return Err("Fill in Username AND Password, or clear both to turn it off".into());
    }
    Ok(json!([{ "username": u, "password": p }]))
}

/// Fields for the add-mount form. The type decides which fields show: volume→Name,
/// bind→Host path, file→Content (file contents, opened in $EDITOR). Mount path is
/// always shown.
/// Fields for the mount form; `existing` prefills them when editing one.
///
/// `field()` yields "-" for a value that is not there, which would land in the
/// box as literal text — so an absent value becomes empty instead.
pub(crate) fn mount_fields(existing: Option<&Value>) -> Vec<Field> {
    let get = |ptr: &str, default: &str| match existing.map(|m| field(m, ptr)) {
        Some(v) if v != "-" => v,
        _ => default.to_string(),
    };
    vec![
        Field::choice("Type", MOUNT_TYPES, &get("/type", "volume")),
        Field::text("Name", &get("/name", "")).when("Type", "volume"),
        Field::text("Host path", &get("/hostPath", "")).when("Type", "bind"),
        Field::editor("Content", &get("/content", "")).when("Type", "file"),
        Field::text("Mount path", &get("/mountPath", "")),
    ]
}

/// The `values` object for createMount, per type (shape verified against the
/// server). Required fields are validated here so the message is clear, rather than
/// a raw server error.
pub(crate) fn mount_body(form: &Form) -> std::result::Result<Value, String> {
    let mount_path = form.by_label("Mount path");
    if mount_path.trim().is_empty() {
        return Err("Mount path is required".into());
    }
    match form.by_label("Type").as_str() {
        "bind" => {
            let host = form.by_label("Host path");
            if host.trim().is_empty() {
                return Err("Host path is required for a bind mount".into());
            }
            Ok(json!({ "type": "bind", "hostPath": host, "mountPath": mount_path }))
        }
        "file" => {
            let content = form.by_label("Content");
            Ok(json!({ "type": "file", "content": content, "mountPath": mount_path }))
        }
        _ => {
            let name = form.by_label("Name");
            if name.trim().is_empty() {
                return Err("Name is required for a volume mount".into());
            }
            Ok(json!({ "type": "volume", "name": name, "mountPath": mount_path }))
        }
    }
}

/// A fresh listMounts value supplies the path as well as the contents, so an
/// editor save cannot turn a volume/bind mount into a file by accident.
pub(crate) fn mount_file_target(values: &Value) -> std::result::Result<(String, String), String> {
    let kind = field(values, "/type");
    if kind != "file" {
        return Err(format!(
            "A {kind} mount has no contents to edit — press e to change where it is mounted"
        ));
    }
    let path = values
        .get("mountPath")
        .and_then(Value::as_str)
        .unwrap_or("");
    if path.trim().is_empty() {
        return Err("Mount path is required".into());
    }
    // Missing contents mean an empty file, not field()'s display placeholder.
    let content = values.get("content").and_then(Value::as_str).unwrap_or("");
    Ok((path.to_string(), content.to_string()))
}

/// The `values` object for createPort: `{published, target, protocol}`.
///
/// Both are numbers in the API, so they're parsed; a non-numeric value is rejected
/// with a clear message rather than sending a 0 that opens the wrong port.
pub(crate) fn port_body(form: &Form) -> std::result::Result<Value, String> {
    let published: u32 = form
        .by_label("Published")
        .parse()
        .map_err(|_| "Published must be a port number (e.g. 8080)")?;
    let target: u32 = form
        .by_label("Target")
        .parse()
        .map_err(|_| "Target must be a port number (e.g. 80)")?;
    Ok(json!({
        "published": published,
        "target": target,
        "protocol": form.by_label("Protocol"),
    }))
}

// ---------- Form (ratatui has no input widget, so we build our own) ----------

#[derive(PartialEq, Clone)]
pub(crate) enum FieldKind {
    Text,
    Secret,
    Bool,
    /// A choice from real data (project/service/protocol), cycled with space/←/→.
    /// Dynamic so its contents can come from the API, not be typed by hand.
    Choice(Vec<String>),
    /// SEVERAL values from real data, ticked in the same dropdown a Choice opens.
    ///
    /// `None` means the list has been ASKED FOR and has not arrived yet. That is
    /// not the same as an empty list, and the difference matters: an empty
    /// dropdown reads as "this service holds no databases", which is a far more
    /// alarming answer than "not back from the panel yet".
    Multi(Option<Vec<String>>),
    /// Multi-line content edited in $EDITOR, like env.
    ///
    /// A Dockerfile never fits in a single-line field; forcing it into
    /// FieldKind::Text means the user types a literal "\n" and sends one long line
    /// that will never build.
    Editor,
}

impl FieldKind {
    pub(crate) fn is_typed(&self) -> bool {
        matches!(self, FieldKind::Text | FieldKind::Secret)
    }
}

pub(crate) struct Field {
    pub(crate) label: &'static str,
    pub(crate) value: String,
    pub(crate) kind: FieldKind,
    /// Show conditions, AND-combined: (label of the deciding field, tags separated
    /// by commas e.g. "github,git"). Empty = always shown.
    ///
    /// The panel works this way too: choosing Service/Custom swaps the fields
    /// below it, rather than showing both at once.
    pub(crate) only_for: Vec<(&'static str, &'static str)>,
    /// The wizard step this field appears on. 0 = first step. A form whose fields
    /// are all on step 0 stays an ordinary single page; as soon as a field lands on
    /// a step > 0, that form becomes a staged wizard. Submit values are still read
    /// from ALL steps at once.
    pub(crate) step: u8,
    /// For a `Multi` field, what ticking NOTHING actually does ("all of them").
    /// Empty on every other kind, which has no such meaning.
    pub(crate) blank_means: &'static str,
}

impl Field {
    /// A field whose contents are edited in $EDITOR, not typed in the form.
    pub(crate) fn editor(label: &'static str, value: &str) -> Self {
        Self {
            label,
            value: value.into(),
            kind: FieldKind::Editor,
            only_for: Vec::new(),
            step: 0,
            blank_means: "",
        }
    }
    pub(crate) fn text(label: &'static str, value: &str) -> Self {
        Self {
            label,
            value: value.into(),
            kind: FieldKind::Text,
            only_for: Vec::new(),
            step: 0,
            blank_means: "",
        }
    }
    /// Show this field only when the `switch` field holds one of `tags` (comma
    /// separated). Can be called more than once: the conditions are AND-combined,
    /// so a single form can have more than one decider — the "New service" form
    /// needs "service type = app" AND "source type = github" at once.
    pub(crate) fn when(mut self, switch: &'static str, tags: &'static str) -> Self {
        self.only_for.push((switch, tags));
        self
    }

    /// Place the field on wizard step `n` (0 = first step).
    pub(crate) fn step(mut self, n: u8) -> Self {
        self.step = n;
        self
    }
    pub(crate) fn secret(label: &'static str) -> Self {
        Self::secret_val(label, "")
    }
    pub(crate) fn secret_val(label: &'static str, value: &str) -> Self {
        Self {
            label,
            value: value.into(),
            kind: FieldKind::Secret,
            only_for: Vec::new(),
            step: 0,
            blank_means: "",
        }
    }
    pub(crate) fn boolean(label: &'static str, on: bool) -> Self {
        Self {
            label,
            value: if on { "yes".into() } else { "no".into() },
            kind: FieldKind::Bool,
            only_for: Vec::new(),
            step: 0,
            blank_means: "",
        }
    }
    pub(crate) fn choice(label: &'static str, options: &[&str], value: &str) -> Self {
        Self::choice_owned(
            label,
            options.iter().map(|o| o.to_string()).collect(),
            value,
        )
    }
    pub(crate) fn choice_owned(label: &'static str, options: Vec<String>, value: &str) -> Self {
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
            blank_means: "",
        }
    }
    /// Replace the list of choices (e.g. services filled in after a project is
    /// chosen).
    ///
    /// The current value is always kept, even if it's absent from the new list:
    /// silently jumping to the first choice would change config the user didn't ask
    /// for — e.g. a `ref` that's a tag would switch to the first branch
    /// alphabetically, then get deployed along with it.
    pub(crate) fn set_options(&mut self, mut options: Vec<String>) {
        if !self.value.is_empty() && !options.contains(&self.value) {
            options.insert(0, self.value.clone());
        }
        if !options.contains(&self.value) {
            self.value = options.first().cloned().unwrap_or_default();
        }
        self.kind = FieldKind::Choice(options);
    }
    /// A field whose value is SEVERAL names ticked from a list that is still on
    /// its way. `blank_means` is what an empty selection actually does, shown in
    /// place of the value so the contract is on screen rather than implied.
    pub(crate) fn multi(label: &'static str, blank_means: &'static str) -> Self {
        Self {
            label,
            value: String::new(),
            kind: FieldKind::Multi(None),
            only_for: Vec::new(),
            step: 0,
            blank_means,
        }
    }
    /// The list arrived. An EMPTY list is not offered as a picker — the caller
    /// falls the field back to typing, because a dropdown with nothing in it
    /// cannot be told apart from a list that hasn't loaded.
    pub(crate) fn set_multi_options(&mut self, options: Vec<String>) {
        self.kind = FieldKind::Multi(Some(options));
    }
    /// The names currently ticked, in the order they are stored.
    pub(crate) fn ticked(&self) -> Vec<String> {
        self.value
            .split(',')
            .map(str::trim)
            .filter(|s| !s.is_empty())
            .map(str::to_string)
            .collect()
    }
    /// Store `names` as this field's value — the same comma-separated spelling a
    /// typed fallback produces, so submit reads one shape either way.
    pub(crate) fn set_ticked(&mut self, names: &[String]) {
        self.value = names.join(", ");
    }
    /// Cycle to the next choice (Bool is treated as yes/no).
    pub(crate) fn cycle(&mut self) {
        match self.kind {
            FieldKind::Bool => {
                self.value = if self.is_on() {
                    "no".into()
                } else {
                    "yes".into()
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
    pub(crate) fn is_on(&self) -> bool {
        self.value == "yes"
    }
    pub(crate) fn shown(&self) -> String {
        match self.kind {
            FieldKind::Secret => "•".repeat(self.value.chars().count()),
            // The contents can be hundreds of lines: what's useful here is whether
            // it exists and how big it is, not a snippet of its first line.
            FieldKind::Editor if self.value.trim().is_empty() => "(empty)".into(),
            FieldKind::Editor => format!("{} lines", self.value.lines().count()),
            // Nothing ticked is a MEANING, not an absence — so it is spelled out
            // rather than left as an empty box the operator has to remember the
            // rule for. And a list that has not arrived says so: an empty picker
            // would otherwise read as "there are none".
            FieldKind::Multi(None) if self.value.is_empty() => {
                "(reading the source's databases…)".into()
            }
            FieldKind::Multi(_) if self.value.is_empty() => self.blank_means.into(),
            _ => self.value.clone(),
        }
    }
}

/// What the form does when submitted.
pub(crate) enum FormKind {
    ServerAdd,
    /// Add a Cloudflare account (name, API token, optional account id) to the local
    /// config. A LOCAL submit — resolved by the event loop, never the worker.
    CfAccountAdd,
    /// Edit a stored Cloudflare account (name, API token, optional account id).
    CfAccountEdit {
        name: String,
    },
    ServerEdit {
        name: String,
    },
    ProjectCreate,
    /// Project is one of the form fields: a flat list has no "currently open
    /// project" to inherit.
    ServiceCreate,
    DomainCreate,
    DomainEdit {
        id: String,
    },
    /// Edit what an enrolled domain is checked WITH: method, body, headers,
    /// the status it should answer, and how long to wait.
    CheckEdit {
        url: String,
    },
    /// Compare this service against the same one on another host.
    DiffAcross {
        project: String,
        service: String,
        stype: String,
    },
    /// Compare a WHOLE project against the same project on another host.
    DiffProjectAcross {
        project: String,
    },
    /// Rewrite one part of MANY domains at once. Which domains is decided by the
    /// filter that is already on screen, not by a second selection UI.
    DomainBulkEdit,
    /// Replicas, start command and zero-downtime — the `deploy` block.
    DeployEdit {
        project: String,
        service: String,
    },
    /// Change an existing mount. `index` is its position as the server listed it.
    MountEdit {
        project: String,
        service: String,
        index: usize,
    },
    /// Add a port exposed to a service.
    PortCreate {
        project: String,
        service: String,
    },
    /// Search for a keyword in the logs of all services at once.
    LogSearch,
    /// Run a statement against the database being browsed. The engine's own
    /// language (SQL, or JavaScript for mongosh) — it is sent verbatim.
    DbQuery,
    /// Pick which OTHER server to read backups from, to restore one here.
    RestoreFrom {
        project: String,
        service: String,
    },
    SourceEdit {
        project: String,
        service: String,
    },
    BuildEdit {
        project: String,
        service: String,
    },
    /// Set a service's CPU/memory limit (any type). `stype` decides the endpoint
    /// group (services/{stype}/updateResources).
    ResourceEdit {
        project: String,
        service: String,
        stype: String,
    },
    /// Set the SAME CPU/memory limit on every marked service at once. Carries no
    /// target — the marked set is read at submit time, each service updated under
    /// its own `services/{stype}` group.
    BulkResourceEdit,
    /// Clone a service's config into a new service (name filled by the user).
    CloneService {
        project: String,
        service: String,
        stype: String,
    },
    /// Add a mount (volume/bind/file) to a service.
    MountCreate {
        project: String,
        service: String,
    },
    /// Set basic auth (user/password protection) on a web service.
    BasicAuthEdit {
        project: String,
        service: String,
        stype: String,
    },
    /// Add one redirect rule to a web service.
    RedirectCreate {
        project: String,
        service: String,
        stype: String,
    },
    /// Copy config to another host. `service` empty = the whole project.
    Migrate {
        project: String,
        service: String,
        stype: String,
    },
    /// Copy DATABASES (not config) out of a service into another one — another
    /// project, or another host. Unlike `Migrate` the target host may be the
    /// current one, so its picker includes this server rather than excluding it.
    ///
    /// No `stype`: the engine is gated before the form opens and resolved again by
    /// the worker from BOTH panels, so carrying a copy of it here would be a third
    /// place for it to be wrong.
    CopyDatabase {
        project: String,
        service: String,
    },
    /// Add a DNS record to the current Cloudflare zone (type/name/content/…).
    CfRecordCreate,
    /// Edit the selected DNS record. `kind` decides which fields are sent
    /// (proxied only for A/AAAA/CNAME, priority only for MX).
    CfRecordEdit {
        id: String,
        kind: String,
    },
    /// Create a zone under the active account.
    CfZoneCreate,
    /// Delete a zone behind a typed-name confirmation — deleting a zone destroys
    /// all its DNS records, so a bare y/n is not enough.
    CfZoneDelete {
        zone_id: String,
        name: String,
    },
    /// Set one attribute on every marked DNS record at once.
    CfBulkSet(CfBulkAttr),
    /// Create an R2 bucket under the active account.
    CfBucketCreate,
    /// Delete an R2 bucket behind a typed-name confirmation — a destructive op, so
    /// a bare y/n is not enough.
    CfBucketDelete {
        name: String,
    },
    /// Deploy or replace a Worker script from one local file.
    CfWorkerDeploy,
    /// Delete a Worker script behind a typed-name confirmation.
    CfWorkerDelete {
        name: String,
    },
    /// Create a remotely configured Cloudflare Tunnel under the active account.
    CfTunnelCreate,
    /// Delete one Cloudflare Tunnel behind typed-name confirmation.
    CfTunnelDelete {
        tunnel_id: String,
        name: String,
    },
    /// Add one route to the currently open Cloudflare Tunnel config.
    CfTunnelRouteCreate {
        tunnel_id: String,
    },
    /// Edit the selected Cloudflare Tunnel route. `path` identifies the old route
    /// when a hostname has multiple path-specific entries.
    CfTunnelRouteEdit {
        tunnel_id: String,
        hostname: String,
        path: String,
    },
    /// Delete one Cloudflare Tunnel route behind typed-host confirmation.
    CfTunnelRouteDelete {
        tunnel_id: String,
        hostname: String,
        path: String,
    },
    /// Upload a local file into the currently-browsed bucket/prefix. The worker reads
    /// the file and computes the key; the form carries only the local path.
    R2Upload,
    /// Download the selected object to a local path. `key` is the object being fetched;
    /// the field defaults to its basename (saved in the CWD).
    R2Download {
        key: String,
    },
}

/// Which attribute a Cloudflare bulk edit sets on the marked records.
#[derive(Clone, Copy)]
pub(crate) enum CfBulkAttr {
    Content,
    Proxied,
    Ttl,
    Priority,
}

pub(crate) struct Form {
    pub(crate) kind: FormKind,
    pub(crate) title: String,
    pub(crate) fields: Vec<Field>,
    pub(crate) focus: usize,
    /// The original JSON in edit mode. Submit starts from here so fields not in the
    /// form (middlewares on a domain, nixpacksVersion on a build) stay intact.
    pub(crate) original: Option<Value>,
    /// The wizard step currently shown. 0 for an ordinary single-page form.
    pub(crate) step: usize,
    /// Guidance that belongs to THIS form — "0 = unlimited", "config only, no
    /// data". It used to be written to the status line, which fades after six
    /// seconds while the form is still open, so the explanation vanished from
    /// under the user mid-edit. Drawn on the form's own border instead, where it
    /// lasts exactly as long as the form does.
    pub(crate) note: Option<String>,
    /// Why the last attempt to leave this step was refused. Shown on the form's
    /// own border, in place of the note, so it cannot fade out from under the
    /// user while they are looking for the field it names.
    pub(crate) error: Option<String>,
}

impl Form {
    pub(crate) fn new(kind: FormKind, title: impl Into<String>, fields: Vec<Field>) -> Self {
        Self {
            kind,
            title: title.into(),
            fields,
            focus: 0,
            original: None,
            step: 0,
            note: None,
            error: None,
        }
    }
    /// Guidance shown on the form's border for as long as the form is open.
    pub(crate) fn with_note(mut self, note: impl Into<String>) -> Self {
        self.note = Some(note.into());
        self
    }
    pub(crate) fn with_original(mut self, original: Value) -> Self {
        self.original = Some(original);
        self
    }
    /// Indices of the fields shown right now.
    ///
    /// Each field carries its own condition, so a single form can have more than
    /// one decider. Without that, the "New service" form couldn't carry a source
    /// too: it needs "service type = app" AND "source type = github".
    pub(crate) fn visible(&self) -> Vec<usize> {
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

    /// The steps that actually have visible fields, sorted. For a database service
    /// this is just `[0]` (source/build fields depend on Kind=app), so its form
    /// stays a single page; for an app `[0, 1, 2]` → wizard.
    pub(crate) fn steps_present(&self) -> Vec<u8> {
        let mut steps: Vec<u8> = self
            .visible()
            .iter()
            .map(|&i| self.fields[i].step)
            .collect();
        steps.sort_unstable();
        steps.dedup();
        steps
    }

    /// This form is staged (more than one step holds fields).
    pub(crate) fn is_wizard(&self) -> bool {
        self.steps_present().len() > 1
    }

    /// Fields shown ON THE CURRENT STEP. Differs from visible(), which spans steps
    /// and is used at submit to read every value at once.
    pub(crate) fn visible_here(&self) -> Vec<usize> {
        let step = self.step as u8;
        self.visible()
            .into_iter()
            .filter(|&i| self.fields[i].step == step)
            .collect()
    }

    /// The next populated step after the current one, if any.
    pub(crate) fn next_present_step(&self) -> Option<usize> {
        self.steps_present()
            .into_iter()
            .map(usize::from)
            .find(|&s| s > self.step)
    }

    /// The populated step before the current one, if any.
    pub(crate) fn prev_present_step(&self) -> Option<usize> {
        self.steps_present()
            .into_iter()
            .rev()
            .map(usize::from)
            .find(|&s| s < self.step)
    }

    /// Move to step `step` and put focus on its first field.
    pub(crate) fn goto_step(&mut self, step: usize) {
        self.step = step;
        self.focus = self.visible_here().first().copied().unwrap_or(0);
    }

    /// Move focus `delta` steps among the fields shown on this step.
    pub(crate) fn move_focus(&mut self, delta: isize) {
        let vis = self.visible_here();
        if vis.is_empty() {
            return;
        }
        let at = vis.iter().position(|i| *i == self.focus).unwrap_or(0) as isize;
        let next = (at + delta).rem_euclid(vis.len() as isize) as usize;
        self.focus = vis[next];
    }

    /// After Destination changes, focus may be left on a now-hidden field.
    pub(crate) fn clamp_focus(&mut self) {
        let vis = self.visible_here();
        if !vis.contains(&self.focus) {
            self.focus = vis.first().copied().unwrap_or(0);
        }
    }

    pub(crate) fn val(&self, i: usize) -> String {
        let field = &self.fields[i];
        match field.kind {
            FieldKind::Editor | FieldKind::Secret => field.value.clone(),
            _ => field.value.trim().to_string(),
        }
    }
    pub(crate) fn by_label(&self, label: &str) -> String {
        self.fields
            .iter()
            .find(|f| f.label == label)
            .map(|f| match f.kind {
                FieldKind::Editor | FieldKind::Secret => f.value.clone(),
                _ => f.value.trim().to_string(),
            })
            .unwrap_or_default()
    }
    pub(crate) fn is_on_label(&self, label: &str) -> bool {
        self.fields
            .iter()
            .find(|f| f.label == label)
            .map(Field::is_on)
            .unwrap_or(false)
    }
}

const TUNNEL_SERVICE_TYPES: &[&str] = &[
    "http",
    "https",
    "unix",
    "unix+tls",
    "tcp",
    "ssh",
    "rdp",
    "smb",
    "http_status",
    "bastion",
    "hello_world",
];

pub(crate) fn tunnel_service_parts(service: &str) -> (&'static str, String) {
    for service_type in TUNNEL_SERVICE_TYPES {
        let prefix = format!("{service_type}://");
        if let Some(rest) = service.strip_prefix(&prefix) {
            return (service_type, rest.to_string());
        }
    }
    if let Some(rest) = service.strip_prefix("http_status:") {
        return ("http_status", rest.to_string());
    }
    match service {
        "bastion" => ("bastion", String::new()),
        "hello_world" => ("hello_world", String::new()),
        _ => ("http", service.trim().to_string()),
    }
}

pub(crate) fn tunnel_service_from_form(form: &Form) -> String {
    let service_type = form.by_label("Service Type");
    let value = form.by_label("Service URL").trim().to_string();
    match service_type.as_str() {
        "http_status" => format!("http_status:{value}"),
        "bastion" | "hello_world" => service_type.clone(),
        "unix" | "unix+tls" => format!("{service_type}:{value}"),
        _ => format!("{service_type}://{value}"),
    }
}

pub(crate) fn tunnel_service_requires_value(service_type: &str) -> bool {
    !matches!(service_type, "bastion" | "hello_world")
}

pub(crate) fn tunnel_service_fields(service: &str) -> Vec<Field> {
    let (service_type, service_url) = tunnel_service_parts(service);
    vec![
        Field::choice("Service Type", TUNNEL_SERVICE_TYPES, service_type),
        Field::text("Service URL", &service_url).when(
            "Service Type",
            "http,https,unix,unix+tls,tcp,ssh,rdp,smb,http_status",
        ),
    ]
}

fn origin_object(origin: Option<&Value>) -> serde_json::Map<String, Value> {
    origin
        .and_then(Value::as_object)
        .cloned()
        .unwrap_or_default()
}

fn origin_bool(origin: Option<&Value>, key: &str) -> &'static str {
    match origin.and_then(|v| v.get(key)).and_then(Value::as_bool) {
        Some(true) => "Enabled",
        Some(false) => "Disabled",
        None => "Inherit",
    }
}

fn origin_text(origin: Option<&Value>, key: &str) -> String {
    origin
        .and_then(|v| v.get(key))
        .and_then(Value::as_str)
        .unwrap_or_default()
        .to_string()
}

fn origin_integer_text(origin: Option<&Value>, key: &str) -> String {
    origin
        .and_then(|v| v.get(key))
        .map(Value::to_string)
        .unwrap_or_default()
}

fn origin_access(origin: Option<&Value>) -> Option<&serde_json::Map<String, Value>> {
    origin
        .and_then(|v| v.get("access"))
        .and_then(Value::as_object)
}

fn origin_access_requirement(origin: Option<&Value>) -> &'static str {
    match origin_access(origin) {
        None => "Inherit",
        Some(access) if access.get("required").and_then(Value::as_bool) == Some(true) => "Require",
        Some(_) => "Disable requirement",
    }
}

fn origin_access_team(origin: Option<&Value>) -> String {
    origin_access(origin)
        .and_then(|v| v.get("teamName"))
        .and_then(Value::as_str)
        .unwrap_or_default()
        .to_string()
}

fn origin_access_aud(origin: Option<&Value>) -> String {
    origin_access(origin)
        .and_then(|v| v.get("audTag"))
        .and_then(Value::as_array)
        .map(|items| {
            items
                .iter()
                .filter_map(Value::as_str)
                .collect::<Vec<_>>()
                .join(", ")
        })
        .unwrap_or_default()
}

pub(crate) fn tunnel_origin_fields(origin: Option<&Value>) -> Vec<Field> {
    let proxy_type = match origin
        .and_then(|v| v.get("proxyType"))
        .and_then(Value::as_str)
    {
        Some("") => "regular",
        Some(value) => value,
        None => "Inherit",
    };
    let mut proxy_choices = vec!["Inherit".into(), "regular".into(), "socks".into()];
    if !proxy_choices.iter().any(|value| value == proxy_type) {
        proxy_choices.push(proxy_type.into());
    }
    let bool_choices = &["Inherit", "Enabled", "Disabled"];
    vec![
        Field::text(
            "Origin Server Name",
            &origin_text(origin, "originServerName"),
        ),
        Field::choice(
            "Match SNI to Host",
            bool_choices,
            origin_bool(origin, "matchSNItoHost"),
        ),
        Field::text("Certificate Authority Pool", &origin_text(origin, "caPool")),
        Field::choice(
            "No TLS Verify",
            bool_choices,
            origin_bool(origin, "noTLSVerify"),
        ),
        Field::text(
            "TLS Timeout (seconds)",
            &origin_integer_text(origin, "tlsTimeout"),
        ),
        Field::choice(
            "HTTP2 Connection",
            bool_choices,
            origin_bool(origin, "http2Origin"),
        ),
        Field::text("HTTP Host Header", &origin_text(origin, "httpHostHeader")),
        Field::choice(
            "Disable Chunked Encoding",
            bool_choices,
            origin_bool(origin, "disableChunkedEncoding"),
        ),
        Field::text(
            "Connect Timeout (seconds)",
            &origin_integer_text(origin, "connectTimeout"),
        ),
        Field::choice(
            "No Happy Eyeballs",
            bool_choices,
            origin_bool(origin, "noHappyEyeballs"),
        ),
        Field::choice_owned("Proxy Type", proxy_choices, proxy_type),
        Field::text("Proxy Address", &origin_text(origin, "proxyAddress")),
        Field::text("Proxy Port", &origin_integer_text(origin, "proxyPort")),
        Field::text(
            "Keep Alive Timeout (seconds)",
            &origin_integer_text(origin, "keepAliveTimeout"),
        ),
        Field::text(
            "Keep Alive Connections",
            &origin_integer_text(origin, "keepAliveConnections"),
        ),
        Field::text(
            "TCP Keep Alive (seconds)",
            &origin_integer_text(origin, "tcpKeepAlive"),
        ),
        Field::choice(
            "Access Required",
            &["Inherit", "Require", "Disable requirement"],
            origin_access_requirement(origin),
        ),
        Field::text("Access Team Name", &origin_access_team(origin))
            .when("Access Required", "Require,Disable requirement"),
        Field::text("Access AUD Tags", &origin_access_aud(origin))
            .when("Access Required", "Require,Disable requirement"),
    ]
}

fn set_origin_string(object: &mut serde_json::Map<String, Value>, key: &str, value: String) {
    let trimmed = value.trim();
    if trimmed.is_empty() {
        object.remove(key);
    } else {
        object.insert(key.into(), Value::String(trimmed.to_string()));
    }
}

pub(crate) fn tunnel_origin_request_from_form(form: &Form) -> anyhow::Result<Option<Value>> {
    let mut object = origin_object(form.original.as_ref());
    let original_fields = tunnel_origin_fields(form.original.as_ref());
    // Only changed controls replace raw values; absence, empty overrides and opaque JSON survive.
    let changed = |label: &str| {
        original_fields
            .iter()
            .find(|f| f.label == label)
            .map(|f| &f.value)
            != Some(&form.by_label(label))
    };
    for (label, key) in [
        ("Origin Server Name", "originServerName"),
        ("Certificate Authority Pool", "caPool"),
        ("HTTP Host Header", "httpHostHeader"),
        ("Proxy Address", "proxyAddress"),
    ] {
        if changed(label) {
            set_origin_string(&mut object, key, form.by_label(label));
        }
    }
    for (label, key) in [
        ("Match SNI to Host", "matchSNItoHost"),
        ("No TLS Verify", "noTLSVerify"),
        ("HTTP2 Connection", "http2Origin"),
        ("Disable Chunked Encoding", "disableChunkedEncoding"),
        ("No Happy Eyeballs", "noHappyEyeballs"),
    ] {
        if changed(label) {
            match form.by_label(label).as_str() {
                "Inherit" => {
                    object.remove(key);
                }
                "Enabled" => {
                    object.insert(key.into(), Value::Bool(true));
                }
                "Disabled" => {
                    object.insert(key.into(), Value::Bool(false));
                }
                _ => anyhow::bail!("{label} must be Inherit, Enabled or Disabled"),
            }
        }
    }
    for (label, key) in [
        ("TLS Timeout (seconds)", "tlsTimeout"),
        ("Connect Timeout (seconds)", "connectTimeout"),
        ("Keep Alive Timeout (seconds)", "keepAliveTimeout"),
        ("TCP Keep Alive (seconds)", "tcpKeepAlive"),
        ("Keep Alive Connections", "keepAliveConnections"),
        ("Proxy Port", "proxyPort"),
    ] {
        let value = form.by_label(label);
        let value = value.trim();
        if !changed(label) {
            if let Some(raw) = object.get(key) {
                let valid = if key == "proxyPort" {
                    raw.is_u64()
                } else {
                    raw.is_i64()
                };
                if !valid {
                    anyhow::bail!(
                        "{label} must be a JSON integer; correct the value or clear it to inherit"
                    );
                }
            }
            continue;
        }
        if value.is_empty() {
            object.remove(key);
        } else {
            let number =
                if key == "proxyPort" {
                    serde_json::Number::from(value.parse::<u64>().map_err(|_| {
                        anyhow::anyhow!("Proxy Port must be a non-negative integer")
                    })?)
                } else {
                    serde_json::Number::from(
                        value
                            .parse::<i64>()
                            .map_err(|_| anyhow::anyhow!("{label} must be an integer"))?,
                    )
                };
            object.insert(key.into(), Value::Number(number));
        }
    }
    if changed("Proxy Type") {
        match form.by_label("Proxy Type").as_str() {
            "Inherit" => {
                object.remove("proxyType");
            }
            "regular" => {
                object.insert("proxyType".into(), Value::String(String::new()));
            }
            "socks" => {
                object.insert("proxyType".into(), Value::String("socks".into()));
            }
            _ => anyhow::bail!("Proxy Type must be Inherit, regular or socks"),
        }
    }
    let access_changed = changed("Access Required");
    if access_changed || changed("Access Team Name") || changed("Access AUD Tags") {
        let requirement = form.by_label("Access Required");
        match requirement.as_str() {
            "Inherit" => {
                object.remove("access");
            }
            "Require" | "Disable requirement" => {
                let required = requirement == "Require";
                let team_name = form.by_label("Access Team Name").trim().to_string();
                let aud_tags: Vec<Value> = form
                    .by_label("Access AUD Tags")
                    .split(',')
                    .map(str::trim)
                    .filter(|v| !v.is_empty())
                    .map(|v| Value::String(v.to_string()))
                    .collect();
                if required && team_name.is_empty() {
                    anyhow::bail!("Access Team Name is required when Access is enabled");
                }
                if required && aud_tags.is_empty() {
                    anyhow::bail!("Access AUD Tags is required when Access is enabled");
                }
                let mut access = origin_access(form.original.as_ref())
                    .cloned()
                    .unwrap_or_default();
                if access_changed {
                    access.insert("required".into(), Value::Bool(required));
                }
                // REST requires both members, but disabled Access needs no invented identity.
                if changed("Access Team Name") || !access.contains_key("teamName") {
                    access.insert("teamName".into(), Value::String(team_name));
                }
                if changed("Access AUD Tags") || !access.contains_key("audTag") {
                    access.insert("audTag".into(), Value::Array(aud_tags));
                }
                object.insert("access".into(), Value::Object(access));
            }
            _ => anyhow::bail!("Access Required must be Inherit, Require or Disable requirement"),
        }
    }
    if object.is_empty() {
        Ok(None)
    } else {
        Ok(Some(Value::Object(object)))
    }
}

pub(crate) enum ServerAction {
    Save {
        /// The name it is stored under today. `Some` only when the edit form
        /// changed it, which makes the save a rename as well.
        rename_from: Option<String>,
        name: String,
        url: String,
        /// None = keep the stored token (an edit form left blank).
        token: Option<String>,
        /// The SSH hop; None = connect directly. Its `secret` is None when the
        /// form left it blank, which on an edit means "keep the stored one".
        ssh: Option<SshTunnel>,
    },
    Remove(String),
}

/// A configured server as the UI knows it — no token and no SSH secret, which
/// the App never holds. The URL and tunnel are kept so the edit form can be
/// prefilled with the current values, not left blank like the add form.
#[derive(Clone)]
pub(crate) struct ServerEntry {
    pub(crate) name: String,
    pub(crate) url: String,
    /// Always with `secret: None`.
    pub(crate) ssh: Option<SshTunnel>,
}

impl ServerEntry {
    /// The address as a person reads it: a tunnelled panel's URL alone
    /// (`http://localhost:3000`) would name every tunnelled host the same.
    pub(crate) fn display_url(&self) -> String {
        match &self.ssh {
            Some(t) => format!("{} via ssh {}", self.url, t.describe()),
            None => self.url.clone(),
        }
    }
}

impl From<crate::config::Server> for ServerEntry {
    fn from(s: crate::config::Server) -> Self {
        Self {
            name: s.name,
            url: s.url,
            ssh: s.ssh.map(|t| SshTunnel { secret: None, ..t }),
        }
    }
}

// The server form's labels: `submit_form` and the Ctrl-T test read by label.
pub(crate) const SRV_CONNECTION: &str = "Connection";
pub(crate) const SRV_SSH_HOST: &str = "SSH host";
pub(crate) const SRV_SSH_PORT: &str = "SSH port";
pub(crate) const SRV_SSH_USER: &str = "SSH user";
pub(crate) const SRV_SSH_AUTH: &str = "Login method";
pub(crate) const SRV_SSH_KEY: &str = "Key path";
pub(crate) const SRV_SSH_PASSPHRASE: &str = "Key passphrase";
pub(crate) const SRV_SSH_PASSWORD: &str = "SSH password";
pub(crate) const SRV_SSH_TIMEOUT: &str = "SSH timeout (s)";

/// The add (`entry` None) or edit server form. Name, URL and token keep indexes
/// 0-2; the tunnel fields appear only when Connection is `ssh`, and each login
/// method shows only its own fields.
pub(crate) fn server_form(entry: Option<&ServerEntry>) -> Form {
    let t = entry.and_then(|e| e.ssh.clone());
    let tunnel = t.clone().unwrap_or_default();
    let (kind, title, token) = match entry {
        None => (
            FormKind::ServerAdd,
            " Add server ".to_string(),
            Field::secret("Token"),
        ),
        Some(e) => (
            FormKind::ServerEdit {
                name: e.name.clone(),
            },
            format!(" Edit server: {} ", e.name),
            // The token is deliberately not re-filled: there's no need to put
            // it back on screen. Empty = keep the stored token.
            Field::secret("Token (empty = unchanged)"),
        ),
    };
    let ssh = |f: Field| f.when(SRV_CONNECTION, "ssh");
    let note = match entry {
        None => {
            "SSH: the URL is then the panel as seen from the SSH host, e.g. \
                 http://localhost:3000. Ctrl-T tests the tunnel."
        }
        Some(_) => {
            "SSH: the URL is the panel as seen from the SSH host. A blank \
                    passphrase or password keeps the stored one. Ctrl-T tests the tunnel."
        }
    };
    Form::new(
        kind,
        title,
        vec![
            // Editable: a server's name is the label the whole UI identifies it
            // by — the title bar, the confirmations and its colour — so a typo in
            // it was permanent, and the only way out was to delete the server and
            // lose its token with it.
            Field::text("Name", entry.map(|e| e.name.as_str()).unwrap_or("")),
            Field::text("URL", entry.map(|e| e.url.as_str()).unwrap_or("https://")),
            token,
            Field::choice(
                SRV_CONNECTION,
                &["direct", "ssh"],
                if t.is_some() { "ssh" } else { "direct" },
            ),
            ssh(Field::text(SRV_SSH_HOST, &tunnel.host)),
            ssh(Field::text(
                SRV_SSH_PORT,
                &tunnel.port.unwrap_or(22).to_string(),
            )),
            ssh(Field::text(
                SRV_SSH_USER,
                tunnel.user.as_deref().unwrap_or(""),
            )),
            ssh(Field::choice(
                SRV_SSH_AUTH,
                &crate::tunnel::SshAuth::NAMES,
                tunnel.auth.as_str(),
            )),
            ssh(Field::text(
                SRV_SSH_KEY,
                tunnel.key_path.as_deref().unwrap_or("~/.ssh/id_ed25519"),
            ))
            .when(SRV_SSH_AUTH, "key"),
            ssh(Field::secret(SRV_SSH_PASSPHRASE)).when(SRV_SSH_AUTH, "key"),
            ssh(Field::secret(SRV_SSH_PASSWORD)).when(SRV_SSH_AUTH, "password"),
            ssh(Field::text(
                SRV_SSH_TIMEOUT,
                &tunnel
                    .timeout
                    .unwrap_or(crate::tunnel::DEFAULT_TIMEOUT)
                    .to_string(),
            )),
        ],
    )
    .with_note(note)
}

/// Is this the add/edit server form with the tunnel switched on?
pub(crate) fn server_form_uses_ssh(form: &Form) -> bool {
    matches!(form.kind, FormKind::ServerAdd | FormKind::ServerEdit { .. })
        && form.by_label(SRV_CONNECTION) == "ssh"
}

/// The tunnel the server form describes (None = direct), checked but for the
/// secret, which a blank edit field keeps from the stored server.
pub(crate) fn tunnel_from_form(form: &Form, url: &str) -> Result<Option<SshTunnel>, String> {
    use crate::tunnel::SshAuth;
    if !server_form_uses_ssh(form) {
        return Ok(None);
    }
    let opt = |label| Some(form.by_label(label)).filter(|v| !v.is_empty());
    let port = match opt(SRV_SSH_PORT) {
        None => None,
        Some(p) => Some(
            p.parse::<u16>()
                .map_err(|_| "SSH port must be 1-65535".to_string())?,
        ),
    };
    let timeout = match opt(SRV_SSH_TIMEOUT) {
        None => None,
        Some(t) => Some(
            t.parse::<u64>()
                .map_err(|_| "SSH timeout must be a number of seconds".to_string())?,
        ),
    };
    let auth = SshAuth::parse(&form.by_label(SRV_SSH_AUTH)).unwrap_or_default();
    let tunnel = SshTunnel {
        host: form.by_label(SRV_SSH_HOST),
        // 22 is what the field starts at; storing it would override the
        // alias's own `Port` in ~/.ssh/config.
        port: port.filter(|p| *p != 22),
        user: opt(SRV_SSH_USER),
        auth,
        key_path: (auth == SshAuth::Key).then(|| opt(SRV_SSH_KEY)).flatten(),
        secret: match auth {
            SshAuth::Agent => None,
            SshAuth::Key => opt(SRV_SSH_PASSPHRASE),
            SshAuth::Password => opt(SRV_SSH_PASSWORD),
        },
        timeout: timeout.filter(|t| *t != crate::tunnel::DEFAULT_TIMEOUT),
    };
    crate::tunnel::validate(&tunnel, url).map_err(|e| e.to_string())?;
    Ok(Some(tunnel))
}

/// The URL the server form means: a tunnel left on the add form's `https://`
/// placeholder means EasyPanel's own port on the SSH host.
pub(crate) fn server_form_url(form: &Form) -> String {
    let url = form.val(1);
    let url = url.trim_end_matches('/');
    if server_form_uses_ssh(form) && matches!(url, "" | "https:" | "https://" | "http://") {
        return crate::tunnel::DEFAULT_REMOTE_URL.to_string();
    }
    url.to_string()
}

pub(crate) fn service_create_form(projects: &[String], project: &str) -> Form {
    // The database fields follow Kind, like the panel dialog. All optional:
    // empty means the server creates them (a random password, a database named
    // after the project, the latest official image) — exactly like the panel.
    let mut fields = vec![
        Field::choice_owned("Project", projects.to_vec(), project),
        Field::text("Name", ""),
        Field::choice("Kind", SERVICE_TYPES, "app"),
        Field::text("Database", "").when("Kind", "mysql,mariadb,postgres"),
        Field::text("User", "").when("Kind", "mysql,mariadb,postgres,mongo"),
        Field::secret("Password").when("Kind", "mysql,mariadb,postgres,mongo,redis"),
        Field::secret("Root password").when("Kind", "mysql,mariadb"),
        Field::text("Image", "").when("Kind", "mysql,mariadb,postgres,mongo,redis"),
    ];
    // The source fields carry their own condition (Source=github/git/image);
    // .when() adds a condition rather than replacing it, so both apply: shown
    // only when service type = app AND the source type matches.
    //
    // The repo list follows via Resp::Repos: waiting for it here would freeze
    // the TUI until searchRepos finishes.
    // The wizard follows the EasyPanel dashboard flow: Basics → Source → Build.
    // The source & build fields are app-only (`.when("Kind","app")`), so a
    // database service stays a single step. `.step()` puts them on their own
    // pages; submit values are still read across steps.
    fields.extend(
        source_fields(None, Vec::new())
            .into_iter()
            .map(|f| f.when("Kind", "app").step(1)),
    );
    fields.extend(build_fields(None).into_iter().map(|f| {
        f.when("Kind", "app")
            .when("Source", "github,git,dockerfile")
            .step(2)
    }));
    // Continuing the dashboard flow: Environment then Domains. Both are accepted
    // inline by createService (`env` string, `domains` array; only `host`
    // required). The domain labels are prefixed with "Domain " so "Path" doesn't
    // collide with the source's "Path" — by_label() uses find().
    fields.push(Field::editor("Environment", "").when("Kind", "app").step(3));
    // "Create env file" in the dashboard: write env as a .env file at that path
    // (API: dotEnvPath). The path only shows when its toggle is on.
    fields.push(
        Field::boolean("Create .env file", false)
            .when("Kind", "app")
            .step(3),
    );
    fields.push(
        Field::text(".env file path", ".env")
            .when("Kind", "app")
            .when("Create .env file", "yes")
            .step(3),
    );
    fields.extend(
        [
            Field::text("Domain host", ""),
            Field::text("Domain port", "3000"),
            Field::boolean("Domain HTTPS", true),
            Field::text("Domain path", "/"),
        ]
        .map(|f| f.when("Kind", "app").step(4)),
    );
    Form::new(FormKind::ServiceCreate, " New service ", fields)
}

pub(crate) fn service_create_request(form: &Form) -> Result<crate::worker::Req, String> {
    let (project, service, stype) = (form.val(0), form.val(1), form.val(2));
    if !crate::operations::valid_name(&service) || project.is_empty() {
        return Err("Service names may only contain a-z, 0-9, - and _".into());
    }
    let source = create_source(form)?;
    let mut extra = service_extra(form);
    if let Some(build) = create_build(form) {
        extra["build"] = build;
    }
    if let Some(env) = create_env(form) {
        extra["env"] = json!(env);
        if form.is_on_label("Create .env file") {
            let path = form.by_label(".env file path");
            extra["dotEnvPath"] = json!(if path.is_empty() { ".env".into() } else { path });
        }
    }
    if let Some(domains) = create_domains(form) {
        extra["domains"] = domains;
    }
    Ok(crate::worker::Req::ServiceCreate {
        project,
        service,
        stype,
        extra,
        source,
    })
}

/// Build the patch an edit form describes. `content` is always sent (the field is
/// prefilled from the record); `proxied` only for proxyable types, `priority` only
/// for MX — the same gating `record_body` applies on create.
pub(crate) fn cf_record_patch(
    kind: &str,
    content: &str,
    ttl: &str,
    proxied: bool,
    priority: &str,
) -> crate::cloudflare::RecordPatch {
    crate::cloudflare::RecordPatch {
        content: (!content.is_empty()).then(|| content.to_string()),
        ttl: ttl.trim().parse().ok(),
        proxied: crate::cloudflare::proxyable(kind).then_some(proxied),
        priority: if kind.eq_ignore_ascii_case("MX") {
            priority.trim().parse().ok()
        } else {
            None
        },
    }
}

pub(crate) struct CloudflareFormContext<'a> {
    pub(crate) token: Option<&'a str>,
    pub(crate) account_id: Option<&'a str>,
    pub(crate) zone_id: Option<&'a str>,
    pub(crate) bucket: Option<&'a str>,
    pub(crate) prefix: &'a str,
    pub(crate) record_ids: &'a [String],
}

/// Prepare an explicit Cloudflare action without performing network or file I/O.
pub(crate) fn cloudflare_form_request(
    form: &Form,
    cx: &CloudflareFormContext<'_>,
) -> Result<crate::worker::CfReq, String> {
    use crate::cloudflare::{apply_patch, record_body, valid_record_type, WorkerUploadMode};
    use crate::worker::CfReq;
    match &form.kind {
        FormKind::CfRecordCreate => {
            let kind = form.val(0).trim().to_string();
            if !valid_record_type(&kind) {
                return Err(format!("Unsupported record type '{kind}'"));
            }
            let (name, content) = (
                form.val(1).trim().to_string(),
                form.val(2).trim().to_string(),
            );
            if name.is_empty() || content.is_empty() {
                return Err("Name and content are required".into());
            }
            let ttl_text = form.val(3);
            let ttl = match ttl_text.trim().parse() {
                Ok(ttl) => ttl,
                Err(_) => {
                    return Err("TTL must be a number (use 1 for automatic)".into());
                }
            };
            let proxied = form.val(4) == "yes";
            let priority_text = form.val(5);
            let priority = if priority_text.trim().is_empty() {
                None
            } else {
                match priority_text.trim().parse() {
                    Ok(p) => Some(p),
                    Err(_) => {
                        return Err("Priority must be a number between 0 and 65535".into());
                    }
                }
            };
            let body = record_body(&kind, &name, &content, ttl, proxied, priority);
            let (Some(token), Some(zone)) =
                (cx.token.map(str::to_string), cx.zone_id.map(str::to_string))
            else {
                return Err("No active zone".into());
            };
            Ok(CfReq::CreateRecord {
                token,
                zone_id: zone,
                body,
            })
        }
        FormKind::CfRecordEdit { id, kind } => {
            let (id, kind) = (id.clone(), kind.clone());
            if form.by_label("Content").trim().is_empty() {
                return Err("Content is required".into());
            }
            if form.by_label("TTL").trim().parse::<u32>().is_err() {
                return Err("TTL must be a number (use 1 for automatic)".into());
            }
            let priority_text = form.by_label("Priority");
            if !priority_text.trim().is_empty() && priority_text.trim().parse::<u16>().is_err() {
                return Err("Priority must be a number between 0 and 65535".into());
            }
            let patch = cf_record_patch(
                &kind,
                &form.by_label("Content"),
                &form.by_label("TTL"),
                form.by_label("Proxied") == "yes",
                &form.by_label("Priority"),
            );
            if patch.is_empty() {
                return Err("Nothing to change".into());
            }
            let (Some(token), Some(zone)) =
                (cx.token.map(str::to_string), cx.zone_id.map(str::to_string))
            else {
                return Err("No active zone".into());
            };
            Ok(CfReq::PatchRecord {
                token,
                zone_id: zone,
                id,
                body: apply_patch(&patch),
            })
        }
        FormKind::CfZoneCreate => {
            let name = form.val(0).trim().to_string();
            if name.is_empty() {
                return Err("Zone name is required".into());
            }
            let (Some(token), Some(account_id)) = (
                cx.token.map(str::to_string),
                cx.account_id.map(str::to_string),
            ) else {
                return Err("This account has no account-id — cannot create a zone".into());
            };
            Ok(CfReq::CreateZone {
                token,
                name,
                account_id,
            })
        }
        FormKind::CfZoneDelete { zone_id, name } => {
            let (zone_id, name) = (zone_id.clone(), name.clone());
            if form.val(0) != name {
                return Err("Name did not match — nothing deleted".into());
            }
            let Some(token) = cx.token.map(str::to_string) else {
                return Err("No active account".into());
            };
            Ok(CfReq::DeleteZone { token, zone_id })
        }
        FormKind::CfBulkSet(attr) => {
            let body = match attr {
                CfBulkAttr::Content => {
                    let content = form.by_label("Content").trim().to_string();
                    if content.is_empty() {
                        return Err("Content is required".into());
                    }
                    json!({ "content": content })
                }
                CfBulkAttr::Proxied => {
                    json!({ "proxied": form.by_label("Proxied") == "yes" })
                }
                CfBulkAttr::Ttl => match form.by_label("TTL").trim().parse::<u32>() {
                    Ok(ttl) => json!({ "ttl": ttl }),
                    Err(_) => {
                        return Err("TTL must be a number (use 1 for automatic)".into());
                    }
                },
                CfBulkAttr::Priority => {
                    let priority = form
                        .by_label("Priority")
                        .trim()
                        .parse::<u16>()
                        .map_err(|_| "Priority must be a number between 0 and 65535".to_string())?;
                    apply_patch(&crate::cloudflare::RecordPatch {
                        content: None,
                        ttl: None,
                        proxied: None,
                        priority: Some(priority),
                    })
                }
            };
            let ids = cx.record_ids.to_vec();
            if ids.is_empty() {
                return Err("Nothing marked any more — cancelled".into());
            }
            let (Some(token), Some(zone)) =
                (cx.token.map(str::to_string), cx.zone_id.map(str::to_string))
            else {
                return Err("No active zone".into());
            };
            Ok(CfReq::BulkPatch {
                token,
                zone_id: zone,
                ids,
                body,
            })
        }
        FormKind::CfBucketCreate => {
            let name = form.val(0).trim().to_string();
            if name.is_empty() {
                return Err("Bucket name is required".into());
            }
            let (Some(token), Some(account_id)) = (
                cx.token.map(str::to_string),
                cx.account_id.map(str::to_string),
            ) else {
                return Err("This account has no account-id — R2 is account-scoped".into());
            };
            Ok(CfReq::CreateR2Bucket {
                token,
                account_id,
                name,
            })
        }
        FormKind::CfBucketDelete { name } => {
            let name = name.clone();
            if form.val(0) != name {
                return Err("Name did not match — nothing deleted".into());
            }
            let (Some(token), Some(account_id)) = (
                cx.token.map(str::to_string),
                cx.account_id.map(str::to_string),
            ) else {
                return Err("No active account".into());
            };
            Ok(CfReq::DeleteR2Bucket {
                token,
                account_id,
                name,
            })
        }
        FormKind::CfWorkerDeploy => {
            let name = form.by_label("Worker name").trim().to_string();
            let path = crate::output::expand_tilde(
                form.by_label("Local file").trim(),
                std::env::var("HOME")
                    .or_else(|_| std::env::var("USERPROFILE"))
                    .ok()
                    .as_deref(),
            );
            if name.is_empty() || path.is_empty() {
                return Err("Worker name and local file are required".into());
            }
            let mode = match form.by_label("Mode").as_str() {
                "service-worker" => WorkerUploadMode::ServiceWorker,
                _ => WorkerUploadMode::Module,
            };
            let (Some(token), Some(account_id)) = (
                cx.token.map(str::to_string),
                cx.account_id.map(str::to_string),
            ) else {
                return Err("No active account".into());
            };
            Ok(CfReq::WorkerDeploy {
                token,
                account_id,
                name,
                path,
                mode,
            })
        }
        FormKind::CfWorkerDelete { name } => {
            let name = name.clone();
            if form.val(0) != name {
                return Err("Name did not match — nothing deleted".into());
            }
            let (Some(token), Some(account_id)) = (
                cx.token.map(str::to_string),
                cx.account_id.map(str::to_string),
            ) else {
                return Err("No active account".into());
            };
            Ok(CfReq::WorkerDelete {
                token,
                account_id,
                name,
                force: false,
            })
        }
        FormKind::CfTunnelCreate => {
            let name = form.by_label("Tunnel name").trim().to_string();
            if name.is_empty() {
                return Err("Tunnel name is required".into());
            }
            let (Some(token), Some(account_id)) = (
                cx.token.map(str::to_string),
                cx.account_id.map(str::to_string),
            ) else {
                return Err("This account has no account-id — Tunnels are account-scoped".into());
            };
            Ok(CfReq::TunnelCreate {
                token,
                account_id,
                name,
            })
        }
        FormKind::CfTunnelDelete { tunnel_id, name } => {
            if form.val(0) != name.as_str() {
                return Err("Tunnel name did not match — nothing deleted".into());
            }
            let (Some(token), Some(account_id)) = (
                cx.token.map(str::to_string),
                cx.account_id.map(str::to_string),
            ) else {
                return Err("No active account".into());
            };
            Ok(CfReq::TunnelDelete {
                token,
                account_id,
                tunnel_id: tunnel_id.clone(),
                name: name.clone(),
            })
        }
        FormKind::CfTunnelRouteCreate { tunnel_id } => {
            let (hostname, service_type, service_url, path) = (
                form.by_label("Hostname").trim().to_string(),
                form.by_label("Service Type"),
                form.by_label("Service URL").trim().to_string(),
                form.by_label("Path").trim().to_string(),
            );
            if hostname.is_empty() {
                return Err("Hostname is required".into());
            }
            if tunnel_service_requires_value(&service_type) && service_url.is_empty() {
                return Err("Service URL is required for this service type".into());
            }
            let service = tunnel_service_from_form(form);
            let origin_request = match tunnel_origin_request_from_form(form) {
                Ok(v) => v,
                Err(e) => {
                    return Err(e.to_string());
                }
            };
            let dns = form.by_label("Sync DNS CNAME") == "yes";
            let zone = (!form.by_label("DNS Zone").trim().is_empty())
                .then(|| form.by_label("DNS Zone").trim().to_string());
            let proxied = form.by_label("DNS Proxied") == "yes";
            let (Some(token), Some(account_id)) = (
                cx.token.map(str::to_string),
                cx.account_id.map(str::to_string),
            ) else {
                return Err("No active account".into());
            };
            Ok(CfReq::TunnelRouteAdd {
                token,
                account_id,
                tunnel_id: tunnel_id.clone(),
                hostname,
                service,
                path,
                origin_request,
                dns,
                zone,
                proxied,
            })
        }
        FormKind::CfTunnelRouteEdit {
            tunnel_id,
            hostname,
            path,
        } => {
            let service_type = form.by_label("Service Type");
            let service_url = form.by_label("Service URL").trim().to_string();
            if tunnel_service_requires_value(&service_type) && service_url.is_empty() {
                return Err("Service URL is required for this service type".into());
            }
            let service = tunnel_service_from_form(form);
            let clear = form.by_label("Clear origin request") == "yes";
            let origin_request = if clear {
                Some(None)
            } else {
                match tunnel_origin_request_from_form(form) {
                    Ok(v) => Some(v),
                    Err(e) => {
                        return Err(e.to_string());
                    }
                }
            };
            let dns = form.by_label("Sync DNS CNAME") == "yes";
            let zone = (!form.by_label("DNS Zone").trim().is_empty())
                .then(|| form.by_label("DNS Zone").trim().to_string());
            let proxied = form.by_label("DNS Proxied") == "yes";
            let (Some(token), Some(account_id)) = (
                cx.token.map(str::to_string),
                cx.account_id.map(str::to_string),
            ) else {
                return Err("No active account".into());
            };
            Ok(CfReq::TunnelRouteEdit {
                token,
                account_id,
                tunnel_id: tunnel_id.clone(),
                hostname: hostname.clone(),
                path: path.clone(),
                service,
                origin_request,
                dns,
                zone,
                proxied,
            })
        }
        FormKind::CfTunnelRouteDelete {
            tunnel_id,
            hostname,
            path,
        } => {
            if form.val(0) != hostname.as_str() {
                return Err("Hostname did not match — nothing deleted".into());
            }
            let (Some(token), Some(account_id)) = (
                cx.token.map(str::to_string),
                cx.account_id.map(str::to_string),
            ) else {
                return Err("No active account".into());
            };
            let delete_dns = form.by_label("Delete DNS CNAME") == "yes";
            let zone = (!form.by_label("DNS Zone").trim().is_empty())
                .then(|| form.by_label("DNS Zone").trim().to_string());
            Ok(CfReq::TunnelRouteDelete {
                token,
                account_id,
                tunnel_id: tunnel_id.clone(),
                hostname: hostname.clone(),
                path: path.clone(),
                delete_dns,
                zone,
            })
        }
        FormKind::R2Upload => {
            // A path typed into the form is raw text — expand a leading `~` the way a
            // shell would, so `~/dump.gz` finds the home directory, not a `~` folder.
            let path = crate::output::expand_tilde(
                form.val(0).trim(),
                std::env::var("HOME")
                    .or_else(|_| std::env::var("USERPROFILE"))
                    .ok()
                    .as_deref(),
            );
            if path.is_empty() {
                return Err("Give a local file path".into());
            }
            let (Some(token), Some(account_id), Some(bucket)) = (
                cx.token.map(str::to_string),
                cx.account_id.map(str::to_string),
                cx.bucket.map(str::to_string),
            ) else {
                return Err("No bucket open".into());
            };
            Ok(CfReq::R2Put {
                token,
                account_id,
                bucket,
                prefix: cx.prefix.to_string(),
                path,
            })
        }
        FormKind::R2Download { key } => {
            let key = key.clone();
            // Same `~` expansion as the upload path — a raw form string, not a shell arg.
            let dest = crate::output::expand_tilde(
                form.val(0).trim(),
                std::env::var("HOME")
                    .or_else(|_| std::env::var("USERPROFILE"))
                    .ok()
                    .as_deref(),
            );
            if dest.is_empty() {
                return Err("Give a path to save to".into());
            }
            let (Some(token), Some(account_id), Some(bucket)) = (
                cx.token.map(str::to_string),
                cx.account_id.map(str::to_string),
                cx.bucket.map(str::to_string),
            ) else {
                return Err("No bucket open".into());
            };
            Ok(CfReq::R2Get {
                token,
                account_id,
                bucket,
                key,
                dest,
            })
        }
        _ => Err("This is not a Cloudflare operation form".into()),
    }
}

/// Validate a server editor while keeping blank edit credentials unchanged.
pub(crate) fn server_form_action(form: &Form) -> Result<ServerAction, String> {
    let url = server_form_url(form);
    let name = form.val(0);
    let token = match &form.kind {
        FormKind::ServerAdd => Some(form.val(2)),
        FormKind::ServerEdit { .. } => Some(form.val(2)).filter(|token| !token.is_empty()),
        _ => return Err("This is not a server form".into()),
    };
    if name.is_empty() || url.is_empty() {
        return Err("Name and URL are required".into());
    }
    if token.as_deref() == Some("") {
        return Err("Token is required".into());
    }
    if !crate::operations::valid_name(&name) {
        return Err("Server name may only contain a-z, 0-9, - and _".into());
    }
    let ssh = tunnel_from_form(form, &url).map_err(|e| format!("Error: {e}"))?;
    if matches!(form.kind, FormKind::ServerAdd)
        && ssh
            .as_ref()
            .is_some_and(|t| t.auth == crate::tunnel::SshAuth::Password && t.secret.is_none())
    {
        return Err("Error: SSH password is required".into());
    }
    Ok(ServerAction::Save {
        rename_from: match &form.kind {
            FormKind::ServerEdit { name: old } if *old != name => Some(old.clone()),
            _ => None,
        },
        name,
        url,
        token,
        ssh,
    })
}

pub(crate) fn cloudflare_account_from_form(
    form: &Form,
) -> Result<crate::cloudflare::CloudflareAccount, String> {
    let (name, token, account_id) = (
        form.val(0).trim().to_string(),
        form.val(1).trim().to_string(),
        form.val(2).trim().to_string(),
    );
    if name.is_empty() || token.is_empty() {
        return Err("Name and API token are required".into());
    }
    if !crate::operations::valid_name(&name) {
        return Err("Cloudflare account names may only contain a-z, 0-9, - and _".into());
    }
    Ok(crate::cloudflare::CloudflareAccount {
        name,
        api_token: token,
        account_id: (!account_id.is_empty()).then_some(account_id),
        default: false,
    })
}

/// The UI resolves credentials only after validating an independent destination.
pub(crate) fn validate_copy_target(
    source: (&str, &str, &str),
    target: (&str, &str, &str),
) -> Result<(), String> {
    if target.0.is_empty() {
        return Err("Choose the target server first".into());
    }
    if target.1.is_empty() || target.2.is_empty() {
        return Err("Name the target project and service first".into());
    }
    if source == target {
        return Err("That is the source itself — name a different service".into());
    }
    Ok(())
}

pub(crate) fn clone_service_request(form: &Form) -> Result<crate::worker::Req, String> {
    let FormKind::CloneService {
        project,
        service,
        stype,
    } = &form.kind
    else {
        return Err("This is not a clone form".into());
    };
    let new_name = form.by_label("New name");
    let new_name = new_name.trim();
    let target = form.by_label("Project");
    let target = if target.is_empty() {
        project.clone()
    } else {
        target
    };
    if new_name.is_empty() {
        return Err("Enter the new service name first".into());
    }
    if target == *project && new_name == service {
        return Err(
            "Use a different project, or a different name — they can't be identical".into(),
        );
    }
    Ok(crate::worker::Req::CloneService {
        project: project.clone(),
        service: service.clone(),
        stype: stype.clone(),
        target,
        new_name: new_name.to_string(),
    })
}

pub(crate) fn migration_target(form: &Form) -> Result<(String, String), String> {
    let server = form.by_label("To server");
    let project = form.by_label("Target project").trim().to_string();
    if server.is_empty() {
        return Err("Choose the destination server first".into());
    }
    if project.is_empty() {
        return Err("Enter the target project name first".into());
    }
    Ok((server, project))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn set(form: &mut Form, label: &str, value: &str) {
        form.fields
            .iter_mut()
            .find(|field| field.label == label)
            .unwrap()
            .value = value.into();
    }

    #[test]
    fn create_request_preserves_separate_source_and_omits_blank_database_options() {
        let mut form = service_create_form(&["project".into()], "project");
        set(&mut form, "Name", "web");
        set(&mut form, "Source", "image");
        set(&mut form, "Docker image", "nginx:alpine");
        match service_create_request(&form).unwrap() {
            crate::worker::Req::ServiceCreate {
                extra,
                source: Some(("updateSourceImage", body, None)),
                ..
            } => {
                assert!(extra.get("source").is_none());
                assert_eq!(body["image"], "nginx:alpine");
                assert!(extra.get("build").is_none());
            }
            _ => panic!("source must be stored separately"),
        }
        set(&mut form, "Kind", "postgres");
        match service_create_request(&form).unwrap() {
            crate::worker::Req::ServiceCreate {
                extra,
                source: None,
                ..
            } => {
                assert!(extra.get("password").is_none());
                assert!(extra.get("databaseName").is_none());
            }
            _ => panic!("database creation must omit the app source"),
        }
    }

    #[test]
    fn cloudflare_submission_checks_typed_target_and_numeric_fields_without_a_ui() {
        let cx = CloudflareFormContext {
            token: Some("test-only"),
            account_id: Some("account"),
            zone_id: Some("zone"),
            bucket: None,
            prefix: "",
            record_ids: &[],
        };
        let mut form = Form::new(
            FormKind::CfTunnelDelete {
                tunnel_id: "id".into(),
                name: "edge".into(),
            },
            "Delete",
            vec![Field::text("Confirm", "wrong")],
        );
        assert!(cloudflare_form_request(&form, &cx).is_err());
        form.fields[0].value = "edge".into();
        assert!(
            matches!(cloudflare_form_request(&form,&cx),Ok(crate::worker::CfReq::TunnelDelete { tunnel_id, .. }) if tunnel_id == "id")
        );
        let form = Form::new(
            FormKind::CfRecordEdit {
                id: "record".into(),
                kind: "MX".into(),
            },
            "Edit",
            vec![
                Field::text("Content", "mail.example.com"),
                Field::text("TTL", "invalid"),
                Field::text("Priority", "10"),
            ],
        );
        assert_eq!(
            cloudflare_form_request(&form, &cx).err().as_deref(),
            Some("TTL must be a number (use 1 for automatic)")
        );
    }

    #[test]
    fn clone_rejects_only_the_same_project_and_name_and_migration_requires_a_target() {
        let mut form = Form::new(
            FormKind::CloneService {
                project: "project".into(),
                service: "web".into(),
                stype: "app".into(),
            },
            "Clone",
            vec![
                Field::text("Project", "project"),
                Field::text("New name", "web"),
            ],
        );
        assert!(clone_service_request(&form).is_err());
        set(&mut form, "Project", "other");
        assert!(
            matches!(clone_service_request(&form),Ok(crate::worker::Req::CloneService { target,.. }) if target == "other")
        );
        let mut form = Form::new(
            FormKind::Migrate {
                project: "project".into(),
                service: "web".into(),
                stype: "app".into(),
            },
            "Migrate",
            vec![
                Field::text("To server", ""),
                Field::text("Target project", "destination"),
            ],
        );
        assert!(migration_target(&form).is_err());
        set(&mut form, "To server", "host");
        assert_eq!(
            migration_target(&form).unwrap(),
            ("host".into(), "destination".into())
        );
    }

    #[test]
    fn copy_target_and_blank_server_credentials_remain_safe() {
        assert!(validate_copy_target(("one", "project", "db"), ("one", "project", "db")).is_err());
        assert!(validate_copy_target(("one", "project", "db"), ("two", "project", "db")).is_ok());
        let entry = ServerEntry {
            name: "server".into(),
            url: "https://panel.example".into(),
            ssh: None,
        };
        let form = server_form(Some(&entry));
        assert!(matches!(
            server_form_action(&form),
            Ok(ServerAction::Save { token: None, .. })
        ));
    }
}
