mod backup;
mod client;
mod cloudflare;
mod commands;
mod config;
mod container;
mod credentials;
mod domains;
mod dump;
mod lifecycle;
mod logs;
mod migrate;
mod monitor;
mod output;
mod s3;
mod services;
mod source;
mod tui;
mod uptime;

use anyhow::Result;
use clap::{CommandFactory, Parser, Subcommand};
use clap_complete::Shell;

use commands::resolve_client;
use config::ServerConfig;

#[derive(Parser)]
#[command(
    name = "easypanel",
    version,
    about = "Manage many EasyPanel hosts from one terminal"
)]
struct Cli {
    /// Target server (default: the one marked default)
    #[arg(long, global = true)]
    server: Option<String>,

    /// Print the API's raw JSON instead of a table (read-only commands)
    #[arg(long, global = true)]
    json: bool,

    #[command(subcommand)]
    command: Option<Command>,
}

#[derive(Subcommand)]
enum Command {
    /// Manage EasyPanel hosts
    #[command(subcommand)]
    Server(ServerCmd),
    /// Manage projects
    #[command(subcommand)]
    Project(ProjectCmd),
    /// Manage services (--type defaults to app)
    #[command(subcommand)]
    Service(ServiceCmd),
    /// Host system stats (CPU/mem/disk/load)
    Stats,
    /// Manage cluster nodes
    #[command(subcommand)]
    Node(NodeCmd),
    /// Manage domains (by id)
    #[command(subcommand)]
    Domain(DomainCmd),
    /// Manage SSL certificates
    #[command(subcommand)]
    Certificate(CertificateCmd),
    /// Manage notification channels
    #[command(subcommand)]
    Notification(NotificationCmd),
    /// Run or delete backups (by id)
    #[command(subcommand)]
    Backup(BackupCmd),
    /// Non-locking database dump to object storage, and cross-server restore
    #[command(subcommand)]
    Db(DbCmd),
    /// Cloudflare — manage accounts, zones, and DNS records (outside EasyPanel)
    #[command(subcommand)]
    Cf(CfCmd),
    /// Action history (deploy, destroy, login, ...)
    #[command(subcommand)]
    Action(ActionCmd),
    /// Per-service monitoring and storage
    #[command(subcommand)]
    Monitor(MonitorCmd),
    /// Server info and Docker cleanup
    Maintenance {
        #[command(subcommand)]
        cmd: MaintenanceCmd,
    },
    /// Print a shell completion script (bash, zsh, fish, elvish, powershell)
    Completions {
        /// Target shell; omit to guess from $SHELL
        shell: Option<Shell>,
    },
    /// Print the man page (roff) to stdout
    Man,
    /// Interactive TUI
    Menu,
}

#[derive(Subcommand)]
enum MaintenanceCmd {
    /// Docker version, server IP, update availability
    Info,
    /// Remove unused containers, networks, images and build cache
    Prune {
        #[arg(long)]
        yes: bool,
    },
    /// Remove unused Docker images
    CleanupImages {
        #[arg(long)]
        yes: bool,
    },
    /// Remove the Docker build cache
    CleanupBuilder {
        #[arg(long)]
        yes: bool,
    },
}

#[derive(Subcommand)]
enum ServerCmd {
    /// Add an EasyPanel host
    Add {
        name: Option<String>,
        #[arg(long)]
        url: Option<String>,
        #[arg(long)]
        token: Option<String>,
    },
    /// List configured hosts
    List,
    /// Make a server the default
    Use { name: String },
    /// Remove a host
    Remove { name: String },
}

#[derive(Subcommand)]
enum ProjectCmd {
    /// List projects
    List,
    /// Create a project
    Create { name: String },
    /// Inspect a project and its services
    Inspect { name: String },
    /// Destroy a project and every service in it
    Destroy {
        name: String,
        #[arg(long)]
        yes: bool,
    },
    /// Export a project's config to a file (secrets redacted, no data)
    Export {
        name: String,
        /// Where to write it; default `<project>.easypanel.json`. Use `-` for stdout.
        #[arg(long)]
        file: Option<String>,
    },
}

