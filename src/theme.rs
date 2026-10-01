//! Follow the desktop environment's look: colors, light/dark mode, accent, and UI font.
//!
//! Sources, in priority order:
//!  1. KDE Plasma color scheme + fonts (`~/.config/kdeglobals`)
//!  2. GTK / libadwaita colors (`~/.config/gtk-{4,3}.0/colors.css|gtk.css`, theme's gtk.css)
//!  3. XDG desktop portal `org.freedesktop.appearance` (color-scheme + accent) — works on
//!     GNOME, Hyprland (xdg-desktop-portal-gtk/hyprland), COSMIC, etc.
//!
//! A background watcher re-detects every 2 s and the UI re-applies when something changes,
//! so switching the system theme updates InputForge live.

use eframe::egui::{
    self, Color32, CornerRadius, FontData, FontDefinitions, FontFamily, FontId, Stroke, TextStyle,
    Visuals,
};
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};
use std::time::{Duration, SystemTime};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
pub enum ThemeMode {
    /// Follow the desktop environment.
    #[default]
    System,
    Dark,
    Light,
}

impl ThemeMode {
    pub fn label(self) -> &'static str {
        match self {
            ThemeMode::System => "System",
            ThemeMode::Dark => "Dark",
            ThemeMode::Light => "Light",
        }
    }
}

/// A full palette derived from the desktop color scheme.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Palette {
    pub dark: bool,
    pub window_bg: Color32,
    pub window_fg: Color32,
    pub view_bg: Color32,
    pub view_alt: Color32,
    pub button_bg: Color32,
    pub button_fg: Color32,
    pub selection_bg: Color32,
    pub selection_fg: Color32,
    pub hover: Color32,
    pub focus: Color32,
    pub inactive_fg: Color32,
    pub link: Color32,
    pub positive: Color32,
    pub neutral: Color32,
    pub negative: Color32,
}

#[derive(Debug, Clone, PartialEq)]
pub struct FontSpec {
    pub family: String,
    pub points: f32,
}

#[derive(Debug, Clone, PartialEq, Default)]
pub struct SystemTheme {
    /// Where the palette came from (shown in the UI).
    pub source: String,
    pub palette: Option<Palette>,
    /// Portal preference when no full palette is available: Some(true)=dark.
    pub prefers_dark: Option<bool>,
    pub accent: Option<Color32>,
    pub ui_font: Option<FontSpec>,
    pub mono_font: Option<FontSpec>,
}

/// Semantic colors the app uses for status text.
#[derive(Debug, Clone, Copy)]
pub struct Semantic {
    pub positive: Color32,
    pub negative: Color32,
    pub warning: Color32,
    pub muted: Color32,
    pub accent: Color32,
}

impl Default for Semantic {
    fn default() -> Self {
        Self {
            positive: Color32::LIGHT_GREEN,
            negative: Color32::LIGHT_RED,
            warning: Color32::YELLOW,
            muted: Color32::GRAY,
            accent: Color32::LIGHT_BLUE,
        }
    }
}

// ───────────────────────────── parsing helpers ─────────────────────────────

fn mix(a: Color32, b: Color32, t: f32) -> Color32 {
    let l = |x: u8, y: u8| (x as f32 + (y as f32 - x as f32) * t).round() as u8;
    Color32::from_rgb(l(a.r(), b.r()), l(a.g(), b.g()), l(a.b(), b.b()))
}

fn luminance(c: Color32) -> f32 {
    (0.2126 * c.r() as f32 + 0.7152 * c.g() as f32 + 0.0722 * c.b() as f32) / 255.0
}

fn parse_kde_color(s: &str) -> Option<Color32> {
    let p: Vec<u8> = s.split(',').filter_map(|x| x.trim().parse().ok()).collect();
    (p.len() >= 3).then(|| Color32::from_rgb(p[0], p[1], p[2]))
}

