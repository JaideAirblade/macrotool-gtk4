//! Buff timers tab — edit buff timer list for the active spec.

use crate::config::BuffTimer;
use gtk4::prelude::*;
use gtk4::{
    Box as GtkBox, Button, ComboBoxText, Entry, Label, Orientation, ScrolledWindow, SpinButton, Switch,
};
use std::sync::Arc;

pub struct BuffsTab {
    container: gtk4::Box,
    content_box: gtk4::Box,
}

impl BuffsTab {
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
        let title = Label::new(Some("Buff Timers"));
        title.add_css_class("tab-title");
        header.append(&title);
        let add_btn = Button::with_label("+ Add Buff");
        add_btn.add_css_class("btn-primary");
        header.append(&add_btn);
        content_box.append(&header);

        scrolled.set_child(Some(&content_box));
        container.append(&scrolled);

        let tab = BuffsTab { container, content_box };
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

        if spec.buff_timers.is_empty() {
            let empty = Label::new(Some("No buff timers"));
            empty.add_css_class("empty-state");
            self.content_box.append(&empty);
        } else {
            for b in &spec.buff_timers {
                self.content_box.append(&make_buff_card(b));
            }
        }
    }

    pub fn widget(&self) -> &gtk4::Widget {
        self.container.upcast_ref()
    }
}

fn make_buff_card(b: &BuffTimer) -> gtk4::Box {
    let card = gtk4::Box::new(Orientation::Vertical, 8);
    card.add_css_class("card");
    card.set_margin_bottom(8);

    // Header
    let header = GtkBox::new(Orientation::Horizontal, 8);
    let name_entry = Entry::new();
    name_entry.set_text(&b.name);
    name_entry.add_css_class("card-title");
    name_entry.set_hexpand(true);
    header.append(&name_entry);

    let badge = Label::new(Some(if b.enabled { "ON" } else { "OFF" }));
    badge.add_css_class(if b.enabled { "badge" } else { "badge-off" });
    header.append(&badge);

    let delete_btn = Button::with_label("✕");
    delete_btn.add_css_class("icon-btn");
    header.append(&delete_btn);
    card.append(&header);

    // Grid
    let grid = gtk4::Grid::new();
    grid.set_column_spacing(12);
    grid.set_row_spacing(8);

    let dur_lbl = Label::new(Some("Duration (ms)"));
    dur_lbl.set_halign(gtk4::Align::Start);
    let dur_spin = SpinButton::with_range(0.0, 999999.0, 100.0);
    dur_spin.set_value(b.duration as f64);
    grid.attach(&dur_lbl, 0, 0, 1, 1);
    grid.attach(&dur_spin, 1, 0, 1, 1);

    let key_lbl = Label::new(Some("Action Key"));
    key_lbl.set_halign(gtk4::Align::Start);
    let key_entry = Entry::new();
    key_entry.set_text(&b.action_key);
    grid.attach(&key_lbl, 2, 0, 1, 1);
    grid.attach(&key_entry, 3, 0, 1, 1);

    let refresh_lbl = Label::new(Some("On Refresh"));
    refresh_lbl.set_halign(gtk4::Align::Start);
    let refresh_combo = ComboBoxText::new();
    refresh_combo.append_text("Reset");
    refresh_combo.append_text("Extend");
    refresh_combo.append_text("Ignore");
    refresh_combo.set_active(Some(match b.on_refresh.as_str() {
        "extend" => 1,
        "ignore" => 2,
        _ => 0,
    }));
    grid.attach(&refresh_lbl, 0, 1, 1, 1);
    grid.attach(&refresh_combo, 1, 1, 1, 1);

    let ext_lbl = Label::new(Some("Extend (ms)"));
    ext_lbl.set_halign(gtk4::Align::Start);
    let ext_spin = SpinButton::with_range(0.0, 999999.0, 100.0);
    ext_spin.set_value(b.extend_ms as f64);
    grid.attach(&ext_lbl, 2, 1, 1, 1);
    grid.attach(&ext_spin, 3, 1, 1, 1);

    card.append(&grid);

    // Toggles
    let toggles = GtkBox::new(Orientation::Horizontal, 24);
    let en_box = GtkBox::new(Orientation::Horizontal, 8);
    en_box.append(&Label::new(Some("Enabled")));
    let en_sw = Switch::new();
    en_sw.set_active(b.enabled);
    en_box.append(&en_sw);
    toggles.append(&en_box);

    let trig_box = GtkBox::new(Orientation::Horizontal, 8);
    trig_box.append(&Label::new(Some("Trigger")));
    let trig_combo = ComboBoxText::new();
    trig_combo.append_text("Keys");
    trig_combo.append_text("Pixel");
    trig_combo.set_active(Some(if b.trigger_type == "pixel" { 1 } else { 0 }));
    trig_box.append(&trig_combo);
    toggles.append(&trig_box);
    card.append(&toggles);

    // Watch keys
    let wk_label = Label::new(Some(&format!("Watch Keys: {}", b.watch_keys.join(", "))));
    wk_label.set_halign(gtk4::Align::Start);
    card.append(&wk_label);

    card
}