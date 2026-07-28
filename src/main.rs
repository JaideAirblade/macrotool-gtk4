//! Macrotool — native GTK 4 edition.
//! Game macro/automation tool for Linux/Wayland.

mod config;
mod engine;
mod platform;
mod ui;

use gtk4::prelude::*;
use gtk4::Application;
use std::cell::RefCell;
use std::rc::Rc;
use std::sync::atomic::{AtomicBool, Ordering};
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
    let app = Application::builder().application_id(APP_ID).build();
    app.connect_activate(build_ui);

    // Signal-hook does only an atomic store in signal context. The GLib main
    // loop observes it and performs the normal GTK shutdown on its own thread.
    let shutdown_requested = Arc::new(AtomicBool::new(false));
    for signal in [libc::SIGINT, libc::SIGTERM] {
        signal_hook::flag::register(signal, shutdown_requested.clone())
            .expect("install Unix signal handler");
    }
    let app_signal = app.clone();
    glib::timeout_add_local(std::time::Duration::from_millis(50), move || {
        if shutdown_requested.swap(false, Ordering::AcqRel) {
            app_signal.quit();
            glib::ControlFlow::Break
        } else {
            glib::ControlFlow::Continue
        }
    });

    let args: Vec<String> = std::env::args().collect();
    app.run_with_args(&args);
}

fn build_ui(app: &Application) {
    // `activate` is emitted again when the user launches the single-instance
    // application while it is already running. Reuse the registered window;
    // creating another controller here would duplicate the engine, tray and
    // QML writer inside the same process.
    if let Some(window) = app.windows().into_iter().next() {
        window.present();
        return;
    }

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

    let tray = match ui::tray::TrayController::start() {
        Ok(tray) => Some(tray),
        Err(error) => {
            log::warn!("[tray] {error}");
            None
        }
    };
    let tray_available = tray
        .as_ref()
        .map(|tray| tray.availability())
        .unwrap_or_else(|| Arc::new(AtomicBool::new(false)));
    let tray_started = tray.is_some();
    let tray = Rc::new(RefCell::new(tray));

    // Keep the Rust controller alive for the full GTK application lifetime.
    // Its weak self-reference is also what lets sidebar changes refresh the
    // sidebar after the originating row signal has returned.
    let window = Rc::new(ui::MainWindow::new(
        app,
        cfg.clone(),
        engine.clone(),
        tray_available,
    ));
    install_live_accent_css(window.widget().upcast_ref());

    if tray_started {
        let tray_commands = tray.clone();
        let main_window = window.widget().clone();
        let app_command = app.clone();
        glib::timeout_add_local(std::time::Duration::from_millis(100), move || {
            let command = tray_commands
                .borrow()
                .as_ref()
                .and_then(|tray| tray.take_command());
            match command {
                Some(ui::tray::TrayCommand::Show) => main_window.present(),
                Some(ui::tray::TrayCommand::Quit) => app_command.quit(),
                None => {}
            }
            glib::ControlFlow::Continue
        });
    }

    // On shutdown: terminate and reap the QML child before Macrotool exits,
    // unregister the tray item, and flush the current configuration.
    let cfg_shutdown = cfg.clone();
    let engine_shutdown = engine.clone();
    let window_shutdown = window.clone();
    let tray_shutdown = tray.clone();
    app.connect_shutdown(move |_| {
        window_shutdown.close();
        if let Some(mut tray) = tray_shutdown.borrow_mut().take() {
            tray.shutdown();
        }
        let _ = cfg_shutdown.flush();
        drop(engine_shutdown.clone());
    });

    window.present();
}

/// GTK's plain `suggested-action` class is not fully styled by every GTK 4
/// theme. Resolve the same accent colors GTK exposes to the window and apply
/// them to primary action buttons. Refresh periodically so DMS theme changes
/// propagate without restarting Macrotool.
fn install_live_accent_css(widget: &gtk4::Widget) {
    let provider = gtk4::CssProvider::new();
    if let Some(display) = gtk4::gdk::Display::default() {
        gtk4::style_context_add_provider_for_display(
            &display,
            &provider,
            gtk4::STYLE_PROVIDER_PRIORITY_APPLICATION + 1,
        );
    }

    let widget = widget.clone();
    let last_css = Rc::new(RefCell::new(String::new()));
    let update = move || {
        let context = widget.style_context();
        let accent = ["accent_bg_color", "accent_color", "theme_selected_bg_color"]
            .iter()
            .find_map(|name| context.lookup_color(name));
        let foreground = ["accent_fg_color", "theme_selected_fg_color"]
            .iter()
            .find_map(|name| context.lookup_color(name));

        if let Some(accent) = accent {
            let foreground = foreground.unwrap_or_else(|| context.color());
            let css = format!(
                "
                button.suggested-action {{
                    background-color: {accent};
                    color: {foreground};
                    font-weight: bold;
                }}
                switch:checked,
                scale highlight,
                progressbar progress {{
                    background-color: {accent};
                }}
                selection {{
                    background-color: {accent};
                    color: {foreground};
                }}
                list row:selected {{
                    background-color: alpha({accent}, 0.18);
                }}
                list row:selected label,
                label.accent {{
                    color: {accent};
                }}
                notebook > header > tabs > tab:checked {{
                    box-shadow: inset 0 -2px 0 {accent};
                }}
                "
            );
            if *last_css.borrow() != css {
                provider.load_from_data(&css);
                *last_css.borrow_mut() = css;
            }
        }
    };

    update();
    glib::timeout_add_local(std::time::Duration::from_secs(1), move || {
        update();
        glib::ControlFlow::Continue
    });
}