fn parse_css_color(s: &str) -> Option<Color32> {
    let s = s.trim().trim_end_matches(';').trim();
    if let Some(h) = s.strip_prefix('#') {
        let h = h.trim();
        let v = |i: usize, n: usize| u8::from_str_radix(&h[i..i + n], 16).ok();
        return match h.len() {
            3 | 4 => Some(Color32::from_rgb(
                v(0, 1)? * 17,
                v(1, 1)? * 17,
                v(2, 1)? * 17,
            )),
            6 | 8 => Some(Color32::from_rgb(v(0, 2)?, v(2, 2)?, v(4, 2)?)),
            _ => None,
        };
    }
    if let Some(inner) = s.strip_prefix("rgba(").or_else(|| s.strip_prefix("rgb(")) {
        let p: Vec<f32> = inner
            .trim_end_matches(')')
            .split(',')
            .filter_map(|x| x.trim().trim_end_matches('%').parse().ok())
            .collect();
        if p.len() >= 3 {
            return Some(Color32::from_rgb(p[0] as u8, p[1] as u8, p[2] as u8));
        }
    }
    match s {
        "white" => Some(Color32::WHITE),
        "black" => Some(Color32::BLACK),
        _ => None,
    }
}

type Ini = HashMap<String, HashMap<String, String>>;

fn parse_ini(text: &str) -> Ini {
    let mut out: Ini = HashMap::new();
    let mut section = String::new();
    for line in text.lines() {
        let line = line.trim();
        if line.starts_with('[') && line.ends_with(']') {
            section = line[1..line.len() - 1].to_string();
        } else if let Some((k, v)) = line.split_once('=') {
            out.entry(section.clone())
                .or_default()
                .insert(k.trim().to_string(), v.trim().to_string());
        }
    }
    out
}

/// "Noto Sans,10,-1,5,400,..." (Qt) or "Noto Sans,  10" / "Cantarell 11" (GTK).
fn parse_font(s: &str) -> Option<FontSpec> {
    let s = s.trim().trim_matches(|c| c == '"' || c == '\'');
    if s.is_empty() {
        return None;
    }
    if let Some((fam, rest)) = s.split_once(',') {
        let pts = rest.split(',').next()?.trim().parse::<f32>().ok()?;
        return Some(FontSpec {
            family: fam.trim().to_string(),
            points: pts,
        });
    }
    let (fam, size) = s.rsplit_once(' ')?;
    Some(FontSpec {
        family: fam.trim().to_string(),
        points: size.parse().ok()?,
    })
}

fn config_dir() -> PathBuf {
    dirs::config_dir().unwrap_or_else(|| PathBuf::from("~/.config"))
}

// ───────────────────────────── KDE ─────────────────────────────

fn kdeglobals_path() -> PathBuf {
    config_dir().join("kdeglobals")
}

fn detect_kde() -> Option<SystemTheme> {
    let ini = parse_ini(&std::fs::read_to_string(kdeglobals_path()).ok()?);
    let c = |sec: &str, key: &str| {
        ini.get(sec)
            .and_then(|s| s.get(key))
            .and_then(|v| parse_kde_color(v))
    };
    let window_bg = c("Colors:Window", "BackgroundNormal")?;
    let window_fg = c("Colors:Window", "ForegroundNormal")?;
    let view_bg = c("Colors:View", "BackgroundNormal").unwrap_or(window_bg);
    let accent = ini
        .get("General")
        .and_then(|g| g.get("AccentColor"))
        .and_then(|v| parse_kde_color(v));
    let selection_bg = c("Colors:Selection", "BackgroundNormal")
        .or(accent)
        .unwrap_or(window_fg);
    let palette = Palette {
        dark: luminance(window_bg) < 0.5,
        window_bg,
        window_fg,
        view_bg,
        view_alt: c("Colors:View", "BackgroundAlternate").unwrap_or(mix(view_bg, window_fg, 0.05)),
        button_bg: c("Colors:Button", "BackgroundNormal").unwrap_or(view_bg),
        button_fg: c("Colors:Button", "ForegroundNormal").unwrap_or(window_fg),
        selection_bg,
        selection_fg: c("Colors:Selection", "ForegroundNormal").unwrap_or(window_fg),
        hover: c("Colors:Button", "DecorationHover")
            .or(accent)
            .unwrap_or(selection_bg),
        focus: c("Colors:Button", "DecorationFocus")
            .or(accent)
            .unwrap_or(selection_bg),
        inactive_fg: c("Colors:Window", "ForegroundInactive")
            .unwrap_or(mix(window_fg, window_bg, 0.45)),
        link: c("Colors:Window", "ForegroundLink").unwrap_or(selection_bg),
        positive: c("Colors:Window", "ForegroundPositive").unwrap_or(Color32::LIGHT_GREEN),
        neutral: c("Colors:Window", "ForegroundNeutral").unwrap_or(Color32::YELLOW),
        negative: c("Colors:Window", "ForegroundNegative").unwrap_or(Color32::LIGHT_RED),
    };
    let general = ini.get("General");
    let scheme = general.and_then(|g| g.get("ColorScheme")).cloned();
    let lnf = ini
        .get("KDE")
        .and_then(|g| g.get("LookAndFeelPackage"))
        .cloned();
    Some(SystemTheme {
        source: format!(
            "KDE Plasma ({})",
            scheme.or(lnf).unwrap_or_else(|| "custom scheme".into())
        ),
        palette: Some(palette),
        prefers_dark: Some(palette.dark),
        accent,
        ui_font: general
            .and_then(|g| g.get("font"))
            .and_then(|f| parse_font(f))
            .or(Some(FontSpec {
                family: "Noto Sans".into(),
                points: 10.0,
            })),
        mono_font: general
            .and_then(|g| g.get("fixed"))
            .and_then(|f| parse_font(f))
            .or(Some(FontSpec {
                family: "monospace".into(),
                points: 10.0,
            })),
    })
}