#[derive(Subcommand)]
enum ServiceCmd {
    /// Deploy a service
    Deploy {
        project: String,
        service: String,
        #[arg(long = "type", default_value = "app")]
        service_type: String,
        #[arg(long)]
        force: bool,
    },
    /// Restart a service
    Restart {
        project: String,
        service: String,
        #[arg(long = "type", default_value = "app")]
        service_type: String,
    },
    /// Start a service
    Start {
        project: String,
        service: String,
        #[arg(long = "type", default_value = "app")]
        service_type: String,
    },
    /// Stop a service
    Stop {
        project: String,
        service: String,
        #[arg(long = "type", default_value = "app")]
        service_type: String,
    },
    /// Show service logs
    Logs {
        project: String,
        service: String,
        #[arg(long, default_value_t = 100)]
        limit: u32,
    },
    /// Show service environment variables
    Env {
        project: String,
        service: String,
        #[arg(long = "type", default_value = "app")]
        service_type: String,
    },
    /// Replace environment variables (from --file or stdin; overwrites all)
    SetEnv {
        project: String,
        service: String,
        #[arg(long = "type", default_value = "app")]
        service_type: String,
        #[arg(long)]
        file: Option<String>,
    },
    /// List exposed ports
    Ports { project: String, service: String },
    /// Add a port (published:target)
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
    /// Remove a port by index
    PortRemove {
        project: String,
        service: String,
        #[arg(long)]
        index: u32,
    },
    /// List mounts
    Mounts { project: String, service: String },
    /// Add a mount (volume|bind)
    MountAdd {
        project: String,
        service: String,
        #[arg(long, default_value = "volume")]
        kind: String,
        #[arg(long = "mount-path")]
        mount_path: String,
        /// Volume name (for --kind volume)
        #[arg(long)]
        name: Option<String>,
        /// Host path (for --kind bind)
        #[arg(long = "host-path")]
        host_path: Option<String>,
    },
    /// Remove a mount by index
    MountRemove {
        project: String,
        service: String,
        #[arg(long)]
        index: u32,
    },
    /// List a service's domains
    Domains { project: String, service: String },
    /// List databases inside a database service
    Databases { project: String, service: String },
    /// List database backup schedules
    Backups { project: String, service: String },
    /// List volume backup schedules
    VolumeBackups { project: String, service: String },
    /// Create an empty service (configure its source afterwards)
    Create {
        project: String,
        service: String,
        #[arg(long = "type", default_value = "app")]
        service_type: String,
    },
    /// Destroy a service
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
    /// List cluster nodes
    List,
}

#[derive(Subcommand)]
enum DomainCmd {
    /// List every domain on the host (source -> destination)
    List,
    /// Delete a domain by id
    Delete { id: String },
    /// Make a domain primary
    SetPrimary { id: String },
}

#[derive(Subcommand)]
enum ActionCmd {
    /// List recent actions
    List {
        #[arg(long, default_value_t = 25)]
        limit: u32,
        #[arg(long)]
        project: Option<String>,
        #[arg(long)]
        service: Option<String>,
        /// Filter by type (e.g. deployment)
        #[arg(long = "type")]
        action_type: Option<String>,
    },
    /// Kill a running action
    Kill { id: String },
}

#[derive(Subcommand)]
enum MonitorCmd {
    /// CPU/memory/network per project and service
    Services,
    /// Storage usage per service
    Storage,
}

#[derive(Subcommand)]
enum CertificateCmd {
    /// List certificates
    List,
    /// Remove a certificate by domain
    Remove { domain: String },
}

#[derive(Subcommand)]
enum NotificationCmd {
    /// List notification channels
    List,
    /// Delete a notification channel by id
    Delete { id: String },
}

