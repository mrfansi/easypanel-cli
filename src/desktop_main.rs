#![cfg_attr(target_os = "windows", windows_subsystem = "windows")]

// Shared operations and models; no terminal renderer or CLI bootstrap.
#[allow(dead_code)]
mod backup;
#[allow(dead_code)]
mod client;
#[allow(dead_code)]
mod cloudflare;
#[allow(dead_code)]
mod config;
#[allow(dead_code)]
mod container;
#[allow(dead_code)]
mod credentials;
#[allow(dead_code)]
mod dbms;
mod desktop;
#[allow(dead_code)]
mod domains;
#[allow(dead_code)]
mod dump;
#[allow(dead_code)]
mod filter;
#[allow(dead_code)]
mod form;
#[allow(dead_code)]
mod lifecycle;
#[allow(dead_code)]
mod logs;
#[allow(dead_code)]
mod migrate;
#[allow(dead_code)]
mod monitor;
#[allow(dead_code)]
mod operations;
#[allow(dead_code)]
mod output;
#[allow(dead_code)]
mod s3;
#[allow(dead_code)]
mod services;
#[allow(dead_code)]
mod session;
#[allow(dead_code)]
mod source;
#[allow(dead_code)]
mod tunnel;
#[allow(dead_code)]
mod uptime;
#[allow(dead_code)]
mod worker;

use clap::Parser;

#[derive(Parser)]
#[command(name = "easypanel-desktop", about = "EasyPanel desktop console")]
struct Args {
    /// Connection file; defaults to the same servers.json as the CLI.
    #[arg(long)]
    config: Option<std::path::PathBuf>,
}

fn main() -> eframe::Result {
    if tunnel::answer_askpass() {
        return Ok(());
    }
    let path = Args::parse()
        .config
        .unwrap_or_else(config::ServerConfig::default_path);
    let result = eframe::run_native(
        "EasyPanel",
        eframe::NativeOptions {
            viewport: eframe::egui::ViewportBuilder::default()
                .with_inner_size([1280.0, 800.0])
                .with_min_inner_size([1024.0, 680.0])
                .with_app_id("io.github.mrfansi.easypanel"),
            ..Default::default()
        },
        Box::new(move |cc| Ok(Box::new(desktop::Desktop::new(cc, path)))),
    );
    tunnel::close_all();
    result
}
