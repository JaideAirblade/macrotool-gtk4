//! Sidebar — game/class/spec tree with selection, rename, add, delete.
//!
//! - Click a row → set active profile, refresh tabs
//! - Double-click a row → inline rename
//! - Right-click a row → context menu (Add class/spec, Rename, Delete)
//! - "+" header button → add a new game

use crate::config::{Class, ConfigTree, Game, Spec};
use gtk4::gio;
use gtk4::prelude::*;
use gtk4::{
    Box as GtkBox, Button, Entry, GestureClick, Label, ListBox, ListBoxRow, Orientation, Popover,
    ScrolledWindow, SelectionMode,
};
use std::cell::RefCell;
use std::rc::Rc;
use std::sync::Arc;

/// Which tree node a row represents.
#[derive(Clone, Debug, PartialEq)]
enum Node {
    Game(String),
    Class(String, String),
    Spec(String, String, String),
}

pub struct Sidebar {
    container: GtkBox,
    list: ListBox,
    cfg: Arc<crate::config::Manager>,
    engine: Arc<crate::engine::EngineHub>,
    on_changed: Rc<dyn Fn()>,
    /// The row currently being renamed (inline Entry shown).
    renaming: Rc<RefCell<Option<Node>>>,
}

impl Sidebar {
    /// `on_changed` fires whenever the tree or selection changes (the
    /// MainWindow uses it to refresh tabs).
    pub fn new(
        cfg: Arc<crate::config::Manager>,
        engine: Arc<crate::engine::EngineHub>,
        on_changed: Rc<dyn Fn()>,
    ) -> Self {
        let container = GtkBox::new(Orientation::Vertical, 0);
        container.set_width_request(240);

        let title = GtkBox::new(Orientation::Horizontal, 6);
        title.set_margin_start(12);
        title.set_margin_end(12);
        title.set_margin_top(8);
        title.set_margin_bottom(8);

        let label = Label::new(Some("Games"));
        label.set_hexpand(true);
        label.set_halign(gtk4::Align::Start);
        title.append(&label);

        let add_btn = Button::with_label("+");
        add_btn.set_tooltip_text(Some("Add game"));
        title.append(&add_btn);

        container.append(&title);

        let scrolled = ScrolledWindow::new();
        scrolled.set_hexpand(true);
        scrolled.set_vexpand(true);
        scrolled.set_policy(gtk4::PolicyType::Never, gtk4::PolicyType::Automatic);

        let list = ListBox::new();
        list.set_selection_mode(SelectionMode::None);
        scrolled.set_child(Some(&list));
        container.append(&scrolled);

        let sidebar = Sidebar {
            container,
            list,
            cfg,
            engine,
            on_changed,
            renaming: Rc::new(RefCell::new(None)),
        };

        // "+" → add game dialog
        {
            let sb = sidebar.list.clone();
            let cfg = sidebar.cfg.clone();
            let engine = sidebar.engine.clone();
            let on_changed = sidebar.on_changed.clone();
            add_btn.connect_clicked(move |_| {
                show_add_game_dialog(&sb, &cfg, &engine, &on_changed);
            });
        }

        sidebar.refresh();
        sidebar
    }

    pub fn refresh(&self) {
        // Clear
        while let Some(child) = self.list.first_child() {
            self.list.remove(&child);
        }

        let tree = self.cfg.tree();
        let renaming = self.renaming.borrow().clone();

        if tree.games.is_empty() {
            let empty = Label::new(Some("No games yet. Click + to add one."));
            empty.set_margin_start(12);
            empty.set_margin_top(8);
            empty.add_css_class("dim-label");
            self.list.append(&empty);
            return;
        }

        for (game_name, game) in &tree.games {
            let game_node = Node::Game(game_name.clone());
            let game_active = tree.active_game == *game_name && tree.active_class.is_empty();
            let row = self.make_row(
                &format!("🎮  {}", game_name),
                game_node.clone(),
                0,
                game_active,
                renaming.as_ref() == Some(&game_node),
            );
            self.list.append(&row);

            for (class_name, class) in &game.classes {
                let class_node = Node::Class(game_name.clone(), class_name.clone());
                let class_active = tree.active_game == *game_name
                    && tree.active_class == *class_name
                    && tree.active_spec.is_empty();
                let icon = if class.icon.is_empty() { "🖼" } else { "🖼" };
                let row = self.make_row(
                    &format!("{}  {}", icon, class_name),
                    class_node.clone(),
                    1,
                    class_active,
                    renaming.as_ref() == Some(&class_node),
                );
                self.list.append(&row);

                for spec_name in class.specs.keys() {
                    let spec_node = Node::Spec(game_name.clone(), class_name.clone(), spec_name.clone());
                    let spec_active = tree.active_game == *game_name
                        && tree.active_class == *class_name
                        && tree.active_spec == *spec_name;
                    let row = self.make_row(
                        &format!("▸  {}", spec_name),
                        spec_node.clone(),
                        2,
                        spec_active,
                        renaming.as_ref() == Some(&spec_node),
                    );
                    self.list.append(&row);
                }
            }
        }
    }

