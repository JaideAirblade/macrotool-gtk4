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
    // Install signal handlers so the overlay is destroyed on SIGTERM/SIGINT.
    // Without this, killing the process leaves the layer-shell surface visible.
    unsafe {
        libc::signal(libc::SIGINT, sig_handler as usize);
        libc::signal(libc::SIGTERM, sig_handler as usize);
    }

    let app = Application::builder().application_id(APP_ID).build();
    app.connect_activate(build_ui);

    let args: Vec<String> = std::env::args().collect();
    app.run_with_args(&args);
}

/// Signal handler — quit the app gracefully so GTK destroys all windows
/// (including the layer-shell overlay) before the process exits.
extern "C" fn sig_handler(_sig: libc::c_int) {
    let app = gtk4::Application::default();
    let is_ours = app
        .application_id()
        .map(|id| id.to_string())
        .as_deref()
        .map(|id| id == APP_ID)
        .unwrap_or(false);
    if is_ours {
        glib::idle_add_local_once(move || {
            app.quit();
        });
    } else {
        std::process::exit(0);
    }
}

fn build_ui(app: &Application) {
    let cfg = Arc::new(config::Manager::new());
    if let Err(e) = cfg.load() {
        log::warn!("[config] load failed: {}", e);
    }

    let engine = Arc::new(engine::EngineHub::new(cfg.clone()));
    engine.start();

    let window = ui::MainWindow::new(app, cfg.clone(), engine.clone());

    // On shutdown: destroy overlay window + flush config so the
    // layer-shell surface doesn't linger after the process exits.
    let cfg_shutdown = cfg.clone();
    let engine_shutdown = engine.clone();
    let overlay_window = window.overlay_window();
    app.connect_shutdown(move |_| {
        overlay_window.destroy();
        let _ = cfg_shutdown.flush();
        drop(engine_shutdown.clone());
    });

    window.present();
}