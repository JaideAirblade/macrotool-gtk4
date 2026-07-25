//! Macrotool — native GTK 4 edition.
//! Game macro/automation tool for Linux/Wayland.

mod config;
mod engine;
mod platform;
mod ui;

use gtk4::prelude::*;
use gtk4::Application;
use std::sync::Arc;

const APP_ID: &str = "com.jaide.macrotool";

fn main() {
    // Let GTK use the native display — Wayland by default, falls back to X11.
    // The overlay needs Wayland's layer-shell protocol to stay on top.

    let app = Application::builder().application_id(APP_ID).build();

    app.connect_activate(build_ui);

    let args: Vec<String> = std::env::args().collect();
    app.run_with_args(&args);
}

fn build_ui(app: &Application) {
    // Load config
    let cfg = Arc::new(config::Manager::new());
    if let Err(e) = cfg.load() {
        log::warn!("[config] load failed: {}", e);
    }

    // Create and start engine hub
    let engine = Arc::new(engine::EngineHub::new(cfg.clone()));
    engine.start();

    // Flush config on shutdown
    {
        let cfg = cfg.clone();
        let engine = engine.clone();
        app.connect_shutdown(move |_| {
            let _ = cfg.flush();
            // engine is dropped here via the Arc in the closure
            drop(engine.clone());
        });
    }

    // Build the main window
    let window = ui::MainWindow::new(app, cfg.clone(), engine.clone());
    window.present();
}