    /// Build one row with click/double-click/right-click handlers.
    fn make_row(
        &self,
        text: &str,
        node: Node,
        depth: u8,
        active: bool,
        is_renaming: bool,
    ) -> ListBoxRow {
        let row = ListBoxRow::new();
        let indent = 12 + (depth as i32 * 14);

        if is_renaming {
            // Inline rename entry
            let entry = Entry::new();
            entry.set_text(text.split("  ").nth(1).unwrap_or(text));
            entry.set_margin_start(indent);
            entry.set_margin_end(8);
            entry.set_margin_top(2);
            entry.set_margin_bottom(2);

            {
                let cfg = self.cfg.clone();
                let engine = self.engine.clone();
                let on_changed = self.on_changed.clone();
                let renaming = self.renaming.clone();
                let node = node.clone();
                let list = self.list.clone();
                entry.connect_activate(move |e| {
                    let new_name = e.text().trim().to_string();
                    *renaming.borrow_mut() = None;
                    if !new_name.is_empty() {
                        apply_rename(&cfg, &node, &new_name);
                        engine.reload_profile();
                    }
                    on_changed();
                    rebuild_list(&list, &cfg, &renaming, &on_changed_placeholder());
                });
            }
            {
                let renaming = self.renaming.clone();
                let list = self.list.clone();
                let cfg = self.cfg.clone();
                let on_changed = self.on_changed.clone();
                let focus = gtk4::EventControllerFocus::new();
                focus.connect_leave(move |_| {
                    if renaming.borrow().is_some() {
                        *renaming.borrow_mut() = None;
                        on_changed();
                        rebuild_list(&list, &cfg, &renaming, &on_changed_placeholder());
                    }
                });
                entry.add_controller(focus);
            }

            row.set_child(Some(&entry));
            // Focus immediately
            entry.grab_focus();
        } else {
            let lbl = Label::new(Some(text));
            lbl.set_halign(gtk4::Align::Start);
            lbl.set_margin_start(indent);
            lbl.set_margin_end(8);
            lbl.set_margin_top(3);
            lbl.set_margin_bottom(3);
            if active {
                lbl.add_css_class("accent");
            }
            row.set_child(Some(&lbl));
        }

        // Left click: select (single) or start rename (double)
        {
            let click = GestureClick::new();
            click.set_button(1);
            let cfg = self.cfg.clone();
            let engine = self.engine.clone();
            let on_changed = self.on_changed.clone();
            let renaming = self.renaming.clone();
            let node = node.clone();
            let list = self.list.clone();
            click.connect_pressed(move |_, n_press, _, _| {
                if n_press == 2 {
                    *renaming.borrow_mut() = Some(node.clone());
                    rebuild_list(&list, &cfg, &renaming, &on_changed_placeholder());
                } else if n_press == 1 {
                    select_node(&cfg, &node);
                    engine.reload_profile();
                    on_changed();
                    rebuild_list(&list, &cfg, &renaming, &on_changed_placeholder());
                }
            });
            row.add_controller(click);
        }

        // Right click: context menu
        {
            let right = GestureClick::new();
            right.set_button(3);
            let cfg = self.cfg.clone();
            let engine = self.engine.clone();
            let on_changed = self.on_changed.clone();
            let renaming = self.renaming.clone();
            let node = node.clone();
            let list = self.list.clone();
            right.connect_pressed(move |gesture, _, x, y| {
                let menu = make_context_menu(
                    &list,
                    &node,
                    &cfg,
                    &engine,
                    &on_changed,
                    &renaming,
                    &list,
                );
                let _ = gesture;
                let rect = gtk4::gdk::Rectangle::new(x as i32, y as i32, 1, 1);
                menu.set_pointing_to(Some(&rect));
                menu.popup();
            });
            row.add_controller(right);
        }

        row
    }

