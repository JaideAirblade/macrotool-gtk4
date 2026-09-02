//! Settings tab — general app settings. Every control saves immediately.

use crate::config::Settings;
use crate::ui::key_capture::KeyCapture;
use gtk4::prelude::*;
use gtk4::{Box as GtkBox, ComboBoxText, Label, Orientation, ScrolledWindow, SpinButton, Switch};
use std::sync::Arc;

pub struct SettingsTab {
    container: gtk4::Box,
}

impl SettingsTab {
    pub fn new(cfg: Arc<crate::config::Manager>, engine: Arc<crate::engine::EngineHub>) -> Self {
        let container = gtk4::Box::new(Orientation::Vertical, 0);
        let scrolled = ScrolledWindow::new();
        scrolled.set_hexpand(true);
        scrolled.set_vexpand(true);
        container.append(&scrolled);

        let content = gtk4::Box::new(Orientation::Vertical, 16);
        content.set_margin_start(16);
        content.set_margin_end(16);
        content.set_margin_top(16);
        content.set_margin_bottom(16);

        let settings = cfg.settings();

        // General
        content.append(&make_general_card(&settings, &cfg, &engine));
        // Appearance
        content.append(&make_appearance_card(&settings, &cfg));
        // Pixel Engine
        content.append(&make_pixel_card(&settings, &cfg));
        // Advanced
        content.append(&make_advanced_card(&settings, &cfg));

        let version = Label::new(Some(&format!("Macrotool v{}", env!("CARGO_PKG_VERSION"))));
        version.add_css_class("dim-label");
        content.append(&version);

        scrolled.set_child(Some(&content));

        SettingsTab { container }
    }

    pub fn refresh(&self, _cfg: &Arc<crate::config::Manager>) {}

    pub fn widget(&self) -> &gtk4::Widget {
        self.container.upcast_ref()
    }
}

// ── helpers ──────────────────────────────────────────────────────────────

fn set_setting<F: FnOnce(&mut Settings)>(cfg: &Arc<crate::config::Manager>, f: F) {
    let mut s = cfg.settings();
    f(&mut s);
    cfg.set_settings(s);
}

fn make_card(title: &str) -> gtk4::Box {
    let card = gtk4::Box::new(Orientation::Vertical, 8);
    card.add_css_class("card");
    card.set_margin_bottom(12);

    let title_label = Label::new(Some(title));
    title_label.add_css_class("heading");
    title_label.set_halign(gtk4::Align::Start);
    card.append(&title_label);
    card
}

fn field_row(label: &str, widget: &impl IsA<gtk4::Widget>) -> gtk4::Box {
    let row = GtkBox::new(Orientation::Horizontal, 12);
    row.set_margin_start(4);
    row.set_margin_end(4);
    row.set_margin_top(2);
    row.set_margin_bottom(2);

    let lbl = Label::new(Some(label));
    lbl.set_halign(gtk4::Align::Start);
    lbl.set_size_request(180, -1);
    row.append(&lbl);

    widget.set_hexpand(true);
    row.append(widget);
    row
}

fn toggle_row(
    label: &str,
    active: bool,
    cfg: &Arc<crate::config::Manager>,
    engine: Option<Arc<crate::engine::EngineHub>>,
    f: impl Fn(&mut Settings, bool) + 'static,
) -> gtk4::Box {
    let sw = Switch::new();
    sw.set_active(active);
    sw.set_halign(gtk4::Align::End);
    {
        let cfg = cfg.clone();
        sw.connect_state_set(move |_, state| {
            set_setting(&cfg, |s| f(s, state));
            if let Some(e) = &engine {
                e.reload_profile();
            }
            glib::Propagation::Proceed
        });
    }
    field_row(label, &sw)
}

// ── cards ────────────────────────────────────────────────────────────────

fn make_general_card(
    s: &Settings,
    cfg: &Arc<crate::config::Manager>,
    engine: &Arc<crate::engine::EngineHub>,
) -> gtk4::Box {
    let card = make_card("General");

    let delay_spin = SpinButton::with_range(0.0, 100000.0, 1.0);
    delay_spin.set_value(s.default_delay as f64);
    {
        let cfg = cfg.clone();
        delay_spin.connect_value_changed(move |spin| {
            let v = spin.value() as i32;
            set_setting(&cfg, |s| s.default_delay = v);
        });
    }
    card.append(&field_row("Default Delay (ms)", &delay_spin));

    let toggle_cap = KeyCapture::new(&s.toggle_key, {
        let cfg = cfg.clone();
        Box::new(move |key: &str| {
            let key = key.to_string();
            set_setting(&cfg, |s| s.toggle_key = key);
        })
    });
    card.append(&field_row("Toggle Key", toggle_cap.widget()));

    card.append(&toggle_row(
        "Only in Game",
        s.only_in_game,
        cfg,
        Some(engine.clone()),
        |s, v| {
            s.only_in_game = v;
        },
    ));
    card.append(&toggle_row(
        "Auto Detect Game",
        s.auto_detect_game,
        cfg,
        Some(engine.clone()),
        |s, v| {
            s.auto_detect_game = v;
        },
    ));
    card.append(&toggle_row(
        "Minimize to Tray",
        s.minimize_to_tray,
        cfg,
        None,
        |s, v| {
            s.minimize_to_tray = v;
        },
    ));

    card
}

fn make_appearance_card(s: &Settings, cfg: &Arc<crate::config::Manager>) -> gtk4::Box {
    let card = make_card("Appearance");
    card.append(&toggle_row("Dark Mode", s.dark_mode, cfg, None, |s, v| {
        s.dark_mode = v;
    }));

    let position = ComboBoxText::new();
    for (id, label) in [
        ("top-left", "Top left"),
        ("top-right", "Top right"),
        ("bottom-left", "Bottom left"),
        ("bottom-right", "Bottom right"),
        ("hidden", "Hidden"),
    ] {
        position.append(Some(id), label);
    }
    position.set_active_id(Some(&s.overlay_position));
    {
        let cfg = cfg.clone();
        position.connect_changed(move |combo| {
            if let Some(id) = combo.active_id() {
                let position = id.to_string();
                set_setting(&cfg, |s| s.overlay_position = position);
            }
        });
    }
    card.append(&field_row("Overlay Position", &position));

    card
}

fn make_pixel_card(s: &Settings, cfg: &Arc<crate::config::Manager>) -> gtk4::Box {
    let card = make_card("Pixel Engine");

    let rate_spin = SpinButton::with_range(1.0, 1000.0, 1.0);
    rate_spin.set_value(s.pixel_check_rate as f64);
    {
        let cfg = cfg.clone();
        rate_spin.connect_value_changed(move |spin| {
            let v = spin.value() as i32;
            set_setting(&cfg, |s| s.pixel_check_rate = v);
        });
    }
    card.append(&field_row("Check Rate (checks/sec)", &rate_spin));

    card
}

fn make_advanced_card(s: &Settings, cfg: &Arc<crate::config::Manager>) -> gtk4::Box {
    let card = make_card("Advanced");
    card.append(&toggle_row(
        "Show Terminal",
        s.show_terminal,
        cfg,
        None,
        |s, v| {
            s.show_terminal = v;
        },
    ));
    card
}
