//! Overlay window — stays on top via wlr-layer-shell.
//!
//! Shows live state: game/class header, active macros with running indicators,
//! buff timers with animated progress bars, pixel-fire flash. Rebuilds every
//! 500ms from engine state.

use std::sync::Arc;
use gtk4::prelude::*;
use gtk4::glib::object::ObjectExt;
use gtk4::{
    Box as GtkBox, CssProvider, Label, Orientation, ProgressBar, Revealer, Window,
};
use gtk4_layer_shell::{Layer, LayerShell};

use crate::config;
use crate::engine;

pub struct Overlay {
    window: Window,
    content: GtkBox,
    cfg: Arc<config::Manager>,
    engine: Arc<engine::EngineHub>,
}

impl Overlay {
    pub fn new(cfg: Arc<config::Manager>, engine: Arc<engine::EngineHub>) -> Self {
        let window = Window::new();

        let is_wayland = std::env::var("WAYLAND_DISPLAY").is_ok()
            && std::env::var("GDK_BACKEND").unwrap_or_default() != "x11";

        if is_wayland {
            window.init_layer_shell();
            window.set_layer(Layer::Overlay);
            window.set_anchor(gtk4_layer_shell::Edge::Top, true);
            window.set_anchor(gtk4_layer_shell::Edge::Left, true);
            window.set_margin(gtk4_layer_shell::Edge::Top, 10);
            window.set_margin(gtk4_layer_shell::Edge::Left, 10);
        }

        window.set_decorated(false);

        // Inject overlay CSS
        let provider = CssProvider::new();
        provider.load_from_data(OVERLAY_CSS);
        if let Some(display) = gtk4::gdk::Display::default() {
            gtk4::style_context_add_provider_for_display(
                &display,
                &provider,
                gtk4::STYLE_PROVIDER_PRIORITY_APPLICATION,
            );
        }

        let content = GtkBox::new(Orientation::Vertical, 0);
        content.add_css_class("ovl-root");
        window.set_child(Some(&content));

        let ov = Overlay {
            window,
            content,
            cfg: cfg.clone(),
            engine: engine.clone(),
        };

        // Initial render
        ov.render();

        // Live timer: rebuild every 500ms
        let cfg_t = cfg.clone();
        let engine_t = engine.clone();
        let content_t = ov.content.clone();
        glib::timeout_add_local(std::time::Duration::from_millis(50), move || {
            render_into(&content_t, &cfg_t, &engine_t);
            glib::ControlFlow::Continue
        });

        ov
    }

    pub fn render(&self) {
        render_into(&self.content, &self.cfg, &self.engine);
    }

    pub fn show(&self) {
        self.window.show();
    }

    pub fn hide(&self) {
        self.window.hide();
    }

    pub fn window(&self) -> &Window {
        &self.window
    }

    pub fn is_visible(&self) -> bool {
        self.window.is_visible()
    }

    pub fn set_position(&self, pos: &str) {
        if pos == "hidden" {
            self.hide();
            return;
        }

        let is_wayland = std::env::var("WAYLAND_DISPLAY").is_ok()
            && std::env::var("GDK_BACKEND").unwrap_or_default() != "x11";

        if is_wayland {
            for edge in [
                gtk4_layer_shell::Edge::Top,
                gtk4_layer_shell::Edge::Bottom,
                gtk4_layer_shell::Edge::Left,
                gtk4_layer_shell::Edge::Right,
            ] {
                self.window.set_anchor(edge, false);
            }
            match pos {
                "top-left" => {
                    self.window.set_anchor(gtk4_layer_shell::Edge::Top, true);
                    self.window.set_anchor(gtk4_layer_shell::Edge::Left, true);
                }
                "top-right" => {
                    self.window.set_anchor(gtk4_layer_shell::Edge::Top, true);
                    self.window.set_anchor(gtk4_layer_shell::Edge::Right, true);
                }
                "bottom-left" => {
                    self.window.set_anchor(gtk4_layer_shell::Edge::Bottom, true);
                    self.window.set_anchor(gtk4_layer_shell::Edge::Left, true);
                }
                "bottom-right" => {
                    self.window.set_anchor(gtk4_layer_shell::Edge::Bottom, true);
                    self.window.set_anchor(gtk4_layer_shell::Edge::Right, true);
                }
                _ => {}
            }
        }
        if !self.is_visible() {
            self.show();
        }
    }
}

