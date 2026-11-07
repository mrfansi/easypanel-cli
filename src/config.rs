use crate::cloudflare::CloudflareAccount;
use anyhow::Result;
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;
use std::fs;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};

/// Resolve aliases before locking; replacing the resolved target keeps an
/// existing config-file symlink intact, matching the original write behavior.
fn store_target(path: &Path) -> Result<PathBuf> {
    match fs::canonicalize(path) {
        Ok(target) => Ok(target),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
            let parent = path
                .parent()
                .filter(|p| !p.as_os_str().is_empty())
                .unwrap_or_else(|| Path::new("."));
            if let Ok(link) = fs::read_link(path) {
                return store_target(&if link.is_absolute() {
                    link
                } else {
                    parent.join(link)
                });
            }
            fs::create_dir_all(parent)?;
            let name = path
                .file_name()
                .ok_or_else(|| anyhow::anyhow!("Config path has no filename"))?;
            Ok(fs::canonicalize(parent)?.join(name))
        }
        Err(error) => Err(error.into()),
    }
}

fn update_store<T: Serialize>(
    path: &Path,
    read: impl FnOnce() -> Result<T>,
    mutate: impl FnOnce(&mut T) -> Result<()>,
) -> Result<()> {
    let target = store_target(path)?;
    let mut lock_path = target.as_os_str().to_os_string();
    lock_path.push(".lock");
    let mut options = fs::OpenOptions::new();
    options.read(true).write(true).create(true).truncate(false);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.mode(0o600);
    }
    // This inode must persist: deleting it would let new writers bypass waiters.
    let lock = options.open(PathBuf::from(lock_path))?;
    lock.lock()?;
    let mut value = read()?;
    mutate(&mut value)?;
    atomic_store(&target, &value)
}

