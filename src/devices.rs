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
    /// Supported EV_REL codes (pointer and wheel axes).
    pub rel: Vec<u16>,
    /// Whether this user can open the node. Devices are listed from sysfs
    /// either way, so they can be shown and access requested per device.
    pub accessible: bool,
}

pub fn classify(dev: &Device) -> DeviceKind {
    let keys: Vec<u16> = dev
        .supported_keys()
        .map(|k| k.iter().map(|c| c.code()).collect())
        .unwrap_or_default();
    let rel: Vec<u16> = dev
        .supported_relative_axes()
        .map(|r| r.iter().map(|a| a.0).collect())
        .unwrap_or_default();
    let abs: Vec<u16> = dev
        .supported_absolute_axes()
        .map(|r| r.iter().map(|a| a.0).collect())
        .unwrap_or_default();
    classify_codes(&keys, &rel, &abs)
}

pub fn classify_codes(keys: &[u16], rel: &[u16], abs: &[u16]) -> DeviceKind {
    let has_key = |k: KeyCode| keys.contains(&k.code());
    let keyboard =
        has_key(KeyCode::KEY_A) && has_key(KeyCode::KEY_Z) && has_key(KeyCode::KEY_ENTER);
    let mouse = rel.contains(&RelativeAxisCode::REL_X.0) && has_key(KeyCode::BTN_LEFT);
    if abs.contains(&AbsoluteAxisCode::ABS_X.0) && !mouse {
        return DeviceKind::Absolute;
    }
    match (keyboard, mouse) {
        (true, true) => DeviceKind::Combo,
        (true, false) => DeviceKind::Keyboard,
        (false, true) => DeviceKind::Mouse,
        _ if !keys.is_empty() => DeviceKind::Keys,
        _ => DeviceKind::Other,
    }
}

/// Parse a sysfs capability bitmap ("ffff0000 0 0 0 0": hex longs, most
/// significant first) into the set bit numbers.
fn parse_caps(s: &str) -> Vec<u16> {
    let words: Vec<u64> = s
        .split_whitespace()
        .map(|w| u64::from_str_radix(w, 16).unwrap_or(0))
        .collect();
    let bits = usize::BITS as usize;
    let mut out = vec![];
    for (i, w) in words.iter().enumerate() {
        let base = (words.len() - 1 - i) * bits;
        for b in 0..bits.min(64) {
            if (w >> b) & 1 == 1 {
                out.push((base + b) as u16);
            }
        }
    }
    out.sort_unstable();
    out
}

/// Describe an event node from sysfs, without opening it.
fn from_sysfs(path: &std::path::Path) -> Option<DeviceInfo> {
    let node = path.file_name()?.to_str()?;
    let dir = std::path::Path::new("/sys/class/input")
        .join(node)
        .join("device");
    let read = |p: &str| {
        std::fs::read_to_string(dir.join(p))
            .map(|s| s.trim().to_string())
            .unwrap_or_default()
    };
    let name = read("name");
    if name.is_empty() {
        return None;
    }
    let hex = |p: &str| u16::from_str_radix(&read(p), 16).unwrap_or(0);
    let keys = parse_caps(&read("capabilities/key"));
    let rel = parse_caps(&read("capabilities/rel"));
    let abs = parse_caps(&read("capabilities/abs"));
    Some(DeviceInfo {
        path: path.to_path_buf(),
        kind: classify_codes(&keys, &rel, &abs),
        phys: read("phys"),
        name,
        vendor: hex("id/vendor"),
        product: hex("id/product"),
        keys,
        rel,
        accessible: false,
    })
}

pub struct ScanResult {
    pub devices: Vec<DeviceInfo>,
    /// Event nodes we couldn't open (permission denied).
    pub unreadable: usize,
}

