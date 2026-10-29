// Hide the console window on Windows release builds.
#![cfg_attr(all(windows, not(debug_assertions)), windows_subsystem = "windows")]

mod agent;
mod app;
mod approval;
mod artifacts;
mod attach;
mod background;
mod branches;
mod checkpoints;
mod commands;
mod compaction;
mod config;
mod documents;
mod env_path;
mod exec;
mod export;
mod git;
mod hooks;
mod instructions;
mod llm;
mod mcp;
mod model_options;
mod native;
mod plan;
mod remote;
mod review;
mod sandbox;
mod schedule;
mod secrets;
mod servers;
mod skills;
mod sse;
mod stats;
mod subagent;
#[cfg(test)]
mod testutil;
mod tools;
mod tray;
mod types;
mod ui;
mod update;

use eframe::egui;

fn main() -> eframe::Result {
    // Must run before any threads exist (it mutates the process environment).
    env_path::fix_path();
    env_logger::Builder::from_env(env_logger::Env::default().default_filter_or("warn")).init();
    // The command line never opens the window: `cleanrstai exec …` and `--version`.
    let args: Vec<String> = std::env::args().skip(1).collect();
    match args.first().map(String::as_str) {
        Some("exec") => std::process::exit(exec::run(&args[1..])),
        Some("--version") => {
            println!("cleanrstai {}", env!("CARGO_PKG_VERSION"));
            std::process::exit(0);
        }
        _ => {}
    }
    update::remove_leftovers();

    let runtime = tokio::runtime::Builder::new_multi_thread()
        .enable_all()
        .thread_name("cleanrstai-worker")
        .build()
        .expect("failed to start async runtime");
    let handle = runtime.handle().clone();

    let options = eframe::NativeOptions {
        viewport: egui::ViewportBuilder::default()
            .with_title("CleanRSTAI")
            .with_app_id("cleanrstai")
            .with_inner_size([1180.0, 780.0])
            .with_min_inner_size([640.0, 420.0])
            // Light until the first frame knows the theme; the app swaps it to match.
            .with_icon(ui::theme::window_icon(false)),
        ..Default::default()
    };

    let result = eframe::run_native(
        "CleanRSTAI",
        options,
        Box::new(move |cc| Ok(Box::new(app::ChatApp::new(cc, handle)))),
    );
    // Don't wait indefinitely for blocking I/O (e.g. a pipe held open by an MCP server's
    // child process) when quitting.
    runtime.shutdown_timeout(std::time::Duration::from_millis(500));
    result
}
