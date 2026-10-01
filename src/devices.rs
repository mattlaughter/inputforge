//! Enumerate physical input devices from /dev/input.

use evdev::{AbsoluteAxisCode, Device, KeyCode, RelativeAxisCode};
use std::path::PathBuf;

/// Name prefix used for our own uinput devices; never grab them.
pub const VIRTUAL_PREFIX: &str = "InputForge Virtual";

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DeviceKind {
    Keyboard,
    Mouse,
    /// Has both a full keyboard and pointer (e.g. combo receivers).
    Combo,
    /// Has keys but isn't a full keyboard (media keys, extra mouse buttons, power button…).
    Keys,
    /// Touchpads / tablets / joysticks: absolute axes we don't proxy.
    Absolute,
    Other,
}

impl DeviceKind {
    pub fn label(self) -> &'static str {
        match self {
            DeviceKind::Keyboard => "Keyboard",
            DeviceKind::Mouse => "Mouse",
            DeviceKind::Combo => "Keyboard + Mouse",
            DeviceKind::Keys => "Extra keys",
            DeviceKind::Absolute => "Touchpad / tablet",
            DeviceKind::Other => "Other",
        }
    }

    /// Whether InputForge can safely grab and proxy this device.
    pub fn grabbable(self) -> bool {
        matches!(
            self,
            DeviceKind::Keyboard | DeviceKind::Mouse | DeviceKind::Combo | DeviceKind::Keys
        )
    }
}

#[derive(Debug, Clone)]
pub struct DeviceInfo {
    pub path: PathBuf,
    pub name: String,
    pub phys: String,
    pub kind: DeviceKind,
    pub vendor: u16,
    pub product: u16,
    /// Supported EV_KEY codes (keys and buttons).
    pub keys: Vec<u16>,
}

pub fn classify(dev: &Device) -> DeviceKind {
    let keys = dev.supported_keys();
    let has_key = |k| keys.is_some_and(|s| s.contains(k));
    let rel = dev.supported_relative_axes();
    let has_rel = |a| rel.is_some_and(|s| s.contains(a));
    let abs = dev.supported_absolute_axes();
    let has_abs = |a| abs.is_some_and(|s| s.contains(a));

    let keyboard =
        has_key(KeyCode::KEY_A) && has_key(KeyCode::KEY_Z) && has_key(KeyCode::KEY_ENTER);
    let mouse = has_rel(RelativeAxisCode::REL_X) && has_key(KeyCode::BTN_LEFT);
    if has_abs(AbsoluteAxisCode::ABS_X) && !mouse {
        return DeviceKind::Absolute;
    }
    match (keyboard, mouse) {
        (true, true) => DeviceKind::Combo,
        (true, false) => DeviceKind::Keyboard,
        (false, true) => DeviceKind::Mouse,
        _ if keys.is_some_and(|k| k.iter().next().is_some()) => DeviceKind::Keys,
        _ => DeviceKind::Other,
    }
}

pub struct ScanResult {
    pub devices: Vec<DeviceInfo>,
    /// Event nodes we couldn't open (permission denied).
    pub unreadable: usize,
}

pub fn scan() -> ScanResult {
    let mut devices = vec![];
    let mut unreadable = 0;
    let Ok(dir) = std::fs::read_dir("/dev/input") else {
        return ScanResult {
            devices,
            unreadable,
        };
    };
    let mut paths: Vec<PathBuf> = dir
        .flatten()
        .map(|e| e.path())
        .filter(|p| {
            p.file_name()
                .and_then(|n| n.to_str())
                .is_some_and(|n| n.starts_with("event"))
        })
        .collect();
    paths.sort_by_key(|p| {
        p.file_name()
            .and_then(|n| n.to_str())
            .and_then(|n| n.trim_start_matches("event").parse::<u32>().ok())
            .unwrap_or(u32::MAX)
    });
    for path in paths {
        match Device::open(&path) {
            Ok(dev) => {
                let name = dev.name().unwrap_or("Unknown").to_string();
                if name.starts_with(VIRTUAL_PREFIX) {
                    continue;
                }
                let id = dev.input_id();
                devices.push(DeviceInfo {
                    kind: classify(&dev),
                    phys: dev.physical_path().unwrap_or("").to_string(),
                    name,
                    path,
                    vendor: id.vendor(),
                    product: id.product(),
                    keys: dev
                        .supported_keys()
                        .map(|k| k.iter().map(|c| c.code()).collect())
                        .unwrap_or_default(),
                });
            }
            Err(_) => unreadable += 1,
        }
    }
    ScanResult {
        devices,
        unreadable,
    }
}

