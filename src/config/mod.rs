//! Config data model — direct port of the Go `internal/config/manager.go`.
//!
//! All structs use serde for IPC serialization to the frontend.

pub mod kdl;

use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;
use std::path::PathBuf;
use std::sync::Arc;
use std::time::Instant;

use crate::config::kdl as kdl_mod;
use crate::config::kdl::{Document, Node, Value};
use parking_lot::RwLock;

// ── Data structs ────────────────────────────────────────────────────────

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct Resolution {
    pub w: i32,
    pub h: i32,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Macro {
    pub name: String,
    pub hotkey: String,
    pub delay: i32,
    pub mode: String,
    #[serde(rename = "holdMode", default)]
    pub hold_mode: bool,
    pub keys: Vec<String>,
    #[serde(rename = "interKeyDelay", default)]
    pub inter_key_delay: i32,
    #[serde(default = "default_true")]
    pub enabled: bool,
    #[serde(rename = "maxHoldDuration", default)]
    pub max_hold_duration: i32,
    #[serde(default)]
    pub background: bool,
}

impl Default for Macro {
    fn default() -> Self {
        Macro {
            name: "Unnamed".into(),
            hotkey: String::new(),
            delay: 50,
            mode: "press".into(),
            hold_mode: false,
            keys: Vec::new(),
            inter_key_delay: 0,
            enabled: true,
            max_hold_duration: 0,
            background: false,
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct Pixel {
    pub x: i32,
    pub y: i32,
    pub color: String,
    #[serde(default = "default_10")]
    pub variation: i32,
}

fn default_10() -> i32 {
    10
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct Anchor {
    pub pixels: Vec<Pixel>,
    #[serde(rename = "matchMode", default = "default_all")]
    pub match_mode: String,
}

fn default_all() -> String {
    "all".to_string()
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct PixelTrigger {
    pub name: String,
    #[serde(rename = "actionKey", default)]
    pub action_key: String,
    pub pixels: Vec<Pixel>,
    #[serde(rename = "matchMode", default = "default_all")]
    pub match_mode: String,
    #[serde(default)]
    pub inverse: bool,
    #[serde(default = "default_true")]
    pub enabled: bool,
    #[serde(default = "default_1000")]
    pub cooldown: i32,
    #[serde(skip)]
    pub last_fired: i64,
    #[serde(rename = "triggerMode", default = "default_macro_mode")]
    pub trigger_mode: String,
    #[serde(rename = "macroHotkey", default)]
    pub macro_hotkey: String,
    #[serde(rename = "captureRes", default)]
    pub capture_res: Option<Resolution>,
    #[serde(default)]
    pub anchor: Option<Anchor>,
    #[serde(default)]
    pub blocker: Option<Anchor>,
}

fn default_1000() -> i32 {
    1000
}
fn default_macro_mode() -> String {
    "macro".to_string()
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct BuffTimer {
    pub name: String,
    #[serde(rename = "watchKeys", default)]
    pub watch_keys: Vec<String>,
    #[serde(default = "default_5000")]
    pub duration: i32,
    #[serde(rename = "actionKey", default)]
    pub action_key: String,
    #[serde(rename = "onRefresh", default = "default_reset")]
    pub on_refresh: String,
    #[serde(rename = "extendMs", default)]
    pub extend_ms: i32,
    #[serde(default = "default_true")]
    pub enabled: bool,
    #[serde(rename = "soundOnExpiry", default)]
    pub sound_on_expiry: bool,
    #[serde(rename = "triggerType", default = "default_keys")]
    pub trigger_type: String,
    #[serde(rename = "triggerPixels", default)]
    pub trigger_pixels: Vec<Pixel>,
    #[serde(rename = "triggerMatchMode", default = "default_all")]
    pub trigger_match_mode: String,
    #[serde(rename = "captureRes", default)]
    pub capture_res: Option<Resolution>,
}

impl Default for BuffTimer {
    fn default() -> Self {
        BuffTimer {
            name: "Unnamed".into(),
            watch_keys: Vec::new(),
            duration: 5000,
            action_key: String::new(),
            on_refresh: "reset".into(),
            extend_ms: 0,
            enabled: true,
            sound_on_expiry: false,
            trigger_type: "keys".into(),
            trigger_pixels: Vec::new(),
            trigger_match_mode: "all".into(),
            capture_res: None,
        }
    }
}

fn default_5000() -> i32 {
    5000
}
fn default_reset() -> String {
    "reset".to_string()
}
fn default_keys() -> String {
    "keys".to_string()
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct Detect {
    pub pixels: Vec<Pixel>,
    #[serde(rename = "matchMode", default = "default_all")]
    pub match_mode: String,
    #[serde(rename = "captureRes", default)]
    pub capture_res: Option<Resolution>,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct Spec {
    #[serde(default)]
    pub macros: Vec<Macro>,
    #[serde(rename = "pixelTriggers", default)]
    pub pixel_triggers: Vec<PixelTrigger>,
    #[serde(rename = "buffTimers", default)]
    pub buff_timers: Vec<BuffTimer>,
    #[serde(default)]
    pub detect: Option<Detect>,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct Class {
    #[serde(default)]
    pub specs: BTreeMap<String, Spec>,
    #[serde(default)]
    pub icon: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct Game {
    pub path: String,
    #[serde(default)]
    pub classes: BTreeMap<String, Class>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Settings {
    #[serde(rename = "defaultDelay", default = "default_delay")]
    pub default_delay: i32,
    #[serde(rename = "autoDetectGame", default)]
    pub auto_detect_game: bool,
    #[serde(rename = "onlyInGame", default = "default_true")]
    pub only_in_game: bool,
    #[serde(rename = "allowBackground", default)]
    pub allow_background: bool,
    #[serde(rename = "minimizeToTray", default)]
    pub minimize_to_tray: bool,
    #[serde(default = "default_true")]
    pub dark_mode: bool,
    #[serde(rename = "toggleKey", default = "default_toggle_key")]
    pub toggle_key: String,
    #[serde(rename = "pixelCheckRate", default = "default_250")]
    pub pixel_check_rate: i32,
    #[serde(rename = "showTerminal", default)]
    pub show_terminal: bool,
    #[serde(rename = "overlayPosition", default = "default_overlay_position")]
    pub overlay_position: String,
}

impl Default for Settings {
    fn default() -> Self {
        Settings {
            default_delay: 50,
            auto_detect_game: false,
            only_in_game: true,
            allow_background: false,
            minimize_to_tray: false,
            dark_mode: true,
            toggle_key: "ScrollLock".into(),
            pixel_check_rate: 250,
            show_terminal: false,
            overlay_position: default_overlay_position(),
        }
    }
}

fn default_delay() -> i32 {
    50
}
fn default_true() -> bool {
    true
}
fn default_250() -> i32 {
    250
}
fn default_toggle_key() -> String {
    "ScrollLock".to_string()
}
fn default_overlay_position() -> String {
    "top-left".to_string()
}

impl Settings {}

// ── Manager ─────────────────────────────────────────────────────────────

/// The complete config tree, loaded from config.kdl.
#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct ConfigTree {
    pub settings: Settings,
    #[serde(default)]
    pub games: BTreeMap<String, Game>,
    #[serde(rename = "activeGame", default)]
    pub active_game: String,
    #[serde(rename = "activeClass", default)]
    pub active_class: String,
    #[serde(rename = "activeSpec", default)]
    pub active_spec: String,
}

/// Thread-safe config manager. Wraps ConfigTree in an RwLock.
/// Saves are debounced (300ms) matching the Go version.
pub struct Manager {
    inner: Arc<RwLock<ManagerInner>>,
}

struct ManagerInner {
    tree: ConfigTree,
    config_dir: PathBuf,
    config_file: PathBuf,
    backup_file: PathBuf,
    save_deadline: Option<Instant>,
    persistence_blocked: bool,
}

// SAFETY: ManagerInner is behind RwLock, so Manager is Send + Sync.
unsafe impl Send for ManagerInner {}
unsafe impl Sync for ManagerInner {}

fn create_default_config(path: &std::path::Path, default_text: &str) -> Result<bool, String> {
    let mut file = match std::fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(path)
    {
        Ok(file) => file,
        Err(error) if error.kind() == std::io::ErrorKind::AlreadyExists => return Ok(false),
        Err(error) => return Err(format!("create default config failed: {error}")),
    };

    if let Err(error) = std::io::Write::write_all(&mut file, default_text.as_bytes())
        .and_then(|_| file.sync_all())
    {
        return Err(format!("write default config failed: {error}"));
    }
    Ok(true)
}

fn read_config_or_initialize(
    path: &std::path::Path,
    default_text: &str,
) -> Result<Option<String>, String> {
    match std::fs::read_to_string(path) {
        Ok(data) => Ok(Some(data)),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
            if create_default_config(path, default_text)? {
                Ok(None)
            } else {
                std::fs::read_to_string(path)
                    .map(Some)
                    .map_err(|error| format!("read concurrently created config failed: {error}"))
            }
        }
        Err(error) => Err(format!("read config failed: {error}")),
    }
}

impl Manager {
    pub fn new() -> Self {
        let config_dir = config_dir();
        let config_file = config_dir.join("config.kdl");
        let backup_file = config_dir.join("config.kdl.bak");

        let inner = ManagerInner {
            tree: ConfigTree::default(),
            config_dir,
            config_file,
            backup_file,
            save_deadline: None,
            persistence_blocked: false,
        };

        Manager {
            inner: Arc::new(RwLock::new(inner)),
        }
    }

    /// Get the config directory under ~/.config/Jaides_Macro_tool.
    pub fn config_dir(&self) -> PathBuf {
        self.inner.read().config_dir.clone()
    }

    /// Get the config file path.
    pub fn config_path(&self) -> PathBuf {
        self.inner.read().config_file.clone()
    }

    /// Load config from disk. Falls back to defaults if file is missing.
    pub fn load(&self) -> Result<(), String> {
        let result = (|| {
            let mut inner = self.inner.write();
            inner.tree = ConfigTree::default();

            std::fs::create_dir_all(&inner.config_dir)
                .map_err(|e| format!("mkdir failed: {}", e))?;

            let default_text = kdl_mod::dump(&build_doc(&inner.tree));
            let data = match read_config_or_initialize(&inner.config_file, &default_text)? {
                Some(data) => data,
                None => return Ok(()),
            };

            let doc = match kdl_mod::parse(&data) {
                Ok(d) => d,
                Err(_) => {
                    // Try backup
                    match std::fs::read_to_string(&inner.backup_file) {
                        Ok(backup) => kdl_mod::parse(&backup).map_err(|e| e)?,
                        Err(_) => return Err("config parse failed and no backup".into()),
                    }
                }
            };

            parse_doc(&doc, &mut inner.tree);
            validate(&mut inner.tree);
            Ok(())
        })();

        self.inner.write().persistence_blocked = result.is_err();
        result
    }

    /// Debounced save — schedules a save 300ms in the future.
    /// The actual flush is driven by a background tick in the app.
    pub fn save_debounced(&self) {
        self.inner.write().save_deadline =
            Some(Instant::now() + std::time::Duration::from_millis(300));
    }

    /// Immediate flush to disk (temp file + atomic rename).
    pub fn flush(&self) -> Result<(), String> {
        self.inner.write().save_deadline = None;
        self.save_now()
    }

    /// Check if the debounced save deadline has passed; if so, flush.
    /// Called by the background save-ticker.
    pub fn check_debounced_save(&self) -> bool {
        let mut inner = self.inner.write();
        if let Some(deadline) = inner.save_deadline {
            if Instant::now() >= deadline {
                inner.save_deadline = None;
                drop(inner);
                let _ = self.save_now();
                return true;
            }
        }
        false
    }

    fn save_now(&self) -> Result<(), String> {
        let inner = self.inner.read();
        if inner.persistence_blocked {
            return Err("config persistence blocked after load failure".into());
        }
        let doc = build_doc(&inner.tree);
        let text = kdl_mod::dump(&doc);

        std::fs::create_dir_all(&inner.config_dir).map_err(|e| format!("mkdir failed: {}", e))?;

        // Backup current file before overwriting
        if inner.config_file.exists() {
            let _ = std::fs::copy(&inner.config_file, &inner.backup_file);
        }

        let tmp = inner.config_file.with_extension("kdl.tmp");
        std::fs::write(&tmp, &text).map_err(|e| format!("write tmp failed: {}", e))?;
        std::fs::rename(&tmp, &inner.config_file).map_err(|e| format!("rename failed: {}", e))?;
        Ok(())
    }

    // ── Read accessors (return clones for thread safety) ─────────────────

    pub fn tree(&self) -> ConfigTree {
        self.inner.read().tree.clone()
    }

    pub fn settings(&self) -> Settings {
        self.inner.read().tree.settings.clone()
    }

    pub fn set_settings(&self, s: Settings) {
        self.inner.write().tree.settings = s;
        self.save_debounced();
    }

    pub fn active_game(&self) -> String {
        self.inner.read().tree.active_game.clone()
    }
    pub fn active_class(&self) -> String {
        self.inner.read().tree.active_class.clone()
    }
    pub fn active_spec(&self) -> String {
        self.inner.read().tree.active_spec.clone()
    }
    pub fn active_class_icon(&self) -> String {
        let inner = self.inner.read();
        inner
            .tree
            .games
            .get(&inner.tree.active_game)
            .and_then(|g| g.classes.get(&inner.tree.active_class))
            .map(|c| c.icon.clone())
            .unwrap_or_default()
    }

    pub fn set_active_profile(&self, game: &str, class: &str, spec: &str) {
        let mut inner = self.inner.write();
        inner.tree.active_game = game.into();
        inner.tree.active_class = class.into();
        inner.tree.active_spec = spec.into();
        drop(inner);
        self.save_debounced();
    }

    /// Get the active spec's macros (deep clone).
    pub fn get_macros(&self) -> Vec<Macro> {
        let inner = self.inner.read();
        get_spec(&inner.tree)
            .map(|s| s.macros.clone())
            .unwrap_or_default()
    }

    pub fn get_pixel_triggers(&self) -> Vec<PixelTrigger> {
        let inner = self.inner.read();
        get_spec(&inner.tree)
            .map(|s| s.pixel_triggers.clone())
            .unwrap_or_default()
    }

    pub fn get_buff_timers(&self) -> Vec<BuffTimer> {
        let inner = self.inner.read();
        get_spec(&inner.tree)
            .map(|s| s.buff_timers.clone())
            .unwrap_or_default()
    }

    /// Get the full game tree (for frontend display).
    pub fn get_games(&self) -> BTreeMap<String, Game> {
        self.inner.read().tree.games.clone()
    }

    /// Replace the entire game tree (from frontend edit).
    pub fn set_games(&self, games: BTreeMap<String, Game>) {
        self.inner.write().tree.games = games;
        self.save_debounced();
    }

    /// Replace the full config tree (frontend sends entire state).
    pub fn set_tree(&self, tree: ConfigTree) {
        self.inner.write().tree = tree;
        self.save_debounced();
    }

    pub fn game_path(&self, game_name: &str) -> Option<String> {
        self.inner
            .read()
            .tree
            .games
            .get(game_name)
            .map(|g| g.path.clone())
    }
}

fn get_spec(tree: &ConfigTree) -> Option<&Spec> {
    tree.games
        .get(&tree.active_game)?
        .classes
        .get(&tree.active_class)?
        .specs
        .get(&tree.active_spec)
}

fn config_dir() -> PathBuf {
    if let Ok(cfg) = std::env::var("XDG_CONFIG_HOME") {
        return PathBuf::from(cfg).join("Jaides_Macro_tool");
    }
    if let Ok(home) = std::env::var("HOME") {
        return PathBuf::from(home)
            .join(".config")
            .join("Jaides_Macro_tool");
    }
    PathBuf::from(".").join("Jaides_Macro_tool")
}

// ── KDL parsing ─────────────────────────────────────────────────────────

fn parse_doc(doc: &Document, tree: &mut ConfigTree) {
    for node in &doc.nodes {
        match node.name.as_str() {
            "settings" => parse_settings(node, tree),
            "active" => {
                tree.active_game = node.prop_str("game", "");
                tree.active_class = node.prop_str("class", "");
                tree.active_spec = node.prop_str("spec", "");
            }
            "game" => parse_game(node, tree),
            _ => {}
        }
    }
}

fn parse_settings(node: &Node, tree: &mut ConfigTree) {
    let s = &mut tree.settings;
    s.default_delay = node.prop_int("defaultDelay", s.default_delay.into()) as i32;
    s.auto_detect_game = node.prop_bool("autoDetectGame", s.auto_detect_game);
    s.only_in_game = node.prop_bool("onlyInGame", s.only_in_game);
    s.allow_background = node.prop_bool("allowBackground", s.allow_background);
    s.minimize_to_tray = node.prop_bool("minimizeToTray", s.minimize_to_tray);
    s.dark_mode = node.prop_bool("darkMode", s.dark_mode);
    s.toggle_key = node.prop_str("toggleKey", &s.toggle_key.clone());
    s.pixel_check_rate = node.prop_int("pixelCheckRate", s.pixel_check_rate as i64) as i32;
    s.show_terminal = node.prop_bool("showTerminal", s.show_terminal);
    s.overlay_position =
        normalize_overlay_position(&node.prop_str("overlayPosition", &s.overlay_position.clone()));
}

fn parse_game(node: &Node, tree: &mut ConfigTree) {
    let name = node.args.first().and_then(|v| v.as_str()).unwrap_or("");
    if name.is_empty() {
        return;
    }
    let mut game = Game {
        path: node.prop_str("path", ""),
        classes: BTreeMap::new(),
    };

    for class_node in node.children_named("class") {
        let class_name = class_node
            .args
            .first()
            .and_then(|v| v.as_str())
            .unwrap_or("");
        if class_name.is_empty() {
            continue;
        }
        let mut class = Class {
            specs: BTreeMap::new(),
            icon: class_node.prop_str("icon", ""),
        };
        for spec_node in class_node.children_named("spec") {
            let spec_name = spec_node
                .args
                .first()
                .and_then(|v| v.as_str())
                .unwrap_or("");
            if spec_name.is_empty() {
                continue;
            }
            let mut spec = Spec::default();
            for item in &spec_node.children {
                match item.name.as_str() {
                    "macro" => spec
                        .macros
                        .push(parse_macro(item, tree.settings.default_delay)),
                    "proc" => spec.pixel_triggers.push(parse_proc(item)),
                    "buff" => spec.buff_timers.push(parse_buff(item)),
                    "detect" => spec.detect = Some(parse_detect(item)),
                    _ => {}
                }
            }
            class.specs.insert(spec_name.into(), spec);
        }
        game.classes.insert(class_name.into(), class);
    }
    tree.games.insert(name.into(), game);
}

fn parse_macro(node: &Node, default_delay: i32) -> Macro {
    let keys: Vec<String> = node
        .children_named("key")
        .filter_map(|c| {
            c.args
                .first()
                .and_then(|v| v.as_str())
                .map(|s| s.to_lowercase())
        })
        .collect();

    let hold_mode = node.prop_bool("holdMode", false);
    let mode = {
        let m = node.prop_str("mode", "");
        if m.is_empty() {
            if hold_mode {
                "hold".to_string()
            } else {
                "press".to_string()
            }
        } else {
            m
        }
    };

    Macro {
        name: node.prop_str("name", "Unnamed"),
        hotkey: node.prop_str("hotkey", "").to_lowercase(),
        delay: node.prop_int("delay", default_delay as i64) as i32,
        mode,
        hold_mode,
        keys,
        inter_key_delay: node.prop_int("interKeyDelay", 0) as i32,
        enabled: node.prop_bool("enabled", true),
        max_hold_duration: node.prop_int("maxHoldDuration", 0) as i32,
        background: node.prop_bool("background", false),
    }
}

fn parse_proc(node: &Node) -> PixelTrigger {
    let mut pixels = Vec::new();
    let mut anchor: Option<Anchor> = None;
    let mut blocker: Option<Anchor> = None;

    for child in &node.children {
        match child.name.as_str() {
            "pixel" => pixels.push(parse_pixel(child)),
            "anchor" => {
                let ap: Vec<Pixel> = child.children_named("pixel").map(parse_pixel).collect();
                anchor = Some(Anchor {
                    pixels: ap,
                    match_mode: child.prop_str("matchMode", "all"),
                });
            }
            "blocker" => {
                let bp: Vec<Pixel> = child.children_named("pixel").map(parse_pixel).collect();
                blocker = Some(Anchor {
                    pixels: bp,
                    match_mode: child.prop_str("matchMode", "all"),
                });
            }
            _ => {}
        }
    }

    let capture_res = parse_res(&node.prop_str("captureRes", ""));

    PixelTrigger {
        name: node.prop_str("name", "Unnamed"),
        action_key: {
            let ak = node.prop_str("actionKey", "");
            if ak.is_empty() {
                node.prop_str("key", "")
            } else {
                ak
            }
        }
        .to_lowercase(),
        pixels,
        match_mode: node.prop_str("matchMode", "all"),
        inverse: node.prop_bool("inverse", false),
        enabled: node.prop_bool("enabled", true),
        cooldown: node.prop_int("cooldown", 1000) as i32,
        trigger_mode: node.prop_str("triggerMode", "macro"),
        macro_hotkey: node.prop_str("macroHotkey", "").to_lowercase(),
        capture_res,
        anchor,
        blocker,
        last_fired: 0,
    }
}

fn parse_buff(node: &Node) -> BuffTimer {
    let mut watch_keys = Vec::new();
    let mut pixels = Vec::new();

    for child in &node.children {
        match child.name.as_str() {
            "watchKey" => {
                if let Some(k) = child.args.first().and_then(|v| v.as_str()) {
                    watch_keys.push(k.to_lowercase());
                }
            }
            "pixel" => pixels.push(parse_pixel(child)),
            _ => {}
        }
    }

    BuffTimer {
        name: node.prop_str("name", "Unnamed"),
        watch_keys,
        duration: node.prop_int("duration", 5000) as i32,
        action_key: node.prop_str("actionKey", "").to_lowercase(),
        on_refresh: node.prop_str("onRefresh", "reset"),
        extend_ms: node.prop_int("extendMs", 0) as i32,
        enabled: node.prop_bool("enabled", true),
        sound_on_expiry: node.prop_bool("soundOnExpiry", false),
        trigger_type: node.prop_str("triggerType", "keys"),
        trigger_pixels: pixels,
        trigger_match_mode: node.prop_str("triggerMatchMode", "all"),
        capture_res: parse_res(&node.prop_str("captureRes", "")),
    }
}

fn parse_detect(node: &Node) -> Detect {
    let pixels: Vec<Pixel> = node.children_named("pixel").map(parse_pixel).collect();
    Detect {
        pixels,
        match_mode: node.prop_str("matchMode", "all"),
        capture_res: parse_res(&node.prop_str("captureRes", "")),
    }
}

fn parse_pixel(node: &Node) -> Pixel {
    let color = node.prop_str("color", "0x000000");
    Pixel {
        x: node.prop_int("x", 0) as i32,
        y: node.prop_int("y", 0) as i32,
        color,
        variation: node.prop_int("variation", 10) as i32,
    }
}

fn parse_res(val: &str) -> Option<Resolution> {
    let parts: Vec<&str> = val.split('x').collect();
    if parts.len() != 2 {
        return None;
    }
    let w = parts[0].parse::<i32>().ok()?;
    let h = parts[1].parse::<i32>().ok()?;
    Some(Resolution { w, h })
}

fn res_str(r: &Option<Resolution>) -> String {
    match r {
        Some(r) => format!("{}x{}", r.w, r.h),
        None => String::new(),
    }
}

// ── KDL emission ────────────────────────────────────────────────────────

fn build_doc(tree: &ConfigTree) -> Document {
    let mut nodes = Vec::new();

    // settings
    let mut s_node = Node::new("settings");
    let s = &tree.settings;
    s_node.set_int("defaultDelay", s.default_delay as i64);
    s_node.set_bool("autoDetectGame", s.auto_detect_game);
    s_node.set_bool("onlyInGame", s.only_in_game);
    s_node.set_bool("allowBackground", s.allow_background);
    s_node.set_bool("minimizeToTray", s.minimize_to_tray);
    s_node.set_bool("darkMode", s.dark_mode);
    s_node.set_str("toggleKey", &s.toggle_key);
    s_node.set_int("pixelCheckRate", s.pixel_check_rate as i64);
    s_node.set_bool("showTerminal", s.show_terminal);
    s_node.set_str("overlayPosition", &s.overlay_position);
    nodes.push(s_node);

    // active
    let mut a_node = Node::new("active");
    a_node.set_str("game", &tree.active_game);
    a_node.set_str("class", &tree.active_class);
    a_node.set_str("spec", &tree.active_spec);
    nodes.push(a_node);

    // games (sorted by BTreeMap)
    for (g_name, g) in &tree.games {
        let mut g_node = Node::new("game");
        g_node.args.push(Value::Str(g_name.clone()));
        g_node.set_str("path", &g.path);

        for (c_name, c) in &g.classes {
            let mut c_node = Node::new("class");
            c_node.args.push(Value::Str(c_name.clone()));
            if !c.icon.is_empty() {
                c_node.set_str("icon", &c.icon);
            }
            for (sp_name, sp) in &c.specs {
                let mut sp_node = Node::new("spec");
                sp_node.args.push(Value::Str(sp_name.clone()));

                // detect
                if let Some(d) = &sp.detect {
                    if !d.pixels.is_empty() {
                        let mut d_node = Node::new("detect");
                        d_node.set_str("matchMode", &d.match_mode);
                        if let Some(cr) = res_str(&d.capture_res).into() {
                            if !cr.is_empty() {
                                d_node.set_str("captureRes", &cr);
                            }
                        }
                        for px in &d.pixels {
                            d_node.children.push(pixel_node(px));
                        }
                        sp_node.children.push(d_node);
                    }
                }

                // macros
                for m in &sp.macros {
                    let mut m_node = Node::new("macro");
                    m_node.set_str("name", &m.name);
                    m_node.set_str("hotkey", &m.hotkey);
                    m_node.set_int("delay", m.delay as i64);
                    m_node.set_str("mode", &m.mode);
                    m_node.set_int("interKeyDelay", m.inter_key_delay as i64);
                    m_node.set_bool("enabled", m.enabled);
                    m_node.set_int("maxHoldDuration", m.max_hold_duration as i64);
                    m_node.set_bool("background", m.background);
                    for k in &m.keys {
                        let mut kn = Node::new("key");
                        kn.args.push(Value::Str(k.clone()));
                        m_node.children.push(kn);
                    }
                    sp_node.children.push(m_node);
                }

                // procs
                for p in &sp.pixel_triggers {
                    let mut p_node = Node::new("proc");
                    p_node.set_str("name", &p.name);
                    p_node.set_str("actionKey", &p.action_key);
                    p_node.set_str("matchMode", &p.match_mode);
                    p_node.set_str("triggerMode", &p.trigger_mode);
                    p_node.set_bool("inverse", p.inverse);
                    p_node.set_bool("enabled", p.enabled);
                    p_node.set_int("cooldown", p.cooldown as i64);
                    p_node.set_str("macroHotkey", &p.macro_hotkey);
                    let cr = res_str(&p.capture_res);
                    if !cr.is_empty() {
                        p_node.set_str("captureRes", &cr);
                    }
                    if let Some(a) = &p.anchor {
                        if !a.pixels.is_empty() {
                            let mut an = Node::new("anchor");
                            an.set_str("matchMode", &a.match_mode);
                            for px in &a.pixels {
                                an.children.push(pixel_node(px));
                            }
                            p_node.children.push(an);
                        }
                    }
                    if let Some(b) = &p.blocker {
                        if !b.pixels.is_empty() {
                            let mut bn = Node::new("blocker");
                            bn.set_str("matchMode", &b.match_mode);
                            for px in &b.pixels {
                                bn.children.push(pixel_node(px));
                            }
                            p_node.children.push(bn);
                        }
                    }
                    for px in &p.pixels {
                        p_node.children.push(pixel_node(px));
                    }
                    sp_node.children.push(p_node);
                }

                // buffs
                for b in &sp.buff_timers {
                    let mut b_node = Node::new("buff");
                    b_node.set_str("name", &b.name);
                    b_node.set_str("triggerType", &b.trigger_type);
                    b_node.set_int("duration", b.duration as i64);
                    b_node.set_str("actionKey", &b.action_key);
                    b_node.set_str("onRefresh", &b.on_refresh);
                    b_node.set_int("extendMs", b.extend_ms as i64);
                    b_node.set_bool("enabled", b.enabled);
                    b_node.set_bool("soundOnExpiry", b.sound_on_expiry);
                    b_node.set_str("triggerMatchMode", &b.trigger_match_mode);
                    let cr = res_str(&b.capture_res);
                    if !cr.is_empty() {
                        b_node.set_str("captureRes", &cr);
                    }
                    for wk in &b.watch_keys {
                        let mut wkn = Node::new("watchKey");
                        wkn.args.push(Value::Str(wk.clone()));
                        b_node.children.push(wkn);
                    }
                    for px in &b.trigger_pixels {
                        b_node.children.push(pixel_node(px));
                    }
                    sp_node.children.push(b_node);
                }

                c_node.children.push(sp_node);
            }
            g_node.children.push(c_node);
        }
        nodes.push(g_node);
    }

    Document { nodes }
}

fn pixel_node(px: &Pixel) -> Node {
    let mut n = Node::new("pixel");
    n.set_int("x", px.x as i64);
    n.set_int("y", px.y as i64);
    n.set_str("color", &px.color);
    n.set_int("variation", px.variation as i64);
    n
}

// ── Validation ──────────────────────────────────────────────────────────

fn validate(tree: &mut ConfigTree) {
    tree.settings.overlay_position = normalize_overlay_position(&tree.settings.overlay_position);
    for g in tree.games.values_mut() {
        for c in g.classes.values_mut() {
            for s in c.specs.values_mut() {
                for m in &mut s.macros {
                    if m.name.is_empty() {
                        m.name = "Unnamed".into();
                    }
                    if m.delay == 0 {
                        m.delay = tree.settings.default_delay;
                    }
                    if m.mode.is_empty() {
                        m.mode = if m.hold_mode {
                            "hold".into()
                        } else {
                            "press".into()
                        };
                    }
                }
                for p in &mut s.pixel_triggers {
                    if p.name.is_empty() {
                        p.name = "Unnamed".into();
                    }
                    if p.cooldown == 0 {
                        p.cooldown = 1000;
                    }
                    if p.match_mode.is_empty() {
                        p.match_mode = "all".into();
                    }
                    if p.pixels.is_empty() {
                        p.pixels.push(Pixel {
                            x: 0,
                            y: 0,
                            color: "0x000000".into(),
                            variation: 10,
                        });
                    }
                }
                for b in &mut s.buff_timers {
                    if b.name.is_empty() {
                        b.name = "Unnamed".into();
                    }
                    if b.duration == 0 {
                        b.duration = 5000;
                    }
                    if b.on_refresh.is_empty() {
                        b.on_refresh = "reset".into();
                    }
                    if b.trigger_type.is_empty() {
                        b.trigger_type = "keys".into();
                    }
                }
            }
        }
    }
}

fn normalize_overlay_position(position: &str) -> String {
    match position {
        "top-left" | "top-right" | "bottom-left" | "bottom-right" | "hidden" => {
            position.to_string()
        }
        _ => default_overlay_position(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn manager_in(config_dir: &std::path::Path) -> Manager {
        Manager {
            inner: Arc::new(RwLock::new(ManagerInner {
                tree: ConfigTree::default(),
                config_dir: config_dir.to_path_buf(),
                config_file: config_dir.join("config.kdl"),
                backup_file: config_dir.join("config.kdl.bak"),
                save_deadline: None,
                persistence_blocked: false,
            })),
        }
    }

    #[test]
    fn invalid_utf8_config_is_not_overwritten() {
        let dir = tempfile::tempdir().expect("config fixture directory");
        let path = dir.path().join("config.kdl");
        let invalid = b"settings {\n  broken=\xff\n}\n";
        std::fs::write(&path, invalid).expect("write invalid config fixture");

        let result = read_config_or_initialize(&path, "replacement defaults");

        assert!(result.is_err());
        assert_eq!(std::fs::read(path).expect("read preserved config"), invalid);
    }

    #[test]
    fn malformed_config_blocks_shutdown_flush_and_preserves_original() {
        let dir = tempfile::tempdir().expect("config fixture directory");
        let path = dir.path().join("config.kdl");
        let malformed = b"settings {\n  defaultDelay=\n";
        std::fs::write(&path, malformed).expect("write malformed config fixture");
        let manager = manager_in(dir.path());

        assert!(manager.load().is_err());
        assert!(manager.flush().is_err());
        assert_eq!(std::fs::read(path).expect("read preserved config"), malformed);
    }

    #[test]
    fn atomic_default_creation_never_overwrites_a_concurrent_winner() {
        let dir = tempfile::tempdir().expect("config fixture directory");
        let path = dir.path().join("config.kdl");
        let winner = b"settings defaultDelay=42\n";
        std::fs::write(&path, winner).expect("publish concurrent config");

        assert!(!create_default_config(&path, "replacement defaults").expect("create result"));
        assert_eq!(std::fs::read(path).expect("read winning config"), winner);
    }
}
