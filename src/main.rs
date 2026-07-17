mod client;
mod commands;
mod config;
mod logs;
mod output;
mod tui;

use anyhow::Result;
use clap::{CommandFactory, Parser, Subcommand};
use clap_complete::Shell;

use commands::resolve_client;
use config::ServerConfig;

#[derive(Parser)]
#[command(name = "easypanel", version, about = "Kelola banyak host EasyPanel")]
struct Cli {
    /// Nama server target (default: server bertanda default)
    #[arg(long, global = true)]
    server: Option<String>,

    #[command(subcommand)]
    command: Option<Command>,
}

#[derive(Subcommand)]
enum Command {
    /// Kelola host EasyPanel
    #[command(subcommand)]
    Server(ServerCmd),
    /// Kelola project
    #[command(subcommand)]
    Project(ProjectCmd),
    /// Kelola service (--type default: app)
    #[command(subcommand)]
    Service(ServiceCmd),
    /// System stats host (CPU/mem/disk/load)
    Stats,
    /// Kelola node cluster
    #[command(subcommand)]
    Node(NodeCmd),
    /// Kelola domain (by id)
    #[command(subcommand)]
    Domain(DomainCmd),
    /// Kelola SSL certificates
    #[command(subcommand)]
    Certificate(CertificateCmd),
    /// Kelola notification channels
    #[command(subcommand)]
    Notification(NotificationCmd),
    /// Jalankan/hapus backup (by id)
    #[command(subcommand)]
    Backup(BackupCmd),
    /// Riwayat action (deploy, destroy, login, ...)
    #[command(subcommand)]
    Action(ActionCmd),
    /// Monitoring per-service dan storage
    #[command(subcommand)]
    Monitor(MonitorCmd),
    /// Info server & pembersihan Docker
    Maintenance {
        #[command(subcommand)]
        cmd: MaintenanceCmd,
    },
    /// Cetak skrip completion shell (bash, zsh, fish, elvish, powershell)
    Completions {
        /// Shell target; kosongkan untuk menebak dari $SHELL
        shell: Option<Shell>,
    },
    /// Menu interaktif
    Menu,
}

#[derive(Subcommand)]
enum MaintenanceCmd {
    /// Versi Docker, IP server, ketersediaan update
    Info,
    /// Hapus container/network/image/build cache yang tak terpakai
    Prune {
        #[arg(long)]
        yes: bool,
    },
    /// Hapus image Docker yang tak terpakai
    CleanupImages {
        #[arg(long)]
        yes: bool,
    },
    /// Hapus build cache Docker
    CleanupBuilder {
        #[arg(long)]
        yes: bool,
    },
}

#[derive(Subcommand)]
enum ServerCmd {
    /// Tambah host EasyPanel
    Add {
        name: Option<String>,
        #[arg(long)]
        url: Option<String>,
        #[arg(long)]
        token: Option<String>,
    },
    /// Daftar host terkonfigurasi
    List,
    /// Jadikan sebuah server sebagai default
    Use { name: String },
    /// Hapus host
    Remove { name: String },
}

#[derive(Subcommand)]
enum ProjectCmd {
    /// Daftar project
    List,
    /// Buat project baru
    Create { name: String },
    /// Lihat detail project dan service-nya
    Inspect { name: String },
    /// Hapus project beserta semua service-nya
    Destroy {
        name: String,
        #[arg(long)]
        yes: bool,
    },
}

