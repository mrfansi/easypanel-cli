#![cfg_attr(target_os = "windows", windows_subsystem = "windows")]

// Compile the platform-neutral core, not the Unix terminal/command modules.
#[allow(dead_code)]
mod client;
#[allow(dead_code)]
mod cloudflare;
#[allow(dead_code)]
mod config;
mod desktop;
#[allow(dead_code)]
mod filter;
#[allow(dead_code)]
mod lifecycle;
#[allow(dead_code)]
mod logs;
#[allow(dead_code)]
mod tunnel;
#[allow(dead_code)]
mod uptime;

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
                .with_inner_size([1180.0, 780.0])
                .with_min_inner_size([900.0, 620.0])
                .with_app_id("io.github.mrfansi.easypanel"),
            ..Default::default()
        },
        Box::new(move |cc| Ok(Box::new(desktop::Desktop::new(cc, path)))),
    );
    tunnel::close_all();
    result
}
