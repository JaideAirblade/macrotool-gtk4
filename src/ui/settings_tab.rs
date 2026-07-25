//! Settings tab — general app settings.

use crate::config::Settings;
use gtk4::prelude::*;
use gtk4::{
    Box as GtkBox, Button, Entry, Label, Orientation, ScrolledWindow, SpinButton, Switch,
};
use std::sync::Arc;
use glib::object::Cast;

pub struct SettingsTab {
    container: gtk4::Box,
}

impl SettingsTab {
    pub fn new(cfg: Arc<crate::config::Manager>, _engine: Arc<crate::engine::EngineHub>) -> Self {
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

        // General card
        content.append(&make_general_card(&settings));

        // Appearance card
        content.append(&make_appearance_card(&settings));

        // Pixel Engine card
        content.append(&make_pixel_card(&settings));

        // Advanced card
        content.append(&make_advanced_card(&settings));

        // Version
        let version = Label::new(Some(&format!("Macrotool v{}", env!("CARGO_PKG_VERSION"))));
        version.add_css_class("version-footer");
        content.append(&version);

        scrolled.set_child(Some(&content));

        SettingsTab { container }
    }

    pub fn refresh(&self, _cfg: &Arc<crate::config::Manager>) {
        // Settings are static at construction; could rebuild on change
    }

    pub fn widget(&self) -> &gtk4::Widget {
        self.container.upcast_ref()
    }
}

fn make_card(title: &str) -> gtk4::Box {
    let card = gtk4::Box::new(Orientation::Vertical, 8);
    card.add_css_class("card");
    card.set_margin_bottom(12);

    let title_label = Label::new(Some(title));
    title_label.add_css_class("card-section-title");
    title_label.set_halign(gtk4::Align::Start);
    card.append(&title_label);
    card
}

fn make_field(label: &str, widget: &impl IsA<gtk4::Widget>) -> gtk4::Box {
    let row = GtkBox::new(Orientation::Horizontal, 12);
    row.set_margin_start(12);
    row.set_margin_end(12);
    row.set_margin_top(4);
    row.set_margin_bottom(4);

    let lbl = Label::new(Some(label));
    lbl.set_halign(gtk4::Align::Start);
    lbl.set_size_request(160, -1);
    row.append(&lbl);

    let right = gtk4::Box::new(Orientation::Horizontal, 0);
    right.set_hexpand(true);
    right.append(widget);
    row.append(&right);
    row
}

fn make_toggle_field(label: &str, active: bool) -> gtk4::Box {
    let sw = Switch::new();
    sw.set_active(active);
    make_field(label, &sw)
}

fn make_general_card(s: &Settings) -> gtk4::Box {
    let card = make_card("General");

    let delay_spin = SpinButton::with_range(0.0, 100000.0, 1.0);
    delay_spin.set_value(s.default_delay as f64);
    card.append(&make_field("Default Delay (ms)", &delay_spin));

    let toggle_entry = Entry::new();
    toggle_entry.set_text(&s.toggle_key);
    card.append(&make_field("Toggle Key", &toggle_entry));

    card.append(&make_toggle_field("Only in Game", s.only_in_game));
    card.append(&make_toggle_field("Allow Background", s.allow_background));
    card.append(&make_toggle_field("Auto Detect Game", s.auto_detect_game));
    card.append(&make_toggle_field("Minimize to Tray", s.minimize_to_tray));

    card
}

fn make_appearance_card(s: &Settings) -> gtk4::Box {
    let card = make_card("Appearance");
    card.append(&make_toggle_field("Dark Mode", s.dark_mode));
    card
}

fn make_pixel_card(s: &Settings) -> gtk4::Box {
    let card = make_card("Pixel Engine");

    let rate_spin = SpinButton::with_range(1.0, 1000.0, 1.0);
    rate_spin.set_value(s.pixel_check_rate as f64);
    card.append(&make_field("Check Rate (checks/sec)", &rate_spin));

    card
}

fn make_advanced_card(s: &Settings) -> gtk4::Box {
    let card = make_card("Advanced");
    card.append(&make_toggle_field("Show Terminal", s.show_terminal));
    card
}