#[derive(Subcommand)]
enum ServiceCmd {
    /// Deploy service
    Deploy {
        project: String,
        service: String,
        #[arg(long = "type", default_value = "app")]
        service_type: String,
        #[arg(long)]
        force: bool,
    },
    /// Restart service
    Restart {
        project: String,
        service: String,
        #[arg(long = "type", default_value = "app")]
        service_type: String,
    },
    /// Start service
    Start {
        project: String,
        service: String,
        #[arg(long = "type", default_value = "app")]
        service_type: String,
    },
    /// Stop service
    Stop {
        project: String,
        service: String,
        #[arg(long = "type", default_value = "app")]
        service_type: String,
    },
    /// Tampilkan log service
    Logs {
        project: String,
        service: String,
        #[arg(long, default_value_t = 100)]
        limit: u32,
    },
    /// Tampilkan environment variables service
    Env {
        project: String,
        service: String,
        #[arg(long = "type", default_value = "app")]
        service_type: String,
    },
    /// Ganti environment variables (dari --file atau stdin, menimpa yang lama)
    SetEnv {
        project: String,
        service: String,
        #[arg(long = "type", default_value = "app")]
        service_type: String,
        #[arg(long)]
        file: Option<String>,
    },
    /// Daftar port ter-expose
    Ports { project: String, service: String },
    /// Tambah port (published:target)
    PortAdd {
        project: String,
        service: String,
        #[arg(long)]
        published: u32,
        #[arg(long)]
        target: u32,
        #[arg(long, default_value = "tcp")]
        protocol: String,
    },
    /// Hapus port berdasarkan index
    PortRemove {
        project: String,
        service: String,
        #[arg(long)]
        index: u32,
    },
    /// Daftar mount
    Mounts { project: String, service: String },
    /// Tambah mount (volume|bind)
    MountAdd {
        project: String,
        service: String,
        #[arg(long, default_value = "volume")]
        kind: String,
        #[arg(long = "mount-path")]
        mount_path: String,
        /// Nama volume (untuk --kind volume)
        #[arg(long)]
        name: Option<String>,
        /// Path host (untuk --kind bind)
        #[arg(long = "host-path")]
        host_path: Option<String>,
    },
    /// Hapus mount berdasarkan index
    MountRemove {
        project: String,
        service: String,
        #[arg(long)]
        index: u32,
    },
    /// Daftar domain service
    Domains { project: String, service: String },
    /// Daftar database dalam service database
    Databases { project: String, service: String },
    /// Daftar jadwal backup database service
    Backups { project: String, service: String },
    /// Daftar jadwal backup volume service
    VolumeBackups { project: String, service: String },
    /// Buat service kosong (konfigurasi source menyusul)
    Create {
        project: String,
        service: String,
        #[arg(long = "type", default_value = "app")]
        service_type: String,
    },
    /// Hapus service
    Destroy {
        project: String,
        service: String,
        #[arg(long = "type", default_value = "app")]
        service_type: String,
        #[arg(long)]
        yes: bool,
    },
}

#[derive(Subcommand)]
enum NodeCmd {
    /// Daftar node cluster
    List,
}

#[derive(Subcommand)]
enum DomainCmd {
    /// Daftar semua domain di host (source -> destination)
    List,
    /// Hapus domain berdasarkan id
    Delete { id: String },
    /// Jadikan domain sebagai primary
    SetPrimary { id: String },
}

#[derive(Subcommand)]
enum ActionCmd {
    /// Daftar action terbaru
    List {
        #[arg(long, default_value_t = 25)]
        limit: u32,
        #[arg(long)]
        project: Option<String>,
        #[arg(long)]
        service: Option<String>,
        /// Filter tipe (mis. deployment)
        #[arg(long = "type")]
        action_type: Option<String>,
    },
    /// Hentikan action yang sedang berjalan
    Kill { id: String },
}

#[derive(Subcommand)]
enum MonitorCmd {
    /// CPU/memori/network per project & service
    Services,
    /// Pemakaian storage per service
    Storage,
}

#[derive(Subcommand)]
enum CertificateCmd {
    /// Daftar certificate
    List,
    /// Hapus certificate berdasarkan domain
    Remove { domain: String },
}

#[derive(Subcommand)]
enum NotificationCmd {
    /// Daftar notification channel
    List,
    /// Hapus notification channel berdasarkan id
    Delete { id: String },
}

#[derive(Subcommand)]
enum BackupCmd {
    /// Jalankan backup database sekarang (by id)
    DbRun { id: String },
    /// Hapus jadwal backup database (by id)
    DbDelete { id: String },
    /// Jalankan backup volume sekarang (by id)
    VolumeRun { id: String },
    /// Hapus jadwal backup volume (by id)
    VolumeDelete { id: String },
    /// Storage provider terdaftar (id-nya dibutuhkan db-restore)
    Providers,
    /// Restore database dari sebuah file backup (MENIMPA isi database)
    DbRestore {
        #[arg(long)]
        project: String,
        #[arg(long)]
        service: String,
        /// Nama database tujuan
        #[arg(long)]
        database: String,
        /// Path file backup di storage provider. Wajib: API EasyPanel tak punya
        /// endpoint untuk mendaftar file backup yang ada.
        #[arg(long)]
        path: String,
        /// Id storage provider (opsional bila cuma ada satu)
        #[arg(long)]
        provider: Option<String>,
        #[arg(long)]
        yes: bool,
    },
}

fn main() {
    let cli = Cli::parse();
    let cfg = ServerConfig::new(ServerConfig::default_path());

    if let Err(e) = run(cli, &cfg) {
        eprintln!("{e}");
        std::process::exit(1);
    }
}

