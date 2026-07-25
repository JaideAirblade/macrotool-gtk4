//! Sidebar — game/class/spec tree view.

use crate::config::{ConfigTree, Game};
use gtk4::prelude::*;
use gtk4::{
    Box as GtkBox, Label, ListBox, Orientation, ScrolledWindow, SelectionMode,
};
use std::sync::Arc;

pub struct Sidebar {
    container: GtkBox,
    list: ListBox,
}

impl Sidebar {
    pub fn new(
        cfg: Arc<crate::config::Manager>,
        engine: Arc<crate::engine::EngineHub>,
    ) -> Self {
        let container = GtkBox::new(Orientation::Vertical, 0);
        container.set_width_request(240);

        let title = GtkBox::new(Orientation::Horizontal, 6);
        title.set_margin_start(12);
        title.set_margin_end(12);
        title.set_margin_top(8);
        title.set_margin_bottom(8);

        let label = Label::new(Some("Games"));
        label.add_css_class("sidebar-title");
        title.append(&label);

        let add_btn = gtk4::Button::with_label("+");
        add_btn.add_css_class("sidebar-add");
        add_btn.set_tooltip_text(Some("Add game"));
        title.append(&add_btn);

        container.append(&title);

        let scrolled = ScrolledWindow::new();
        scrolled.set_hexpand(true);
        scrolled.set_vexpand(true);
        scrolled.set_policy(gtk4::PolicyType::Never, gtk4::PolicyType::Automatic);

        let list = ListBox::new();
        list.set_selection_mode(SelectionMode::Single);
        list.add_css_class("sidebar-list");

        // Add game button handler
        {
            add_btn.connect_clicked(move |_| {
                // For now, add a default game. A proper dialog would be better.
                // TODO: show a dialog to enter game name + pick executable
            });
        }

        scrolled.set_child(Some(&list));
        container.append(&scrolled);

        let mut sidebar = Sidebar { container, list };
        sidebar.refresh(&cfg, &engine);
        sidebar
    }

    pub fn refresh(&mut self, cfg: &Arc<crate::config::Manager>, engine: &Arc<crate::engine::EngineHub>) {
        // Clear existing rows
        while let Some(child) = self.list.first_child() {
            self.list.remove(&child);
        }

        let tree = cfg.tree();
        let active_game = cfg.active_game();
        let active_class = cfg.active_class();
        let active_spec = cfg.active_spec();

        for (game_name, game) in &tree.games {
            let game_row = self.make_game_row(game_name, game, &active_game, &active_class, &active_spec);
            self.list.append(&game_row);
        }

        if tree.games.is_empty() {
            let empty = Label::new(Some("No games yet. Click + to add one."));
            empty.set_margin_start(12);
            empty.set_margin_end(12);
            empty.set_margin_top(8);
            empty.add_css_class("sidebar-empty");
            self.list.append(&empty);
        }
    }

    fn make_game_row(
        &self,
        game_name: &str,
        game: &Game,
        active_game: &str,
        active_class: &str,
        active_spec: &str,
    ) -> GtkBox {
        let col = GtkBox::new(Orientation::Vertical, 0);

        // Game row
        let game_box = GtkBox::new(Orientation::Horizontal, 6);
        game_box.set_margin_start(12);
        game_box.set_margin_end(12);
        game_box.set_margin_top(4);
        game_box.set_margin_bottom(4);

        let game_label = Label::new(Some(game_name));
        if active_game == game_name && active_class.is_empty() {
            game_label.add_css_class("sidebar-active");
        }
        game_box.append(&game_label);
        col.append(&game_box);

        // Classes
        for (class_name, class) in &game.classes {
            let class_box = GtkBox::new(Orientation::Horizontal, 6);
            class_box.set_margin_start(24);
            class_box.set_margin_end(12);
            class_box.set_margin_top(2);
            class_box.set_margin_bottom(2);

            if !class.icon.is_empty() {
                let icon_label = Label::new(Some("🖼"));
                class_box.append(&icon_label);
            }

            let class_label = Label::new(Some(class_name));
            if active_game == game_name && active_class == class_name && active_spec.is_empty() {
                class_label.add_css_class("sidebar-active");
            }
            class_box.append(&class_label);
            col.append(&class_box);

            // Specs
            for spec_name in class.specs.keys() {
                let spec_box = GtkBox::new(Orientation::Horizontal, 6);
                spec_box.set_margin_start(36);
                spec_box.set_margin_end(12);
                spec_box.set_margin_top(2);
                spec_box.set_margin_bottom(2);

                let spec_label = Label::new(Some(spec_name));
                if active_game == game_name && active_class == class_name && active_spec == spec_name {
                    spec_label.add_css_class("sidebar-active");
                }
                spec_box.append(&spec_label);
                col.append(&spec_box);
            }
        }

        col
    }

    pub fn widget(&self) -> &gtk4::Widget {
        self.container.upcast_ref()
    }

    /// Borrow the internal ListBox so callers can connect to selection
    /// signals (e.g. `row-selected`).
    pub fn list(&self) -> &ListBox {
        &self.list
    }
}