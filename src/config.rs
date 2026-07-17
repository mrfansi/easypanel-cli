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

    /// Server terkonfigurasi untuk jalur baca; kosong bila file rusak.
    ///
    /// Aman untuk membaca (list/get/default): paling buruk user melihat daftar
    /// kosong. TIDAK aman untuk jalur tulis — pakai `try_all()` di sana.
    pub fn all(&self) -> Vec<Server> {
        self.try_all().unwrap_or_default()
    }

    /// Server terkonfigurasi, dengan error bila file ADA tapi tak terbaca.
    ///
    /// Wajib dipakai oleh setiap jalur yang menyimpan: add/remove/set_default
    /// membaca lalu menulis kembali hasilnya, jadi menganggap file rusak sebagai
    /// "kosong" akan membuat perintah berikutnya menulis daftar baru dan
    /// MENGHAPUS seluruh server — beserta tokennya, yang tak bisa dibaca balik
    /// dari mana pun. File hilang memang berarti kosong; file rusak harus
    /// menghentikan penulisan.
    pub fn try_all(&self) -> Result<Vec<Server>> {
        let raw = match fs::read_to_string(&self.path) {
            Ok(raw) => raw,
            // Belum pernah dipakai: itu bukan error.
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
            // Default bila: server pertama, ATAU tadinya default (rotasi token),
            // ATAU tak ada server lain yang bertanda default.
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
        // Ini yang paling penting di berkas ini. add/remove/set_default membaca
        // lalu menulis kembali hasilnya. Kalau file rusak dibaca sebagai "kosong",
        // perintah berikutnya menyimpan daftar baru dan MENGHAPUS semua server —
        // beserta tokennya, yang tak bisa dibaca balik dari mana pun.
        let (dir, cfg) = temp_config();
        cfg.add("prod", "https://prod.test", "tok-berharga")
            .unwrap();
        fs::write(path_of(&dir), "{ ini bukan json").unwrap();

        for result in [
            cfg.add("baru", "https://x.test", "t"),
            cfg.remove("prod"),
            cfg.set_default("prod"),
        ] {
            assert!(result.is_err(), "jalur tulis harus menolak file rusak");
        }

        // File asli harus tetap utuh, bukan tertimpa daftar baru.
        assert_eq!(
            fs::read_to_string(path_of(&dir)).unwrap(),
            "{ ini bukan json"
        );
    }

    #[test]
    fn unreadable_file_errors_and_never_wipes() {
        use std::os::unix::fs::PermissionsExt;
        // Sama bahayanya dengan file rusak: kalau read gagal (izin dicabut) dibaca
        // sebagai "kosong", perintah tulis berikutnya menghapus semua server. Read
        // yang gagal selain NotFound harus jadi error, dan jalur tulis menolak.
        let (dir, cfg) = temp_config();
        cfg.add("prod", "https://prod.test", "tok-berharga")
            .unwrap();
        let path = path_of(&dir);
        fs::set_permissions(&path, fs::Permissions::from_mode(0o000)).unwrap();

        // Root menembus izin 0o000; kalau file masih terbaca, uji ini tak berlaku.
        if fs::read_to_string(&path).is_ok() {
            fs::set_permissions(&path, fs::Permissions::from_mode(0o600)).unwrap();
            return;
        }

        assert!(cfg.try_all().is_err(), "read tak-terbaca harus error");
        assert!(cfg.all().is_empty(), "jalur baca lunak tetap tak panik");
        assert!(
            cfg.add("baru", "https://x.test", "t").is_err(),
            "jalur tulis harus menolak, bukan menimpa"
        );

        // Kembalikan izin supaya tempdir bisa dibersihkan.
        fs::set_permissions(&path, fs::Permissions::from_mode(0o600)).unwrap();
    }

    #[test]
    fn missing_file_is_empty_not_an_error() {
        // File hilang = belum pernah dipakai. Itu bukan kerusakan.
        let (_dir, cfg) = temp_config();
        assert!(cfg.try_all().unwrap().is_empty());
        assert!(cfg.add("prod", "https://prod.test", "t").is_ok());
    }

    #[test]
    fn corrupt_file_reads_as_empty_but_does_not_throw() {
        // Jalur baca tetap lunak: user melihat daftar kosong, bukan panik yang
        // meninggalkan terminal dalam raw mode saat TUI terbuka.
        let (dir, cfg) = temp_config();
        fs::write(path_of(&dir), "bukan json").unwrap();
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
