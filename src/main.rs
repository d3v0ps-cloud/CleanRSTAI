// Hide the console window on Windows release builds.
#![cfg_attr(all(windows, not(debug_assertions)), windows_subsystem = "windows")]

mod agent;
mod app;
mod attach;
mod config;
mod env_path;
mod export;
mod llm;
mod mcp;
mod secrets;
mod sse;
mod stats;
#[cfg(test)]
mod testutil;
mod types;
mod ui;

use eframe::egui;

fn main() -> eframe::Result {
    // Must run before any threads exist (it mutates the process environment).
    env_path::fix_path();
    env_logger::Builder::from_env(env_logger::Env::default().default_filter_or("warn")).init();

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
            .with_min_inner_size([640.0, 420.0]),
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
