//! Built-in RGB drivers (no OpenRGB needed). Talks to devices over Linux `hidraw`.
//!
//! Supported:
//!   * Logitech G815 / G813 (wired, 046d:c33f / c232) — per-key direct colors, static,
//!     breathing, color cycle. HID++ long reports (id 0x11) on the 0xFF43 interface.
//!   * Logitech G915 TKL (wired 046d:c343, or through a LIGHTSPEED receiver 046d:c545 / c547; wirelessly the
//!     keyboard itself shows up as 046d:408e) — same HID++ 4522/8071/8081
//!     lighting features as the G815, but on the 0xFF00 interface 2 with feature indices that are
//!     looked up at run time, and no numpad / G-keys. Untested on hardware by the author.
//!   * Logitech G600 (046d:c24a) — static, breathing, cycle via feature report 0xF1.
//!
//! Protocol details follow OpenRGB's GPL drivers (LogitechG815Controller,
//! LogitechG600Controller); this is an independent Rust implementation.

use anyhow::{Context, Result, bail};
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::fs::{File, OpenOptions};
use std::io::{Read, Write};
use std::os::fd::AsRawFd;
use std::os::unix::fs::OpenOptionsExt;
use std::path::{Path, PathBuf};
use std::sync::mpsc::{self, Receiver, Sender};
use std::sync::{Arc, Mutex};
use std::time::Duration;

pub type Rgb = [u8; 3];

/// Device index meaning "a receiver: find which paired slot holds the keyboard".
const RECEIVER_AUTO: u8 = 0;

// ───────────────────────────── settings ─────────────────────────────

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
pub enum KbMode {
    Off,
    #[default]
    Static,
    Breathing,
    Cycle,
    PerKey,
}

