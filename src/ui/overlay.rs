//! Overlay window — stays on top of the game via the wlr-layer-shell protocol.
//!
//! Modular tiling layout: the overlay is a grid of independent "boxes":
//!   - Macros box: shows active macros + their hotkeys + running indicator
//!   - Buffs box: shows active buff timers with progress bars
//!   - Pixel-fire box: shows a flash indicator when pixel triggers fire
//!
//! The overlay is click-through (non-interactive) by default and can be
//! positioned at any screen edge or corner. It uses the system GTK theme.

use std::sync::Arc;
use gtk4::prelude::*;
use gtk4::glib::object::ObjectExt;
use gtk4::{
    Box as GtkBox, Label, Orientation, ProgressBar, Revealer, Window,
};
use gtk4_layer_shell::{Layer, LayerShell};

use crate::config;
use crate::engine;

pub struct Overlay {
    window: Window,
    macros_box: GtkBox,
    buffs_box: GtkBox,
    pixel_fire_label: Label,
    pixel_fire_revealer: Revealer,
}

impl Overlay {
    pub fn new(cfg: Arc<config::Manager>, engine: Arc<engine::EngineHub>) -> Self {
        let window = Window::new();

        // Initialize layer shell — stays on top on Wayland via wlr-layer-shell.
        // On X11, layer-shell is unavailable; the window just shows as a
        // normal borderless window.
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

        // No decorations — borderless overlay
        window.set_decorated(false);

        // Main container — vertical flow of boxes
        let container = GtkBox::new(Orientation::Vertical, 6);
        container.add_css_class("overlay-container");
        container.set_margin_start(8);
        container.set_margin_end(8);
        container.set_margin_top(8);
        container.set_margin_bottom(8);

        // ── Macros box ──────────────────────────────────────────────
        let macros_box = GtkBox::new(Orientation::Vertical, 2);
        macros_box.add_css_class("overlay-box");
        let macros_title = Label::new(Some("Macros"));
        macros_title.add_css_class("overlay-box-title");
        macros_title.set_halign(gtk4::Align::Start);
        macros_box.append(&macros_title);
        container.append(&macros_box);

        // ── Buffs box ───────────────────────────────────────────────
        let buffs_box = GtkBox::new(Orientation::Vertical, 2);
        buffs_box.add_css_class("overlay-box");
        let buffs_title = Label::new(Some("Buffs"));
        buffs_title.add_css_class("overlay-box-title");
        buffs_title.set_halign(gtk4::Align::Start);
        buffs_box.append(&buffs_title);
        container.append(&buffs_box);

        // ── Pixel-fire indicator ────────────────────────────────────
        let pixel_fire_label = Label::new(Some("⚡ Pixel Trigger"));
        pixel_fire_label.add_css_class("overlay-pixel-fire");
        let pixel_fire_revealer = Revealer::new();
        pixel_fire_revealer.set_child(Some(&pixel_fire_label));
        pixel_fire_revealer.set_reveal_child(false);
        container.append(&pixel_fire_revealer);

        window.set_child(Some(&container));

        let mut overlay = Overlay {
            window,
            macros_box,
            buffs_box,
            pixel_fire_label,
            pixel_fire_revealer,
        };

        overlay.refresh(&cfg, &engine);
        overlay
    }

    /// Update the overlay contents from the current config + engine state.
    pub fn refresh(&mut self, cfg: &Arc<config::Manager>, engine: &Arc<engine::EngineHub>) {
        self.refresh_macros(cfg, engine);
        self.refresh_buffs(cfg, engine);
    }

