//! Pixel triggers tab — edit pixel trigger list for the active spec.

use crate::config::PixelTrigger;
use gtk4::prelude::*;
use gtk4::{
    Box as GtkBox, Button, ComboBoxText, Entry, Label, Orientation, ScrolledWindow, SpinButton, Switch,
};
use std::sync::Arc;

pub struct ProcsTab {
    container: gtk4::Box,
    content_box: gtk4::Box,
}

impl ProcsTab {
    pub fn new(cfg: Arc<crate::config::Manager>, _engine: Arc<crate::engine::EngineHub>) -> Self {
        let container = gtk4::Box::new(Orientation::Vertical, 0);

        let scrolled = ScrolledWindow::new();
        scrolled.set_hexpand(true);
        scrolled.set_vexpand(true);

        let content_box = gtk4::Box::new(Orientation::Vertical, 12);
        content_box.set_margin_start(16);
        content_box.set_margin_end(16);
        content_box.set_margin_top(16);
        content_box.set_margin_bottom(16);

        // Header
        let header = GtkBox::new(Orientation::Horizontal, 12);
        header.set_hexpand(true);
        let title = Label::new(Some("Pixel Triggers"));
        title.add_css_class("tab-title");
        header.append(&title);
        let add_btn = Button::with_label("+ Add Trigger");
        add_btn.add_css_class("btn-primary");
        header.append(&add_btn);
        content_box.append(&header);

        scrolled.set_child(Some(&content_box));
        container.append(&scrolled);

        let tab = ProcsTab { container, content_box };
        tab.refresh(&cfg);
        tab
    }

    pub fn refresh(&self, cfg: &Arc<crate::config::Manager>) {
        // Clear all children except the header (first child)
        let mut child = self.content_box.first_child();
        if let Some(first) = child {
            child = first.next_sibling();
            while let Some(c) = child {
                let next = c.next_sibling();
                self.content_box.remove(&c);
                child = next;
            }
        }

        let tree = cfg.tree();
        let spec = match super::macros_tab::get_active_spec(&tree) {
            Some(s) => s,
            None => {
                let empty = Label::new(Some("Select a spec"));
                empty.add_css_class("empty-state");
                self.content_box.append(&empty);
                return;
            }
        };

        if spec.pixel_triggers.is_empty() {
            let empty = Label::new(Some("No pixel triggers"));
            empty.add_css_class("empty-state");
            self.content_box.append(&empty);
        } else {
            for t in &spec.pixel_triggers {
                self.content_box.append(&make_trigger_card(t));
            }
        }
    }

    pub fn widget(&self) -> &gtk4::Widget {
        self.container.upcast_ref()
    }
}

fn make_trigger_card(t: &PixelTrigger) -> gtk4::Box {
    let card = gtk4::Box::new(Orientation::Vertical, 8);
    card.add_css_class("card");
    card.set_margin_bottom(8);

    // Header
    let header = GtkBox::new(Orientation::Horizontal, 8);
    let name_entry = Entry::new();
    name_entry.set_text(&t.name);
    name_entry.add_css_class("card-title");
    name_entry.set_hexpand(true);
    header.append(&name_entry);

    let badge = Label::new(Some(if t.enabled { "ON" } else { "OFF" }));
    badge.add_css_class(if t.enabled { "badge" } else { "badge-off" });
    header.append(&badge);

    let delete_btn = Button::with_label("✕");
    delete_btn.add_css_class("icon-btn");
    header.append(&delete_btn);
    card.append(&header);

    // Grid
    let grid = gtk4::Grid::new();
    grid.set_column_spacing(12);
    grid.set_row_spacing(8);

    let action_lbl = Label::new(Some("Action Key"));
    action_lbl.set_halign(gtk4::Align::Start);
    let action_entry = Entry::new();
    action_entry.set_text(&t.action_key);
    grid.attach(&action_lbl, 0, 0, 1, 1);
    grid.attach(&action_entry, 1, 0, 1, 1);

    let match_lbl = Label::new(Some("Match Mode"));
    match_lbl.set_halign(gtk4::Align::Start);
    let match_combo = ComboBoxText::new();
    match_combo.append_text("All");
    match_combo.append_text("Any");
    match_combo.set_active(Some(if t.match_mode == "any" { 1 } else { 0 }));
    grid.attach(&match_lbl, 2, 0, 1, 1);
    grid.attach(&match_combo, 3, 0, 1, 1);

    let cd_lbl = Label::new(Some("Cooldown (ms)"));
    cd_lbl.set_halign(gtk4::Align::Start);
    let cd_spin = SpinButton::with_range(0.0, 100000.0, 1.0);
    cd_spin.set_value(t.cooldown as f64);
    grid.attach(&cd_lbl, 0, 1, 1, 1);
    grid.attach(&cd_spin, 1, 1, 1, 1);

    let tm_lbl = Label::new(Some("Trigger Mode"));
    tm_lbl.set_halign(gtk4::Align::Start);
    let tm_combo = ComboBoxText::new();
    tm_combo.append_text("Only when macro running");
    tm_combo.append_text("Always");
    tm_combo.set_active(Some(if t.trigger_mode == "always" { 1 } else { 0 }));
    grid.attach(&tm_lbl, 2, 1, 1, 1);
    grid.attach(&tm_combo, 3, 1, 1, 1);

    card.append(&grid);

    // Toggles
    let toggles = GtkBox::new(Orientation::Horizontal, 24);
    let en_box = GtkBox::new(Orientation::Horizontal, 8);
    en_box.append(&Label::new(Some("Enabled")));
    let en_sw = Switch::new();
    en_sw.set_active(t.enabled);
    en_box.append(&en_sw);
    toggles.append(&en_box);

    let inv_box = GtkBox::new(Orientation::Horizontal, 8);
    inv_box.append(&Label::new(Some("Inverse")));
    let inv_sw = Switch::new();
    inv_sw.set_active(t.inverse);
    inv_box.append(&inv_sw);
    toggles.append(&inv_box);
    card.append(&toggles);

    // Pixels list
    let pixels_label = Label::new(Some(&format!("Pixels ({})", t.pixels.len())));
    pixels_label.set_halign(gtk4::Align::Start);
    card.append(&pixels_label);

    if !t.pixels.is_empty() {
        let pixels_box = GtkBox::new(Orientation::Vertical, 4);
        for p in &t.pixels {
            let color_str = if p.color.starts_with("0x") {
                format!("#{}", &p.color[2..])
            } else {
                p.color.clone()
            };
            let pixel_label = Label::new(Some(&format!("({}, {}) {} — var {}", p.x, p.y, color_str, p.variation)));
            pixel_label.set_halign(gtk4::Align::Start);
            pixel_label.set_margin_start(12);
            pixels_box.append(&pixel_label);
        }
        card.append(&pixels_box);
    }

    let pick_btn = Button::with_label("🎯 Pick from screen");
    pick_btn.add_css_class("btn-secondary");
    card.append(&pick_btn);

    card
}