// ───────────────────────────── GTK ─────────────────────────────

fn gtk_settings() -> Ini {
    for v in ["gtk-4.0", "gtk-3.0"] {
        if let Ok(t) = std::fs::read_to_string(config_dir().join(v).join("settings.ini")) {
            return parse_ini(&t);
        }
    }
    Ini::new()
}

fn gtk_css_files() -> Vec<PathBuf> {
    let mut v = vec![];
    for ver in ["gtk-4.0", "gtk-3.0"] {
        for f in ["colors.css", "gtk.css"] {
            v.push(config_dir().join(ver).join(f));
        }
    }
    if let Some(name) = gtk_settings()
        .get("Settings")
        .and_then(|s| s.get("gtk-theme-name"))
        .cloned()
    {
        let home = dirs::home_dir().unwrap_or_default();
        for base in [
            home.join(".themes"),
            home.join(".local/share/themes"),
            PathBuf::from("/usr/share/themes"),
        ] {
            for ver in ["gtk-4.0", "gtk-3.0"] {
                v.push(base.join(&name).join(ver).join("gtk.css"));
                v.push(base.join(&name).join(ver).join("gtk-dark.css"));
            }
        }
    }
    v
}

fn parse_define_colors(text: &str, out: &mut HashMap<String, String>) {
    for line in text.lines() {
        let l = line.trim();
        if let Some(rest) = l.strip_prefix("@define-color") {
            let rest = rest.trim();
            if let Some((name, val)) = rest.split_once(char::is_whitespace) {
                let name = name.trim().trim_end_matches("_breeze").to_string();
                out.entry(name)
                    .or_insert_with(|| val.trim().trim_end_matches(';').to_string());
            }
        }
    }
}

fn resolve(defs: &HashMap<String, String>, name: &str) -> Option<Color32> {
    let mut v = defs.get(name)?.clone();
    for _ in 0..8 {
        if let Some(r) = v.strip_prefix('@') {
            v = defs.get(r.trim().trim_end_matches("_breeze"))?.clone();
        } else {
            return parse_css_color(&v);
        }
    }
    None
}