/// Rebuild the entire overlay content from live engine state.
fn render_into(content: &GtkBox, cfg: &Arc<config::Manager>, engine: &Arc<engine::EngineHub>) {
    // Clear
    while let Some(child) = content.first_child() {
        content.remove(&child);
    }

    let enabled = engine.macro_enabled.load(std::sync::atomic::Ordering::Acquire);
    let tree = cfg.tree();

    // ── Header: game / class / spec ──────────────────────────────
    let header = GtkBox::new(Orientation::Horizontal, 8);
    header.add_css_class("ovl-header");

    let dot = Label::new(Some(if enabled { "●" } else { "○" }));
    dot.add_css_class(if enabled { "ovl-dot-on" } else { "ovl-dot-off" });

    let title_text = if !tree.active_game.is_empty() {
        let mut s = tree.active_game.clone();
        if !tree.active_class.is_empty() {
            s.push_str(" / ");
            s.push_str(&tree.active_class);
        }
        if !tree.active_spec.is_empty() {
            s.push_str(" / ");
            s.push_str(&tree.active_spec);
        }
        s
    } else {
        "No game selected".to_string()
    };
    let title = Label::new(Some(&title_text));
    title.add_css_class("ovl-title");

    header.append(&dot);
    header.append(&title);
    content.append(&header);

    if !enabled {
        let warn = Label::new(Some("⚠ Macros disabled (toggle key)"));
        warn.add_css_class("ovl-warn");
        content.append(&warn);
        return;
    }

    // ── Macros section ───────────────────────────────────────────
    let macros = cfg.get_macros();
    let active_macros: Vec<_> = macros.iter().filter(|m| m.enabled).collect();

    if !active_macros.is_empty() {
        let section = GtkBox::new(Orientation::Vertical, 3);
        section.add_css_class("ovl-section");

        let lbl = Label::new(Some("MACROS"));
        lbl.add_css_class("ovl-section-label");
        section.append(&lbl);

        for m in &active_macros {
            let row = GtkBox::new(Orientation::Horizontal, 6);
            row.add_css_class("ovl-macro-row");

            let running = engine.macros.is_running(&m.hotkey);

            let hotkey = Label::new(Some(if m.hotkey.is_empty() { "—" } else { &m.hotkey }));
            hotkey.add_css_class(if running { "ovl-key-on" } else { "ovl-key" });

            let name = Label::new(Some(&m.name));
            name.add_css_class("ovl-macro-name");

            row.append(&hotkey);
            row.append(&name);

            if running {
                let indicator = Label::new(Some("▶"));
                indicator.add_css_class("ovl-running");
                row.append(&indicator);
            }
            section.append(&row);
        }
        content.append(&section);
    }

    // ── Buffs section ────────────────────────────────────────────
    let timers = engine.buffs.get_active_timers();
    if !timers.is_empty() {
        let section = GtkBox::new(Orientation::Vertical, 3);
        section.add_css_class("ovl-section");

        let lbl = Label::new(Some("BUFFS"));
        lbl.add_css_class("ovl-section-label");
        section.append(&lbl);

        let buff_cfgs = cfg.get_buff_timers();
        for (name, remaining) in &timers {
            let row = GtkBox::new(Orientation::Horizontal, 4);
            row.add_css_class("ovl-buff-row");

            let name_lbl = Label::new(Some(name.as_str()));
            name_lbl.add_css_class("ovl-buff-name");

            let bar = ProgressBar::new();
            bar.add_css_class("ovl-bar");
            let duration = buff_cfgs
                .iter()
                .find(|b| b.name == name.as_str())
                .map(|b| b.duration)
                .unwrap_or(5000) as f64;
            let frac = (*remaining as f64 / duration).clamp(0.0, 1.0);
            bar.set_fraction(frac);

            let time = Label::new(Some(&format!("{:.0}s", *remaining as f64 / 1000.0)));
            time.add_css_class("ovl-buff-time");

            row.append(&name_lbl);
            row.append(&bar);
            row.append(&time);
            section.append(&row);
        }
        content.append(&section);
    }

    // ── Game status ──────────────────────────────────────────────
    let game_active = engine.detector.is_active(cfg);
    let game_alive = engine.detector.is_game_alive();

    if !game_active && !tree.active_game.is_empty() {
        let status = Label::new(Some(if game_alive {
            "⚠ Game not focused (background)"
        } else {
            "⚠ Game not running"
        }));
        status.add_css_class("ovl-warn");
        content.append(&status);
    }
}

const OVERLAY_CSS: &str = "
.ovl-root {
    background: alpha(#1a1a2e, 0.85);
    border-radius: 12px;
    border: 1px solid alpha(#ffffff, 0.12);
    padding: 10px 14px;
    color: #e0e0e0;
    box-shadow: 0 4px 20px alpha(#000000, 0.4);
}

.ovl-header {
    margin-bottom: 8px;
}

.ovl-dot-on {
    color: #50fa7b;
    font-size: 14px;
}
.ovl-dot-off {
    color: #ff5555;
    font-size: 14px;
}

.ovl-title {
    font-weight: bold;
    font-size: 13px;
    color: #f8f8f2;
}

.ovl-section {
    margin-top: 6px;
    margin-bottom: 4px;
}

.ovl-section-label {
    font-size: 9px;
    color: alpha(#ffffff, 0.45);
    text-transform: uppercase;
    letter-spacing: 1px;
    margin-bottom: 2px;
}

.ovl-macro-row {
    margin-bottom: 1px;
}

.ovl-key {
    background: alpha(#ffffff, 0.1);
    border-radius: 4px;
    padding: 1px 6px;
    font-size: 10px;
    font-weight: bold;
    color: alpha(#ffffff, 0.6);
    min-width: 20px;
}
.ovl-key-on {
    background: #bd93f9;
    border-radius: 4px;
    padding: 1px 6px;
    font-size: 10px;
    font-weight: bold;
    color: #1a1a2e;
    min-width: 20px;
}

.ovl-macro-name {
    font-size: 11px;
    color: #f8f8f2;
}

.ovl-running {
    color: #50fa7b;
    font-size: 10px;
}

.ovl-buff-row {
    margin-bottom: 2px;
}

.ovl-buff-name {
    font-size: 10px;
    color: #f8f8f2;
    min-width: 55px;
}

.ovl-bar {
    min-width: 80px;
}
.ovl-bar trough {
    background: alpha(#ffffff, 0.1);
    border-radius: 3px;
}
.ovl-bar progress {
    background: #bd93f9;
    border-radius: 3px;
}

.ovl-buff-time {
    font-size: 10px;
    color: alpha(#ffffff, 0.5);
    min-width: 28px;
}

.ovl-warn {
    color: #ffb86c;
    font-size: 10px;
    margin-top: 2px;
}
";