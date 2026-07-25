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
        // Tag the window so we can scope its surface background to
        // transparent — otherwise the rounded .ovl-root corners sit on an
        // opaque black square (the window's own background).
        window.add_css_class("ovl-window");

        // Inject overlay CSS — start neutral, then swap in theme-derived
        // colors once the window is realized and we can read the theme.
        let provider = CssProvider::new();
        provider.load_from_data(&overlay_css(&ThemeColors::fallback()));
        if let Some(display) = gtk4::gdk::Display::default() {
            gtk4::style_context_add_provider_for_display(
                &display,
                &provider,
                gtk4::STYLE_PROVIDER_PRIORITY_APPLICATION,
            );
        }

        // On realize: read the ACTUAL loaded theme's colors via
        // lookup_color() (works with any theme that @define-colors them —
        // Adwaita, matugen/DMS, etc.) and rebuild the CSS with those values.
        {
            let provider = provider.clone();
            window.connect_realize(move |win| {
                let colors = ThemeColors::from_widget(win.upcast_ref());
                provider.load_from_data(&overlay_css(&colors));
            });
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

    /// Fully destroy the overlay window so it disappears immediately
    /// (not just hidden — the layer-shell surface is torn down).
    pub fn destroy(&self) {
        self.window.destroy();
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

// ── Runtime theme colors ─────────────────────────────────────────────────
//
// GTK4 app CSS cannot reference @theme_* variables (the parser drops them),
// but we CAN read the loaded theme's @define-color values at runtime via
// StyleContext::lookup_color(). We then generate the overlay CSS with those
// hex values so it follows the desktop theme (Adwaita, matugen/DMS, …).

struct ThemeColors {
    bg: String,
    fg: String,
    accent: String,
    accent_fg: String,
    ok: String,
    err: String,
    warn: String,
}

impl ThemeColors {
    fn fallback() -> Self {
        ThemeColors {
            bg: "#101018".into(),
            fg: "#eeeeee".into(),
            accent: "#eeeeee".into(),
            accent_fg: "#101018".into(),
            ok: "#5be37d".into(),
            err: "#ff6b6b".into(),
            warn: "#ffc46b".into(),
        }
    }

    /// Read colors from a realized widget's style context. Tries the common
    /// @define-color names in priority order and falls back to neutral
    /// values for anything the theme doesn't define.
    fn from_widget(w: &gtk4::Widget) -> Self {
        let sc = w.style_context();
        let pick = |names: &[&str], fallback: &str| -> String {
            for n in names {
                if let Some(c) = sc.lookup_color(n) {
                    return rgba_to_hex(&c);
                }
            }
            fallback.to_string()
        };

        ThemeColors {
            bg: pick(&["theme_bg_color", "window_bg_color", "base_color"], "#101018"),
            fg: pick(&["theme_fg_color", "window_fg_color", "text_color"], "#eeeeee"),
            accent: pick(
                &["accent_bg_color", "accent_color", "theme_selected_bg_color"],
                "#eeeeee",
            ),
            accent_fg: pick(
                &["accent_fg_color", "theme_selected_fg_color"],
                "#101018",
            ),
            ok: pick(&["success_color"], "#5be37d"),
            err: pick(&["error_color", "destructive_bg_color"], "#ff6b6b"),
            warn: pick(&["warning_color"], "#ffc46b"),
        }
    }
}

fn rgba_to_hex(c: &gtk4::gdk::RGBA) -> String {
    format!(
        "#{:02x}{:02x}{:02x}",
        (c.red() * 255.0).round() as u8,
        (c.green() * 255.0).round() as u8,
        (c.blue() * 255.0).round() as u8
    )
}

fn overlay_css(t: &ThemeColors) -> String {
    format!(
        "
/* The window surface itself must be transparent or the rounded .ovl-root
 * corners render on an opaque black square. */
.ovl-window, .ovl-window.background {{
    background: transparent;
    background-color: transparent;
}}

.ovl-root {{
    background: alpha({bg}, 0.85);
    border-radius: 12px;
    border: 1px solid alpha({fg}, 0.15);
    padding: 10px 14px;
    color: {fg};
    box-shadow: 0 4px 20px alpha(#000000, 0.4);
}}

.ovl-header {{
    margin-bottom: 8px;
}}

.ovl-dot-on {{
    color: {ok};
    font-size: 14px;
}}
.ovl-dot-off {{
    color: {err};
    font-size: 14px;
}}

.ovl-title {{
    font-weight: bold;
    font-size: 13px;
}}

.ovl-section {{
    margin-top: 6px;
    margin-bottom: 4px;
}}

.ovl-section-label {{
    font-size: 9px;
    color: alpha({fg}, 0.45);
    text-transform: uppercase;
    letter-spacing: 1px;
    margin-bottom: 2px;
}}

.ovl-macro-row {{
    margin-bottom: 1px;
}}

.ovl-key {{
    background: alpha({fg}, 0.12);
    border-radius: 4px;
    padding: 1px 6px;
    font-size: 10px;
    font-weight: bold;
    color: alpha({fg}, 0.65);
    min-width: 20px;
}}
.ovl-key-on {{
    background: {accent};
    border-radius: 4px;
    padding: 1px 6px;
    font-size: 10px;
    font-weight: bold;
    color: {accent_fg};
    min-width: 20px;
}}

.ovl-macro-name {{
    font-size: 11px;
}}

.ovl-running {{
    color: {ok};
    font-size: 10px;
}}

.ovl-buff-row {{
    margin-bottom: 2px;
}}

.ovl-buff-name {{
    font-size: 10px;
    min-width: 55px;
}}

.ovl-bar {{
    min-width: 80px;
}}
.ovl-bar trough {{
    background: alpha({fg}, 0.12);
    border-radius: 3px;
}}
.ovl-bar progress {{
    background: {accent};
    border-radius: 3px;
}}

.ovl-buff-time {{
    font-size: 10px;
    color: alpha({fg}, 0.5);
    min-width: 28px;
}}

.ovl-warn {{
    color: {warn};
    font-size: 10px;
    margin-top: 2px;
}}
",
        bg = t.bg,
        fg = t.fg,
        accent = t.accent,
        accent_fg = t.accent_fg,
        ok = t.ok,
        err = t.err,
        warn = t.warn
    )
}