fn detect_gtk() -> Option<SystemTheme> {
    let mut defs = HashMap::new();
    let mut used = None;
    for f in gtk_css_files() {
        if let Ok(t) = std::fs::read_to_string(&f) {
            let before = defs.len();
            parse_define_colors(&t, &mut defs);
            if defs.len() > before && used.is_none() {
                used = Some(f);
            }
        }
    }
    let r = |names: &[&str]| names.iter().find_map(|n| resolve(&defs, n));
    let window_bg = r(&["window_bg_color", "theme_bg_color", "bg_color"])?;
    let window_fg = r(&["window_fg_color", "theme_fg_color", "fg_color"])?;
    let view_bg = r(&[
        "view_bg_color",
        "theme_base_color",
        "base_color",
        "content_view_bg",
    ])
    .unwrap_or(window_bg);
    let accent = r(&[
        "accent_bg_color",
        "accent_color",
        "theme_selected_bg_color",
        "selected_bg_color",
    ]);
    let sel = accent.unwrap_or(window_fg);
    let palette = Palette {
        dark: luminance(window_bg) < 0.5,
        window_bg,
        window_fg,
        view_bg,
        view_alt: mix(view_bg, window_fg, 0.04),
        button_bg: r(&[
            "theme_button_background_normal",
            "headerbar_bg_color",
            "card_bg_color",
        ])
        .unwrap_or(mix(window_bg, window_fg, 0.08)),
        button_fg: r(&["theme_button_foreground_normal"]).unwrap_or(window_fg),
        selection_bg: sel,
        selection_fg: r(&[
            "accent_fg_color",
            "theme_selected_fg_color",
            "selected_fg_color",
        ])
        .unwrap_or(Color32::WHITE),
        hover: r(&[
            "theme_button_decoration_hover",
            "theme_hovering_selected_bg_color",
        ])
        .unwrap_or(sel),
        focus: r(&["theme_button_decoration_focus"]).unwrap_or(sel),
        inactive_fg: r(&["insensitive_fg_color", "theme_unfocused_fg_color"])
            .unwrap_or(mix(window_fg, window_bg, 0.45)),
        link: r(&["link_color"]).unwrap_or(sel),
        positive: r(&["success_color"]).unwrap_or(Color32::LIGHT_GREEN),
        neutral: r(&["warning_color"]).unwrap_or(Color32::YELLOW),
        negative: r(&["error_color", "destructive_color"]).unwrap_or(Color32::LIGHT_RED),
    };
    let settings = gtk_settings();
    let font = settings
        .get("Settings")
        .and_then(|s| s.get("gtk-font-name"))
        .and_then(|f| parse_font(f))
        .or_else(gsettings_font);
    Some(SystemTheme {
        source: format!(
            "GTK ({})",
            used.map(|p| p.display().to_string()).unwrap_or_default()
        ),
        palette: Some(palette),
        prefers_dark: Some(palette.dark),
        accent,
        ui_font: font,
        mono_font: gsettings_get("monospace-font-name").and_then(|f| parse_font(&f)),
    })
}

fn gsettings_get(key: &str) -> Option<String> {
    let out = std::process::Command::new("gsettings")
        .args(["get", "org.gnome.desktop.interface", key])
        .output()
        .ok()?;
    out.status.success().then(|| {
        String::from_utf8_lossy(&out.stdout)
            .trim()
            .trim_matches('\'')
            .to_string()
    })
}

fn gsettings_font() -> Option<FontSpec> {
    gsettings_get("font-name").and_then(|f| parse_font(&f))
}

// ───────────────────────────── XDG portal ─────────────────────────────

struct Portal {
    conn: zbus::blocking::Connection,
}

impl Portal {
    fn new() -> Option<Self> {
        Some(Self {
            conn: zbus::blocking::Connection::session().ok()?,
        })
    }

    fn read(&self, key: &str) -> Option<zbus::zvariant::OwnedValue> {
        use zbus::zvariant::{OwnedValue, Value};
        let proxy = zbus::blocking::Proxy::new(
            &self.conn,
            "org.freedesktop.portal.Desktop",
            "/org/freedesktop/portal/desktop",
            "org.freedesktop.portal.Settings",
        )
        .ok()?;
        if let Ok(v) =
            proxy.call::<_, _, OwnedValue>("ReadOne", &("org.freedesktop.appearance", key))
        {
            return Some(v);
        }
        // Older portals: Read returns a doubly-wrapped variant.
        let v: OwnedValue = proxy
            .call("Read", &("org.freedesktop.appearance", key))
            .ok()?;
        match &*v {
            Value::Value(inner) => OwnedValue::try_from(&**inner).ok(),
            _ => Some(v),
        }
    }

    /// (prefers_dark, accent)
    fn appearance(&self) -> (Option<bool>, Option<Color32>) {
        use zbus::zvariant::Value;
        let scheme = self.read("color-scheme").and_then(|v| match &*v {
            Value::U32(1) => Some(true),
            Value::U32(2) => Some(false),
            _ => None,
        });
        let accent = self.read("accent-color").and_then(|v| match &*v {
            Value::Structure(s) => {
                let f: Vec<f64> = s
                    .fields()
                    .iter()
                    .filter_map(|x| f64::try_from(x).ok())
                    .collect();
                (f.len() == 3 && f.iter().all(|x| (0.0..=1.0).contains(x))).then(|| {
                    Color32::from_rgb(
                        (f[0] * 255.0) as u8,
                        (f[1] * 255.0) as u8,
                        (f[2] * 255.0) as u8,
                    )
                })
            }
            _ => None,
        });
        (scheme, accent)
    }
}