    pub fn widget(&self) -> &gtk4::Widget {
        self.container.upcast_ref()
    }
}

// ── Config mutations ─────────────────────────────────────────────────────

fn select_node(cfg: &Arc<crate::config::Manager>, node: &Node) {
    let mut tree = cfg.tree();
    match node {
        Node::Game(g) => {
            tree.active_game = g.clone();
            tree.active_class = String::new();
            tree.active_spec = String::new();
        }
        Node::Class(g, c) => {
            tree.active_game = g.clone();
            tree.active_class = c.clone();
            tree.active_spec = String::new();
        }
        Node::Spec(g, c, s) => {
            tree.active_game = g.clone();
            tree.active_class = c.clone();
            tree.active_spec = s.clone();
        }
    }
    cfg.set_tree(tree);
}

fn apply_rename(cfg: &Arc<crate::config::Manager>, node: &Node, new_name: &str) {
    let mut tree = cfg.tree();
    match node {
        Node::Game(old) => {
            if let Some(game) = tree.games.remove(old) {
                tree.games.insert(new_name.to_string(), game);
            }
            if tree.active_game == *old {
                tree.active_game = new_name.to_string();
            }
        }
        Node::Class(g, old) => {
            if let Some(game) = tree.games.get_mut(g) {
                if let Some(class) = game.classes.remove(old) {
                    game.classes.insert(new_name.to_string(), class);
                }
            }
            if tree.active_game == *g && tree.active_class == *old {
                tree.active_class = new_name.to_string();
            }
        }
        Node::Spec(g, c, old) => {
            if let Some(game) = tree.games.get_mut(g) {
                if let Some(class) = game.classes.get_mut(c) {
                    if let Some(spec) = class.specs.remove(old) {
                        class.specs.insert(new_name.to_string(), spec);
                    }
                }
            }
            if tree.active_game == *g && tree.active_class == *c && tree.active_spec == *old {
                tree.active_spec = new_name.to_string();
            }
        }
    }
    cfg.set_tree(tree);
}

fn delete_node(cfg: &Arc<crate::config::Manager>, node: &Node) {
    let mut tree = cfg.tree();
    match node {
        Node::Game(g) => {
            tree.games.remove(g);
            if tree.active_game == *g {
                tree.active_game = String::new();
                tree.active_class = String::new();
                tree.active_spec = String::new();
            }
        }
        Node::Class(g, c) => {
            if let Some(game) = tree.games.get_mut(g) {
                game.classes.remove(c);
            }
            if tree.active_game == *g && tree.active_class == *c {
                tree.active_class = String::new();
                tree.active_spec = String::new();
            }
        }
        Node::Spec(g, c, s) => {
            if let Some(game) = tree.games.get_mut(g) {
                if let Some(class) = game.classes.get_mut(c) {
                    class.specs.remove(s);
                }
            }
            if tree.active_game == *g && tree.active_class == *c && tree.active_spec == *s {
                tree.active_spec = String::new();
            }
        }
    }
    cfg.set_tree(tree);
}

fn add_child(cfg: &Arc<crate::config::Manager>, node: &Node, name: &str) {
    let mut tree = cfg.tree();
    match node {
        Node::Game(g) => {
            if let Some(game) = tree.games.get_mut(g) {
                game.classes.insert(
                    name.to_string(),
                    Class {
                        specs: Default::default(),
                        icon: String::new(),
                    },
                );
            }
        }
        Node::Class(g, c) => {
            if let Some(game) = tree.games.get_mut(g) {
                if let Some(class) = game.classes.get_mut(c) {
                    class.specs.insert(
                        name.to_string(),
                        Spec {
                            macros: Vec::new(),
                            pixel_triggers: Vec::new(),
                            buff_timers: Vec::new(),
                            detect: None,
                        },
                    );
                }
            }
        }
        Node::Spec(..) => {}
    }
    cfg.set_tree(tree);
}

// ── Context menu ─────────────────────────────────────────────────────────