#[derive(Subcommand)]
enum BackupCmd {
    /// Run a database backup now (by id)
    DbRun { id: String },
    /// Delete a database backup schedule (by id)
    DbDelete { id: String },
    /// Run a volume backup now (by id)
    VolumeRun { id: String },
    /// Delete a volume backup schedule (by id)
    VolumeDelete { id: String },
    /// List storage providers (db-restore needs their id)
    Providers,
    /// Restore a database from a backup file (OVERWRITES the database)
    DbRestore {
        #[arg(long)]
        project: String,
        #[arg(long)]
        service: String,
        /// Target database name
        #[arg(long)]
        database: String,
        /// Path to the backup file in the storage provider. Required: the EasyPanel
        /// API has no endpoint that lists existing backup files.
        #[arg(long)]
        path: String,
        /// Storage provider id (optional when only one exists)
        #[arg(long)]
        provider: Option<String>,
        #[arg(long)]
        yes: bool,
    },
}

#[derive(Subcommand)]
enum DbCmd {
    /// List the dumps this tool has written for a service (its object keys).
    List {
        project: String,
        service: String,
        /// Storage provider id or name (optional when one remote provider exists).
        #[arg(long)]
        provider: Option<String>,
    },
    /// Dump mysql/mariadb databases to object storage — non-locking, one gzip file,
    /// uploaded straight from the container to the existing remote storage (R2).
    Dump {
        project: String,
        service: String,
        /// Databases to include (comma-separated). Omit and pass --all instead.
        #[arg(long, value_delimiter = ',')]
        databases: Vec<String>,
        /// Dump every non-system database the service holds.
        #[arg(long)]
        all: bool,
        /// Storage provider id or name (optional when one remote provider exists).
        #[arg(long)]
        provider: Option<String>,
    },
    /// Restore a dump written by `db dump`. It recreates the databases, so it works
    /// on a host where they never existed — the cross-server case EasyPanel can't do.
    Restore {
        project: String,
        service: String,
        /// Object key (path) of the dump in the storage provider.
        #[arg(long)]
        path: String,
        /// Storage provider id or name (optional when one remote provider exists).
        #[arg(long)]
        provider: Option<String>,
        #[arg(long)]
        yes: bool,
    },
}

/// Cloudflare — a bounded context outside EasyPanel. Accounts are managed here (config
/// only); zones and DNS records arrive with the Cloudflare client.
#[derive(Subcommand)]
enum CfCmd {
    /// Manage stored Cloudflare accounts (independent of EasyPanel servers)
    #[command(subcommand)]
    Account(CfAccountCmd),
    /// Manage zones on the active account
    #[command(subcommand)]
    Zone(CfZoneCmd),
    /// Manage DNS records within a zone
    #[command(subcommand)]
    Record(CfRecordCmd),
}

#[derive(Subcommand)]
enum CfZoneCmd {
    /// List zones on the account
    List {
        #[arg(long)]
        account: Option<String>,
    },
    /// Add (create) a zone — needs the account's account-id
    Add {
        name: String,
        #[arg(long)]
        account: Option<String>,
    },
    /// Delete a zone and ALL its DNS records (asks you to type the zone name)
    Delete {
        zone: String,
        #[arg(long)]
        account: Option<String>,
        #[arg(long)]
        yes: bool,
    },
}

