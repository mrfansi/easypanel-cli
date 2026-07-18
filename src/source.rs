//! The source rule: which `updateSource*` endpoint a source type uses, and which
//! keys its body carries.
//!
//! Two callers need this: the create/edit form (which reads it off labelled fields)
//! and the clone path (which reads it off `inspectService`). They used to carry a
//! copy each, and the copies drifted — the clone dropped the registry credentials an
//! image source needs, so cloning a service that pulls from a private registry
//! produced a clone that could not pull, while reporting success. One home now, so
//! the next key added lands in both places.

use serde_json::{json, Value};

/// A string field of a source object, or `""` when absent.
///
/// Deliberately not `output::field`, which yields `"-"` for a missing value — that
/// would send `"-"` as a username.
fn s(src: &Value, key: &str) -> String {
    src.get(key)
        .and_then(Value::as_str)
        .unwrap_or_default()
        .to_string()
}

/// Endpoint + body (without `projectName`/`serviceName`) for a source object
/// `{type, owner, repo, ref, path, image, username, password, dockerfile}`.
///
/// `None` for a missing or unknown type — the caller has nothing to send.
pub fn source_call(src: &Value) -> Option<(&'static str, Value)> {
    let path = match s(src, "path").as_str() {
        "" => "/".to_string(),
        p => p.to_string(),
    };
    match src.get("type").and_then(Value::as_str).unwrap_or_default() {
        "github" => Some((
            "updateSourceGithub",
            json!({
                "owner": s(src, "owner"), "repo": s(src, "repo"),
                "ref": s(src, "ref"), "path": path,
            }),
        )),
        "git" => Some((
            "updateSourceGit",
            json!({ "repo": s(src, "repo"), "ref": s(src, "ref"), "path": path }),
        )),
        "dockerfile" => Some((
            "updateSourceDockerfile",
            json!({ "dockerfile": s(src, "dockerfile") }),
        )),
        "image" => {
            let mut body = json!({ "image": s(src, "image") });
            // Registry credentials are optional: absent means "not sent", never "".
            // A private image without them is exactly the clone bug this file exists
            // to prevent.
            for key in ["username", "password"] {
                let v = s(src, key);
                if !v.is_empty() {
                    body[key] = json!(v);
                }
            }
            Some(("updateSourceImage", body))
        }
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn image_source_carries_registry_credentials() {
        // The clone path used to send only {image}: a clone of a private-registry
        // service could not pull, while the status bar reported success.
        let src = json!({ "type": "image", "image": "ghcr.io/acme/private:1",
                          "username": "reg-user", "password": "reg-pass" });
        let (op, body) = source_call(&src).expect("image is a known source type");
        assert_eq!(op, "updateSourceImage");
        assert_eq!(body["image"], json!("ghcr.io/acme/private:1"));
        assert_eq!(body["username"], json!("reg-user"));
        assert_eq!(body["password"], json!("reg-pass"));

        // Absent credentials stay absent — a public image must not be sent "".
        let bare = json!({ "type": "image", "image": "nginx:alpine" });
        let (_, body) = source_call(&bare).expect("image is a known source type");
        assert!(body.get("username").is_none());
        assert!(body.get("password").is_none());
    }

    #[test]
    fn a_source_type_with_no_endpoint_sends_nothing() {
        // "upload" has no updateSource* endpoint, and a service may carry no source
        // at all. Both mean: there is nothing to apply.
        assert!(source_call(&json!({ "type": "upload" })).is_none());
        assert!(source_call(&json!({})).is_none());
    }
}
