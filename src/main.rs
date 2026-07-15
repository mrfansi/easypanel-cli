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
}

#[derive(Subcommand)]
enum NodeCmd {
    /// Daftar node cluster
    List,
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
    }
}