// ───────────────────────────── detection + watcher ─────────────────────────────

fn is_kde() -> bool {
    std::env::var("XDG_CURRENT_DESKTOP")
        .map(|d| d.to_uppercase().contains("KDE"))
        .unwrap_or(false)
}

pub fn detect(portal: Option<&(Option<bool>, Option<Color32>)>) -> SystemTheme {
    let full = if is_kde() {
        detect_kde().or_else(detect_gtk)
    } else {
        detect_gtk().or_else(detect_kde)
    };
    let (p_dark, p_accent) = portal.copied().unwrap_or((None, None));
    match full {
        Some(mut t) => {
            // If the portal says the opposite light/dark of the file palette (e.g. GNOME
            // dark-mode toggle with a light GTK theme), trust the portal: drop the palette.
            if let (Some(pd), Some(p)) = (p_dark, t.palette)
                && pd != p.dark
                && !is_kde()
            {
                t.palette = None;
                t.prefers_dark = Some(pd);
                t.source = "XDG portal".into();
            }
            if t.accent.is_none() {
                t.accent = p_accent;
            }
            t
        }
        None => SystemTheme {
            source: if p_dark.is_some() || p_accent.is_some() {
                "XDG portal".into()
            } else {
                "default".into()
            },
            palette: None,
            prefers_dark: p_dark,
            accent: p_accent,
            ui_font: gsettings_font(),
            mono_font: None,
        },
    }
}

fn watched_files() -> Vec<PathBuf> {
    let mut v = vec![kdeglobals_path()];
    for ver in ["gtk-4.0", "gtk-3.0"] {
        for f in ["colors.css", "gtk.css", "settings.ini"] {
            v.push(config_dir().join(ver).join(f));
        }
    }
    v
}

fn mtime(p: &Path) -> Option<SystemTime> {
    std::fs::metadata(p).and_then(|m| m.modified()).ok()
}

/// Spawn a thread that keeps `slot` up to date with the system theme.
pub fn spawn_watcher(slot: Arc<Mutex<Option<SystemTheme>>>, repaint: impl Fn() + Send + 'static) {
    std::thread::Builder::new()
        .name("theme-watcher".into())
        .spawn(move || {
            let portal = Portal::new();
            let mut last_key: Option<(Vec<Option<SystemTime>>, (Option<bool>, Option<Color32>))> =
                None;
            loop {
                let files: Vec<_> = watched_files().iter().map(|p| mtime(p)).collect();
                let app = portal
                    .as_ref()
                    .map(|p| p.appearance())
                    .unwrap_or((None, None));
                let key = (files, app);
                if last_key.as_ref() != Some(&key) {
                    let theme = detect(Some(&key.1));
                    *slot.lock().unwrap() = Some(theme);
                    repaint();
                    last_key = Some(key);
                }
                std::thread::sleep(Duration::from_secs(2));
            }
        })
        .expect("spawn theme watcher");
}

// ───────────────────────────── applying to egui ─────────────────────────────

fn visuals_from_palette(p: &Palette) -> Visuals {
    let mut v = if p.dark {
        Visuals::dark()
    } else {
        Visuals::light()
    };
    let border = mix(p.window_bg, p.window_fg, 0.18);
    v.override_text_color = None;
    v.panel_fill = p.window_bg;
    v.window_fill = p.window_bg;
    v.window_stroke = Stroke::new(1.0, border);
    v.extreme_bg_color = p.view_bg;
    v.text_edit_bg_color = Some(p.view_bg);
    v.faint_bg_color = p.view_alt;
    v.code_bg_color = p.view_alt;
    v.hyperlink_color = p.link;
    v.warn_fg_color = p.neutral;
    v.error_fg_color = p.negative;
    v.weak_text_color = Some(p.inactive_fg);
    v.selection.bg_fill = p.selection_bg;
    v.selection.stroke = Stroke::new(1.0, p.selection_fg);

    let r = CornerRadius::same(4);
    let w = &mut v.widgets;
    w.noninteractive.bg_fill = p.window_bg;
    w.noninteractive.weak_bg_fill = p.window_bg;
    w.noninteractive.bg_stroke = Stroke::new(1.0, border);
    w.noninteractive.fg_stroke = Stroke::new(1.0, p.window_fg);
    w.noninteractive.corner_radius = r;

    w.inactive.bg_fill = p.button_bg;
    w.inactive.weak_bg_fill = p.button_bg;
    w.inactive.bg_stroke = Stroke::new(1.0, mix(p.button_bg, p.button_fg, 0.12));
    w.inactive.fg_stroke = Stroke::new(1.0, p.button_fg);
    w.inactive.corner_radius = r;

    let hov = mix(p.button_bg, p.hover, 0.35);
    w.hovered.bg_fill = hov;
    w.hovered.weak_bg_fill = hov;
    w.hovered.bg_stroke = Stroke::new(1.0, p.hover);
    w.hovered.fg_stroke = Stroke::new(1.5, p.button_fg);
    w.hovered.corner_radius = r;

    let act = mix(p.button_bg, p.focus, 0.55);
    w.active.bg_fill = act;
    w.active.weak_bg_fill = act;
    w.active.bg_stroke = Stroke::new(1.0, p.focus);
    w.active.fg_stroke = Stroke::new(2.0, p.button_fg);
    w.active.corner_radius = r;

    w.open = w.hovered;
    v
}