#[derive(Subcommand)]
enum CfRecordCmd {
    /// List a zone's DNS records (filter with --type/--name/--content)
    List {
        zone: String,
        #[arg(long = "type")]
        kind: Option<String>,
        #[arg(long)]
        name: Option<String>,
        #[arg(long)]
        content: Option<String>,
        #[arg(long)]
        account: Option<String>,
    },
    /// Add a DNS record
    Add {
        zone: String,
        #[arg(long = "type")]
        kind: String,
        #[arg(long)]
        name: String,
        #[arg(long)]
        content: String,
        /// TTL in seconds; 1 = automatic (default)
        #[arg(long, default_value_t = 1)]
        ttl: u32,
        #[arg(long)]
        proxied: bool,
        #[arg(long)]
        priority: Option<u16>,
        #[arg(long)]
        account: Option<String>,
    },
    /// Bulk-change a field on selected records (e.g. repoint every record off an old IP)
    Set {
        zone: String,
        /// Explicit record ids to change
        ids: Vec<String>,
        #[arg(long)]
        where_content: Option<String>,
        #[arg(long = "where-type")]
        where_type: Option<String>,
        #[arg(long)]
        where_name: Option<String>,
        #[arg(long)]
        content: Option<String>,
        #[arg(long)]
        proxied: Option<bool>,
        #[arg(long)]
        ttl: Option<u32>,
        #[arg(long)]
        priority: Option<u16>,
        #[arg(long)]
        account: Option<String>,
        #[arg(long)]
        yes: bool,
    },
    /// Delete one or more records by id
    Delete {
        zone: String,
        ids: Vec<String>,
        #[arg(long)]
        account: Option<String>,
    },
}

