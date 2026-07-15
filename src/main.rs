mod client;
mod commands;
mod config;
mod logs;
mod menu;
mod output;

use anyhow::Result;
use clap::{Parser, Subcommand};

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
    /// System stats host (CPU/mem/disk/uptime)
    Stats,
    /// Kelola node cluster
    #[command(subcommand)]
    Node(NodeCmd),
    /// Kelola domain (by id)
    #[command(subcommand)]
    Domain(DomainCmd),
    /// Menu interaktif
    Menu,
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
    /// Hapus domain berdasarkan id
    Delete { id: String },
    /// Jadikan domain sebagai primary
    SetPrimary { id: String },
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
        None | Some(Command::Menu) => commands::run_menu(cfg),

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
                DomainCmd::Delete { id } => commands::domain_delete(&client, &id),
                DomainCmd::SetPrimary { id } => commands::domain_set_primary(&client, &id),
            }
        }
    }
}