fn accent_only(dark: bool, accent: Option<Color32>) -> Visuals {
    let mut v = if dark {
        Visuals::dark()
    } else {
        Visuals::light()
    };
    if let Some(a) = accent {
        v.selection.bg_fill = if dark {
            mix(a, Color32::BLACK, 0.25)
        } else {
            mix(a, Color32::WHITE, 0.35)
        };
        v.hyperlink_color = a;
        v.widgets.hovered.bg_stroke = Stroke::new(1.0, a);
        v.widgets.active.bg_stroke = Stroke::new(1.0, a);
    }
    v
}

/// Resolve a font family name to a file via fontconfig.
fn font_file(family: &str) -> Option<(Vec<u8>, u32)> {
    let out = std::process::Command::new("fc-match")
        .args(["-f", "%{file}\n%{index}", family])
        .output()
        .ok()?;
    let s = String::from_utf8_lossy(&out.stdout);
    let mut lines = s.lines();
    let file = lines.next()?.trim();
    let index = lines
        .next()
        .and_then(|i| i.trim().parse().ok())
        .unwrap_or(0);
    let lower = file.to_lowercase();
    if !(lower.ends_with(".ttf") || lower.ends_with(".otf") || lower.ends_with(".ttc")) {
        return None;
    }
    Some((std::fs::read(file).ok()?, index))
}

/// Applies a detected theme to an egui context, caching expensive font loads.
#[derive(Default)]
pub struct Applier {
    fonts_key: Option<(Option<String>, Option<String>)>,
    applied: Option<(SystemTheme, ThemeMode, bool)>,
}

