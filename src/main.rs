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

/// Theme-aware UI styling. GTK4 app CSS cannot reference @theme_* variables
/// (GTK3/libadwaita-only — the parser warns and drops them), so everything
/// derives from `currentColor`, which the theme sets on each widget. This
/// makes the app follow the desktop theme (light or dark, any accent).
const UI_CSS: &str = "
.card {
    background: alpha(currentColor, 0.05);
    border-radius: 8px;
    padding: 12px;
}

.tab-title {
    font-size: 18px;
    font-weight: bold;
}

.heading {
    font-weight: bold;
}

.dim-label {
    color: alpha(currentColor, 0.55);
}

.accent {
    font-weight: bold;
}

/* suggested-action / destructive-action are libadwaita classes; approximate
 * them with a subtle tinted background so they read as primary/danger
 * without hardcoded colors. */
button.suggested-action {
    background: alpha(currentColor, 0.18);
}

button.destructive-action {
    color: #e06c75;
}

button.capturing {
    background: alpha(currentColor, 0.2);
}

/* ON/OFF state badges on macro/trigger/buff cards */
.badge {
    font-weight: bold;
    font-size: 11px;
    padding: 2px 8px;
    border-radius: 6px;
    background: alpha(currentColor, 0.12);
}
.badge-off {
    color: alpha(currentColor, 0.45);
    background: alpha(currentColor, 0.06);
}

/* Notebook tab bar — make the selected tab visibly follow the theme.
 * GtkNotebook tabs get little styling from most themes (they style
 * libadwaita's AdwTabBar instead), so give them a currentColor-derived
 * highlight. */
notebook > header {
    background: alpha(currentColor, 0.04);
}
notebook > header > tabs > tab {
    padding: 6px 14px;
}
notebook > header > tabs > tab:checked {
    background: alpha(currentColor, 0.12);
    box-shadow: inset 0 -2px 0 currentColor;
}
notebook > header > tabs > tab:hover {
    background: alpha(currentColor, 0.07);
}
";

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
    // Load theme-aware UI CSS now that GTK is initialized
    {
        let provider = gtk4::CssProvider::new();
        provider.load_from_data(UI_CSS);
        if let Some(display) = gtk4::gdk::Display::default() {
            gtk4::style_context_add_provider_for_display(
                &display,
                &provider,
                gtk4::STYLE_PROVIDER_PRIORITY_APPLICATION,
            );
        }
    }

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