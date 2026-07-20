use ratatui::layout::Rect;
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
pub(super) const MOUNT_TYPES: &[&str] = &["volume", "bind", "file"];
pub(super) const SOURCE_TYPES: &[&str] = &["github", "git", "image", "dockerfile"];
pub(super) const BUILD_TYPES: &[&str] = &[
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
pub(super) type SourceCall = (&'static str, Value, Option<bool>);

/// The updateSource call for a new service: (op, body, auto), or None when it's
/// not an app / the source hasn't been filled.
///
/// The source is APPLIED SEPARATELY after createService, not inline. The reason
/// was measured on the server: createService with an inline source triggers a
/// deploy (~100 seconds + can error out during the build), whereas updateSource
/// only stores the config (~2 seconds, no deploy). So the service appears in the
/// table instantly and deploy stays an explicit `d` action — exactly the EasyPanel
/// dashboard flow.
pub(super) fn create_source(form: &Form) -> std::result::Result<Option<SourceCall>, String> {
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
pub(super) fn create_build(form: &Form) -> Option<Value> {
    if form.by_label("Kind") != "app" || form.by_label("Source") == "image" {
        return None;
    }
    build_body(form).ok().and_then(|b| b.get("build").cloned())
}

/// The `env` contents for createService, or None if empty. A multi-line KEY=value
/// string, edited in $EDITOR — same as an existing service's env.
pub(super) fn create_env(form: &Form) -> Option<String> {
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
pub(super) fn create_domains(form: &Form) -> Option<Value> {
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
pub(super) fn resource_fields(resources: Option<&Value>) -> Vec<Field> {
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
pub(super) fn resource_body(form: &Form) -> std::result::Result<Value, String> {
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

pub(super) fn service_extra(form: &Form) -> Value {
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
pub(super) fn validate_step(form: &Form) -> Result<(), String> {
    match form.step {
        0 => {
            if form.by_label("Project").is_empty() {
                return Err("Choose a project first".into());
            }
            let name = form.by_label("Name");
            if name.is_empty() {
                return Err("Give the service a name first".into());
            }
            if !crate::commands::valid_name(&name) {
                return Err("A service name may only contain a-z, 0-9, - and _".into());
            }
            Ok(())
        }
        // The source step validates through the same builder that shapes the
        // request, so the wizard cannot accept what the request would reject.
        1 => source_body(form).map(|_| ()),
        _ => Ok(()),
    }
}

pub(super) fn source_body(
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
pub(super) fn domain_body(form: &Form) -> std::result::Result<Value, String> {
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
pub(super) fn port_fields() -> Vec<Field> {
    vec![
        Field::text("Published", ""),
        Field::text("Target", ""),
        Field::choice("Protocol", PORT_PROTOCOLS, "tcp"),
    ]
}

/// Fields for the add-redirect form. `Regex` matches the source URL, `Replacement`
/// its target (may use groups `${1}`). Permanent = 301, otherwise 302.
pub(super) fn redirect_fields() -> Vec<Field> {
    vec![
        Field::text("Regex", ""),
        Field::text("Replacement", ""),
        Field::boolean("Permanent (301)", true),
        Field::boolean("Enabled", true),
    ]
}

/// A redirect object for updateRedirects: `{enabled, permanent, regex, replacement}`
/// (all four required by the schema). Regex & replacement must not be empty.
pub(super) fn redirect_body(form: &Form) -> std::result::Result<Value, String> {
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
pub(super) fn basic_auth_fields(data: Option<&Value>) -> Vec<Field> {
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
pub(super) fn basic_auth_body(form: &Form) -> std::result::Result<Value, String> {
    let user = form.by_label("Username");
    let pass = form.by_label("Password");
    let (u, p) = (user.trim(), pass.trim());
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
pub(super) fn mount_fields(existing: Option<&Value>) -> Vec<Field> {
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
pub(super) fn mount_body(form: &Form) -> std::result::Result<Value, String> {
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
            if content.is_empty() {
                return Err("Content is still empty — Space to open it in $EDITOR".into());
            }
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

/// The `values` object for createPort: `{published, target, protocol}`.
///
/// Both are numbers in the API, so they're parsed; a non-numeric value is rejected
/// with a clear message rather than sending a 0 that opens the wrong port.
pub(super) fn port_body(form: &Form) -> std::result::Result<Value, String> {
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
pub(super) enum FieldKind {
    Text,
    Secret,
    Bool,
    /// A choice from real data (project/service/protocol), cycled with space/←/→.
    /// Dynamic so its contents can come from the API, not be typed by hand.
    Choice(Vec<String>),
    /// Multi-line content edited in $EDITOR, like env.
    ///
    /// A Dockerfile never fits in a single-line field; forcing it into
    /// FieldKind::Text means the user types a literal "\n" and sends one long line
    /// that will never build.
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
    /// Show conditions, AND-combined: (label of the deciding field, tags separated
    /// by commas e.g. "github,git"). Empty = always shown.
    ///
    /// The panel works this way too: choosing Service/Custom swaps the fields
    /// below it, rather than showing both at once.
    pub(super) only_for: Vec<(&'static str, &'static str)>,
    /// The wizard step this field appears on. 0 = first step. A form whose fields
    /// are all on step 0 stays an ordinary single page; as soon as a field lands on
    /// a step > 0, that form becomes a staged wizard. Submit values are still read
    /// from ALL steps at once.
    pub(super) step: u8,
}

impl Field {
    /// A field whose contents are edited in $EDITOR, not typed in the form.
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
    /// Show this field only when the `switch` field holds one of `tags` (comma
    /// separated). Can be called more than once: the conditions are AND-combined,
    /// so a single form can have more than one decider — the "New service" form
    /// needs "service type = app" AND "source type = github" at once.
    pub(super) fn when(mut self, switch: &'static str, tags: &'static str) -> Self {
        self.only_for.push((switch, tags));
        self
    }

    /// Place the field on wizard step `n` (0 = first step).
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
            value: if on { "yes".into() } else { "no".into() },
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
    /// Replace the list of choices (e.g. services filled in after a project is
    /// chosen).
    ///
    /// The current value is always kept, even if it's absent from the new list:
    /// silently jumping to the first choice would change config the user didn't ask
    /// for — e.g. a `ref` that's a tag would switch to the first branch
    /// alphabetically, then get deployed along with it.
    pub(super) fn set_options(&mut self, mut options: Vec<String>) {
        if !self.value.is_empty() && !options.contains(&self.value) {
            options.insert(0, self.value.clone());
        }
        if !options.contains(&self.value) {
            self.value = options.first().cloned().unwrap_or_default();
        }
        self.kind = FieldKind::Choice(options);
    }
    /// Cycle to the next choice (Bool is treated as yes/no).
    pub(super) fn cycle(&mut self) {
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
    pub(super) fn is_on(&self) -> bool {
        self.value == "yes"
    }
    pub(super) fn shown(&self) -> String {
        match self.kind {
            FieldKind::Secret => "•".repeat(self.value.chars().count()),
            // The contents can be hundreds of lines: what's useful here is whether
            // it exists and how big it is, not a snippet of its first line.
            FieldKind::Editor if self.value.trim().is_empty() => "(empty)".into(),
            FieldKind::Editor => format!("{} lines", self.value.lines().count()),
            _ => self.value.clone(),
        }
    }
}

/// What the form does when submitted.
pub(super) enum FormKind {
    ServerAdd,
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
}

pub(super) struct Form {
    pub(super) kind: FormKind,
    pub(super) title: String,
    pub(super) fields: Vec<Field>,
    pub(super) focus: usize,
    /// The original JSON in edit mode. Submit starts from here so fields not in the
    /// form (middlewares on a domain, nixpacksVersion on a build) stay intact.
    pub(super) original: Option<Value>,
    /// The wizard step currently shown. 0 for an ordinary single-page form.
    pub(super) step: usize,
    /// The area of the field rows (filled in at render), to map a click to a field.
    pub(super) rect: Rect,
    /// Guidance that belongs to THIS form — "0 = unlimited", "config only, no
    /// data". It used to be written to the status line, which fades after six
    /// seconds while the form is still open, so the explanation vanished from
    /// under the user mid-edit. Drawn on the form's own border instead, where it
    /// lasts exactly as long as the form does.
    pub(super) note: Option<String>,
    /// Why the last attempt to leave this step was refused. Shown on the form's
    /// own border, in place of the note, so it cannot fade out from under the
    /// user while they are looking for the field it names.
    pub(super) error: Option<String>,
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
            rect: Rect::default(),
            note: None,
            error: None,
        }
    }
    /// Guidance shown on the form's border for as long as the form is open.
    pub(super) fn with_note(mut self, note: impl Into<String>) -> Self {
        self.note = Some(note.into());
        self
    }
    pub(super) fn with_original(mut self, original: Value) -> Self {
        self.original = Some(original);
        self
    }
    /// Indices of the fields shown right now.
    ///
    /// Each field carries its own condition, so a single form can have more than
    /// one decider. Without that, the "New service" form couldn't carry a source
    /// too: it needs "service type = app" AND "source type = github".
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

    /// The steps that actually have visible fields, sorted. For a database service
    /// this is just `[0]` (source/build fields depend on Kind=app), so its form
    /// stays a single page; for an app `[0, 1, 2]` → wizard.
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

    /// This form is staged (more than one step holds fields).
    pub(super) fn is_wizard(&self) -> bool {
        self.steps_present().len() > 1
    }

    /// Fields shown ON THE CURRENT STEP. Differs from visible(), which spans steps
    /// and is used at submit to read every value at once.
    pub(super) fn visible_here(&self) -> Vec<usize> {
        let step = self.step as u8;
        self.visible()
            .into_iter()
            .filter(|&i| self.fields[i].step == step)
            .collect()
    }

    /// The next populated step after the current one, if any.
    pub(super) fn next_present_step(&self) -> Option<usize> {
        self.steps_present()
            .into_iter()
            .map(usize::from)
            .find(|&s| s > self.step)
    }

    /// The populated step before the current one, if any.
    pub(super) fn prev_present_step(&self) -> Option<usize> {
        self.steps_present()
            .into_iter()
            .rev()
            .map(usize::from)
            .find(|&s| s < self.step)
    }

    /// Move to step `step` and put focus on its first field.
    pub(super) fn goto_step(&mut self, step: usize) {
        self.step = step;
        self.focus = self.visible_here().first().copied().unwrap_or(0);
    }

    /// Move focus `delta` steps among the fields shown on this step.
    pub(super) fn move_focus(&mut self, delta: isize) {
        let vis = self.visible_here();
        if vis.is_empty() {
            return;
        }
        let at = vis.iter().position(|i| *i == self.focus).unwrap_or(0) as isize;
        let next = (at + delta).rem_euclid(vis.len() as isize) as usize;
        self.focus = vis[next];
    }

    /// After Destination changes, focus may be left on a now-hidden field.
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

/// A dropdown for a Choice field: the list of options + a type-to-filter box.
///
/// Cycling options with space doesn't scale to long lists (11 services), so a
/// Choice field opens a real, searchable list.
pub(super) struct Chooser {
    pub(super) field: usize,
    pub(super) label: &'static str,
    pub(super) options: Vec<String>,
    pub(super) filter: String,
    pub(super) state: ListState,
    /// The dropdown box as drawn (filled in at render), for click/hover hit-testing.
    pub(super) rect: Rect,
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
            rect: Rect::default(),
        }
    }

    /// The option index (within `matches()`) under (col,row), None if outside. The
    /// box's first & last rows are borders; the list may be scrolled (offset).
    pub(super) fn item_at(&self, col: u16, row: u16) -> Option<usize> {
        let r = self.rect;
        let inside = col >= r.x
            && col < r.x.saturating_add(r.width)
            && row > r.y
            && row < r.y.saturating_add(r.height).saturating_sub(1);
        if !inside {
            return None;
        }
        let idx = (row - r.y - 1) as usize + self.state.offset();
        (idx < self.matches().len()).then_some(idx)
    }

    /// The options that pass the filter (case-insensitive, substring).
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

    /// Keep the selected index valid after the filter changes.
    pub(super) fn clamp(&mut self) {
        let len = self.matches().len();
        let i = self.state.selected().unwrap_or(0);
        self.state
            .select(if len == 0 { None } else { Some(i.min(len - 1)) });
    }
}
