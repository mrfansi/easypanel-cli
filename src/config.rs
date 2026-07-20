use anyhow::Result;
use serde::{Deserialize, Serialize};
use std::fs;
use std::path::PathBuf;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Server {
    pub name: String,
    pub url: String,
    pub token: String,
    #[serde(default)]
    pub default: bool,
}

/// Storage for the list of EasyPanel hosts (servers.json), managed via commands.
pub struct ServerConfig {
    path: PathBuf,
}

impl ServerConfig {
    pub fn new(path: PathBuf) -> Self {
        Self { path }
    }

    /// ~/.config/easypanel/servers.json (compatible with the previous PHP version).
    pub fn default_path() -> PathBuf {
        let home = std::env::var("HOME")
            .or_else(|_| std::env::var("USERPROFILE"))
            .unwrap_or_else(|_| ".".into());
        PathBuf::from(home)
            .join(".config")
            .join("easypanel")
            .join("servers.json")
    }

    /// Configured servers for read paths; empty if the file is corrupt.
    ///
    /// Safe for reading (list/get/default): worst case the user sees an empty
    /// list. NOT safe for write paths — use `try_all()` there.
    pub fn all(&self) -> Vec<Server> {
        self.try_all().unwrap_or_default()
    }

    /// Configured servers, erroring if the file EXISTS but can't be read.
    ///
    /// Must be used by every path that saves: add/remove/set_default read then
    /// write back the result, so treating a corrupt file as "empty" would make
    /// the next command write a fresh list and DELETE every server — along with
    /// their tokens, which can't be recovered from anywhere. A missing file
    /// really does mean empty; a corrupt file must stop the write.
    pub fn try_all(&self) -> Result<Vec<Server>> {
        let raw = match fs::read_to_string(&self.path) {
            Ok(raw) => raw,
            // Never used before: that's not an error.
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(Vec::new()),
            Err(e) => {
                return Err(anyhow::anyhow!(
                    "cannot read {}: {e}. Fix or move that file; continuing would \
                     overwrite it and delete every server.",
                    self.path.display()
                ))
            }
        };
        serde_json::from_str(&raw).map_err(|e| {
            anyhow::anyhow!(
                "{} is corrupt: {e}. Fix or move that file; continuing would \
                 overwrite it and delete every server.",
                self.path.display()
            )
        })
    }

    pub fn get(&self, name: &str) -> Option<Server> {
        self.all().into_iter().find(|s| s.name == name)
    }

    pub fn default(&self) -> Option<Server> {
        self.all().into_iter().find(|s| s.default)
    }

    pub fn add(&self, name: &str, url: &str, token: &str) -> Result<()> {
        let existing = self.try_all()?;
        let was_default = existing.iter().any(|s| s.name == name && s.default);
        let mut servers: Vec<Server> = existing.into_iter().filter(|s| s.name != name).collect();
        let is_first = servers.is_empty();
        let has_default = servers.iter().any(|s| s.default);

        servers.push(Server {
            name: name.to_string(),
            url: url.to_string(),
            token: token.to_string(),
            // Default when: it's the first server, OR it was already the
            // default (token rotation), OR no other server is marked default.
            default: is_first || was_default || !has_default,
        });

        self.save(&servers)
    }

    pub fn remove(&self, name: &str) -> Result<()> {
        let mut servers: Vec<Server> = self
            .try_all()?
            .into_iter()
            .filter(|s| s.name != name)
            .collect();
        if !servers.is_empty() && !servers.iter().any(|s| s.default) {
            servers[0].default = true;
        }
        self.save(&servers)
    }

    /// Rename a server, keeping everything else about it.
    ///
    /// In place rather than remove-then-add: the token cannot be read back from
    /// anywhere, the default flag has to survive, and the list order is how the
    /// user recognises their own hosts. Doing it as a delete and an insert risks
    /// all three, and a half-completed one loses a credential for good.
    pub fn rename(&self, old: &str, new: &str) -> Result<()> {
        let mut servers = self.try_all()?;
        if servers.iter().any(|s| s.name == new) {
            anyhow::bail!("A server called '{new}' already exists");
        }
        let Some(server) = servers.iter_mut().find(|s| s.name == old) else {
            anyhow::bail!("No server called '{old}'");
        };
        server.name = new.to_string();
        self.save(&servers)
    }

