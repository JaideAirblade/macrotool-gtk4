//! Macros tab — edit macro list for the active spec.

use crate::config::{ConfigTree, Macro, Spec};
use gtk4::prelude::*;
use gtk4::{
    Box as GtkBox, Button, ComboBoxText, Entry, Label, Orientation, ScrolledWindow, SpinButton, Switch,
};
use std::sync::Arc;

pub struct MacrosTab {
    container: gtk4::Box,
    macros_box: gtk4::Box,
}

impl MacrosTab {
    pub fn new(cfg: Arc<crate::config::Manager>, _engine: Arc<crate::engine::EngineHub>) -> Self {
        let container = gtk4::Box::new(Orientation::Vertical, 0);

        let scrolled = ScrolledWindow::new();
        scrolled.set_hexpand(true);
        scrolled.set_vexpand(true);

        let content = gtk4::Box::new(Orientation::Vertical, 12);
        content.set_margin_start(16);
        content.set_margin_end(16);
        content.set_margin_top(16);
        content.set_margin_bottom(16);

        // Header
        let header = GtkBox::new(Orientation::Horizontal, 12);
        header.set_hexpand(true);
        let title = Label::new(Some("Macros"));
        title.add_css_class("tab-title");
        header.append(&title);
        let add_btn = Button::with_label("+ Add Macro");
        add_btn.add_css_class("btn-primary");
        header.append(&add_btn);
        content.append(&header);

        // Macros list container
        let macros_box = gtk4::Box::new(Orientation::Vertical, 12);
        content.append(&macros_box);

        scrolled.set_child(Some(&content));
        container.append(&scrolled);

        let tab = MacrosTab { container, macros_box };
        tab.refresh(&cfg);
        tab
    }

    pub fn refresh(&self, cfg: &Arc<crate::config::Manager>) {
        // Clear existing children
        while let Some(child) = self.macros_box.first_child() {
            self.macros_box.remove(&child);
        }

        let tree = cfg.tree();
        let spec = match get_active_spec(&tree) {
            Some(s) => s,
            None => {
                let empty = Label::new(Some("Select a spec to edit macros"));
                empty.add_css_class("empty-state");
                self.macros_box.append(&empty);
                return;
            }
        };

        if spec.macros.is_empty() {
            let empty = Label::new(Some("No macros yet"));
            empty.add_css_class("empty-state");
            self.macros_box.append(&empty);
            return;
        }

        for m in &spec.macros {
            self.macros_box.append(&make_macro_card(m));
        }
    }

    pub fn widget(&self) -> &gtk4::Widget {
        self.container.upcast_ref()
    }
}

fn make_macro_card(m: &Macro) -> gtk4::Box {
    let card = gtk4::Box::new(Orientation::Vertical, 8);
    card.add_css_class("card");
    card.set_margin_bottom(8);

    // Header row
    let header = GtkBox::new(Orientation::Horizontal, 8);
    let name_entry = Entry::new();
    name_entry.set_text(&m.name);
    name_entry.add_css_class("card-title");
    name_entry.set_hexpand(true);
    header.append(&name_entry);

    let badge = Label::new(Some(if m.enabled { "ON" } else { "OFF" }));
    badge.add_css_class(if m.enabled { "badge" } else { "badge-off" });
    header.append(&badge);

    let delete_btn = Button::with_label("✕");
    delete_btn.add_css_class("icon-btn");
    delete_btn.set_tooltip_text(Some("Delete"));
    header.append(&delete_btn);
    card.append(&header);

    // Grid of fields
    let grid = gtk4::Grid::new();
    grid.set_column_spacing(12);
    grid.set_row_spacing(8);

    // Hotkey
    let hotkey_lbl = Label::new(Some("Hotkey"));
    hotkey_lbl.set_halign(gtk4::Align::Start);
    let hotkey_entry = Entry::new();
    hotkey_entry.set_text(&m.hotkey);
    hotkey_entry.set_hexpand(true);
    grid.attach(&hotkey_lbl, 0, 0, 1, 1);
    grid.attach(&hotkey_entry, 1, 0, 1, 1);

    // Mode
    let mode_lbl = Label::new(Some("Mode"));
    mode_lbl.set_halign(gtk4::Align::Start);
    let mode_combo = ComboBoxText::new();
    mode_combo.append_text("Press");
    mode_combo.append_text("Hold");
    mode_combo.append_text("Toggle");
    match m.mode.as_str() {
        "press" => mode_combo.set_active(Some(0)),
        "hold" => mode_combo.set_active(Some(1)),
        "toggle" => mode_combo.set_active(Some(2)),
        _ => mode_combo.set_active(Some(0)),
    }
    grid.attach(&mode_lbl, 2, 0, 1, 1);
    grid.attach(&mode_combo, 3, 0, 1, 1);

    // Delay
    let delay_lbl = Label::new(Some("Delay (ms)"));
    delay_lbl.set_halign(gtk4::Align::Start);
    let delay_spin = SpinButton::with_range(0.0, 100000.0, 1.0);
    delay_spin.set_value(m.delay as f64);
    grid.attach(&delay_lbl, 0, 1, 1, 1);
    grid.attach(&delay_spin, 1, 1, 1, 1);

    // Inter-key delay
    let ikd_lbl = Label::new(Some("Inter-Key Delay"));
    ikd_lbl.set_halign(gtk4::Align::Start);
    let ikd_spin = SpinButton::with_range(0.0, 100000.0, 1.0);
    ikd_spin.set_value(m.inter_key_delay as f64);
    grid.attach(&ikd_lbl, 2, 1, 1, 1);
    grid.attach(&ikd_spin, 3, 1, 1, 1);

    card.append(&grid);

    // Toggles
    let toggles = GtkBox::new(Orientation::Horizontal, 24);
    let enabled_box = GtkBox::new(Orientation::Horizontal, 8);
    enabled_box.append(&Label::new(Some("Enabled")));
    let enabled_switch = Switch::new();
    enabled_switch.set_active(m.enabled);
    enabled_box.append(&enabled_switch);
    toggles.append(&enabled_box);

    let bg_box = GtkBox::new(Orientation::Horizontal, 8);
    bg_box.append(&Label::new(Some("Background")));
    let bg_switch = Switch::new();
    bg_switch.set_active(m.background);
    bg_box.append(&bg_switch);
    toggles.append(&bg_box);
    card.append(&toggles);

    // Keys
    let keys_label = Label::new(Some(&format!("Keys: {}", m.keys.join(", "))));
    keys_label.set_halign(gtk4::Align::Start);
    card.append(&keys_label);

    card
}

pub fn get_active_spec(tree: &ConfigTree) -> Option<Spec> {
    let game = tree.games.get(&tree.active_game)?;
    let class = game.classes.get(&tree.active_class)?;
    class.specs.get(&tree.active_spec).cloned()
}