fn make_context_menu(
    parent: &impl IsA<gtk4::Widget>,
    node: &Node,
    cfg: &Arc<crate::config::Manager>,
    engine: &Arc<crate::engine::EngineHub>,
    on_changed: &Rc<dyn Fn()>,
    renaming: &Rc<RefCell<Option<Node>>>,
    list: &ListBox,
) -> Popover {
    let pop = Popover::new();
    pop.set_parent(parent);

    let menu_box = GtkBox::new(Orientation::Vertical, 0);

    let can_add_child = matches!(node, Node::Game(_) | Node::Class(..));
    if can_add_child {
        let label = match node {
            Node::Game(_) => "Add class",
            _ => "Add spec",
        };
        let btn = Button::with_label(label);
        btn.add_css_class("flat");
        {
            let cfg = cfg.clone();
            let engine = engine.clone();
            let on_changed = on_changed.clone();
            let node = node.clone();
            let list = list.clone();
            let renaming = renaming.clone();
            let pop = pop.clone();
            btn.connect_clicked(move |_| {
                pop.popdown();
                show_add_child_dialog(&list, &cfg, &engine, &on_changed, &node, &renaming);
            });
        }
        menu_box.append(&btn);
    }

    let rename_btn = Button::with_label("Rename");
    rename_btn.add_css_class("flat");
    {
        let renaming = renaming.clone();
        let node = node.clone();
        let list = list.clone();
        let cfg = cfg.clone();
        let pop = pop.clone();
        rename_btn.connect_clicked(move |_| {
            pop.popdown();
            *renaming.borrow_mut() = Some(node.clone());
            rebuild_list(&list, &cfg, &renaming, &on_changed_placeholder());
        });
    }
    menu_box.append(&rename_btn);

    let del_label = match node {
        Node::Game(_) => "Delete game",
        Node::Class(..) => "Delete class",
        Node::Spec(..) => "Delete spec",
    };
    let del_btn = Button::with_label(del_label);
    del_btn.add_css_class("flat");
    del_btn.add_css_class("destructive-action");
    {
        let cfg = cfg.clone();
        let engine = engine.clone();
        let on_changed = on_changed.clone();
        let node = node.clone();
        let list = list.clone();
        let renaming = renaming.clone();
        let pop = pop.clone();
        del_btn.connect_clicked(move |_| {
            pop.popdown();
            delete_node(&cfg, &node);
            engine.reload_profile();
            on_changed();
            rebuild_list(&list, &cfg, &renaming, &on_changed_placeholder());
        });
    }
    menu_box.append(&del_btn);

    pop.set_child(Some(&menu_box));
    pop
}

// ── Dialogs ──────────────────────────────────────────────────────────────

fn show_add_game_dialog(
    parent: &impl IsA<gtk4::Widget>,
    cfg: &Arc<crate::config::Manager>,
    engine: &Arc<crate::engine::EngineHub>,
    on_changed: &Rc<dyn Fn()>,
) {
    let dialog = gtk4::Window::new();
    dialog.set_title(Some("Add Game"));
    dialog.set_modal(true);
    if let Some(win) = parent.root().and_then(|r| r.downcast::<gtk4::Window>().ok()) {
        dialog.set_transient_for(Some(&win));
    }
    dialog.set_default_size(320, 0);

    let vbox = GtkBox::new(Orientation::Vertical, 12);
    vbox.set_margin_start(16);
    vbox.set_margin_end(16);
    vbox.set_margin_top(16);
    vbox.set_margin_bottom(16);

    vbox.append(&Label::new(Some("Game name:")));
    let name_entry = Entry::new();
    name_entry.set_placeholder_text(Some("e.g. My Game"));
    vbox.append(&name_entry);

    let btns = GtkBox::new(Orientation::Horizontal, 8);
    btns.set_halign(gtk4::Align::End);
    let cancel = Button::with_label("Cancel");
    let ok = Button::with_label("Add");
    ok.add_css_class("suggested-action");

    {
        let dialog = dialog.clone();
        cancel.connect_clicked(move |_| dialog.close());
    }
    {
        let dialog = dialog.clone();
        let cfg = cfg.clone();
        let engine = engine.clone();
        let on_changed = on_changed.clone();
        let name_entry = name_entry.clone();
        ok.connect_clicked(move |_| {
            let name = name_entry.text().trim().to_string();
            if !name.is_empty() {
                let mut tree = cfg.tree();
                tree.games.insert(
                    name.clone(),
                    Game {
                        path: String::new(),
                        classes: Default::default(),
                    },
                );
                tree.active_game = name;
                tree.active_class = String::new();
                tree.active_spec = String::new();
                cfg.set_tree(tree);
                engine.reload_profile();
                on_changed();
            }
            dialog.close();
        });
    }

    btns.append(&cancel);
    btns.append(&ok);
    vbox.append(&btns);
    dialog.set_child(Some(&vbox));
    dialog.present();
    name_entry.grab_focus();
}