fn atomic_store(path: &Path, value: &impl Serialize) -> Result<()> {
    static NEXT_TEMP: AtomicU64 = AtomicU64::new(0);
    let (temp, mut file) = loop {
        let mut temp = path.as_os_str().to_os_string();
        temp.push(format!(
            ".tmp-{}-{}",
            std::process::id(),
            NEXT_TEMP.fetch_add(1, Ordering::Relaxed)
        ));
        let temp = PathBuf::from(temp);
        let mut options = fs::OpenOptions::new();
        options.write(true).create_new(true);
        #[cfg(unix)]
        {
            use std::os::unix::fs::OpenOptionsExt;
            options.mode(0o600); // Private before any credential bytes are written.
        }
        match options.open(&temp) {
            Ok(file) => break (temp, file),
            Err(error) if error.kind() == std::io::ErrorKind::AlreadyExists => continue,
            Err(error) => return Err(error.into()),
        }
    };
    let result = (|| -> Result<()> {
        serde_json::to_writer_pretty(&mut file, value)?;
        file.sync_all()?;
        Ok(())
    })();
    drop(file);
    let result = result.and_then(|()| fs::rename(&temp, path).map_err(Into::into));
    if let Err(error) = result {
        if let Err(cleanup) = fs::remove_file(&temp) {
            anyhow::bail!(
                "{error}; could not remove partial config {}: {cleanup}",
                temp.display()
            );
        }
        return Err(error);
    }
    Ok(())
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Server {
    pub name: String,
    pub url: String,
    pub token: String,
    #[serde(default)]
    pub default: bool,
    /// The SSH hop to tunnel through. When set, `url` is the panel as seen FROM
    /// that host, e.g. `http://localhost:3000`. Omitted from the file when unset, so a
    /// servers.json without tunnels reads and writes exactly as before.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub ssh: Option<crate::tunnel::SshTunnel>,
}

/// The domains enrolled for uptime checks, per server (checks.json).
///
/// Separate from servers.json deliberately: that file is credentials the tool
/// cannot work without, this one is a preference it can lose. A corrupt or
/// missing watchlist must never stop the tool from talking to a host.
///
/// Kept per server name because the same URL can exist on two hosts, and
/// "is this watched?" is only meaningful about one of them.
pub struct Watchlist {
    path: PathBuf,
}

impl Watchlist {
    pub fn new(path: PathBuf) -> Self {
        Self { path }
    }

    pub fn default_path() -> PathBuf {
        ServerConfig::default_path().with_file_name("checks.json")
    }

    /// Everything watched on `server`, in the order it was enrolled.
    pub fn all(&self, server: &str) -> Vec<crate::uptime::Check> {
        self.try_read()
            .unwrap_or_default()
            .remove(server)
            .unwrap_or_default()
    }

    /// Enrol a domain, or replace the check already stored for that URL.
    pub fn put(&self, server: &str, check: crate::uptime::Check) -> Result<()> {
        update_store(
            &self.path,
            || self.try_read(),
            |all| {
                let list = all.entry(server.to_string()).or_default();
                match list.iter_mut().find(|c| c.url == check.url) {
                    Some(existing) => *existing = check,
                    None => list.push(check),
                }
                Ok(())
            },
        )
    }

    pub fn remove(&self, server: &str, url: &str) -> Result<()> {
        update_store(
            &self.path,
            || self.try_read(),
            |all| {
                if let Some(list) = all.get_mut(server) {
                    list.retain(|c| c.url != url);
                    if list.is_empty() {
                        all.remove(server);
                    }
                }
                Ok(())
            },
        )
    }

    /// The file, erroring if it EXISTS but cannot be read.
    ///
    /// Same rule as servers.json and for the same reason: every write reads the
    /// whole file and puts it back, so treating a corrupt file as empty would
    /// quietly delete every other server's watchlist on the next enrolment.
    fn try_read(&self) -> Result<BTreeMap<String, Vec<crate::uptime::Check>>> {
        let raw = match fs::read_to_string(&self.path) {
            Ok(r) => r,
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(BTreeMap::new()),
            Err(e) => return Err(e.into()),
        };
        if raw.trim().is_empty() {
            return Ok(BTreeMap::new());
        }
        Ok(serde_json::from_str(&raw)?)
    }
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
    /// list. Writers must use `update()` to read and mutate under the store lock.
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

    pub fn add(
        &self,
        name: &str,
        url: &str,
        token: &str,
        ssh: Option<crate::tunnel::SshTunnel>,
    ) -> Result<()> {
        self.update(|servers| {
            let was_default = servers.iter().any(|s| s.name == name && s.default);
            servers.retain(|s| s.name != name);
            let is_first = servers.is_empty();
            let has_default = servers.iter().any(|s| s.default);
            servers.push(Server {
                name: name.to_string(),
                url: url.to_string(),
                token: token.to_string(),
                // Keep the CLI's first/rotated/no-other-default rules.
                default: is_first || was_default || !has_default,
                ssh,
            });
            Ok(())
        })
    }

    pub fn remove(&self, name: &str) -> Result<()> {
        self.update(|servers| {
            servers.retain(|s| s.name != name);
            if !servers.is_empty() && !servers.iter().any(|s| s.default) {
                servers[0].default = true;
            }
            Ok(())
        })
    }

    /// Rename a server, keeping everything else about it.
    ///
    /// In place rather than remove-then-add: the token cannot be read back from
    /// anywhere, the default flag has to survive, and the list order is how the
    /// user recognises their own hosts. Doing it as a delete and an insert risks
    /// all three, and a half-completed one loses a credential for good.
    pub fn rename(&self, old: &str, new: &str) -> Result<()> {
        self.update(|servers| {
            if servers.iter().any(|s| s.name == new) {
                anyhow::bail!("A server called '{new}' already exists");
            }
            let Some(server) = servers.iter_mut().find(|s| s.name == old) else {
                anyhow::bail!("No server called '{old}'");
            };
            server.name = new.to_string();
            Ok(())
        })
    }

    pub fn set_default(&self, name: &str) -> Result<()> {
        self.update(|servers| {
            for s in servers {
                s.default = s.name == name;
            }
            Ok(())
        })
    }

    pub(crate) fn update(&self, mutate: impl FnOnce(&mut Vec<Server>) -> Result<()>) -> Result<()> {
        update_store(&self.path, || self.try_all(), mutate)
    }
}

/// Standalone store for Cloudflare accounts (cloudflare.json), independent of servers.
///
/// A deliberate copy of the `ServerConfig`/`Watchlist` shape rather than a new
/// abstraction over both: same corrupt-file guard (a corrupt file must never be
/// overwritten and silently delete every stored token), same `0o600`. Cloudflare
/// accounts are NOT tied to any EasyPanel server — an operator may hold several.
pub struct CloudflareConfig {
    path: PathBuf,
}

impl CloudflareConfig {
    pub fn new(path: PathBuf) -> Self {
        Self { path }
    }

    pub fn default_path() -> PathBuf {
        ServerConfig::default_path().with_file_name("cloudflare.json")
    }

    #[cfg(test)]
    pub fn path_for_test(&self) -> PathBuf {
        self.path.clone()
    }

    /// Read path: empty on a missing OR corrupt file (worst case: an empty list).
    pub fn list(&self) -> Vec<CloudflareAccount> {
        self.try_all().unwrap_or_default()
    }

    /// Write path: errors if the file EXISTS but can't be read/parsed, so a corrupt
    /// file can never be overwritten and silently delete every stored token.
    pub fn try_all(&self) -> Result<Vec<CloudflareAccount>> {
        let raw = match fs::read_to_string(&self.path) {
            Ok(raw) => raw,
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(Vec::new()),
            Err(e) => return Err(anyhow::anyhow!("cannot read {}: {e}", self.path.display())),
        };
        if raw.trim().is_empty() {
            return Ok(Vec::new());
        }
        serde_json::from_str(&raw).map_err(|e| {
            anyhow::anyhow!(
                "{} is corrupt: {e}. Fix or move that file; continuing would overwrite it \
                 and delete every Cloudflare token.",
                self.path.display()
            )
        })
    }

    pub fn by_name(&self, name: &str) -> Option<CloudflareAccount> {
        self.list().into_iter().find(|a| a.name == name)
    }

    pub fn default(&self) -> Option<CloudflareAccount> {
        self.list().into_iter().find(|a| a.default)
    }

    /// Add or replace an account by name. Default when it's the first, was already the
    /// default (token rotation), or nothing else is marked default — mirrors ServerConfig.
    pub fn add(&self, account: CloudflareAccount) -> Result<()> {
        self.update(|accounts| {
            let was_default = accounts.iter().any(|a| a.name == account.name && a.default);
            accounts.retain(|a| a.name != account.name);
            let is_first = accounts.is_empty();
            let has_default = accounts.iter().any(|a| a.default);
            let mut account = account;
            account.default = is_first || was_default || !has_default;
            accounts.push(account);
            Ok(())
        })
    }

    pub fn remove(&self, name: &str) -> Result<()> {
        self.update(|accounts| {
            accounts.retain(|a| a.name != name);
            if !accounts.is_empty() && !accounts.iter().any(|a| a.default) {
                accounts[0].default = true;
            }
            Ok(())
        })
    }

    pub fn set_default(&self, name: &str) -> Result<()> {
        self.update(|accounts| {
            if !accounts.iter().any(|a| a.name == name) {
                anyhow::bail!("No Cloudflare account called '{name}'");
            }
            for a in accounts {
                a.default = a.name == name;
            }
            Ok(())
        })
    }

    pub(crate) fn update(
        &self,
        mutate: impl FnOnce(&mut Vec<CloudflareAccount>) -> Result<()>,
    ) -> Result<()> {
        update_store(&self.path, || self.try_all(), mutate)
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
    fn concurrent_server_account_and_host_edits_preserve_every_entry() {
        use std::sync::{Arc, Barrier};
        const WRITERS: usize = 12;
        let dir = tempfile::tempdir().unwrap();
        let start = Arc::new(Barrier::new(WRITERS));
        std::thread::scope(|scope| {
            for i in 0..WRITERS {
                let start = Arc::clone(&start);
                let root = dir.path();
                scope.spawn(move || {
                    start.wait();
                    ServerConfig::new(root.join("servers.json"))
                        .add(
                            &format!("host-{i}"),
                            "https://panel.test",
                            &format!("token-{i}"),
                            None,
                        )
                        .unwrap();
                    CloudflareConfig::new(root.join("cloudflare.json"))
                        .add(cf_account(
                            &format!("account-{i}"),
                            &format!("token-{i}"),
                            None,
                        ))
                        .unwrap();
                    Watchlist::new(root.join("checks.json"))
                        .put(
                            &format!("host-{i}"),
                            crate::uptime::Check::get(&format!("https://host-{i}.test")),
                        )
                        .unwrap();
                });
            }
        });
        let servers = ServerConfig::new(dir.path().join("servers.json"))
            .try_all()
            .unwrap();
        let accounts = CloudflareConfig::new(dir.path().join("cloudflare.json"))
            .try_all()
            .unwrap();
        let checks = Watchlist::new(dir.path().join("checks.json"));
        assert_eq!(servers.len(), WRITERS);
        assert_eq!(accounts.len(), WRITERS);
        assert_eq!(servers.iter().filter(|s| s.default).count(), 1);
        assert_eq!(accounts.iter().filter(|a| a.default).count(), 1);
        for i in 0..WRITERS {
            assert!(servers
                .iter()
                .any(|s| s.name == format!("host-{i}") && s.token == format!("token-{i}")));
            assert!(accounts
                .iter()
                .any(|a| a.name == format!("account-{i}") && a.api_token == format!("token-{i}")));
            assert_eq!(
                checks.all(&format!("host-{i}"))[0].url,
                format!("https://host-{i}.test")
            );
        }
    }

    #[test]
    fn store_lock_is_held_before_the_next_writer_reads() {
        use std::sync::mpsc;
        let (dir, cfg) = temp_config();
        cfg.add("prod", "https://prod.test", "original", None)
            .unwrap();
        let (held_tx, held_rx) = mpsc::channel();
        let (release_tx, release_rx) = mpsc::channel();
        let (reading_tx, reading_rx) = mpsc::channel();
        std::thread::scope(|scope| {
            let cfg = &cfg;
            let path = path_of(&dir);
            let first = scope.spawn(move || {
                cfg.update(|servers| {
                    servers[0].token = "rotated".into();
                    held_tx.send(()).unwrap();
                    release_rx.recv().unwrap();
                    Ok(())
                })
                .unwrap();
            });
            held_rx.recv().unwrap();
            let second = scope.spawn(move || {
                update_store(
                    &path,
                    || {
                        reading_tx.send(()).unwrap();
                        cfg.try_all()
                    },
                    |servers| {
                        assert_eq!(servers[0].token, "rotated");
                        servers[0].url = "https://updated.test".into();
                        Ok(())
                    },
                )
                .unwrap();
            });
            let read_was_blocked = reading_rx
                .recv_timeout(std::time::Duration::from_millis(100))
                .is_err();
            release_tx.send(()).unwrap();
            first.join().unwrap();
            second.join().unwrap();
            assert!(
                read_was_blocked,
                "a waiting writer read a stale snapshot before acquiring the lock"
            );
        });
        assert_eq!(cfg.get("prod").unwrap().token, "rotated");
        assert_eq!(cfg.get("prod").unwrap().url, "https://updated.test");
    }

    #[test]
    fn failed_mutation_and_serialization_leave_original_bytes_and_no_partial_file() {
        let (dir, cfg) = temp_config();
        cfg.add("prod", "https://prod.test", "original", None)
            .unwrap();
        let path = path_of(&dir);
        let before = fs::read(&path).unwrap();
        assert!(cfg
            .update(|servers| {
                servers.clear();
                anyhow::bail!("fixture mutation rejected")
            })
            .is_err());
        assert_eq!(fs::read(&path).unwrap(), before);
        struct Invalid;
        impl Serialize for Invalid {
            fn serialize<S: serde::Serializer>(
                &self,
                _serializer: S,
            ) -> std::result::Result<S::Ok, S::Error> {
                Err(serde::ser::Error::custom("fixture serialization failure"))
            }
        }
        assert!(atomic_store(&path, &Invalid).is_err());
        assert_eq!(fs::read(&path).unwrap(), before);
        assert!(dir.path().join("servers.json.lock").exists());
        assert!(!fs::read_dir(dir.path()).unwrap().any(|entry| entry
            .unwrap()
            .file_name()
            .to_string_lossy()
            .contains(".tmp-")));
    }

    #[cfg(unix)]
    #[test]
    fn linked_config_files_keep_their_symlinks_and_share_target_locks() {
        use std::os::unix::fs::symlink;
        let dir = tempfile::tempdir().unwrap();
        let target = dir.path().join("servers.json");
        let alias = dir.path().join("linked-servers.json");
        ServerConfig::new(target.clone())
            .add("prod", "https://prod.test", "original", None)
            .unwrap();
        symlink(&target, &alias).unwrap();
        let start = std::sync::Barrier::new(2);
        std::thread::scope(|scope| {
            scope.spawn(|| {
                start.wait();
                ServerConfig::new(target.clone())
                    .add("direct", "https://direct.test", "direct", None)
                    .unwrap();
            });
            scope.spawn(|| {
                start.wait();
                ServerConfig::new(alias.clone())
                    .add("linked", "https://linked.test", "linked", None)
                    .unwrap();
            });
        });
        assert!(fs::symlink_metadata(&alias)
            .unwrap()
            .file_type()
            .is_symlink());
        assert_eq!(ServerConfig::new(target).try_all().unwrap().len(), 3);
        assert_eq!(
            store_target(&alias).unwrap(),
            fs::canonicalize(&alias).unwrap()
        );

        let target = dir.path().join("cloudflare.json");
        let alias = dir.path().join("linked-cloudflare.json");
        CloudflareConfig::new(target.clone())
            .add(cf_account("first", "one", None))
            .unwrap();
        symlink(&target, &alias).unwrap();
        CloudflareConfig::new(alias.clone())
            .add(cf_account("second", "two", None))
            .unwrap();
        assert!(fs::symlink_metadata(&alias)
            .unwrap()
            .file_type()
            .is_symlink());
        assert_eq!(CloudflareConfig::new(target).try_all().unwrap().len(), 2);

        let target = dir.path().join("checks.json");
        let alias = dir.path().join("linked-checks.json");
        Watchlist::new(target.clone())
            .put("first", crate::uptime::Check::get("https://first.test"))
            .unwrap();
        symlink(&target, &alias).unwrap();
        Watchlist::new(alias.clone())
            .put("second", crate::uptime::Check::get("https://second.test"))
            .unwrap();
        assert!(fs::symlink_metadata(&alias)
            .unwrap()
            .file_type()
            .is_symlink());
        assert_eq!(Watchlist::new(target).all("second").len(), 1);
    }

    #[test]
    fn first_added_becomes_default_and_persists() {
        let (dir, cfg) = temp_config();
        cfg.add("prod", "https://prod.test", "tok-prod", None)
            .unwrap();

        let reloaded = ServerConfig::new(dir.path().join("servers.json"));
        let def = reloaded.default().unwrap();
        assert_eq!(def.name, "prod");
        assert!(def.default);
    }

    #[test]
    fn a_tunnel_is_kept_and_a_direct_server_is_written_as_before() {
        let (dir, cfg) = temp_config();
        cfg.add("direct", "https://p.test", "t1", None).unwrap();
        let hop = crate::tunnel::SshTunnel {
            host: "203.0.113.7".into(),
            port: Some(2222),
            user: Some("root".into()),
            auth: crate::tunnel::SshAuth::Password,
            secret: Some("pw".into()),
            ..Default::default()
        };
        cfg.add(
            "tunnelled",
            "http://localhost:3000",
            "t2",
            Some(hop.clone()),
        )
        .unwrap();

        assert_eq!(cfg.get("tunnelled").unwrap().ssh, Some(hop));
        assert_eq!(cfg.get("direct").unwrap().ssh, None);
        // Older builds (and the PHP version) read this file: a server without a
        // tunnel must not grow a key they do not know.
        let raw: serde_json::Value =
            serde_json::from_str(&fs::read_to_string(path_of(&dir)).unwrap()).unwrap();
        assert!(raw[0].get("ssh").is_none(), "{raw}");
        // Re-saving without a tunnel removes it — an edit can switch back to direct.
        cfg.add("tunnelled", "https://t.test", "t2", None).unwrap();
        assert_eq!(cfg.get("tunnelled").unwrap().ssh, None);
    }

    #[test]
    fn adding_more_keeps_first_default() {
        let (_dir, cfg) = temp_config();
        cfg.add("prod", "https://prod.test", "t1", None).unwrap();
        cfg.add("staging", "https://staging.test", "t2", None)
            .unwrap();

        assert_eq!(cfg.default().unwrap().name, "prod");
        assert!(!cfg.get("staging").unwrap().default);
        assert_eq!(cfg.all().len(), 2);
    }

    #[test]
    fn re_adding_default_keeps_default_on_token_rotation() {
        let (_dir, cfg) = temp_config();
        cfg.add("prod", "https://prod.test", "t1", None).unwrap();
        cfg.add("staging", "https://staging.test", "t2", None)
            .unwrap();
        cfg.add("prod", "https://prod.test", "tok-NEW", None)
            .unwrap();

        assert_eq!(cfg.default().unwrap().name, "prod");
        assert_eq!(cfg.get("prod").unwrap().token, "tok-NEW");
        assert_eq!(cfg.all().len(), 2);
    }

    #[test]
    fn renaming_keeps_the_token_the_default_and_the_position() {
        let (_d, cfg) = temp_config();
        cfg.add("prod", "https://p", "tok-p", None).unwrap();
        cfg.add("staging", "https://s", "tok-s", None).unwrap();
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
        cfg.add("prod", "https://p", "tok-p", None).unwrap();
        cfg.add("staging", "https://s", "tok-s", None).unwrap();
        // Silently merging two hosts into one entry would point a name at the
        // wrong machine — the exact mistake this tool's colours exist to prevent.
        assert!(cfg.rename("staging", "prod").is_err());
        assert_eq!(cfg.get("prod").unwrap().token, "tok-p");
        assert!(cfg.get("staging").is_some());
    }

    fn temp_watchlist() -> (tempfile::TempDir, Watchlist) {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("checks.json");
        (dir, Watchlist::new(path))
    }

    #[test]
    fn only_enrolled_domains_are_watched_and_they_survive_a_reload() {
        let (dir, w) = temp_watchlist();
        assert!(w.all("prod").is_empty(), "nothing is watched by default");

        w.put("prod", crate::uptime::Check::get("https://a.test/"))
            .unwrap();
        w.put("prod", crate::uptime::Check::get("https://b.test/"))
            .unwrap();
        let reloaded = Watchlist::new(dir.path().join("checks.json"));
        assert_eq!(reloaded.all("prod").len(), 2);
        let watched = reloaded.all("prod");
        let urls: Vec<&str> = watched.iter().map(|c| c.url.as_str()).collect();
        assert_eq!(urls, vec!["https://a.test/", "https://b.test/"]);
    }

    #[test]
    fn the_same_url_on_two_hosts_is_two_different_things() {
        // A staging and a production host can both serve the same hostname; "is
        // this watched?" is only meaningful about one of them.
        let (_d, w) = temp_watchlist();
        w.put("prod", crate::uptime::Check::get("https://a.test/"))
            .unwrap();
        assert!(w.all("staging").is_empty());
    }

    #[test]
    fn re_enrolling_a_url_replaces_its_check_rather_than_duplicating_it() {
        let (_d, w) = temp_watchlist();
        w.put("prod", crate::uptime::Check::get("https://a.test/"))
            .unwrap();
        w.put(
            "prod",
            crate::uptime::Check {
                method: "POST".into(),
                body: Some("{}".into()),
                ..crate::uptime::Check::get("https://a.test/")
            },
        )
        .unwrap();
        let all = w.all("prod");
        assert_eq!(all.len(), 1, "one entry per URL, not two");
        assert_eq!(all[0].method, "POST");

        w.remove("prod", "https://a.test/").unwrap();
        assert!(w.all("prod").is_empty());
    }

    #[test]
    fn a_corrupt_watchlist_never_wipes_another_server_s_entries() {
        // Every write reads the whole file and puts it back, so reading a corrupt
        // file as "empty" would silently delete everything else in it.
        let (dir, w) = temp_watchlist();
        fs::write(dir.path().join("checks.json"), "{ not json").unwrap();
        assert!(w
            .put("prod", crate::uptime::Check::get("https://a.test/"))
            .is_err());
        assert_eq!(
            fs::read_to_string(dir.path().join("checks.json")).unwrap(),
            "{ not json",
            "the file must be left alone"
        );
    }

    #[test]
    fn set_default_moves_flag() {
        let (_dir, cfg) = temp_config();
        cfg.add("prod", "https://prod.test", "t1", None).unwrap();
        cfg.add("staging", "https://staging.test", "t2", None)
            .unwrap();
        cfg.set_default("staging").unwrap();

        assert_eq!(cfg.default().unwrap().name, "staging");
        assert!(!cfg.get("prod").unwrap().default);
    }

    #[test]
    fn remove_reassigns_default() {
        let (_dir, cfg) = temp_config();
        cfg.add("prod", "https://prod.test", "t1", None).unwrap();
        cfg.add("staging", "https://staging.test", "t2", None)
            .unwrap();
        cfg.remove("prod").unwrap();

        assert!(cfg.get("prod").is_none());
        assert_eq!(cfg.default().unwrap().name, "staging");
    }

    #[test]
    #[cfg(unix)]
    fn saves_with_0600_permissions() {
        use std::os::unix::fs::PermissionsExt;
        let (dir, cfg) = temp_config();
        cfg.add("prod", "https://prod.test", "t1", None).unwrap();

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
        cfg.add("prod", "https://prod.test", "secret-token", None)
            .unwrap();
        fs::write(path_of(&dir), "{ not valid json").unwrap();

        for result in [
            cfg.add("staging", "https://x.test", "t", None),
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
        cfg.add("prod", "https://prod.test", "secret-token", None)
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
            cfg.add("staging", "https://x.test", "t", None).is_err(),
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
        assert!(cfg.add("prod", "https://prod.test", "t", None).is_ok());
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

    // ---- Cloudflare account store ----

    fn temp_cf() -> (tempfile::TempDir, CloudflareConfig) {
        let dir = tempfile::tempdir().unwrap();
        let cfg = CloudflareConfig::new(dir.path().join("cloudflare.json"));
        (dir, cfg)
    }

    fn cf_account(name: &str, token: &str, account_id: Option<String>) -> CloudflareAccount {
        CloudflareAccount {
            name: name.into(),
            api_token: token.into(),
            account_id,
            default: false,
        }
    }

    #[test]
    fn first_cloudflare_account_becomes_default() {
        let (_d, cfg) = temp_cf();
        cfg.add(cf_account("personal", "tok-a", None)).unwrap();
        cfg.add(cf_account("work", "tok-b", Some("acc-1".into())))
            .unwrap();
        assert_eq!(cfg.default().unwrap().name, "personal");
        assert_eq!(
            cfg.by_name("work").unwrap().account_id.as_deref(),
            Some("acc-1")
        );
    }

    #[test]
    fn set_default_and_remove_reassigns_default() {
        let (_d, cfg) = temp_cf();
        cfg.add(cf_account("a", "t", None)).unwrap();
        cfg.add(cf_account("b", "t", None)).unwrap();
        cfg.set_default("b").unwrap();
        assert_eq!(cfg.default().unwrap().name, "b");
        cfg.remove("b").unwrap();
        // Removing the default promotes the remaining one, never leaves zero defaults.
        assert_eq!(cfg.default().unwrap().name, "a");
    }

    #[test]
    fn cloudflare_store_lives_beside_servers_json() {
        // Its own file in the same config dir, not inside servers.json.
        let p = CloudflareConfig::default_path();
        assert!(p.ends_with("cloudflare.json"));
        assert_eq!(p.parent(), ServerConfig::default_path().parent());
    }

    #[test]
    fn missing_cloudflare_file_is_empty_but_corrupt_refuses_to_write() {
        let (_d, cfg) = temp_cf();
        assert!(cfg.list().is_empty(), "missing file reads empty");
        fs::write(cfg.path_for_test(), "{ not json").unwrap();
        // try_all (the write path) must error so add() can't wipe the file.
        assert!(cfg.try_all().is_err());
        assert!(cfg.add(cf_account("x", "t", None)).is_err());
    }
}
