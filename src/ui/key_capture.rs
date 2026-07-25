//! Key capture widget — captures keyboard/mouse input and normalizes key names.

use std::cell::RefCell;
use std::rc::Rc;
use gtk4::prelude::*;
use gtk4::glib::object::ObjectExt;
use gtk4::{
    Box as GtkBox, Button, Entry, EventControllerKey, GestureClick, Orientation,
};

pub struct KeyCapture {
    container: GtkBox,
}

impl KeyCapture {
    pub fn new(initial_value: &str, on_change: Box<dyn Fn(&str)>) -> Self {
        let container = GtkBox::new(Orientation::Horizontal, 6);
        let entry = Entry::new();
        entry.set_text(initial_value);
        entry.set_hexpand(true);
        entry.set_editable(false);

        let button = Button::with_label("Set");
        button.set_tooltip_text(Some("Click to capture a key or mouse button"));

        container.append(&entry);
        container.append(&button);

        let capturing = Rc::new(RefCell::new(false));
        let on_change = Rc::new(on_change);

        // Keyboard controller
        let key_ctrl = EventControllerKey::new();
        let cap_k = capturing.clone();
        let on_change_k = on_change.clone();
        let entry_k = entry.clone();
        let btn_k = button.clone();
        key_ctrl.connect_key_pressed(move |_, keyval, _, _| {
            if !*cap_k.borrow() {
                return glib::Propagation::Proceed;
            }
            *cap_k.borrow_mut() = false;
            btn_k.set_label("Set");
            btn_k.remove_css_class("capturing");

            let name = normalize_key(keyval);
            if !name.is_empty() && name != "escape" {
                entry_k.set_text(&name);
                on_change_k(&name);
            }
            glib::Propagation::Stop
        });
        container.add_controller(key_ctrl);

        // Mouse controller
        let mouse_ctrl = GestureClick::new();
        let cap_m = capturing.clone();
        let on_change_m = on_change.clone();
        let entry_m = entry.clone();
        let btn_m = button.clone();
        let mouse_ctrl_clone = mouse_ctrl.clone();
        mouse_ctrl.connect_pressed(move |_, n_press: i32, _, _| {
            if n_press != 1 {
                return;
            }
            if !*cap_m.borrow() {
                return;
            }
            *cap_m.borrow_mut() = false;
            btn_m.set_label("Set");
            btn_m.remove_css_class("capturing");

            let button = mouse_ctrl_clone.current_button();
            let name = mouse_button_name(button as i32);
            if !name.is_empty() {
                entry_m.set_text(&name);
                on_change_m(&name);
            }
        });
        container.add_controller(mouse_ctrl);

        // Button toggles capture mode
        let cap_b = capturing.clone();
        let btn_b = button.clone();
        button.connect_clicked(move |_| {
            let mut cap = cap_b.borrow_mut();
            if *cap {
                *cap = false;
                btn_b.set_label("Set");
                btn_b.remove_css_class("capturing");
            } else {
                *cap = true;
                btn_b.set_label("Listening…");
                btn_b.add_css_class("capturing");
            }
        });

        KeyCapture { container }
    }

    pub fn widget(&self) -> &gtk4::Widget {
        self.container.upcast_ref()
    }
}

fn normalize_key(keyval: gtk4::gdk::Key) -> String {
    let name = keyval.name().unwrap_or_default().to_lowercase();
    match name.as_str() {
        "escape" => "escape".to_string(),
        " " | "space" => "space".to_string(),
        "tab" => "tab".to_string(),
        "return" | "enter" => "enter".to_string(),
        "backspace" => "backspace".to_string(),
        "delete" => "delete".to_string(),
        "insert" => "insert".to_string(),
        "home" => "home".to_string(),
        "end" => "end".to_string(),
        "page_up" | "prior" => "pgup".to_string(),
        "page_down" | "next" => "pgdn".to_string(),
        "up" => "up".to_string(),
        "down" => "down".to_string(),
        "left" => "left".to_string(),
        "right" => "right".to_string(),
        "caps_lock" => "capslock".to_string(),
        "scroll_lock" => "scrolllock".to_string(),
        "num_lock" => "numlock".to_string(),
        "print" | "print_screen" => "printscreen".to_string(),
        "pause" => "pause".to_string(),
        "shift_l" | "shift_r" | "shift" => "shift".to_string(),
        "control_l" | "control_r" | "control" => "ctrl".to_string(),
        "alt_l" | "alt_r" | "alt" => "alt".to_string(),
        "super_l" | "super_r" | "super" | "meta_l" | "meta_r" => "win".to_string(),
        n if n.starts_with("f") && n.len() <= 3 && n[1..].chars().all(|c| c.is_ascii_digit()) => n.to_string(),
        "kp_0" => "numpad0".to_string(),
        "kp_1" => "numpad1".to_string(),
        "kp_2" => "numpad2".to_string(),
        "kp_3" => "numpad3".to_string(),
        "kp_4" => "numpad4".to_string(),
        "kp_5" => "numpad5".to_string(),
        "kp_6" => "numpad6".to_string(),
        "kp_7" => "numpad7".to_string(),
        "kp_8" => "numpad8".to_string(),
        "kp_9" => "numpad9".to_string(),
        "kp_enter" => "numpadenter".to_string(),
        "kp_add" => "numpadadd".to_string(),
        "kp_subtract" => "numpadsub".to_string(),
        "kp_multiply" => "numpadmult".to_string(),
        "kp_divide" => "numpaddiv".to_string(),
        "kp_decimal" | "kp_delete" => "numpaddot".to_string(),
        n if n.len() == 1 && n.chars().all(|c| c.is_alphanumeric()) => n.to_string(),
        _ => name,
    }
}

fn mouse_button_name(button: i32) -> String {
    match button {
        1 => "lbutton".to_string(),
        2 => "mbutton".to_string(),
        3 => "rbutton".to_string(),
        8 => "xbutton1".to_string(),
        9 => "xbutton2".to_string(),
        _ => String::new(),
    }
}