/// Check whether we can create virtual devices.
pub fn uinput_writable() -> bool {
    std::fs::OpenOptions::new()
        .write(true)
        .open("/dev/uinput")
        .is_ok()
}

/// Explain a missing 'input' group: not a member, or a member but this login
/// session predates it (needs log out / reboot).
pub fn group_hint() -> String {
    let Ok(groups) = std::fs::read_to_string("/etc/group") else {
        return String::new();
    };
    let Some(line) = groups.lines().find(|l| l.starts_with("input:")) else {
        return String::new();
    };
    let gid: u32 = line
        .split(':')
        .nth(2)
        .and_then(|g| g.parse().ok())
        .unwrap_or(0);
    let user = std::env::var("USER").unwrap_or_default();
    let member = line
        .rsplit(':')
        .next()
        .unwrap_or("")
        .split(',')
        .any(|u| u == user);
    let mut buf = [0 as libc::gid_t; 256];
    let n = unsafe { libc::getgroups(buf.len() as i32, buf.as_mut_ptr()) };
    let active = n > 0 && buf[..n as usize].contains(&gid);
    match (member, active) {
        (_, true) => String::new(),
        (true, false) => "You were added to the 'input' group, but this login session started \
before that. Log out of the desktop completely and back in (or reboot), then retry."
            .into(),
        (false, _) => "You are not in the 'input' group. Run the installer.".into(),
    }
}

/// One physical product (e.g. "G600"), made of several kernel event nodes.
#[derive(Debug, Clone)]
pub struct PhysDevice {
    /// Stable id: "vvvv:pppp".
    pub id: String,
    /// Friendly display name, e.g. "Logitech G815".
    pub name: String,
    /// Short model line, e.g. "RGB Mechanical Gaming Keyboard".
    pub subtitle: String,
    pub is_keyboard: bool,
    pub vendor: u16,
    pub product: u16,
    /// Substring that matches every node name (used as the rule device filter).
    pub filter: String,
    pub nodes: Vec<DeviceInfo>,
}

impl PhysDevice {
    pub fn grabbable_nodes(&self) -> impl Iterator<Item = &DeviceInfo> {
        self.nodes.iter().filter(|n| n.kind.grabbable())
    }
    pub fn has_pointer(&self) -> bool {
        self.nodes
            .iter()
            .any(|n| matches!(n.kind, DeviceKind::Mouse | DeviceKind::Combo))
    }
}

fn tidy_name(raw: &str) -> String {
    // "Razer Razer Naga" -> "Razer Naga"; title-case SHOUTED words except acronyms.
    let mut words: Vec<String> = vec![];
    for w in raw.split_whitespace() {
        if words.last().is_some_and(|l| l.eq_ignore_ascii_case(w)) {
            continue;
        }
        let shouted = w.len() > 3
            && w.chars().all(|c| !c.is_lowercase())
            && w.chars().any(|c| c.is_alphabetic());
        let has_digit = w.chars().any(|c| c.is_ascii_digit());
        if shouted && !has_digit {
            let mut c = w.chars();
            let first = c.next().unwrap();
            words.push(first.to_string() + &c.as_str().to_lowercase());
        } else {
            words.push(w.to_string());
        }
    }
    words.join(" ")
}

/// Split a product name into (brand + model, description):
/// "Logitech G815 RGB Mechanical Gaming Keyboard" -> ("Logitech G815", "RGB Mechanical Gaming Keyboard")
/// "Logitech Gaming Mouse G600" -> ("Logitech G600", "Gaming Mouse")
fn split_name(pretty: &str) -> (String, String) {
    const DESC: [&str; 6] = ["RGB", "Rgb", "Mechanical", "Gaming", "Keyboard", "Mouse"];
    let words: Vec<&str> = pretty.split(' ').collect();
    let (desc, model): (Vec<&str>, Vec<&str>) =
        words.iter().skip(1).partition(|w| DESC.contains(w));
    let mut name = vec![words[0]];
    name.extend(model);
    let desc = desc.join(" ").replace("Rgb", "RGB");
    (name.join(" "), desc)
}