    pub fn set_default(&self, name: &str) -> Result<()> {
        let mut servers = self.try_all()?;
        for s in &mut servers {
            s.default = s.name == name;
        }
        self.save(&servers)
    }

    fn save(&self, servers: &[Server]) -> Result<()> {
        if let Some(dir) = self.path.parent() {
            fs::create_dir_all(dir)?;
        }
        fs::write(&self.path, serde_json::to_string_pretty(servers)?)?;

        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            fs::set_permissions(&self.path, fs::Permissions::from_mode(0o600))?;
        }

        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn temp_config() -> (tempfile::TempDir, ServerConfig) {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("servers.json");
        (dir, ServerConfig::new(path))
    }

    fn path_of(dir: &tempfile::TempDir) -> PathBuf {
        dir.path().join("servers.json")
    }

    #[test]
    fn first_added_becomes_default_and_persists() {
        let (dir, cfg) = temp_config();
        cfg.add("prod", "https://prod.test", "tok-prod").unwrap();

        let reloaded = ServerConfig::new(dir.path().join("servers.json"));
        let def = reloaded.default().unwrap();
        assert_eq!(def.name, "prod");
        assert!(def.default);
    }

    #[test]
    fn adding_more_keeps_first_default() {
        let (_dir, cfg) = temp_config();
        cfg.add("prod", "https://prod.test", "t1").unwrap();
        cfg.add("staging", "https://staging.test", "t2").unwrap();

        assert_eq!(cfg.default().unwrap().name, "prod");
        assert!(!cfg.get("staging").unwrap().default);
        assert_eq!(cfg.all().len(), 2);
    }

    #[test]
    fn re_adding_default_keeps_default_on_token_rotation() {
        let (_dir, cfg) = temp_config();
        cfg.add("prod", "https://prod.test", "t1").unwrap();
        cfg.add("staging", "https://staging.test", "t2").unwrap();
        cfg.add("prod", "https://prod.test", "tok-NEW").unwrap();

        assert_eq!(cfg.default().unwrap().name, "prod");
        assert_eq!(cfg.get("prod").unwrap().token, "tok-NEW");
        assert_eq!(cfg.all().len(), 2);
    }

    #[test]
    fn renaming_keeps_the_token_the_default_and_the_position() {
        let (_d, cfg) = temp_config();
        cfg.add("prod", "https://p", "tok-p").unwrap();
        cfg.add("staging", "https://s", "tok-s").unwrap();
        cfg.set_default("staging").unwrap();

        cfg.rename("staging", "staging-eu").unwrap();
        let all = cfg.all();
        // The token is unreadable from anywhere else, so losing it in a rename
        // would cost the user a credential they cannot get back.
        let renamed = cfg.get("staging-eu").unwrap();
        assert_eq!(renamed.token, "tok-s");
        assert_eq!(renamed.url, "https://s");
        assert!(renamed.default, "the default must follow the rename");
        assert!(cfg.get("staging").is_none());
        // Position is how the user recognises their own list.
        assert_eq!(all[1].name, "staging-eu");
    }

    #[test]
    fn renaming_onto_an_existing_name_is_refused() {
        let (_d, cfg) = temp_config();
        cfg.add("prod", "https://p", "tok-p").unwrap();
        cfg.add("staging", "https://s", "tok-s").unwrap();
        // Silently merging two hosts into one entry would point a name at the
        // wrong machine — the exact mistake this tool's colours exist to prevent.
        assert!(cfg.rename("staging", "prod").is_err());
        assert_eq!(cfg.get("prod").unwrap().token, "tok-p");
        assert!(cfg.get("staging").is_some());
    }

    #[test]
    fn set_default_moves_flag() {
        let (_dir, cfg) = temp_config();
        cfg.add("prod", "https://prod.test", "t1").unwrap();
        cfg.add("staging", "https://staging.test", "t2").unwrap();
        cfg.set_default("staging").unwrap();

        assert_eq!(cfg.default().unwrap().name, "staging");
        assert!(!cfg.get("prod").unwrap().default);
    }