impl KbMode {
    pub const ALL: [KbMode; 5] = [
        KbMode::Static,
        KbMode::PerKey,
        KbMode::Breathing,
        KbMode::Cycle,
        KbMode::Off,
    ];
    pub fn label(self) -> &'static str {
        match self {
            KbMode::Off => "Off",
            KbMode::Static => "Solid",
            KbMode::Breathing => "Breathing",
            KbMode::Cycle => "Rainbow",
            KbMode::PerKey => "Per key",
        }
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct KbLighting {
    pub mode: KbMode,
    pub color: Rgb,
    /// Effect period in milliseconds (1000 = fast, 20000 = slow).
    pub period_ms: u32,
    /// Per-key colors, indexed like [`G815_LEDS`].
    pub keys: Vec<Rgb>,
}

impl Default for KbLighting {
    fn default() -> Self {
        Self {
            mode: KbMode::Static,
            color: [0, 160, 255],
            period_ms: 5000,
            keys: vec![[0, 160, 255]; G815_LEDS.len()],
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
pub enum MouseMode {
    #[default]
    Static,
    Breathing,
    Cycle,
    Off,
}

impl MouseMode {
    pub const ALL: [MouseMode; 4] = [
        MouseMode::Static,
        MouseMode::Breathing,
        MouseMode::Cycle,
        MouseMode::Off,
    ];
    pub fn label(self) -> &'static str {
        match self {
            MouseMode::Static => "Solid",
            MouseMode::Breathing => "Breathing",
            MouseMode::Cycle => "Rainbow",
            MouseMode::Off => "Off",
        }
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct MouseLighting {
    pub mode: MouseMode,
    pub color: Rgb,
    /// Effect period in seconds (1 = fast, 15 = slow).
    pub period_s: u8,
}

impl Default for MouseLighting {
    fn default() -> Self {
        Self {
            mode: MouseMode::Static,
            color: [0, 160, 255],
            period_s: 3,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, Default)]
#[serde(default)]
pub struct LightingSettings {
    pub keyboard: KbLighting,
    pub mouse: MouseLighting,
    /// Re-apply saved lighting when InputForge starts.
    pub apply_on_launch: bool,
}

// ───────────────────────────── hidraw ─────────────────────────────

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Model {
    G815,
    G915Tkl,
    G600,
}

impl Model {
    pub fn name(self) -> &'static str {
        match self {
            Model::G815 => "Logitech G815 keyboard",
            Model::G915Tkl => "Logitech G915 TKL keyboard",
            Model::G600 => "Logitech G600 mouse",
        }
    }

    pub fn is_keyboard(self) -> bool {
        matches!(self, Model::G815 | Model::G915Tkl)
    }

    /// Which LEDs physically exist (the TKL has no numpad and no G-keys).
    pub fn has_led(self, led: usize) -> bool {
        match self {
            Model::G915Tkl => !(79..=95).contains(&led) && !(112..=116).contains(&led),
            _ => true,
        }
    }

    /// USB product id -> model, for devices that are lighting-capable by id alone.
    pub fn from_product(vendor: u16, product: u16) -> Option<Model> {
        if vendor != 0x046d {
            return None;
        }
        match product {
            0xc33f | 0xc232 => Some(Model::G815),
            // wired, receivers (c545 / c547), and the keyboard as seen over a receiver (408e)
            0xc343 | 0xc545 | 0xc547 | 0x408e => Some(Model::G915Tkl),
            0xc24a => Some(Model::G600),
            _ => None,
        }
    }
}

#[derive(Debug, Clone)]
pub struct Found {
    pub model: Model,
    /// HID++ device index: 0xFF for a wired keyboard, or `RECEIVER_AUTO` behind a LIGHTSPEED
    /// receiver (the paired slot is found when the device is opened).
    pub dev_index: u8,
    pub path: PathBuf,
    #[allow(dead_code)]
    pub product: String,
    pub accessible: bool,
}

fn has_bytes(desc: &[u8], needle: &[u8]) -> bool {
    desc.windows(needle.len()).any(|w| w == needle)
}

/// Decide whether a hidraw node is a lighting interface we drive.
/// `iface` is the USB interface number, `desc` the HID report descriptor.
/// Returns the model and the HID++ device index to address.
pub fn classify_hidraw(
    vendor: u16,
    product: u16,
    iface: Option<u8>,
    desc: &[u8],
) -> Option<(Model, u8)> {
    let model = Model::from_product(vendor, product)?;
    if product == 0x408e {
        return None; // the paired keyboard itself; we talk to it through its receiver
    }
    match model {
        // Usage page 0xFF43 (HID++ for keyboards) => bytes 06 43 FF
        Model::G815 if has_bytes(desc, &[0x06, 0x43, 0xff]) => Some((model, 0xFF)),
        // Usage page 0xFF80 with feature report 0xF1
        Model::G600 if has_bytes(desc, &[0x06, 0x80, 0xff]) => Some((model, 0xFF)),
        // Vendor page 0xFF00 on interface 2 carrying report id 0x11 (HID++ long).
        Model::G915Tkl
            if iface.is_none_or(|i| i == 2)
                && has_bytes(desc, &[0x06, 0x00, 0xff])
                && has_bytes(desc, &[0x85, 0x11]) =>
        {
            Some((
                model,
                if product == 0xc343 {
                    0xFF
                } else {
                    RECEIVER_AUTO
                },
            ))
        }
        _ => None,
    }
}

/// Find supported lighting devices.
pub fn discover() -> Vec<Found> {
    let mut out = vec![];
    let Ok(rd) = std::fs::read_dir("/sys/class/hidraw") else {
        return out;
    };
    let mut entries: Vec<_> = rd.flatten().map(|e| e.path()).collect();
    entries.sort();
    for dir in entries {
        let Ok(uevent) = std::fs::read_to_string(dir.join("device/uevent")) else {
            continue;
        };
        let get = |k: &str| {
            uevent
                .lines()
                .find_map(|l| l.strip_prefix(k))
                .map(|s| s.to_string())
                .unwrap_or_default()
        };
        // HID_ID=0003:0000046D:0000C33F
        let id: Vec<u32> = get("HID_ID=")
            .split(':')
            .filter_map(|s| u32::from_str_radix(s, 16).ok())
            .collect();
        if id.len() != 3 || id[1] != 0x046d {
            continue;
        }
        let desc = std::fs::read(dir.join("device/report_descriptor")).unwrap_or_default();
        let iface = std::fs::read_to_string(dir.join("device/../bInterfaceNumber"))
            .ok()
            .and_then(|s| u8::from_str_radix(s.trim(), 16).ok());
        let Some((model, dev_index)) = classify_hidraw(id[1] as u16, id[2] as u16, iface, &desc)
        else {
            continue;
        };
        let path = PathBuf::from("/dev").join(dir.file_name().unwrap());
        let accessible = OpenOptions::new()
            .read(true)
            .write(true)
            .open(&path)
            .is_ok();
        out.push(Found {
            model,
            dev_index,
            path,
            product: get("HID_NAME="),
            accessible,
        });
    }
    out
}

struct Hidraw {
    f: File,
}

impl Hidraw {
    fn open(path: &Path) -> Result<Self> {
        let f = OpenOptions::new()
            .read(true)
            .write(true)
            .custom_flags(libc::O_NONBLOCK)
            .open(path)
            .with_context(|| {
                format!(
                    "cannot open {} (permission? run the installer again)",
                    path.display()
                )
            })?;
        Ok(Self { f })
    }

    fn write(&mut self, buf: &[u8]) -> Result<()> {
        self.f.write_all(buf)?;
        Ok(())
    }

    /// Read one input report (if any) within `timeout`.
    fn read_timeout(&mut self, timeout: Duration) -> Option<Vec<u8>> {
        let mut pfd = libc::pollfd {
            fd: self.f.as_raw_fd(),
            events: libc::POLLIN,
            revents: 0,
        };
        if unsafe { libc::poll(&mut pfd, 1, timeout.as_millis() as i32) } <= 0 {
            return None;
        }
        let mut b = [0u8; 64];
        self.f.read(&mut b).ok().map(|n| b[..n].to_vec())
    }

    fn set_feature(&mut self, buf: &[u8]) -> Result<()> {
        // HIDIOCSFEATURE(len) = _IOC(_IOC_WRITE|_IOC_READ, 'H', 0x06, len)
        let req: libc::c_ulong =
            (3 << 30) | ((buf.len() as libc::c_ulong) << 16) | (0x48 << 8) | 0x06;
        let r = unsafe { libc::ioctl(self.f.as_raw_fd(), req as _, buf.as_ptr()) };
        if r < 0 {
            bail!("HIDIOCSFEATURE failed: {}", std::io::Error::last_os_error());
        }
        Ok(())
    }
}

// ───────────────────────────── G815 ─────────────────────────────

const G815_READ_TIMEOUT: Duration = Duration::from_millis(300);
const FRAME_LITTLE: u8 = 0x1F;
const FRAME_BIG: u8 = 0x6F;

#[derive(Clone, Copy)]
pub enum Zone {
    Keyboard,
    Media,
    Logo,
    Indicators,
    GKeys,
    Modifiers,
}

/// (label, zone, protocol index) in OpenRGB's LED order; matrix map indexes into this.
pub const G815_LEDS: [(&str, Zone, u8); 117] = {
    use Zone::*;
    [
        ("A", Keyboard, 0x04),
        ("B", Keyboard, 0x05),
        ("C", Keyboard, 0x06),
        ("D", Keyboard, 0x07),
        ("E", Keyboard, 0x08),
        ("F", Keyboard, 0x09),
        ("G", Keyboard, 0x0A),
        ("H", Keyboard, 0x0B),
        ("I", Keyboard, 0x0C),
        ("J", Keyboard, 0x0D),
        ("K", Keyboard, 0x0E),
        ("L", Keyboard, 0x0F),
        ("M", Keyboard, 0x10),
        ("N", Keyboard, 0x11),
        ("O", Keyboard, 0x12),
        ("P", Keyboard, 0x13),
        ("Q", Keyboard, 0x14),
        ("R", Keyboard, 0x15),
        ("S", Keyboard, 0x16),
        ("T", Keyboard, 0x17),
        ("U", Keyboard, 0x18),
        ("V", Keyboard, 0x19),
        ("W", Keyboard, 0x1A),
        ("X", Keyboard, 0x1B),
        ("Y", Keyboard, 0x1C),
        ("Z", Keyboard, 0x1D),
        ("1", Keyboard, 0x1E),
        ("2", Keyboard, 0x1F),
        ("3", Keyboard, 0x20),
        ("4", Keyboard, 0x21),
        ("5", Keyboard, 0x22),
        ("6", Keyboard, 0x23),
        ("7", Keyboard, 0x24),
        ("8", Keyboard, 0x25),
        ("9", Keyboard, 0x26),
        ("0", Keyboard, 0x27),
        ("Enter", Keyboard, 0x28),
        ("Esc", Keyboard, 0x29),
        ("Bksp", Keyboard, 0x2A),
        ("Tab", Keyboard, 0x2B),
        ("Space", Keyboard, 0x2C),
        ("-", Keyboard, 0x2D),
        ("=", Keyboard, 0x2E),
        ("[", Keyboard, 0x2F),
        ("]", Keyboard, 0x30),
        ("\\", Keyboard, 0x31),
        ("#", Keyboard, 0x32),
        (";", Keyboard, 0x33),
        ("'", Keyboard, 0x34),
        ("`", Keyboard, 0x35),
        (",", Keyboard, 0x36),
        (".", Keyboard, 0x37),
        ("/", Keyboard, 0x38),
        ("Caps", Keyboard, 0x39),
        ("F1", Keyboard, 0x3A),
        ("F2", Keyboard, 0x3B),
        ("F3", Keyboard, 0x3C),
        ("F4", Keyboard, 0x3D),
        ("F5", Keyboard, 0x3E),
        ("F6", Keyboard, 0x3F),
        ("F7", Keyboard, 0x40),
        ("F8", Keyboard, 0x41),
        ("F9", Keyboard, 0x42),
        ("F10", Keyboard, 0x43),
        ("F11", Keyboard, 0x44),
        ("F12", Keyboard, 0x45),
        ("PrtSc", Keyboard, 0x46),
        ("ScrLk", Keyboard, 0x47),
        ("Pause", Keyboard, 0x48),
        ("Ins", Keyboard, 0x49),
        ("Home", Keyboard, 0x4A),
        ("PgUp", Keyboard, 0x4B),
        ("Del", Keyboard, 0x4C),
        ("End", Keyboard, 0x4D),
        ("PgDn", Keyboard, 0x4E),
        ("Right", Keyboard, 0x4F),
        ("Left", Keyboard, 0x50),
        ("Down", Keyboard, 0x51),
        ("Up", Keyboard, 0x52),
        ("Num", Keyboard, 0x53),
        ("/", Keyboard, 0x54),
        ("*", Keyboard, 0x55),
        ("-", Keyboard, 0x56),
        ("+", Keyboard, 0x57),
        ("Ent", Keyboard, 0x58),
        ("1", Keyboard, 0x59),
        ("2", Keyboard, 0x5A),
        ("3", Keyboard, 0x5B),
        ("4", Keyboard, 0x5C),
        ("5", Keyboard, 0x5D),
        ("6", Keyboard, 0x5E),
        ("7", Keyboard, 0x5F),
        ("8", Keyboard, 0x60),
        ("9", Keyboard, 0x61),
        ("0", Keyboard, 0x62),
        (".", Keyboard, 0x63),
        ("\\", Keyboard, 0x64),
        ("Menu", Keyboard, 0x65),
        ("Ctrl", Modifiers, 0xE0),
        ("Shift", Modifiers, 0xE1),
        ("Alt", Modifiers, 0xE2),
        ("Win", Modifiers, 0xE3),
        ("Ctrl", Modifiers, 0xE4),
        ("Shift", Modifiers, 0xE5),
        ("Alt", Modifiers, 0xE6),
        ("Win", Modifiers, 0xE7),
        ("Prev", Media, 0x9E),
        ("Play", Media, 0x9B),
        ("Next", Media, 0x9D),
        ("Mute", Media, 0x9C),
        ("Logo", Logo, 0x01),
        ("Light", Indicators, 0x99),
        ("G1", GKeys, 0x01),
        ("G2", GKeys, 0x02),
        ("G3", GKeys, 0x03),
        ("G4", GKeys, 0x04),
        ("G5", GKeys, 0x05),
    ]
};

/// Map a LED to the key id used in direct-mode frames.
fn g815_frame_key(led: usize) -> u8 {
    let (_, zone, idx) = G815_LEDS[led];
    match zone {
        Zone::GKeys => idx.wrapping_add(0xB3),
        Zone::Modifiers => idx.wrapping_sub(0x78),
        Zone::Keyboard => idx.wrapping_sub(0x03),
        Zone::Logo => idx.wrapping_add(0xD1),
        Zone::Media | Zone::Indicators => idx,
    }
}

/// Build direct-mode frames (each = 16 data bytes + frame type) for the given (led, color) list.
fn g815_frames(changes: &[(usize, Rgb)]) -> Vec<(u8, [u8; 16])> {
    // Group keys by color, preserving first-seen order for determinism.
    let mut order: Vec<Rgb> = vec![];
    let mut by_color: HashMap<Rgb, Vec<u8>> = HashMap::new();
    for &(led, c) in changes {
        by_color.entry(c).or_insert_with(|| {
            order.push(c);
            vec![]
        });
        by_color.get_mut(&c).unwrap().push(g815_frame_key(led));
    }
    let mut frames = vec![];
    let mut little: Vec<(u8, Rgb)> = vec![];
    for c in order {
        let keys = &by_color[&c];
        if keys.len() > 4 {
            // Big frame: one color, up to 13 keys, 0xFF-terminated if short.
            for chunk in keys.chunks(13) {
                let mut d = [0u8; 16];
                d[..3].copy_from_slice(&c);
                d[3..3 + chunk.len()].copy_from_slice(chunk);
                if chunk.len() < 13 {
                    d[3 + chunk.len()] = 0xFF;
                }
                frames.push((FRAME_BIG, d));
            }
        } else {
            little.extend(keys.iter().map(|&k| (k, c)));
        }
    }
    // Little frames: up to 4 × (key, r, g, b), 0xFF-terminated if short.
    for chunk in little.chunks(4) {
        let mut d = [0u8; 16];
        for (i, (k, c)) in chunk.iter().enumerate() {
            d[i * 4] = *k;
            d[i * 4 + 1..i * 4 + 4].copy_from_slice(c);
        }
        if chunk.len() < 4 {
            d[chunk.len() * 4] = 0xFF;
        }
        frames.push((FRAME_LITTLE, d));
    }
    frames
}

/// HID++ long report (id 0x11) addressed to device index `dev`
/// (0xFF = wired device, 1 = first device behind a LIGHTSPEED receiver).
fn hidpp(dev: u8, feature: u8, func: u8) -> [u8; 20] {
    let mut b = [0u8; 20];
    b[0] = 0x11;
    b[1] = dev;
    b[2] = feature;
    b[3] = func;
    b
}

/// Slots (feature indices) of the HID++ features used for lighting. The G815's
/// are fixed; the G915 TKL's are looked up from the device at run time.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct Features {
    /// 0x4522: take/return control of the lighting from the onboard profile.
    ctl: u8,
    /// 0x8071: RGB effects (G915 effects and direct-mode switch).
    fx: u8,
    /// 0x8081: per-key colour frames and commit.
    keys: u8,
}

const G815_FEATURES: Features = Features {
    ctl: 0x08,
    fx: 0x0F,
    keys: 0x10,
};

/// G915 effect packet for one zone (1 = keys, 0 = logo). `mode` uses the key
/// numbering: 2 = breathing, 3 = cycle. The logo numbers those two the other way round.
fn g915_effect_packet(
    dev: u8,
    f: Features,
    zone: u8,
    mode: u8,
    color: Rgb,
    period_ms: u32,
) -> [u8; 20] {
    let ms = period_ms.clamp(1000, 20000) as u16;
    let mode = if zone == 0 {
        match mode {
            2 => 3,
            3 => 2,
            m => m,
        }
    } else {
        mode
    };
    let breathing = if zone == 0 { 3 } else { 2 };
    let cycle = if zone == 0 { 2 } else { 3 };
    let mut p = hidpp(dev, f.fx, 0x1E);
    p[4] = zone;
    p[5] = mode;
    p[6..9].copy_from_slice(&color);
    if mode == breathing {
        p[9] = (ms >> 8) as u8;
        p[10] = ms as u8;
        p[12] = 0x64;
    } else if mode == cycle {
        p[11] = (ms >> 8) as u8;
        p[12] = ms as u8;
        p[13] = 0x64;
    } else if mode == 1 {
        p[9] = 0x02;
    }
    p[16] = 0x01;
    p
}

/// Lighting driver for the G815 family and the G915 TKL (same direct-mode protocol).
struct Keyboard {
    dev: Hidraw,
    model: Model,
    index: u8,
    f: Features,
    direct: bool,
    shown: Vec<Option<Rgb>>,
}

impl Keyboard {
    fn open(found: &Found) -> Result<Self> {
        let mut k = Self {
            dev: Hidraw::open(&found.path)?,
            model: found.model,
            index: found.dev_index,
            f: G815_FEATURES,
            direct: false,
            shown: vec![None; G815_LEDS.len()],
        };
        // Feature slots differ between models and firmware; ask the keyboard. If it can't
        // answer (G815 asleep, say), fall back to the G815's usual slots.
        if k.index == RECEIVER_AUTO {
            k.index = k.find_receiver_slot()?;
        }
        match k.lookup_features() {
            Ok(f) => k.f = f,
            Err(e) if found.model == Model::G915Tkl => return Err(e),
            Err(_) => {}
        }
        Ok(k)
    }

    fn pkt(&self, feature: u8, func: u8) -> [u8; 20] {
        hidpp(self.index, feature, func)
    }

    /// HID++ 2.0 IRoot.GetFeature on slot `idx`: where does feature `id` live?
    /// `Ok(None)` = the device answered but lacks the feature.
    fn feature_index_at(&mut self, idx: u8, id: u16) -> Result<Option<u8>> {
        let mut p = hidpp(idx, 0x00, 0x0E);
        p[4] = (id >> 8) as u8;
        p[5] = id as u8;
        self.dev.write(&p)?;
        let deadline = std::time::Instant::now() + Duration::from_millis(1500);
        while let Some(rem) = deadline.checked_duration_since(std::time::Instant::now()) {
            let Some(r) = self.dev.read_timeout(rem) else {
                break;
            };
            // Only replies addressed to this slot (a second program, e.g. Solaar, may be talking too).
            if r.len() < 5 || r[1] != idx {
                continue;
            }
            if r[0] == 0x11 && r[2] == 0x00 && r[3] == 0x0E {
                return Ok((r[4] != 0).then_some(r[4]));
            }
            // HID++ 2.0 error [11 idx FF 00 0E code] or HID++ 1.0 error [10 idx 8F 00 0E code]
            if (r[2] == 0xFF || r[2] == 0x8F) && r.len() >= 6 && r[3] == 0x00 {
                bail!("HID++ error {:#04x} from slot {idx}", r[5]);
            }
        }
        bail!("no answer from slot {idx} (keyboard switched off, asleep or out of range?)")
    }

    fn feature_index(&mut self, id: u16) -> Result<Option<u8>> {
        self.feature_index_at(self.index, id)
    }

    /// Behind a LIGHTSPEED receiver: which paired slot (1..=6) is a per-key-lit keyboard?
    fn find_receiver_slot(&mut self) -> Result<u8> {
        let mut seen = vec![];
        for slot in 1..=6u8 {
            match self.feature_index_at(slot, 0x8081) {
                Ok(Some(_)) => return Ok(slot),
                Ok(None) => seen.push(format!("slot {slot}: paired, no per-key lighting")),
                Err(_) => {}
            }
        }
        if seen.is_empty() {
            bail!(
                "the receiver has no keyboard answering (is the keyboard switched on and \
connected by LIGHTSPEED, not Bluetooth?)"
            )
        }
        bail!(
            "no paired device with per-key lighting ({})",
            seen.join("; ")
        )
    }

    fn lookup_features(&mut self) -> Result<Features> {
        let mut need = |id: u16, what: &str| -> Result<u8> {
            self.feature_index(id)?.with_context(|| {
                format!(
                    "this keyboard has no HID++ {what} feature ({id:#06x}); \
it may be a model InputForge doesn't support yet"
                )
            })
        };
        Ok(Features {
            ctl: need(0x4522, "lighting-control")?,
            fx: need(0x8071, "RGB-effects")?,
            keys: need(0x8081, "per-key-lighting")?,
        })
    }

    fn send(&mut self, pkt: [u8; 20]) -> Result<()> {
        self.dev.write(&pkt)?;
        // Wait for the matching acknowledgement; HID++ 2.0 errors come back as
        // [0x11, dev, 0xFF, feature, func, code].
        let deadline = std::time::Instant::now() + G815_READ_TIMEOUT;
        while let Some(rem) = deadline.checked_duration_since(std::time::Instant::now()) {
            let Some(r) = self.dev.read_timeout(rem) else {
                break;
            };
            if r.len() >= 6 && r[0] == 0x11 && r[2] == 0xFF && r[3] == pkt[2] && r[4] == pkt[3] {
                bail!(
                    "device rejected command {:02x}:{:02x} (error {:#04x})",
                    pkt[2],
                    pkt[3],
                    r[5]
                );
            }
            if r.len() >= 4 && r[0] == 0x11 && r[2] == pkt[2] && r[3] == pkt[3] {
                return Ok(());
            }
        }
        Ok(())
    }

    fn commit(&mut self) -> Result<()> {
        self.send(self.pkt(self.f.keys, 0x7F))
    }

    fn init_direct(&mut self) -> Result<()> {
        self.send(self.pkt(self.f.ctl, 0x3E))?;
        self.send(self.pkt(self.f.ctl, 0x1E))?;
        let mut p = self.pkt(self.f.fx, 0x1E);
        p[0x10] = 0x01;
        self.send(p)?;
        let mut p = self.pkt(self.f.fx, 0x1E);
        p[0x04] = 0x01;
        p[0x10] = 0x01;
        self.send(p)?;
        self.direct = true;
        self.shown = vec![None; G815_LEDS.len()];
        Ok(())
    }

    fn set_leds(&mut self, colors: &[Rgb]) -> Result<()> {
        if !self.direct {
            self.init_direct()?;
        }
        let changes: Vec<(usize, Rgb)> = colors
            .iter()
            .enumerate()
            .take(G815_LEDS.len())
            .filter(|(i, _)| self.model.has_led(*i))
            .filter(|(i, c)| self.shown[*i] != Some(**c))
            .map(|(i, c)| (i, *c))
            .collect();
        if changes.is_empty() {
            return Ok(());
        }
        for (ty, data) in g815_frames(&changes) {
            let mut p = self.pkt(self.f.keys, ty);
            p[4..20].copy_from_slice(&data);
            self.send(p)?;
        }
        self.commit()?;
        for (i, c) in changes {
            self.shown[i] = Some(c);
        }
        Ok(())
    }

    /// Onboard effect (2 = breathing, 3 = cycle) on keyboard + logo zones.
    fn set_effect(&mut self, mode: u8, color: Rgb, period_ms: u32) -> Result<()> {
        // OpenRGB's mode-set preamble, then one packet per zone (keys, logo).
        let mut p = self.pkt(self.f.fx, 0x5E);
        p[4] = 0x01;
        p[5] = 0x03;
        p[6] = 0x07;
        self.send(p)?;
        self.send(self.pkt(self.f.ctl, 0x3E))?;
        self.send(self.pkt(self.f.ctl, 0x1E))?;
        for zone in [1u8, 0u8] {
            self.send(g915_effect_packet(
                self.index, self.f, zone, mode, color, period_ms,
            ))?;
        }
        self.direct = false;
        Ok(())
    }

    fn apply(&mut self, s: &KbLighting) -> Result<()> {
        match s.mode {
            KbMode::Off => self.set_leds(&vec![[0, 0, 0]; G815_LEDS.len()]),
            KbMode::Static => self.set_leds(&vec![s.color; G815_LEDS.len()]),
            KbMode::PerKey => {
                let mut keys = s.keys.clone();
                keys.resize(G815_LEDS.len(), s.color);
                self.set_leds(&keys)
            }
            KbMode::Breathing => self.set_effect(2, s.color, s.period_ms),
            KbMode::Cycle => self.set_effect(3, s.color, s.period_ms),
        }
    }
}

// ───────────────────────────── G600 ─────────────────────────────

fn g600_apply(path: &Path, s: &MouseLighting) -> Result<()> {
    let mut dev = Hidraw::open(path)?;
    let (mode, color) = match s.mode {
        MouseMode::Static => (0u8, s.color),
        MouseMode::Breathing => (1, s.color),
        MouseMode::Cycle => (2, s.color),
        MouseMode::Off => (0, [0, 0, 0]),
    };
    let buf = [
        0xF1,
        color[0],
        color[1],
        color[2],
        mode,
        s.period_s.clamp(1, 15),
        0,
        0,
    ];
    dev.set_feature(&buf)
}

// ───────────────────────────── worker ─────────────────────────────

enum Job {
    Keyboard(KbLighting),
    Mouse(MouseLighting),
}

/// Background writer: coalesces rapid UI changes and keeps device handles open.
pub struct Lighting {
    tx: Sender<Job>,
    pub status: Arc<Mutex<String>>,
}

impl Lighting {
    pub fn new(repaint: impl Fn() + Send + 'static) -> Self {
        let (tx, rx) = mpsc::channel();
        let status = Arc::new(Mutex::new(String::new()));
        let st = status.clone();
        std::thread::Builder::new()
            .name("lighting".into())
            .spawn(move || worker(rx, st, repaint))
            .expect("spawn lighting worker");
        Self { tx, status }
    }

    pub fn apply_keyboard(&self, s: &KbLighting) {
        let _ = self.tx.send(Job::Keyboard(s.clone()));
    }

    pub fn apply_mouse(&self, s: &MouseLighting) {
        let _ = self.tx.send(Job::Mouse(s.clone()));
    }
}

fn worker(rx: Receiver<Job>, status: Arc<Mutex<String>>, repaint: impl Fn()) {
    let mut kb: Option<Keyboard> = None;
    while let Ok(first) = rx.recv() {
        // Coalesce: keep only the latest job per device.
        let (mut kb_job, mut mouse_job) = (None, None);
        let mut take = |j: Job| match j {
            Job::Keyboard(s) => kb_job = Some(s),
            Job::Mouse(s) => mouse_job = Some(s),
        };
        take(first);
        std::thread::sleep(Duration::from_millis(15));
        while let Ok(j) = rx.try_recv() {
            take(j);
        }
        let found = discover();
        let mut msgs = vec![];
        if let Some(s) = kb_job {
            let r = (|| -> Result<()> {
                let f = found
                    .iter()
                    .find(|f| f.model.is_keyboard())
                    .context("no supported keyboard connected")?;
                if kb.as_ref().is_some_and(|k| k.model != f.model) {
                    kb = None;
                }
                if kb.is_none() {
                    kb = Some(Keyboard::open(f)?);
                }
                let res = kb.as_mut().unwrap().apply(&s);
                if res.is_err() {
                    kb = None; // reopen next time (unplug/replug)
                }
                res
            })();
            msgs.push(match r {
                Ok(()) => "✔ keyboard updated".to_string(),
                Err(e) => format!("✖ keyboard: {e}"),
            });
        }
        if let Some(s) = mouse_job {
            let r = found
                .iter()
                .find(|f| f.model == Model::G600)
                .context("G600 not connected")
                .and_then(|f| g600_apply(&f.path, &s));
            msgs.push(match r {
                Ok(()) => "✔ mouse updated".to_string(),
                Err(e) => format!("✖ mouse: {e}"),
            });
        }
        *status.lock().unwrap() = msgs.join("   ");
        repaint();
    }
}

/// Read-only HID++ probe for `--lighting-info` (IRoot.GetFeature only; changes nothing).
fn probe_keyboard(node: &Path, idx: u8) -> String {
    let found = Found {
        model: Model::G915Tkl,
        dev_index: idx,
        path: node.to_path_buf(),
        product: String::new(),
        accessible: true,
    };
    let mut k = match Hidraw::open(&found.path) {
        Ok(dev) => Keyboard {
            dev,
            model: found.model,
            index: idx,
            f: G815_FEATURES,
            direct: false,
            shown: vec![],
        },
        Err(e) => return format!("cannot open: {e}"),
    };
    let mut lines = vec![];
    let slots: Vec<u8> = if idx == RECEIVER_AUTO {
        (1..=6).collect()
    } else {
        vec![idx]
    };
    for slot in slots {
        let mut feats = vec![];
        let mut err = None;
        for (name, id) in [("4522", 0x4522u16), ("8071", 0x8071), ("8081", 0x8081)] {
            match k.feature_index_at(slot, id) {
                Ok(Some(i)) => feats.push(format!("{name}@{i:#04x}")),
                Ok(None) => feats.push(format!("{name}=missing")),
                Err(e) => {
                    err = Some(e.to_string());
                    break;
                }
            }
        }
        match err {
            None => lines.push(format!("slot {slot:#04x}: {}", feats.join(" "))),
            Some(e) if idx != RECEIVER_AUTO => lines.push(format!("slot {slot:#04x}: {e}")),
            Some(_) => {} // empty receiver slots simply don't answer
        }
    }
    if lines.is_empty() {
        "no paired device answered".into()
    } else {
        lines.join(" | ")
    }
}

/// Diagnostics for bug reports (`inputforge --lighting-info`): every Logitech hidraw node, how it
/// was classified and whether this user can open it.
pub fn describe_hidraw() -> Vec<String> {
    let mut out = vec![];
    let Ok(rd) = std::fs::read_dir("/sys/class/hidraw") else {
        return vec!["no /sys/class/hidraw".into()];
    };
    let mut entries: Vec<_> = rd.flatten().map(|e| e.path()).collect();
    entries.sort();
    for dir in entries {
        let uevent = std::fs::read_to_string(dir.join("device/uevent")).unwrap_or_default();
        let get = |k: &str| {
            uevent
                .lines()
                .find_map(|l| l.strip_prefix(k))
                .unwrap_or("")
                .to_string()
        };
        let id: Vec<u32> = get("HID_ID=")
            .split(':')
            .filter_map(|s| u32::from_str_radix(s, 16).ok())
            .collect();
        if id.len() != 3 || id[1] != 0x046d {
            continue;
        }
        let desc = std::fs::read(dir.join("device/report_descriptor")).unwrap_or_default();
        let iface = std::fs::read_to_string(dir.join("device/../bInterfaceNumber"))
            .ok()
            .and_then(|s| u8::from_str_radix(s.trim(), 16).ok());
        let node = PathBuf::from("/dev").join(dir.file_name().unwrap());
        let open = OpenOptions::new()
            .read(true)
            .write(true)
            .open(&node)
            .is_ok();
        let class = match classify_hidraw(id[1] as u16, id[2] as u16, iface, &desc) {
            Some((m, i)) => format!("{} (hid++ index {i:#04x})", m.name()),
            None => "not a lighting interface".into(),
        };
        let head: String = desc
            .iter()
            .take(12)
            .map(|b| format!("{b:02x}"))
            .collect::<Vec<_>>()
            .join(" ");
        let probe = match classify_hidraw(id[1] as u16, id[2] as u16, iface, &desc) {
            Some((Model::G915Tkl, idx)) if open => Some(probe_keyboard(&node, idx)),
            _ => None,
        };
        out.push(format!(
            "{} {:04x}:{:04x} iface={} {}  {}  descriptor[{}B]: {head}",
            node.display(),
            id[1],
            id[2],
            iface.map_or("?".into(), |i| i.to_string()),
            if open { "access" } else { "NO-ACCESS" },
            class,
            desc.len(),
        ));
        if let Some(p) = probe {
            out.push(format!("    probe: {p}"));
        }
    }
    if out.is_empty() {
        out.push("no Logitech hidraw devices found".into());
    }
    out
}

/// Apply saved settings synchronously (for `--apply-lighting` at login).
pub fn apply_now(s: &LightingSettings) -> Vec<String> {
    let mut out = vec![];
    for f in discover() {
        let r = match f.model {
            Model::G815 | Model::G915Tkl => {
                Keyboard::open(&f).and_then(|mut k| k.apply(&s.keyboard))
            }
            Model::G600 => g600_apply(&f.path, &s.mouse),
        };
        out.push(match r {
            Ok(()) => format!("{}: ok", f.model.name()),
            Err(e) => format!("{}: {e}", f.model.name()),
        });
    }
    out
}

pub fn autostart_path() -> PathBuf {
    dirs::config_dir()
        .unwrap_or_default()
        .join("autostart/inputforge-lighting.desktop")
}

pub fn set_autostart(on: bool) -> Result<()> {
    let p = autostart_path();
    if on {
        let exe = std::env::current_exe()?;
        std::fs::create_dir_all(p.parent().unwrap())?;
        std::fs::write(
            &p,
            format!(
                "[Desktop Entry]\nType=Application\nName=InputForge lighting\nComment=Restore keyboard/mouse lighting\nExec={} --apply-lighting\nNoDisplay=true\nX-KDE-autostart-phase=2\n",
                exe.display()
            ),
        )?;
    } else if p.exists() {
        std::fs::remove_file(p)?;
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    // A wired G915 TKL: interface 2, vendor page 0xFF00, HID++ long report id 0x11.
    const TKL_DESC: &[u8] = &[0x06, 0x00, 0xff, 0x09, 0x02, 0xa1, 0x01, 0x85, 0x11];

    #[test]
    fn classifies_g915_tkl_wired_and_receiver() {
        assert_eq!(
            classify_hidraw(0x046d, 0xc343, Some(2), TKL_DESC),
            Some((Model::G915Tkl, 0xFF))
        );
        assert_eq!(
            classify_hidraw(0x046d, 0xc545, Some(2), TKL_DESC),
            Some((Model::G915Tkl, RECEIVER_AUTO))
        );
        assert_eq!(
            classify_hidraw(0x046d, 0xc547, Some(2), TKL_DESC),
            Some((Model::G915Tkl, RECEIVER_AUTO))
        );
        // the keyboard's own wireless id is for the UI only, never a hidraw lighting node
        assert_eq!(classify_hidraw(0x046d, 0x408e, Some(2), TKL_DESC), None);
        assert_eq!(Model::from_product(0x046d, 0x408e), Some(Model::G915Tkl));
        // the typing interface (no vendor HID++ descriptor) and other interfaces are not ours
        assert_eq!(classify_hidraw(0x046d, 0xc343, Some(1), TKL_DESC), None);
        assert_eq!(
            classify_hidraw(0x046d, 0xc343, Some(2), &[0x05, 0x01, 0x09, 0x06]),
            None
        );
        assert_eq!(classify_hidraw(0x1234, 0xc343, Some(2), TKL_DESC), None);
        // G815 and G600 unchanged
        assert_eq!(
            classify_hidraw(0x046d, 0xc33f, Some(1), &[0x06, 0x43, 0xff]),
            Some((Model::G815, 0xFF))
        );
        assert_eq!(
            classify_hidraw(0x046d, 0xc24a, Some(1), &[0x06, 0x80, 0xff]),
            Some((Model::G600, 0xFF))
        );
    }

    #[test]
    fn tkl_skips_numpad_and_gkeys() {
        for led in 0..G815_LEDS.len() {
            let numpad_or_g = (79..=95).contains(&led) || (112..=116).contains(&led);
            assert_eq!(Model::G915Tkl.has_led(led), !numpad_or_g, "led {led}");
            assert!(Model::G815.has_led(led));
        }
        // numpad = HID usages 0x53..=0x63, G-keys = the GKeys zone
        for (i, (_, zone, code)) in G815_LEDS.iter().enumerate() {
            let numpad = matches!(zone, Zone::Keyboard) && (0x53..=0x63).contains(code);
            let gkey = matches!(zone, Zone::GKeys);
            assert_eq!(Model::G915Tkl.has_led(i), !(numpad || gkey), "led {i}");
        }
    }

    #[test]
    fn g915_effect_packets_match_openrgb() {
        let f = Features {
            ctl: 0x0E,
            fx: 0x09,
            keys: 0x0A,
        };
        // keys, breathing, 3 s
        let p = g915_effect_packet(0xFF, f, 1, 2, [1, 2, 3], 3000);
        assert_eq!(&p[..9], &[0x11, 0xFF, 0x09, 0x1E, 1, 2, 1, 2, 3]);
        assert_eq!((p[9], p[10], p[12]), (0x0B, 0xB8, 0x64));
        // keys, cycle
        let p = g915_effect_packet(0xFF, f, 1, 3, [0, 0, 0], 3000);
        assert_eq!((p[5], p[11], p[12], p[13]), (3, 0x0B, 0xB8, 0x64));
        // logo uses the opposite numbering: breathing = 3, cycle = 2
        assert_eq!(g915_effect_packet(0xFF, f, 0, 2, [0; 3], 3000)[5], 3);
        assert_eq!(g915_effect_packet(0xFF, f, 0, 3, [0; 3], 3000)[5], 2);
        // behind a receiver the packet is addressed to device 1
        assert_eq!(g915_effect_packet(1, f, 1, 2, [0; 3], 3000)[1], 1);
    }

    #[test]
    fn frame_key_mapping() {
        assert_eq!(g815_frame_key(37), 0x26); // Esc 0x29 - 3
        assert_eq!(g815_frame_key(98), 0x68); // LCtrl 0xE0 - 0x78
        assert_eq!(g815_frame_key(110), 0xD2); // Logo 0x01 + 0xD1
        assert_eq!(g815_frame_key(112), 0xB4); // G1 0x01 + 0xB3
        assert_eq!(g815_frame_key(107), 0x9B); // play/pause raw
    }

    #[test]
    fn frames_big_and_little() {
        // 6 keys same color → one big frame, terminated.
        let ch: Vec<(usize, Rgb)> = (0..6).map(|i| (i, [1, 2, 3])).collect();
        let f = g815_frames(&ch);
        assert_eq!(f.len(), 1);
        assert_eq!(f[0].0, FRAME_BIG);
        assert_eq!(&f[0].1[..3], &[1, 2, 3]);
        assert_eq!(&f[0].1[3..10], &[0x01, 0x02, 0x03, 0x04, 0x05, 0x06, 0xFF]);
        // 2 keys with distinct colors → one little frame with terminator.
        let f = g815_frames(&[(0, [9, 9, 9]), (1, [8, 8, 8])]);
        assert_eq!(f.len(), 1);
        assert_eq!(f[0].0, FRAME_LITTLE);
        assert_eq!(&f[0].1[..9], &[0x01, 9, 9, 9, 0x02, 8, 8, 8, 0xFF]);
        // 117 keys one color → ceil(117/13) = 9 big frames
        let all: Vec<(usize, Rgb)> = (0..117).map(|i| (i, [5, 5, 5])).collect();
        assert_eq!(g815_frames(&all).len(), 9);
    }
}
