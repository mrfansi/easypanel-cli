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

/// Penyimpanan daftar host EasyPanel (servers.json), dikelola lewat command.
pub struct ServerConfig {
    path: PathBuf,
}

impl ServerConfig {
    pub fn new(path: PathBuf) -> Self {
        Self { path }
    }

    /// ~/.config/easypanel/servers.json (kompatibel dengan versi PHP sebelumnya).
    pub fn default_path() -> PathBuf {
        let home = std::env::var("HOME")
            .or_else(|_| std::env::var("USERPROFILE"))
            .unwrap_or_else(|_| ".".into());
        PathBuf::from(home)
            .join(".config")
            .join("easypanel")
            .join("servers.json")
    }

    pub fn all(&self) -> Vec<Server> {
        fs::read_to_string(&self.path)
            .ok()
            .and_then(|s| serde_json::from_str(&s).ok())
            .unwrap_or_default()
    }

    pub fn get(&self, name: &str) -> Option<Server> {
        self.all().into_iter().find(|s| s.name == name)
    }

    pub fn default(&self) -> Option<Server> {
        self.all().into_iter().find(|s| s.default)
    }

    pub fn add(&self, name: &str, url: &str, token: &str) -> Result<()> {
        let was_default = self.get(name).map(|s| s.default).unwrap_or(false);
        let mut servers: Vec<Server> = self.all().into_iter().filter(|s| s.name != name).collect();
        let is_first = servers.is_empty();
        let has_default = servers.iter().any(|s| s.default);

        servers.push(Server {
            name: name.to_string(),
            url: url.to_string(),
            token: token.to_string(),
            // Default bila: server pertama, ATAU tadinya default (rotasi token),
            // ATAU tak ada server lain yang bertanda default.
            default: is_first || was_default || !has_default,
        });

        self.save(&servers)
    }

    pub fn remove(&self, name: &str) -> Result<()> {
        let mut servers: Vec<Server> = self.all().into_iter().filter(|s| s.name != name).collect();
        if !servers.is_empty() && !servers.iter().any(|s| s.default) {
            servers[0].default = true;
        }
        self.save(&servers)
    }

    pub fn set_default(&self, name: &str) -> Result<()> {
        let mut servers = self.all();
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
    fn empty_when_missing() {
        let (_dir, cfg) = temp_config();
        assert!(cfg.all().is_empty());
        assert!(cfg.default().is_none());
    }
}