impl Applier {
    /// Returns the semantic colors to use for status text.
    pub fn apply(
        &mut self,
        ctx: &egui::Context,
        theme: &SystemTheme,
        mode: ThemeMode,
        system_font: bool,
    ) -> Option<Semantic> {
        let key = (theme.clone(), mode, system_font);
        if self.applied.as_ref() == Some(&key) {
            return None;
        }
        self.applied = Some(key);

        // ── colors ──
        let sys_dark = theme
            .palette
            .map(|p| p.dark)
            .or(theme.prefers_dark)
            .unwrap_or(true);
        let visuals = match (mode, theme.palette) {
            (ThemeMode::System, Some(p)) => visuals_from_palette(&p),
            (ThemeMode::System, None) => accent_only(sys_dark, theme.accent),
            (ThemeMode::Dark, Some(p)) if p.dark => visuals_from_palette(&p),
            (ThemeMode::Light, Some(p)) if !p.dark => visuals_from_palette(&p),
            (ThemeMode::Dark, _) => accent_only(true, theme.accent),
            (ThemeMode::Light, _) => accent_only(false, theme.accent),
        };
        let dark = visuals.dark_mode;
        let used_palette = visuals.panel_fill
            == theme
                .palette
                .map(|p| p.window_bg)
                .unwrap_or(Color32::TRANSPARENT);
        ctx.set_visuals_of(
            if dark {
                egui::Theme::Dark
            } else {
                egui::Theme::Light
            },
            visuals.clone(),
        );
        ctx.set_theme(if dark {
            egui::Theme::Dark
        } else {
            egui::Theme::Light
        });

        let semantic = match theme.palette.filter(|_| used_palette) {
            Some(p) => Semantic {
                positive: p.positive,
                negative: p.negative,
                warning: p.neutral,
                muted: p.inactive_fg,
                accent: p.link,
            },
            None if dark => Semantic {
                accent: theme.accent.unwrap_or(Color32::LIGHT_BLUE),
                ..Default::default()
            },
            None => Semantic {
                positive: Color32::DARK_GREEN,
                negative: Color32::DARK_RED,
                warning: Color32::from_rgb(160, 110, 0),
                muted: Color32::DARK_GRAY,
                accent: theme.accent.unwrap_or(Color32::DARK_BLUE),
            },
        };

        // ── fonts ──
        let ui_font = theme.ui_font.clone().filter(|_| system_font);
        let mono_font = theme.mono_font.clone().filter(|_| system_font);
        let fkey = (
            ui_font.as_ref().map(|f| f.family.clone()),
            mono_font.as_ref().map(|f| f.family.clone()),
        );
        if self.fonts_key.as_ref() != Some(&fkey) {
            let mut defs = FontDefinitions::default();
            if let Some((bytes, index)) = ui_font.as_ref().and_then(|f| font_file(&f.family)) {
                defs.font_data.insert(
                    "system-ui".into(),
                    Arc::new(FontData {
                        index,
                        ..FontData::from_owned(bytes)
                    }),
                );
                defs.families
                    .entry(FontFamily::Proportional)
                    .or_default()
                    .insert(0, "system-ui".into());
            }
            if let Some((bytes, index)) = mono_font.as_ref().and_then(|f| font_file(&f.family)) {
                defs.font_data.insert(
                    "system-mono".into(),
                    Arc::new(FontData {
                        index,
                        ..FontData::from_owned(bytes)
                    }),
                );
                defs.families
                    .entry(FontFamily::Monospace)
                    .or_default()
                    .insert(0, "system-mono".into());
            }
            ctx.set_fonts(defs);
            self.fonts_key = Some(fkey);
        }
        // Points → logical pixels (96 dpi / 72 pt).
        let body = ui_font
            .map(|f| (f.points * 96.0 / 72.0).clamp(9.0, 28.0))
            .unwrap_or(12.5);
        let mono = mono_font
            .map(|f| (f.points * 96.0 / 72.0).clamp(9.0, 28.0))
            .unwrap_or(body * 0.95);
        ctx.all_styles_mut(|s| {
            s.text_styles = [
                (TextStyle::Small, FontId::proportional(body * 0.78)),
                (TextStyle::Body, FontId::proportional(body)),
                (TextStyle::Button, FontId::proportional(body)),
                (TextStyle::Heading, FontId::proportional(body * 1.45)),
                (TextStyle::Monospace, FontId::monospace(mono)),
            ]
            .into();
        });
        Some(semantic)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn colors() {
        assert_eq!(
            parse_kde_color("46,52,64"),
            Some(Color32::from_rgb(46, 52, 64))
        );
        assert_eq!(
            parse_css_color("#3daee9;"),
            Some(Color32::from_rgb(0x3d, 0xae, 0xe9))
        );
        assert_eq!(parse_css_color("#fff"), Some(Color32::WHITE));
        assert_eq!(
            parse_css_color("rgba(10, 20, 30, 0.5)"),
            Some(Color32::from_rgb(10, 20, 30))
        );
    }

    #[test]
    fn fonts() {
        assert_eq!(
            parse_font("Noto Sans,10,-1,5,400,0,0,0,0,0"),
            Some(FontSpec {
                family: "Noto Sans".into(),
                points: 10.0
            })
        );
        assert_eq!(
            parse_font("Noto Sans,  10"),
            Some(FontSpec {
                family: "Noto Sans".into(),
                points: 10.0
            })
        );
        assert_eq!(
            parse_font("'Cantarell 11'"),
            Some(FontSpec {
                family: "Cantarell".into(),
                points: 11.0
            })
        );
    }

    #[test]
    fn gtk_defines_resolve_refs() {
        let mut d = HashMap::new();
        parse_define_colors(
            "@define-color theme_bg_color_breeze #2e3440;\n@define-color window_bg_color @theme_bg_color_breeze;",
            &mut d,
        );
        assert_eq!(
            resolve(&d, "window_bg_color"),
            Some(Color32::from_rgb(0x2e, 0x34, 0x40))
        );
    }
}
