//! Persistent configuration (TOML at ~/.config/inputforge/config.toml).

use serde::{Deserialize, Serialize};
use std::path::PathBuf;

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(default)]
pub struct Config {
    /// Devices InputForge should grab (exclusive access) and process.
    pub devices: Vec<DeviceSettings>,
    pub profiles: Vec<Profile>,
    pub active_profile: usize,
    pub macros: Vec<Macro>,
    pub autoclicker: AutoClicker,
    /// Start the engine automatically when the app launches.
    pub autostart_engine: bool,
    /// Follow the desktop theme, or force dark/light.
    pub theme: crate::theme::ThemeMode,
    /// Use the desktop's UI and monospace fonts.
    pub system_font: bool,
    /// Built-in keyboard/mouse lighting.
    pub lighting: crate::lighting::LightingSettings,
}

impl Default for Config {
    fn default() -> Self {
        Self {
            devices: vec![],
            profiles: vec![Profile {
                name: "Default".into(),
                rules: vec![],
            }],
            active_profile: 0,
            macros: vec![],
            autoclicker: AutoClicker::default(),
            autostart_engine: false,
            theme: crate::theme::ThemeMode::System,
            system_font: true,
            lighting: Default::default(),
        }
    }
}

/// A physical device is identified by its name + physical path, which are stable
/// across reboots (unlike /dev/input/eventN numbers).
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(default)]
pub struct DeviceSettings {
    pub name: String,
    pub phys: String,
    pub enabled: bool,
    /// Pointer speed multiplier (1.0 = unchanged).
    pub pointer_speed: f32,
    /// Scroll wheel multiplier (1.0 = unchanged).
    pub scroll_speed: f32,
    /// Swap vertical scroll direction.
    pub invert_scroll: bool,
}

impl Default for DeviceSettings {
    fn default() -> Self {
        Self {
            name: String::new(),
            phys: String::new(),
            enabled: false,
            pointer_speed: 1.0,
            scroll_speed: 1.0,
            invert_scroll: false,
        }
    }
}

impl DeviceSettings {
    pub fn key(&self) -> (String, String) {
        (self.name.clone(), self.phys.clone())
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Default)]
pub struct Profile {
    pub name: String,
    pub rules: Vec<Rule>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(default)]
pub struct Rule {
    pub enabled: bool,
    /// Only apply to devices whose name contains this (empty = all grabbed devices).
    pub device_filter: String,
    /// evdev key/button name, e.g. "KEY_CAPSLOCK" or "BTN_SIDE".
    pub trigger: String,
    pub action: Action,
}

impl Default for Rule {
    fn default() -> Self {
        Self {
            enabled: true,
            device_filter: String::new(),
            trigger: "KEY_CAPSLOCK".into(),
            action: Action::Key {
                key: "KEY_ESC".into(),
            },
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(tag = "type")]
pub enum Action {
    /// Remap to a different single key/button (held while trigger is held).
    Key { key: String },
    /// Press a key combination, e.g. ["KEY_LEFTCTRL", "KEY_C"].
    Combo { keys: Vec<String> },
    /// Run a named macro.
    Macro { name: String },
    /// Swallow the key entirely.
    Disabled,
    /// Toggle the autoclicker on/off.
    ToggleAutoclicker,
}

impl Action {
    pub fn label(&self) -> &'static str {
        match self {
            Action::Key { .. } => "Remap to key",
            Action::Combo { .. } => "Key combo",
            Action::Macro { .. } => "Run macro",
            Action::Disabled => "Disable",
            Action::ToggleAutoclicker => "Toggle autoclicker",
        }
    }
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq, Default)]
pub enum MacroMode {
    /// Run once per press.
    #[default]
    Once,
    /// Repeat while the trigger is held.
    WhileHeld,
    /// First press starts repeating, second press stops.
    Toggle,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Default)]
#[serde(default)]
pub struct Macro {
    pub name: String,
    pub mode: MacroMode,
    pub steps: Vec<Step>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(tag = "type")]
pub enum Step {
    /// Press and release a key/button.
    Tap {
        key: String,
    },
    KeyDown {
        key: String,
    },
    KeyUp {
        key: String,
    },
    /// Type text (US layout).
    Text {
        text: String,
    },
    Delay {
        ms: u64,
    },
    MouseMove {
        dx: i32,
        dy: i32,
    },
    Scroll {
        amount: i32,
    },
}

impl Step {
    pub fn label(&self) -> &'static str {
        match self {
            Step::Tap { .. } => "Tap",
            Step::KeyDown { .. } => "Key down",
            Step::KeyUp { .. } => "Key up",
            Step::Text { .. } => "Type text",
            Step::Delay { .. } => "Delay",
            Step::MouseMove { .. } => "Mouse move",
            Step::Scroll { .. } => "Scroll",
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(default)]
pub struct AutoClicker {
    pub button: String,
    pub clicks_per_second: f32,
    /// Hotkey (on a grabbed device) that toggles the autoclicker.
    pub hotkey: String,
    /// Optional: stop automatically after N clicks (0 = unlimited).
    pub max_clicks: u64,
}

impl Default for AutoClicker {
    fn default() -> Self {
        Self {
            button: "BTN_LEFT".into(),
            clicks_per_second: 10.0,
            hotkey: "KEY_F8".into(),
            max_clicks: 0,
        }
    }
}

pub fn config_path() -> PathBuf {
    dirs::config_dir()
        .unwrap_or_else(|| PathBuf::from("."))
        .join("inputforge")
        .join("config.toml")
}

impl Config {
    pub fn load() -> anyhow::Result<Self> {
        let path = config_path();
        if !path.exists() {
            return Ok(Self::default());
        }
        let text = std::fs::read_to_string(&path)?;
        let mut cfg: Config = toml::from_str(&text)?;
        if cfg.profiles.is_empty() {
            cfg.profiles.push(Profile {
                name: "Default".into(),
                rules: vec![],
            });
        }
        cfg.active_profile = cfg.active_profile.min(cfg.profiles.len() - 1);
        Ok(cfg)
    }

    pub fn save(&self) -> anyhow::Result<()> {
        let path = config_path();
        if let Some(dir) = path.parent() {
            std::fs::create_dir_all(dir)?;
        }
        let tmp = path.with_extension("toml.tmp");
        std::fs::write(&tmp, toml::to_string_pretty(self)?)?;
        std::fs::rename(tmp, path)?;
        Ok(())
    }

    pub fn active_rules(&self) -> &[Rule] {
        self.profiles
            .get(self.active_profile)
            .map(|p| p.rules.as_slice())
            .unwrap_or(&[])
    }

    pub fn find_macro(&self, name: &str) -> Option<&Macro> {
        self.macros.iter().find(|m| m.name == name)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn roundtrip_toml() {
        let mut c = Config::default();
        c.profiles[0].rules.push(Rule::default());
        c.profiles[0].rules.push(Rule {
            action: Action::Combo {
                keys: vec!["KEY_LEFTCTRL".into(), "KEY_C".into()],
            },
            ..Default::default()
        });
        c.macros.push(Macro {
            name: "hello".into(),
            mode: MacroMode::Toggle,
            steps: vec![Step::Text { text: "Hi!".into() }, Step::Delay { ms: 50 }],
        });
        let s = toml::to_string_pretty(&c).unwrap();
        let back: Config = toml::from_str(&s).unwrap();
        assert_eq!(c, back);
    }
}