    #[test]
    fn remove_reassigns_default() {
        let (_dir, cfg) = temp_config();
        cfg.add("prod", "https://prod.test", "t1").unwrap();
        cfg.add("staging", "https://staging.test", "t2").unwrap();
        cfg.remove("prod").unwrap();

        assert!(cfg.get("prod").is_none());
        assert_eq!(cfg.default().unwrap().name, "staging");
    }

    #[test]
    #[cfg(unix)]
    fn saves_with_0600_permissions() {
        use std::os::unix::fs::PermissionsExt;
        let (dir, cfg) = temp_config();
        cfg.add("prod", "https://prod.test", "t1").unwrap();

        let mode = fs::metadata(path_of(&dir)).unwrap().permissions().mode();
        assert_eq!(mode & 0o777, 0o600);
    }

    #[test]
    fn corrupt_file_never_wipes_the_server_list() {
        // This is the most important test in this file. add/remove/set_default
        // read then write back the result. If a corrupt file is read as "empty",
        // the next command saves a fresh list and DELETES every server — along
        // with their tokens, which can't be recovered from anywhere.
        let (dir, cfg) = temp_config();
        cfg.add("prod", "https://prod.test", "secret-token")
            .unwrap();
        fs::write(path_of(&dir), "{ not valid json").unwrap();

        for result in [
            cfg.add("staging", "https://x.test", "t"),
            cfg.remove("prod"),
            cfg.set_default("prod"),
        ] {
            assert!(result.is_err(), "a write path must reject a corrupt file");
        }

        // The original file must stay intact, not be overwritten by a fresh list.
        assert_eq!(
            fs::read_to_string(path_of(&dir)).unwrap(),
            "{ not valid json"
        );
    }

    #[test]
    fn unreadable_file_errors_and_never_wipes() {
        use std::os::unix::fs::PermissionsExt;
        // Just as dangerous as a corrupt file: if a failed read (permissions
        // revoked) is treated as "empty", the next write command deletes every
        // server. A read failure other than NotFound must become an error, and
        // write paths must reject it.
        let (dir, cfg) = temp_config();
        cfg.add("prod", "https://prod.test", "secret-token")
            .unwrap();
        let path = path_of(&dir);
        fs::set_permissions(&path, fs::Permissions::from_mode(0o000)).unwrap();

        // Root bypasses 0o000 permissions; if the file is still readable, this
        // test doesn't apply.
        if fs::read_to_string(&path).is_ok() {
            fs::set_permissions(&path, fs::Permissions::from_mode(0o600)).unwrap();
            return;
        }

        assert!(cfg.try_all().is_err(), "an unreadable read must error");
        assert!(
            cfg.all().is_empty(),
            "the soft read path still doesn't panic"
        );
        assert!(
            cfg.add("staging", "https://x.test", "t").is_err(),
            "a write path must reject, not overwrite"
        );

        // Restore permissions so the tempdir can be cleaned up.
        fs::set_permissions(&path, fs::Permissions::from_mode(0o600)).unwrap();
    }

    #[test]
    fn missing_file_is_empty_not_an_error() {
        // A missing file = never used before. That's not corruption.
        let (_dir, cfg) = temp_config();
        assert!(cfg.try_all().unwrap().is_empty());
        assert!(cfg.add("prod", "https://prod.test", "t").is_ok());
    }

    #[test]
    fn corrupt_file_reads_as_empty_but_does_not_throw() {
        // The read path stays soft: the user sees an empty list, not a panic
        // that would leave the terminal in raw mode while the TUI is open.
        let (dir, cfg) = temp_config();
        fs::write(path_of(&dir), "not json").unwrap();
        assert!(cfg.all().is_empty());
        assert!(cfg.try_all().is_err());
    }

    #[test]
    fn empty_when_missing() {
        let (_dir, cfg) = temp_config();
        assert!(cfg.all().is_empty());
        assert!(cfg.default().is_none());
    }
}