#[derive(Subcommand)]
enum CfAccountCmd {
    /// Add or replace a Cloudflare account by label (prompts for the token if omitted)
    Add {
        name: String,
        /// Cloudflare account id — needed only to create zones
        #[arg(long)]
        account_id: Option<String>,
        /// The API token; omit to be prompted without echo
        #[arg(long)]
        token: Option<String>,
    },
    /// List stored accounts (token masked)
    List,
    /// Set the default account
    Use { name: String },
    /// Remove an account
    Delete { name: String },
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
    output::set_json_output(cli.json);
    match cli.command {
        // Touches neither the network nor config: handled first so completions
        // can still be printed even before any server is registered.
        Some(Command::Completions { shell }) => {
            let Some(shell) = shell.or_else(Shell::from_env) else {
                anyhow::bail!(
                    "Could not guess the shell from $SHELL. Specify one: easypanel completions <bash|zsh|fish|elvish|powershell>"
                );
            };
            let mut cmd = Cli::command();
            clap_complete::generate(shell, &mut cmd, "easypanel", &mut std::io::stdout());
            Ok(())
        }

        // Same as completions: touches neither the network nor config.
        Some(Command::Man) => {
            let mut out = Vec::new();
            clap_mangen::Man::new(Cli::command()).render(&mut out)?;
            use std::io::Write;
            std::io::stdout().write_all(&out)?;
            Ok(())
        }

        None | Some(Command::Menu) => {
            if cfg.all().is_empty() {
                println!("No servers configured yet. Run: easypanel server add");
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
                ProjectCmd::Export { name, file } => commands::project_export(&client, &name, file),
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

        Some(Command::Db(c)) => {
            let client = resolve_client(cfg, &cli.server)?;
            match c {
                DbCmd::List {
                    project,
                    service,
                    provider,
                } => commands::db_list(&client, &project, &service, provider.as_deref()),
                DbCmd::Dump {
                    project,
                    service,
                    databases,
                    all,
                    provider,
                } => commands::db_dump(
                    &client,
                    &project,
                    &service,
                    &databases,
                    all,
                    provider.as_deref(),
                ),
                DbCmd::Restore {
                    project,
                    service,
                    path,
                    provider,
                    yes,
                } => commands::db_restore(
                    &client,
                    &project,
                    &service,
                    &path,
                    provider.as_deref(),
                    yes,
                ),
            }
        }

        Some(Command::Cf(c)) => {
            // Cloudflare accounts live in their own store, independent of servers.
            let cf = config::CloudflareConfig::new(config::CloudflareConfig::default_path());
            match c {
                CfCmd::Account(a) => match a {
                    CfAccountCmd::Add {
                        name,
                        account_id,
                        token,
                    } => commands::cf_account_add(&cf, name, account_id, token),
                    CfAccountCmd::List => commands::cf_account_list(&cf),
                    CfAccountCmd::Use { name } => commands::cf_account_use(&cf, &name),
                    CfAccountCmd::Delete { name } => commands::cf_account_delete(&cf, &name),
                },
                CfCmd::Zone(z) => match z {
                    CfZoneCmd::List { account } => commands::cf_zone_list(&cf, account.as_deref()),
                    CfZoneCmd::Add { name, account } => {
                        commands::cf_zone_add(&cf, account.as_deref(), &name)
                    }
                    CfZoneCmd::Delete { zone, account, yes } => {
                        commands::cf_zone_delete(&cf, account.as_deref(), &zone, yes)
                    }
                },
                CfCmd::Record(r) => match r {
                    CfRecordCmd::List {
                        zone,
                        kind,
                        name,
                        content,
                        account,
                    } => commands::cf_record_list(
                        &cf,
                        account.as_deref(),
                        &zone,
                        cloudflare::RecordFilter {
                            kind,
                            name,
                            content,
                        },
                    ),
                    CfRecordCmd::Add {
                        zone,
                        kind,
                        name,
                        content,
                        ttl,
                        proxied,
                        priority,
                        account,
                    } => commands::cf_record_add(
                        &cf,
                        account.as_deref(),
                        &zone,
                        &kind,
                        &name,
                        &content,
                        ttl,
                        proxied,
                        priority,
                    ),
                    CfRecordCmd::Set {
                        zone,
                        ids,
                        where_content,
                        where_type,
                        where_name,
                        content,
                        proxied,
                        ttl,
                        priority,
                        account,
                        yes,
                    } => commands::cf_record_set(
                        &cf,
                        account.as_deref(),
                        &zone,
                        cloudflare::Selector {
                            ids,
                            where_content,
                            where_type,
                            where_name,
                        },
                        cloudflare::RecordPatch {
                            content,
                            proxied,
                            ttl,
                            priority,
                        },
                        yes,
                    ),
                    CfRecordCmd::Delete { zone, ids, account } => {
                        commands::cf_record_delete(&cf, account.as_deref(), &zone, &ids)
                    }
                },
            }
        }

        Some(Command::Maintenance { cmd }) => {
            let client = resolve_client(cfg, &cli.server)?;
            match cmd {
                MaintenanceCmd::Info => commands::maintenance_info(&client),
                MaintenanceCmd::Prune { yes } => {
                    commands::maintenance_clean(&client, "systemPrune", "Prune Docker system", yes)
                }
                MaintenanceCmd::CleanupImages { yes } => commands::maintenance_clean(
                    &client,
                    "cleanupDockerImages",
                    "Remove unused images",
                    yes,
                ),
                MaintenanceCmd::CleanupBuilder { yes } => commands::maintenance_clean(
                    &client,
                    "cleanupDockerBuilder",
                    "Remove build cache",
                    yes,
                ),
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// clap validates the entire CLI definition: duplicate names, arg conflicts, etc.
    /// This catches mistakes that would otherwise only surface at runtime.
    #[test]
    fn cli_definition_is_valid() {
        Cli::command().debug_assert();
    }

    #[test]
    fn man_page_renders_with_the_sections_man_expects() {
        let mut out = Vec::new();
        clap_mangen::Man::new(Cli::command())
            .render(&mut out)
            .expect("man page should render");
        let roff = String::from_utf8(out).expect("man page should be UTF-8");
        // Without these sections, `man` shows a broken/empty page.
        for section in ["NAME", "SYNOPSIS", "DESCRIPTION", "OPTIONS"] {
            assert!(
                roff.contains(section),
                "man page is missing section {section}"
            );
        }
        // A real subcommand must show up, otherwise the page is useless.
        assert!(
            roff.contains("maintenance"),
            "man page doesn't include a subcommand"
        );
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
            let script = String::from_utf8(out).expect("completion script should be UTF-8");
            assert!(
                script.len() > 100,
                "{shell} produced an empty/too-short script"
            );
            // A script that doesn't mention any subcommand means the generator
            // lost the CLI definition — passes "no error" but is useless.
            assert!(
                script.contains("maintenance"),
                "{shell} doesn't include a real subcommand"
            );
        }
    }
}