/// Cheap fingerprint of the current /dev/input/event* nodes (names + inode
/// change times), used to detect hot-plug without opening any device.
pub fn node_signature() -> Vec<(String, i64)> {
    use std::os::unix::fs::MetadataExt;
    let mut v: Vec<(String, i64)> = std::fs::read_dir("/dev/input")
        .map(|d| {
            d.flatten()
                .filter_map(|e| {
                    let n = e.file_name().into_string().ok()?;
                    n.starts_with("event")
                        .then(|| (n, e.metadata().map(|m| m.ctime()).unwrap_or(0)))
                })
                .collect()
        })
        .unwrap_or_default();
    v.sort();
    v
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
        let info = match Device::open(&path) {
            Ok(dev) => {
                let id = dev.input_id();
                Some(DeviceInfo {
                    kind: classify(&dev),
                    phys: dev.physical_path().unwrap_or("").to_string(),
                    name: dev.name().unwrap_or("Unknown").to_string(),
                    path,
                    vendor: id.vendor(),
                    product: id.product(),
                    keys: dev
                        .supported_keys()
                        .map(|k| k.iter().map(|c| c.code()).collect())
                        .unwrap_or_default(),
                    rel: dev
                        .supported_relative_axes()
                        .map(|r| r.iter().map(|a| a.0).collect())
                        .unwrap_or_default(),
                    accessible: true,
                })
            }
            Err(_) => from_sysfs(&path),
        };
        let Some(info) = info else { continue };
        if info.name.starts_with(VIRTUAL_PREFIX) {
            continue;
        }
        if !info.accessible {
            unreadable += 1;
        }
        devices.push(info);
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

/// Explain why InputForge can't create its virtual devices, if it can't.
pub fn access_hint() -> String {
    if uinput_writable() {
        return String::new();
    }
    "InputForge can't create its virtual keyboard and mouse (/dev/uinput). Install the \
package or run the installer, then unplug/replug or log out and back in."
        .into()
}

// ───────────────────────── per-device access (udev uaccess) ─────────────────────────

/// Rules written by `inputforge --udev-allow`, one line per device the user
/// turned on. uaccess = only the user at the active local session gets an ACL.
pub const DEVICE_RULES: &str = "/etc/udev/rules.d/70-inputforge-devices.rules";
const RULE_MARK: &str = "# inputforge-device ";

/// Vendor:product ids currently granted.
pub fn granted_ids() -> Vec<(u16, u16)> {
    let s = std::fs::read_to_string(DEVICE_RULES).unwrap_or_default();
    s.lines()
        .filter_map(|l| l.strip_prefix(RULE_MARK))
        .filter_map(parse_id)
        .collect()
}

pub fn parse_id(s: &str) -> Option<(u16, u16)> {
    let (v, p) = s.trim().split_once(':')?;
    if v.len() != 4 || p.len() != 4 {
        return None;
    }
    Some((
        u16::from_str_radix(v, 16).ok()?,
        u16::from_str_radix(p, 16).ok()?,
    ))
}

pub fn render_rules(ids: &[(u16, u16)]) -> String {
    let mut s = String::from(
        "# Managed by InputForge (inputforge --udev-allow / --udev-revoke).\n\
# Gives the user at the active local session (uaccess) read access to the input\n\
# event nodes of these devices only, so InputForge can remap them.\n",
    );
    for (v, p) in ids {
        s.push_str(&format!(
            "{RULE_MARK}{v:04x}:{p:04x}\nSUBSYSTEM==\"input\", KERNEL==\"event*\", ATTRS{{id/vendor}}==\"{v:04x}\", ATTRS{{id/product}}==\"{p:04x}\", TAG+=\"uaccess\"\n"
        ));
    }
    s
}

/// Root side: add or remove device ids, rewrite the rules file, re-apply.
pub fn udev_update(add: &[(u16, u16)], remove: &[(u16, u16)]) -> anyhow::Result<Vec<(u16, u16)>> {
    if unsafe { libc::geteuid() } != 0 {
        anyhow::bail!("must run as root (it is normally started through pkexec)");
    }
    let mut ids = granted_ids();
    ids.retain(|i| !remove.contains(i));
    for i in add {
        if !ids.contains(i) {
            ids.push(*i);
        }
    }
    ids.sort_unstable();
    let tmp = format!("{DEVICE_RULES}.tmp");
    std::fs::write(&tmp, render_rules(&ids))?;
    std::fs::rename(&tmp, DEVICE_RULES)?;
    let run = |args: &[&str]| std::process::Command::new("udevadm").args(args).status();
    run(&["control", "--reload-rules"])?;
    run(&["trigger", "--subsystem-match=input", "--action=change"])?;
    run(&["settle", "--timeout=5"])?;
    Ok(ids)
}

/// User side: ask (polkit password prompt) to grant access to `ids`.
pub fn request_access(ids: &[(u16, u16)]) -> anyhow::Result<()> {
    let exe = std::env::current_exe()?;
    let args: Vec<String> = ids
        .iter()
        .map(|(v, p)| format!("{v:04x}:{p:04x}"))
        .collect();
    let st = std::process::Command::new("pkexec")
        .arg(exe)
        .arg("--udev-allow")
        .args(&args)
        .status()?;
    match st.code() {
        Some(0) => Ok(()),
        Some(126) => anyhow::bail!("permission request was cancelled"),
        Some(127) => anyhow::bail!("not authorized"),
        _ => anyhow::bail!("granting access failed ({st})"),
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
            rel: vec![],
            accessible: true,
        }
    }
    #[test]
    fn caps_and_rules() {
        assert_eq!(parse_caps("1943"), vec![0, 1, 6, 8, 11, 12]);
        let k = parse_caps("ffff0000 0 0 0 0");
        assert!(k.contains(&0x110) && k.contains(&0x11f) && !k.contains(&0x120));
        assert_eq!(parse_id("046d:c24a"), Some((0x046d, 0xc24a)));
        assert_eq!(parse_id("046d:c24a\"; RUN+=\"x"), None);
        assert_eq!(parse_id("46d:c24a"), None);
        let r = render_rules(&[(0x046d, 0xc24a)]);
        assert!(
            r.contains("ATTRS{id/vendor}==\"046d\", ATTRS{id/product}==\"c24a\", TAG+=\"uaccess\"")
        );
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