/// Group event nodes into physical keyboards and mice. Devices that are neither
/// (audio jacks, power button, LED controllers) are left out.
pub fn group(devs: &[DeviceInfo]) -> Vec<PhysDevice> {
    let mut map: std::collections::BTreeMap<(u16, u16), Vec<DeviceInfo>> = Default::default();
    for d in devs {
        if d.vendor == 0 || d.phys.is_empty() && d.vendor < 0x0100 {
            continue;
        }
        map.entry((d.vendor, d.product))
            .or_default()
            .push(d.clone());
    }
    let mut out = vec![];
    for ((vendor, product), nodes) in map {
        let has_kb = nodes.iter().any(|n| n.kind == DeviceKind::Keyboard);
        let has_mouse = nodes
            .iter()
            .any(|n| matches!(n.kind, DeviceKind::Mouse | DeviceKind::Combo));
        if !has_kb && !has_mouse {
            continue;
        }
        // Common prefix of all node names = the product name.
        let mut base = nodes[0].name.clone();
        for n in &nodes[1..] {
            let l = base
                .chars()
                .zip(n.name.chars())
                .take_while(|(a, b)| a == b)
                .count();
            base = base.chars().take(l).collect();
        }
        let base = base.trim().to_string();
        let filter = if base.is_empty() {
            nodes[0].name.clone()
        } else {
            base.clone()
        };
        let pretty = tidy_name(&filter);
        let (name, subtitle) = split_name(&pretty);
        let lname = pretty.to_lowercase();
        let is_keyboard = if lname.contains("keyboard") {
            true
        } else if lname.contains("mouse") || lname.contains("naga") || lname.contains("mice") {
            false
        } else {
            has_kb && !has_mouse
        };
        let subtitle = if subtitle.is_empty() {
            if is_keyboard {
                "Keyboard".to_string()
            } else {
                "Mouse".to_string()
            }
        } else {
            subtitle
        };
        out.push(PhysDevice {
            id: format!("{vendor:04x}:{product:04x}"),
            name,
            subtitle,
            is_keyboard,
            vendor,
            product,
            filter,
            nodes,
        });
    }
    // Keyboards first, then mice.
    out.sort_by_key(|d| (!d.is_keyboard, d.name.clone()));
    out
}

#[cfg(test)]
mod group_tests {
    use super::*;
    fn n(name: &str, kind: DeviceKind, v: u16, p: u16) -> DeviceInfo {
        DeviceInfo {
            path: "/dev/null".into(),
            name: name.into(),
            phys: "usb-x".into(),
            kind,
            vendor: v,
            product: p,
            keys: vec![],
        }
    }
    #[test]
    fn groups_real_devices() {
        let devs = vec![
            n(
                "Logitech G815 RGB MECHANICAL GAMING KEYBOARD",
                DeviceKind::Keyboard,
                0x046d,
                0xc33f,
            ),
            n(
                "Logitech G815 RGB MECHANICAL GAMING KEYBOARD Keyboard",
                DeviceKind::Keyboard,
                0x046d,
                0xc33f,
            ),
            n(
                "Logitech G815 RGB MECHANICAL GAMING KEYBOARD Mouse",
                DeviceKind::Mouse,
                0x046d,
                0xc33f,
            ),
            n(
                "Logitech Gaming Mouse G600",
                DeviceKind::Mouse,
                0x046d,
                0xc24a,
            ),
            n(
                "Logitech Gaming Mouse G600 Keyboard",
                DeviceKind::Keyboard,
                0x046d,
                0xc24a,
            ),
            n(
                "Logitech Gaming Mouse G600",
                DeviceKind::Other,
                0x046d,
                0xc24a,
            ),
            n(
                "Razer Razer Naga V2 HyperSpeed",
                DeviceKind::Mouse,
                0x1532,
                0x00b4,
            ),
            n(
                "Razer Razer Naga V2 HyperSpeed Keyboard",
                DeviceKind::Keyboard,
                0x1532,
                0x00b4,
            ),
            n(
                "Razer Razer Naga V2 HyperSpeed Mouse",
                DeviceKind::Mouse,
                0x1532,
                0x00b4,
            ),
            n("HD-Audio Generic Front Mic", DeviceKind::Other, 0, 0),
            n("Power Button", DeviceKind::Keys, 0, 1),
        ];
        let g = group(&devs);
        let names: Vec<_> = g
            .iter()
            .map(|d| (d.name.as_str(), d.subtitle.as_str(), d.is_keyboard))
            .collect();
        assert_eq!(
            names,
            vec![
                ("Logitech G815", "RGB Mechanical Gaming Keyboard", true),
                ("Logitech G600", "Gaming Mouse", false),
                ("Razer Naga V2 HyperSpeed", "Mouse", false),
            ]
        );
        assert_eq!(g[1].filter, "Logitech Gaming Mouse G600");
        assert_eq!(g[2].filter, "Razer Razer Naga V2 HyperSpeed");
    }
}