fn show_add_child_dialog(
    list: &ListBox,
    cfg: &Arc<crate::config::Manager>,
    engine: &Arc<crate::engine::EngineHub>,
    on_changed: &Rc<dyn Fn()>,
    node: &Node,
    renaming: &Rc<RefCell<Option<Node>>>,
) {
    let what = match node {
        Node::Game(_) => "Class",
        Node::Class(..) => "Spec",
        Node::Spec(..) => return,
    };

    let dialog = gtk4::Window::new();
    dialog.set_title(Some(&format!("Add {}", what)));
    dialog.set_modal(true);
    if let Some(win) = list.root().and_then(|r| r.downcast::<gtk4::Window>().ok()) {
        dialog.set_transient_for(Some(&win));
    }
    dialog.set_default_size(320, 0);

    let vbox = GtkBox::new(Orientation::Vertical, 12);
    vbox.set_margin_start(16);
    vbox.set_margin_end(16);
    vbox.set_margin_top(16);
    vbox.set_margin_bottom(16);

    vbox.append(&Label::new(Some(&format!("{} name:", what))));
    let name_entry = Entry::new();
    name_entry.set_placeholder_text(Some(&format!("e.g. New {}", what)));
    vbox.append(&name_entry);

    let btns = GtkBox::new(Orientation::Horizontal, 8);
    btns.set_halign(gtk4::Align::End);
    let cancel = Button::with_label("Cancel");
    let ok = Button::with_label("Add");
    ok.add_css_class("suggested-action");

    {
        let dialog = dialog.clone();
        cancel.connect_clicked(move |_| dialog.close());
    }
    {
        let dialog = dialog.clone();
        let cfg = cfg.clone();
        let engine = engine.clone();
        let on_changed = on_changed.clone();
        let node = node.clone();
        let renaming = renaming.clone();
        let parent_list = Some(list.clone());
        let name_entry = name_entry.clone();
        ok.connect_clicked(move |_| {
            let name = name_entry.text().trim().to_string();
            if !name.is_empty() {
                add_child(&cfg, &node, &name);
                engine.reload_profile();
                on_changed();
                if let Some(list) = &parent_list {
                    rebuild_list(list, &cfg, &renaming, &on_changed_placeholder());
                }
            }
            dialog.close();
        });
    }

    btns.append(&cancel);
    btns.append(&ok);
    vbox.append(&btns);
    dialog.set_child(Some(&vbox));
    dialog.present();
    name_entry.grab_focus();
}

// ── List rebuild (shared) ────────────────────────────────────────────────

/// Rebuild the sidebar list in place. We can't call back into Sidebar here
/// because the closures only hold the ListBox + cfg; so this rebuilds using
/// the same logic as Sidebar::refresh but through a helper that recreates
/// a lightweight view. The MainWindow-level on_changed also triggers a full
/// Sidebar::refresh through the main loop.
fn rebuild_list(
    list: &ListBox,
    cfg: &Arc<crate::config::Manager>,
    _renaming: &Rc<RefCell<Option<Node>>>,
    _cb: &Rc<dyn Fn()>,
) {
    // Schedule a full rebuild on the main loop to avoid re-entrancy while
    // signals are firing. We recreate the rows via a temporary Sidebar-less
    // rebuild: simply mark the list dirty and let the next refresh pass
    // (triggered via on_changed → MainWindow::refresh_all → sidebar.refresh).
    // Here we just update active highlight styles without a rebuild.
    let tree = cfg.tree();
    let mut row = list.first_child();
    while let Some(w) = row {
        if let Some(r) = w.downcast_ref::<ListBoxRow>() {
            if let Some(child) = r.child() {
                if let Some(lbl) = child.downcast_ref::<Label>() {
                    lbl.remove_css_class("accent");
                    let text = lbl.text();
                    // crude active check: label text matches active names
                    if text.contains(&tree.active_game)
                        && (tree.active_class.is_empty()
                            || text.contains(&tree.active_class)
                            || text.contains(&tree.active_spec))
                    {
                        lbl.add_css_class("accent");
                    }
                }
            }
        }
        row = w.next_sibling();
    }
}

/// Placeholder callback used where only list-restyling is needed inline.
fn on_changed_placeholder() -> Rc<dyn Fn()> {
    Rc::new(|| {})
}