    fn refresh_macros(&self, cfg: &Arc<config::Manager>, engine: &Arc<engine::EngineHub>) {
        // Clear old children (keep the title = first child)
        let mut child = self.macros_box.first_child();
        if let Some(first) = child {
            child = first.next_sibling();
            while let Some(c) = child {
                let next = c.next_sibling();
                self.macros_box.remove(&c);
                child = next;
            }
        }

        let macros = cfg.get_macros();
        let enabled = engine.macro_enabled.load(std::sync::atomic::Ordering::Acquire);

        if !enabled {
            let disabled = Label::new(Some("⚠ Macros disabled"));
            disabled.add_css_class("overlay-warn");
            disabled.set_halign(gtk4::Align::Start);
            self.macros_box.append(&disabled);
            return;
        }

        for m in macros.iter().filter(|m| m.enabled) {
            let row = GtkBox::new(Orientation::Horizontal, 6);
            row.set_halign(gtk4::Align::Start);

            let hotkey = Label::new(Some(&m.hotkey));
            hotkey.add_css_class("overlay-hotkey");

            let name = Label::new(Some(&m.name));
            name.add_css_class("overlay-macro-name");

            let running = if engine.macros.is_running(&m.hotkey) {
                let r = Label::new(Some("▶"));
                r.add_css_class("overlay-running");
                Some(r)
            } else {
                None
            };

            row.append(&hotkey);
            row.append(&name);
            if let Some(r) = running {
                row.append(&r);
            }
            self.macros_box.append(&row);
        }

        if macros.is_empty() || !macros.iter().any(|m| m.enabled) {
            let empty = Label::new(Some("No active macros"));
            empty.add_css_class("overlay-dim");
            empty.set_halign(gtk4::Align::Start);
            self.macros_box.append(&empty);
        }
    }

    fn refresh_buffs(&self, cfg: &Arc<config::Manager>, engine: &Arc<engine::EngineHub>) {
        // Clear old children (keep title)
        let mut child = self.buffs_box.first_child();
        if let Some(first) = child {
            child = first.next_sibling();
            while let Some(c) = child {
                let next = c.next_sibling();
                self.buffs_box.remove(&c);
                child = next;
            }
        }

        let timers = engine.buffs.get_active_timers();
        if timers.is_empty() {
            let empty = Label::new(Some("No active buffs"));
            empty.add_css_class("overlay-dim");
            empty.set_halign(gtk4::Align::Start);
            self.buffs_box.append(&empty);
            return;
        }

        let buff_cfgs = cfg.get_buff_timers();
        for (name, remaining) in &timers {
            let row = GtkBox::new(Orientation::Horizontal, 4);
            row.set_halign(gtk4::Align::Start);

            let name_label = Label::new(Some(name.as_str()));
            name_label.add_css_class("overlay-buff-name");
            name_label.set_xalign(0.0);
            name_label.set_size_request(60, -1);

            let bar = ProgressBar::new();
            bar.add_css_class("overlay-buff-bar");
            let duration = buff_cfgs
                .iter()
                .find(|b| b.name == name.as_str())
                .map(|b| b.duration)
                .unwrap_or(5000) as f64;
            let frac = (*remaining as f64 / duration).clamp(0.0, 1.0);
            bar.set_fraction(frac);

            let time_label = Label::new(Some(&format!("{:.1}s", *remaining as f64 / 1000.0)));
            time_label.add_css_class("overlay-dim");
            time_label.set_size_request(36, -1);

            row.append(&name_label);
            row.append(&bar);
            row.append(&time_label);
            self.buffs_box.append(&row);
        }
    }

    /// Flash the pixel-fire indicator briefly.
    pub fn flash_pixel_fire(&self) {
        self.pixel_fire_revealer.set_reveal_child(true);
        // Hide after 1s
        let revealer = self.pixel_fire_revealer.clone();
        glib::timeout_add_local_once(std::time::Duration::from_secs(1), move || {
            revealer.set_reveal_child(false);
        });
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

    /// Move the overlay to a screen edge/corner.
    pub fn set_position(&self, pos: &str) {
        if pos == "hidden" {
            self.hide();
            return;
        }

        let is_wayland = std::env::var("WAYLAND_DISPLAY").is_ok()
            && std::env::var("GDK_BACKEND").unwrap_or_default() != "x11";

        if is_wayland {
            // Reset all anchors
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