fn run(cli: Cli, cfg: &ServerConfig) -> Result<()> {
    match cli.command {
        // Tak menyentuh jaringan maupun config: didahulukan supaya completion
        // tetap bisa dicetak sebelum ada server terdaftar sekalipun.
        Some(Command::Completions { shell }) => {
            let Some(shell) = shell.or_else(Shell::from_env) else {
                anyhow::bail!(
                    "Tak bisa menebak shell dari $SHELL. Sebutkan: easypanel completions <bash|zsh|fish|elvish|powershell>"
                );
            };
            let mut cmd = Cli::command();
            clap_complete::generate(shell, &mut cmd, "easypanel", &mut std::io::stdout());
            Ok(())
        }

        None | Some(Command::Menu) => {
            if cfg.all().is_empty() {
                println!("Belum ada server. Jalankan: easypanel server add");
                return Ok(());
            }
            let name = cli
                .server
                .clone()
                .or_else(|| cfg.default().map(|s| s.name))
                .unwrap_or_default();
            let client = resolve_client(cfg, &cli.server)?;
            tui::run(cfg, client, name)
        }

        Some(Command::Server(c)) => match c {
            ServerCmd::Add { name, url, token } => commands::server_add(cfg, name, url, token),
            ServerCmd::List => commands::server_list(cfg),
            ServerCmd::Use { name } => commands::server_use(cfg, &name),
            ServerCmd::Remove { name } => commands::server_remove(cfg, &name),
        },

        Some(Command::Project(c)) => {
            let client = resolve_client(cfg, &cli.server)?;
            match c {
                ProjectCmd::List => commands::project_list(&client),
                ProjectCmd::Create { name } => commands::project_create(&client, &name),
                ProjectCmd::Inspect { name } => commands::project_inspect(&client, &name),
                ProjectCmd::Destroy { name, yes } => commands::project_destroy(&client, &name, yes),
            }
        }

        Some(Command::Service(c)) => {
            let client = resolve_client(cfg, &cli.server)?;
            match c {
                ServiceCmd::Deploy {
                    project,
                    service,
                    service_type,
                    force,
                } => commands::service_action(
                    &client,
                    &project,
                    &service,
                    &service_type,
                    "deploy",
                    force,
                ),
                ServiceCmd::Restart {
                    project,
                    service,
                    service_type,
                } => commands::service_action(
                    &client,
                    &project,
                    &service,
                    &service_type,
                    "restart",
                    false,
                ),
                ServiceCmd::Start {
                    project,
                    service,
                    service_type,
                } => commands::service_action(
                    &client,
                    &project,
                    &service,
                    &service_type,
                    "start",
                    false,
                ),
                ServiceCmd::Stop {
                    project,
                    service,
                    service_type,
                } => commands::service_action(
                    &client,
                    &project,
                    &service,
                    &service_type,
                    "stop",
                    false,
                ),
                ServiceCmd::Logs {
                    project,
                    service,
                    limit,
                } => commands::service_logs(&client, &project, &service, limit),
                ServiceCmd::Env {
                    project,
                    service,
                    service_type,
                } => commands::service_env(&client, &project, &service, &service_type),
                ServiceCmd::SetEnv {
                    project,
                    service,
                    service_type,
                    file,
                } => commands::service_set_env(&client, &project, &service, &service_type, file),
                ServiceCmd::Ports { project, service } => {
                    commands::ports_list(&client, &project, &service)
                }
                ServiceCmd::PortAdd {
                    project,
                    service,
                    published,
                    target,
                    protocol,
                } => commands::port_add(&client, &project, &service, published, target, &protocol),
                ServiceCmd::PortRemove {
                    project,
                    service,
                    index,
                } => commands::port_remove(&client, &project, &service, index),
                ServiceCmd::Mounts { project, service } => {
                    commands::mounts_list(&client, &project, &service)
                }
                ServiceCmd::MountAdd {
                    project,
                    service,
                    kind,
                    mount_path,
                    name,
                    host_path,
                } => commands::mount_add(
                    &client,
                    &project,
                    &service,
                    &kind,
                    &mount_path,
                    name,
                    host_path,
                ),
                ServiceCmd::MountRemove {
                    project,
                    service,
                    index,
                } => commands::mount_remove(&client, &project, &service, index),
                ServiceCmd::Domains { project, service } => {
                    commands::domains_list(&client, &project, &service)
                }
                ServiceCmd::Databases { project, service } => {
                    commands::service_databases(&client, &project, &service)
                }
                ServiceCmd::Backups { project, service } => {
                    commands::db_backup_list(&client, &project, &service)
                }
                ServiceCmd::VolumeBackups { project, service } => {
                    commands::volume_backup_list(&client, &project, &service)
                }
                ServiceCmd::Create {
                    project,
                    service,
                    service_type,
                } => commands::service_create(&client, &project, &service, &service_type),
                ServiceCmd::Destroy {
                    project,
                    service,
                    service_type,
                    yes,
                } => commands::service_destroy(&client, &project, &service, &service_type, yes),
            }
        }

        Some(Command::Stats) => {
            let client = resolve_client(cfg, &cli.server)?;
            commands::stats(&client)
        }

        Some(Command::Node(c)) => {
            let client = resolve_client(cfg, &cli.server)?;
            match c {
                NodeCmd::List => commands::node_list(&client),
            }
        }

        Some(Command::Domain(c)) => {
            let client = resolve_client(cfg, &cli.server)?;
            match c {
                DomainCmd::List => commands::domain_list_all(&client),
                DomainCmd::Delete { id } => commands::domain_delete(&client, &id),
                DomainCmd::SetPrimary { id } => commands::domain_set_primary(&client, &id),
            }
        }

        Some(Command::Action(c)) => {
            let client = resolve_client(cfg, &cli.server)?;
            match c {
                ActionCmd::List {
                    limit,
                    project,
                    service,
                    action_type,
                } => commands::action_list(&client, limit, project, service, action_type),
                ActionCmd::Kill { id } => commands::action_kill(&client, &id),
            }
        }

        Some(Command::Monitor(c)) => {
            let client = resolve_client(cfg, &cli.server)?;
            match c {
                MonitorCmd::Services => commands::monitor_services(&client),
                MonitorCmd::Storage => commands::monitor_storage(&client),
            }
        }

        Some(Command::Certificate(c)) => {
            let client = resolve_client(cfg, &cli.server)?;
            match c {
                CertificateCmd::List => commands::certificate_list(&client),
                CertificateCmd::Remove { domain } => commands::certificate_remove(&client, &domain),
            }
        }

        Some(Command::Notification(c)) => {
            let client = resolve_client(cfg, &cli.server)?;
            match c {
                NotificationCmd::List => commands::notification_list(&client),
                NotificationCmd::Delete { id } => commands::notification_delete(&client, &id),
            }
        }

        Some(Command::Backup(c)) => {
            let client = resolve_client(cfg, &cli.server)?;
            match c {
                BackupCmd::DbRun { id } => commands::db_backup_run(&client, &id),
                BackupCmd::DbDelete { id } => commands::db_backup_delete(&client, &id),
                BackupCmd::VolumeRun { id } => commands::volume_backup_run(&client, &id),
                BackupCmd::VolumeDelete { id } => commands::volume_backup_delete(&client, &id),
                BackupCmd::Providers => commands::storage_providers(&client),
                BackupCmd::DbRestore {
                    project,
                    service,
                    database,
                    path,
                    provider,
                    yes,
                } => commands::backup_db_restore(
                    &client,
                    &project,
                    &service,
                    &database,
                    &path,
                    provider.as_deref(),
                    yes,
                ),
            }
        }

        Some(Command::Maintenance { cmd }) => {
            let client = resolve_client(cfg, &cli.server)?;
            match cmd {
                MaintenanceCmd::Info => commands::maintenance_info(&client),
                MaintenanceCmd::Prune { yes } => {
                    commands::maintenance_clean(&client, "systemPrune", "Prune sistem Docker", yes)
                }
                MaintenanceCmd::CleanupImages { yes } => commands::maintenance_clean(
                    &client,
                    "cleanupDockerImages",
                    "Hapus image tak terpakai",
                    yes,
                ),
                MaintenanceCmd::CleanupBuilder { yes } => commands::maintenance_clean(
                    &client,
                    "cleanupDockerBuilder",
                    "Hapus build cache",
                    yes,
                ),
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// clap memvalidasi seluruh definisi CLI: nama ganda, arg konflik, dsb.
    /// Ini menangkap kesalahan yang selama ini hanya muncul saat runtime.
    #[test]
    fn cli_definition_is_valid() {
        Cli::command().debug_assert();
    }

    #[test]
    fn completions_generate_for_every_shell() {
        for shell in [
            Shell::Bash,
            Shell::Zsh,
            Shell::Fish,
            Shell::Elvish,
            Shell::PowerShell,
        ] {
            let mut out = Vec::new();
            clap_complete::generate(shell, &mut Cli::command(), "easypanel", &mut out);
            let script = String::from_utf8(out).expect("skrip completion harus UTF-8");
            assert!(
                script.len() > 100,
                "{shell} menghasilkan skrip kosong/terlalu pendek"
            );
            // Skrip yang tak menyebut subcommand mana pun berarti generatornya
            // kehilangan definisi CLI — lolos "tak error" tapi tak berguna.
            assert!(
                script.contains("maintenance"),
                "{shell} tak memuat subcommand nyata"
            );
        }
    }
}
