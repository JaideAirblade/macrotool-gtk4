//! StatusNotifierItem system tray integration.
//!
//! `ksni` speaks the freedesktop/KDE tray protocol directly over D-Bus, so
//! this works with DMS/Quickshell without the legacy AppIndicator library.

use ksni::blocking::{Handle, TrayMethods};
use ksni::menu::StandardItem;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::{self, Receiver, Sender};
use std::sync::Arc;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TrayCommand {
    Show,
    Quit,
}

struct MacrotoolTray {
    commands: Sender<TrayCommand>,
    online: Arc<AtomicBool>,
}

impl MacrotoolTray {
    fn request(&self, command: TrayCommand) {
        let _ = self.commands.send(command);
    }
}

impl ksni::Tray for MacrotoolTray {
    fn id(&self) -> String {
        "macrotool".into()
    }

    fn title(&self) -> String {
        "Macrotool".into()
    }

    fn icon_name(&self) -> String {
        "input-gaming".into()
    }

    fn activate(&mut self, _x: i32, _y: i32) {
        self.request(TrayCommand::Show);
    }

    fn menu(&self) -> Vec<ksni::MenuItem<Self>> {
        vec![
            StandardItem {
                label: "Show Macrotool".into(),
                icon_name: "window-new".into(),
                activate: Box::new(|tray: &mut Self| tray.request(TrayCommand::Show)),
                ..Default::default()
            }
            .into(),
            StandardItem {
                label: "Quit".into(),
                icon_name: "application-exit".into(),
                activate: Box::new(|tray: &mut Self| tray.request(TrayCommand::Quit)),
                ..Default::default()
            }
            .into(),
        ]
    }

    fn watcher_online(&self) {
        self.online.store(true, Ordering::Release);
    }

    fn watcher_offline(&self, _reason: ksni::OfflineReason) -> bool {
        self.online.store(false, Ordering::Release);
        true
    }
}

pub struct TrayController {
    commands: Receiver<TrayCommand>,
    online: Arc<AtomicBool>,
    handle: Option<Handle<MacrotoolTray>>,
}

impl TrayController {
    pub fn start() -> Result<Self, String> {
        let (command_tx, commands) = mpsc::channel();
        let online = Arc::new(AtomicBool::new(true));
        let tray = MacrotoolTray {
            commands: command_tx,
            online: online.clone(),
        };
        let handle = tray
            .spawn()
            .map_err(|error| format!("could not register system tray: {error}"))?;
        Ok(Self {
            commands,
            online,
            handle: Some(handle),
        })
    }

    pub fn take_command(&self) -> Option<TrayCommand> {
        self.commands.try_recv().ok()
    }

    pub fn availability(&self) -> Arc<AtomicBool> {
        self.online.clone()
    }

    pub fn shutdown(&mut self) {
        if let Some(handle) = self.handle.take() {
            handle.shutdown().wait();
        }
    }
}

impl Drop for TrayController {
    fn drop(&mut self) {
        self.shutdown();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn commands_are_consumed_once() {
        let (command_tx, commands) = mpsc::channel();
        command_tx.send(TrayCommand::Show).unwrap();
        command_tx.send(TrayCommand::Quit).unwrap();
        let controller = TrayController {
            commands,
            online: Arc::new(AtomicBool::new(true)),
            handle: None,
        };
        assert!(matches!(controller.take_command(), Some(TrayCommand::Show)));
        assert!(matches!(controller.take_command(), Some(TrayCommand::Quit)));
        assert!(controller.take_command().is_none());
    }

    #[test]
    fn watcher_state_tracks_offline_and_online_transitions() {
        let (commands, _receiver) = mpsc::channel();
        let online = Arc::new(AtomicBool::new(true));
        let tray = MacrotoolTray {
            commands,
            online: online.clone(),
        };

        assert!(ksni::Tray::watcher_offline(&tray, ksni::OfflineReason::No));
        assert!(!online.load(Ordering::Acquire));
        ksni::Tray::watcher_online(&tray);
        assert!(online.load(Ordering::Acquire));
    }
}
