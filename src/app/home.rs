//! Simplified, G HUB–style navigation: Home (device cards) → Device page
//! (Lighting / Assignments / Sensitivity) and a Settings page for everything else.

use super::*;
use crate::devices::PhysDevice;
use crate::lighting::Model;
use eframe::egui::{Color32, CornerRadius, Rect, Sense, Stroke, StrokeKind, Vec2, pos2, vec2};

#[derive(Clone, PartialEq, Debug)]
pub enum View {
    Home,
    Device(String, Section),
    Settings(SettingsPage),
}

#[derive(Clone, Copy, PartialEq, Debug)]
pub enum Section {
    Lighting,
    Assignments,
    Sensitivity,
}

impl Section {
    fn label(self) -> &'static str {
        match self {
            Section::Lighting => "Lighting",
            Section::Assignments => "Assignments",
            Section::Sensitivity => "Sensitivity",
        }
    }
    fn icon(self) -> &'static str {
        match self {
            Section::Lighting => "💡",
            Section::Assignments => "⌨",
            Section::Sensitivity => "🎯",
        }
    }
}

#[derive(Clone, Copy, PartialEq, Debug)]
pub enum SettingsPage {
    General,
    Macros,
    Autoclicker,
    Advanced,
}

impl SettingsPage {
    const ALL: [SettingsPage; 4] = [
        SettingsPage::General,
        SettingsPage::Macros,
        SettingsPage::Autoclicker,
        SettingsPage::Advanced,
    ];
    fn label(self) -> &'static str {
        match self {
            SettingsPage::General => "General",
            SettingsPage::Macros => "Macros",
            SettingsPage::Autoclicker => "Autoclicker",
            SettingsPage::Advanced => "Advanced",
        }
    }
}

// ───────────────────────── small widgets ─────────────────────────

/// iOS-style on/off switch.
pub fn toggle(ui: &mut egui::Ui, on: &mut bool) -> egui::Response {
    let size = vec2(2.2, 1.2) * ui.spacing().interact_size.y;
    let (rect, mut resp) = ui.allocate_exact_size(size, Sense::click());
    if resp.clicked() {
        *on = !*on;
        resp.mark_changed();
    }
    let t = ui.ctx().animate_bool_responsive(resp.id, *on);
    let v = ui.visuals();
    let off_bg = v.widgets.inactive.bg_fill;
    let on_bg = v.hyperlink_color;
    let bg = off_bg.lerp_to_gamma(on_bg, t);
    let r = rect.height() / 2.0;
    ui.painter().rect_filled(rect, r, bg);
    let x = egui::lerp((rect.left() + r)..=(rect.right() - r), t);
    let knob = if *on {
        Color32::WHITE
    } else {
        v.widgets.inactive.fg_stroke.color
    };
    ui.painter()
        .circle_filled(pos2(x, rect.center().y), r * 0.74, knob);
    resp
}

/// Human-friendly key names: BTN_SIDE → "Back button", KEY_LEFTCTRL → "Left Ctrl".
pub fn pretty_key(k: &str) -> String {
    let fixed = match k {
        "BTN_LEFT" => "Left click",
        "BTN_RIGHT" => "Right click",
        "BTN_MIDDLE" => "Middle click",
        "BTN_SIDE" => "Back button",
        "BTN_EXTRA" => "Forward button",
        "BTN_FORWARD" => "Forward",
        "BTN_BACK" => "Back",
        "KEY_LEFTCTRL" => "Left Ctrl",
        "KEY_RIGHTCTRL" => "Right Ctrl",
        "KEY_LEFTSHIFT" => "Left Shift",
        "KEY_RIGHTSHIFT" => "Right Shift",
        "KEY_LEFTALT" => "Left Alt",
        "KEY_RIGHTALT" => "Right Alt",
        "KEY_LEFTMETA" => "Super",
        "KEY_RIGHTMETA" => "Right Super",
        "KEY_ESC" => "Esc",
        "KEY_CAPSLOCK" => "Caps Lock",
        "KEY_BACKSPACE" => "Backspace",
        "KEY_PAGEUP" => "Page Up",
        "KEY_PAGEDOWN" => "Page Down",
        "KEY_VOLUMEUP" => "Volume Up",
        "KEY_VOLUMEDOWN" => "Volume Down",
        "KEY_MUTE" => "Mute",
        "KEY_PLAYPAUSE" => "Play / Pause",
        "KEY_NEXTSONG" => "Next track",
        "KEY_PREVIOUSSONG" => "Previous track",
        "KEY_SYSRQ" => "Print Screen",
        "KEY_SCROLLLOCK" => "Scroll Lock",
        "KEY_NUMLOCK" => "Num Lock",
        "KEY_GRAVE" => "` (backtick)",
        "KEY_MINUS" => "-",
        "KEY_EQUAL" => "=",
        "KEY_LEFTBRACE" => "[",
        "KEY_RIGHTBRACE" => "]",
        "KEY_BACKSLASH" => "\\",
        "KEY_SEMICOLON" => ";",
        "KEY_APOSTROPHE" => "'",
        "KEY_COMMA" => ",",
        "KEY_DOT" => ".",
        "KEY_SLASH" => "/",
        "KEY_102ND" => "\\ (ISO)",
        "KEY_COMPOSE" => "Menu",
        "KEY_UP" => "Up arrow",
        "KEY_DOWN" => "Down arrow",
        "KEY_LEFT" => "Left arrow",
        "KEY_RIGHT" => "Right arrow",
        "KEY_KPASTERISK" => "Numpad *",
        "KEY_KPMINUS" => "Numpad -",
        "KEY_KPPLUS" => "Numpad +",
        "KEY_KPSLASH" => "Numpad /",
        "KEY_KPDOT" => "Numpad .",
        "KEY_KPENTER" => "Numpad Enter",
        "BTN_TASK" => "Task button",
        "" => "—",
        _ => "",
    };
    if !fixed.is_empty() {
        return fixed.into();
    }
    if let Some(n) = k.strip_prefix("KEY_KP").filter(|n| n.len() == 1) {
        return format!("Numpad {n}");
    }
    let body = k
        .strip_prefix("KEY_")
        .or_else(|| k.strip_prefix("BTN_"))
        .unwrap_or(k);
    if body.len() <= 3 {
        return body.to_string();
    }
    body.split('_')
        .map(|w| {
            let mut c = w.chars();
            c.next()
                .map(|f| f.to_string() + &c.as_str().to_lowercase())
                .unwrap_or_default()
        })
        .collect::<Vec<_>>()
        .join(" ")
}

/// A key chooser: big button showing the friendly name (click → press a key),
/// with a small ▾ list for keys you can't press (e.g. media keys).
fn key_button(
    ui: &mut egui::Ui,
    id: impl std::hash::Hash + std::fmt::Debug,
    value: &mut String,
    capturing: bool,
) -> bool {
    let mut clicked = false;
    ui.horizontal(|ui| {
        ui.spacing_mut().item_spacing.x = 2.0;
        let text = if capturing {
            RichText::new("Press a key or button…").italics()
        } else {
            RichText::new(pretty_key(value)).strong()
        };
        if ui
            .add(egui::Button::new(text).min_size(vec2(150.0, 28.0)))
            .on_hover_text("Click, then press the key or mouse button")
            .clicked()
        {
            clicked = true;
        }
        egui::ComboBox::from_id_salt(egui::Id::new(id).with("list"))
            .selected_text("")
            .width(10.0)
            .height(360.0)
            .show_ui(ui, |ui| {
                for n in all_key_names() {
                    ui.selectable_value(value, n.clone(), pretty_key(n))
                        .on_hover_text(n);
                }
            });
    });
    clicked
}

// ───────────────────────── device art ─────────────────────────

/// Physical full-size layout in key units: each row is a list of (width, is_gap).
const KB_ROWS: [&[f32]; 6] = [
    // Esc, gap, F1-F4, gap, F5-F8, gap, F9-F12 | nav | numpad area (media keys)
    &[
        1.0, -1.0, 1.0, 1.0, 1.0, 1.0, -0.5, 1.0, 1.0, 1.0, 1.0, -0.5, 1.0, 1.0, 1.0, 1.0, -0.25,
        1.0, 1.0, 1.0, -0.25, 1.0, 1.0, 1.0, 1.0,
    ],
    &[
        1.0, 1.0, 1.0, 1.0, 1.0, 1.0, 1.0, 1.0, 1.0, 1.0, 1.0, 1.0, 1.0, 2.0, -0.25, 1.0, 1.0, 1.0,
        -0.25, 1.0, 1.0, 1.0, 1.0,
    ],
    &[
        1.5, 1.0, 1.0, 1.0, 1.0, 1.0, 1.0, 1.0, 1.0, 1.0, 1.0, 1.0, 1.0, 1.5, -0.25, 1.0, 1.0, 1.0,
        -0.25, 1.0, 1.0, 1.0, 1.0,
    ],
    &[
        1.75, 1.0, 1.0, 1.0, 1.0, 1.0, 1.0, 1.0, 1.0, 1.0, 1.0, 1.0, 2.25, -3.5, 1.0, 1.0, 1.0, 1.0,
    ],
    &[
        2.25, 1.0, 1.0, 1.0, 1.0, 1.0, 1.0, 1.0, 1.0, 1.0, 1.0, 2.75, -1.25, 1.0, -1.25, 1.0, 1.0,
        1.0, 1.0,
    ],
    &[
        1.25, 1.25, 1.25, 6.25, 1.25, 1.25, 1.25, 1.25, -0.25, 1.0, 1.0, 1.0, -0.25, 2.0, 1.0, 1.0,
    ],
];

fn paint_keyboard(p: &egui::Painter, r: Rect, color: Color32) {
    let units = 22.5_f32;
    let rows = KB_ROWS.len() as f32 + 0.3;
    let u = (r.width() / (units + 0.8)).min(r.height() / (rows + 0.8));
    let body = Rect::from_center_size(r.center(), vec2(u * (units + 0.8), u * (rows + 0.8)));
    p.rect_filled(body, u * 0.5, Color32::from_gray(26));
    p.rect_stroke(
        body,
        u * 0.5,
        Stroke::new(1.0, Color32::from_gray(70)),
        StrokeKind::Inside,
    );
    let origin = body.min + vec2(u * 0.4, u * 0.4);
    let pad = (u * 0.09).max(0.8);
    for (ri, row) in KB_ROWS.iter().enumerate() {
        let y = origin.y + ri as f32 * u + if ri > 0 { u * 0.3 } else { 0.0 };
        let mut x = origin.x;
        for &w in row.iter() {
            if w < 0.0 {
                x += -w * u;
                continue;
            }
            let k = Rect::from_min_size(pos2(x, y), vec2(w * u, u)).shrink(pad);
            p.rect_filled(k, u * 0.12, color);
            x += w * u;
        }
    }
}

fn paint_mouse(p: &egui::Painter, r: Rect, glow: Color32, side_buttons: bool) {
    let h = r.height().min(r.width() * 1.5);
    let body = Rect::from_center_size(r.center(), vec2(h * 0.6, h));
    // glow
    for i in 0..6 {
        let g = body.expand(2.0 + i as f32 * 3.0);
        p.rect_filled(
            g,
            CornerRadius::same((g.width() / 2.0) as u8),
            glow.gamma_multiply(0.06),
        );
    }
    p.rect_filled(
        body,
        CornerRadius::same((body.width() / 2.0) as u8),
        Color32::from_gray(28),
    );
    p.rect_stroke(
        body,
        CornerRadius::same((body.width() / 2.0) as u8),
        Stroke::new(1.5, glow.gamma_multiply(0.8)),
        StrokeKind::Inside,
    );
    // button split + wheel
    let top = body.top() + body.height() * 0.42;
    p.line_segment(
        [pos2(body.left() + 4.0, top), pos2(body.right() - 4.0, top)],
        Stroke::new(1.0, Color32::from_gray(80)),
    );
    p.line_segment(
        [
            pos2(body.center().x, body.top() + 6.0),
            pos2(body.center().x, top),
        ],
        Stroke::new(1.0, Color32::from_gray(80)),
    );
    let wheel = Rect::from_center_size(
        pos2(body.center().x, body.top() + body.height() * 0.2),
        vec2(body.width() * 0.12, body.height() * 0.14),
    );
    p.rect_filled(wheel, 4.0, glow);
    if side_buttons {
        // side keypad (G600 / Naga)
        let pad = Rect::from_min_size(
            pos2(
                body.left() + body.width() * 0.08,
                top + body.height() * 0.08,
            ),
            vec2(body.width() * 0.3, body.height() * 0.3),
        );
        for yi in 0..4 {
            for xi in 0..3 {
                let c = pos2(
                    pad.left() + (xi as f32 + 0.5) * pad.width() / 3.0,
                    pad.top() + (yi as f32 + 0.5) * pad.height() / 4.0,
                );
                p.circle_filled(c, pad.width() / 9.0, glow.gamma_multiply(0.7));
            }
        }
    }
}

// ───────────────────────── App views ─────────────────────────

impl App {
    pub(super) fn phys_enabled(&self, d: &PhysDevice) -> bool {
        d.grabbable_nodes().any(|n| {
            self.cfg
                .devices
                .iter()
                .any(|s| s.enabled && s.name == n.name && s.phys == n.phys)
        })
    }

    pub(super) fn set_phys_enabled(&mut self, d: &PhysDevice, on: bool) {
        for n in d.grabbable_nodes() {
            match self
                .cfg
                .devices
                .iter_mut()
                .find(|s| s.name == n.name && s.phys == n.phys)
            {
                Some(s) => s.enabled = on,
                None => self.cfg.devices.push(DeviceSettings {
                    name: n.name.clone(),
                    phys: n.phys.clone(),
                    enabled: on,
                    ..Default::default()
                }),
            }
        }
    }

    fn lighting_model(&self, d: &PhysDevice) -> Option<Model> {
        let m = match d.product {
            0xc33f | 0xc232 if d.vendor == 0x046d => Model::G815,
            0xc24a if d.vendor == 0x046d => Model::G600,
            _ => return None,
        };
        self.light_found.iter().any(|f| f.model == m).then_some(m)
    }

    fn sections(&self, d: &PhysDevice) -> Vec<Section> {
        let mut v = vec![];
        if self.lighting_model(d).is_some() {
            v.push(Section::Lighting);
        }
        v.push(Section::Assignments);
        if d.has_pointer() && !d.is_keyboard {
            v.push(Section::Sensitivity);
        }
        v
    }

    // ── top bar ──
    pub(super) fn ui_topbar(&mut self, ui: &mut egui::Ui, _ctx: &egui::Context) {
        let c = self.colors;
        ui.add_space(6.0);
        ui.horizontal(|ui| {
            match &self.view {
                View::Home => {
                    ui.label(RichText::new("InputForge").size(20.0).strong());
                }
                _ => {
                    if ui
                        .add(egui::Button::new(RichText::new("‹  All devices").size(15.0)).frame(false))
                        .clicked()
                    {
                        self.view = View::Home;
                    }
                }
            }
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                let settings_open = matches!(self.view, View::Settings(_));
                if ui
                    .add(egui::Button::new(RichText::new("⚙").size(18.0)).frame(false).selected(settings_open))
                    .on_hover_text("Settings")
                    .clicked()
                {
                    self.view = View::Settings(SettingsPage::General);
                }
                ui.add_space(10.0);
                // Global on/off
                let mut on = self.running();
                let r = toggle(ui, &mut on).on_hover_text(
                    "Turns key assignments and sensitivity on or off.\nEmergency off: Left Ctrl + Right Ctrl + Esc",
                );
                if r.changed() {
                    if on {
                        self.start_engine();
                    } else {
                        self.stop_engine();
                    }
                    self.cfg.autostart_engine = on;
                }
                ui.label(if self.running() {
                    RichText::new("Active").color(c.positive)
                } else {
                    RichText::new("Paused").color(c.muted)
                });
                ui.add_space(14.0);
                // Profile picker
                let names: Vec<String> = self.cfg.profiles.iter().map(|p| p.name.clone()).collect();
                egui::ComboBox::from_id_salt("profile")
                    .selected_text(
                        RichText::new(names.get(self.cfg.active_profile).cloned().unwrap_or_default())
                            .strong(),
                    )
                    .show_ui(ui, |ui| {
                        for (i, n) in names.iter().enumerate() {
                            ui.selectable_value(&mut self.cfg.active_profile, i, n);
                        }
                        ui.separator();
                        if ui.button("New profile").clicked() {
                            self.cfg.profiles.push(Profile {
                                name: format!("Profile {}", self.cfg.profiles.len() + 1),
                                rules: vec![],
                            });
                            self.cfg.active_profile = self.cfg.profiles.len() - 1;
                        }
                        if ui.button("Duplicate this profile").clicked() {
                            let mut np = self.cfg.profiles[self.cfg.active_profile].clone();
                            np.name += " copy";
                            self.cfg.profiles.push(np);
                            self.cfg.active_profile = self.cfg.profiles.len() - 1;
                        }
                    });
                ui.label(RichText::new("Profile").color(c.muted));
            });
        });
        ui.add_space(6.0);
    }

    // ── home ──
    pub(super) fn ui_home(&mut self, ui: &mut egui::Ui) {
        let c = self.colors;
        let hint = devices::group_hint();
        if !hint.is_empty() || !self.uinput_ok {
            egui::Frame::group(ui.style()).show(ui, |ui| {
                ui.label(RichText::new("⚠ Setup needed").strong().color(c.warning));
                ui.label(if hint.is_empty() {
                    "InputForge can't create its virtual devices. Run the installer again."
                        .to_string()
                } else {
                    hint
                });
            });
            ui.add_space(8.0);
        }
        let devs = self.phys.clone();
        if devs.is_empty() {
            ui.add_space(60.0);
            ui.vertical_centered(|ui| {
                ui.label(RichText::new("No keyboards or mice found").size(18.0));
                if ui.button("Look again").clicked() {
                    self.rescan();
                }
            });
            return;
        }
        let card_w = 300.0;
        let card_h = 280.0;
        let gap = 18.0;
        let avail = ui.available_width();
        let per_row = ((avail + gap) / (card_w + gap)).floor().max(1.0) as usize;
        let total_w = per_row.min(devs.len()) as f32 * (card_w + gap) - gap;
        ui.add_space(20.0);
        for chunk in devs.chunks(per_row) {
            ui.horizontal(|ui| {
                ui.add_space(((avail - total_w) / 2.0).max(0.0));
                ui.spacing_mut().item_spacing.x = gap;
                for d in chunk {
                    self.device_card(ui, d, vec2(card_w, card_h));
                }
            });
            ui.add_space(gap);
        }
    }

    fn device_card(&mut self, ui: &mut egui::Ui, d: &PhysDevice, size: Vec2) {
        let c = self.colors;
        let (rect, resp) = ui.allocate_exact_size(size, Sense::click());
        let v = ui.visuals().clone();
        let hovered = resp.hovered();
        ui.painter().rect_filled(rect, 14.0, v.faint_bg_color);
        ui.painter().rect_stroke(
            rect,
            14.0,
            Stroke::new(
                if hovered { 2.0 } else { 1.0 },
                if hovered {
                    c.accent
                } else {
                    v.widgets.noninteractive.bg_stroke.color
                },
            ),
            StrokeKind::Inside,
        );
        // Art
        let art = Rect::from_min_size(rect.min + vec2(24.0, 22.0), vec2(size.x - 48.0, 160.0));
        let p = ui.painter_at(rect);
        let model = self.lighting_model(d);
        let lt = &self.cfg.lighting;
        let rgb = |c: [u8; 3]| Color32::from_rgb(c[0], c[1], c[2]);
        if d.is_keyboard {
            let base = match (model, lt.keyboard.mode) {
                (Some(_), crate::lighting::KbMode::Off) => Color32::from_gray(50),
                (Some(_), _) => rgb(lt.keyboard.color),
                _ => Color32::from_gray(90),
            };
            let base = if model.is_some() && lt.keyboard.mode == crate::lighting::KbMode::PerKey {
                // average of per-key colors
                let n = lt.keyboard.keys.len().max(1) as u32;
                let sum = lt.keyboard.keys.iter().fold([0u32; 3], |a, c| {
                    [a[0] + c[0] as u32, a[1] + c[1] as u32, a[2] + c[2] as u32]
                });
                Color32::from_rgb((sum[0] / n) as u8, (sum[1] / n) as u8, (sum[2] / n) as u8)
            } else {
                base
            };
            paint_keyboard(&p, art, base);
        } else {
            let glow = match model {
                Some(_) if lt.mouse.mode != crate::lighting::MouseMode::Off => rgb(lt.mouse.color),
                Some(_) => Color32::from_gray(60),
                None => c.accent,
            };
            paint_mouse(&p, art, glow, true);
        }
        // Text
        p.text(
            pos2(rect.center().x, rect.bottom() - 74.0),
            egui::Align2::CENTER_CENTER,
            &d.name,
            egui::FontId::proportional(18.0),
            v.strong_text_color(),
        );
        p.text(
            pos2(rect.center().x, rect.bottom() - 52.0),
            egui::Align2::CENTER_CENTER,
            &d.subtitle,
            egui::FontId::proportional(13.0),
            c.muted,
        );
        let enabled = self.phys_enabled(d);
        let n_rules = self.rules_for(d).len();
        let status = match (enabled, n_rules) {
            (false, _) => "Default behavior".to_string(),
            (true, 0) => "Managed".to_string(),
            (true, 1) => "1 assignment".to_string(),
            (true, n) => format!("{n} assignments"),
        };
        p.text(
            pos2(rect.center().x, rect.bottom() - 26.0),
            egui::Align2::CENTER_CENTER,
            status,
            egui::FontId::proportional(12.0),
            if enabled { c.accent } else { c.muted },
        );
        if resp
            .on_hover_cursor(egui::CursorIcon::PointingHand)
            .clicked()
        {
            let first = self.sections(d)[0];
            self.view = View::Device(d.id.clone(), first);
        }
    }

    fn rules_for(&self, d: &PhysDevice) -> Vec<usize> {
        let f = d.filter.to_lowercase();
        self.cfg.profiles[self.cfg.active_profile]
            .rules
            .iter()
            .enumerate()
            .filter(|(_, r)| {
                !r.device_filter.is_empty() && f.contains(&r.device_filter.to_lowercase())
            })
            .map(|(i, _)| i)
            .collect()
    }

    // ── device page ──
    pub(super) fn ui_device(&mut self, ui: &mut egui::Ui, id: &str, section: Section) {
        let Some(d) = self.phys.iter().find(|d| d.id == id).cloned() else {
            self.view = View::Home;
            return;
        };
        let c = self.colors;
        let sections = self.sections(&d);
        let section = if sections.contains(&section) {
            section
        } else {
            sections[0]
        };

        egui::Panel::left("device_rail")
            .resizable(false)
            .exact_size(210.0)
            .show(ui, |ui| {
                ui.add_space(8.0);
                let art = ui.allocate_exact_size(vec2(190.0, 110.0), Sense::hover()).0;
                let p = ui.painter_at(art);
                if d.is_keyboard {
                    let col = c.accent;
                    paint_keyboard(&p, art.shrink(6.0), col);
                } else {
                    paint_mouse(&p, art, c.accent, true);
                }
                ui.add_space(4.0);
                ui.label(RichText::new(&d.name).size(17.0).strong());
                ui.label(RichText::new(&d.subtitle).color(c.muted));
                ui.add_space(14.0);
                for s in &sections {
                    let sel = *s == section;
                    let b = egui::Button::new(
                        RichText::new(format!("{}   {}", s.icon(), s.label())).size(15.0),
                    )
                    .selected(sel)
                    .frame(sel)
                    .min_size(vec2(190.0, 34.0));
                    if ui.add(b).clicked() {
                        self.view = View::Device(d.id.clone(), *s);
                    }
                }
            });

        egui::CentralPanel::default().show(ui, |ui| {
            egui::ScrollArea::vertical()
                .auto_shrink([false, false])
                .show(ui, |ui| {
                    ui.add_space(8.0);
                    ui.label(RichText::new(section.label()).size(22.0).strong());
                    ui.add_space(8.0);
                    match section {
                        Section::Lighting => match self.lighting_model(&d) {
                            Some(Model::G815) => self.ui_kb_lighting(ui),
                            Some(Model::G600) => self.ui_mouse_lighting(ui),
                            None => {}
                        },
                        Section::Assignments => self.ui_assignments(ui, &d),
                        Section::Sensitivity => self.ui_sensitivity(ui, &d),
                    }
                });
        });
    }

    fn managed_banner(&mut self, ui: &mut egui::Ui, d: &PhysDevice) {
        let c = self.colors;
        let enabled = self.phys_enabled(d);
        egui::Frame::group(ui.style()).show(ui, |ui| {
            ui.set_width(ui.available_width().min(640.0));
            ui.horizontal(|ui| {
                let mut on = enabled;
                if toggle(ui, &mut on).changed() {
                    self.set_phys_enabled(d, on);
                }
                ui.vertical(|ui| {
                    ui.label(RichText::new("Use InputForge settings for this device").strong());
                    ui.label(
                        RichText::new(if on {
                            "Assignments and sensitivity below are applied."
                        } else {
                            "Off — the device behaves normally."
                        })
                        .color(c.muted)
                        .small(),
                    );
                });
            });
            if on_and_paused(enabled, self.running()) {
                ui.horizontal(|ui| {
                    ui.label(RichText::new("InputForge is paused.").color(c.warning));
                    if ui.button("Turn on").clicked() {
                        self.start_engine();
                        self.cfg.autostart_engine = true;
                    }
                });
            }
        });
        ui.add_space(10.0);
    }

    /// Index of this device's own rule for `trigger` (if any).
    fn device_rule(&self, d: &PhysDevice, trigger: &str) -> Option<usize> {
        let f = d.filter.to_lowercase();
        self.cfg.profiles[self.cfg.active_profile]
            .rules
            .iter()
            .position(|r| {
                r.trigger == trigger
                    && !r.device_filter.is_empty()
                    && f.contains(&r.device_filter.to_lowercase())
            })
    }

    fn global_rule(&self, trigger: &str) -> Option<usize> {
        self.cfg.profiles[self.cfg.active_profile]
            .rules
            .iter()
            .position(|r| r.trigger == trigger && r.device_filter.is_empty() && r.enabled)
    }

    /// Set (or clear with `None`) what `trigger` does on this device.
    fn set_assignment(&mut self, d: &PhysDevice, trigger: &str, action: Option<Action>) {
        let p = self.cfg.active_profile;
        match (self.device_rule(d, trigger), action) {
            (Some(i), None) => {
                self.cfg.profiles[p].rules.remove(i);
            }
            (Some(i), Some(a)) => {
                let r = &mut self.cfg.profiles[p].rules[i];
                r.action = a;
                r.enabled = true;
            }
            (None, Some(a)) => {
                self.cfg.profiles[p].rules.push(Rule {
                    enabled: true,
                    device_filter: d.filter.clone(),
                    trigger: trigger.to_string(),
                    action: a,
                });
                self.set_phys_enabled(d, true);
            }
            (None, None) => {}
        }
    }

    fn ui_assignments(&mut self, ui: &mut egui::Ui, d: &PhysDevice) {
        let c = self.colors;
        self.managed_banner(ui, d);

        // ── toolbar ──
        ui.horizontal(|ui| {
            ui.add(
                egui::TextEdit::singleline(&mut self.assign_search)
                    .hint_text("🔍 Search keys")
                    .desired_width(200.0),
            );
            let finding = self.capture == Some(CaptureTarget::FindKey);
            if ui
                .add(
                    egui::Button::new(if finding {
                        "Press a key on the device…"
                    } else {
                        "🎯 Find by pressing"
                    })
                    .selected(finding),
                )
                .on_hover_text("Press a key or button and jump to it in the list")
                .clicked()
            {
                self.begin_capture(CaptureTarget::FindKey);
            }
            ui.checkbox(&mut self.assign_changed_only, "Changed only");
        });
        ui.add_space(8.0);

        // ── build the list ──
        let mut sections = key_sections(d);
        let listed: std::collections::HashSet<String> = sections
            .iter()
            .flat_map(|(_, ks)| ks.iter().cloned())
            .collect();
        let f = d.filter.to_lowercase();
        let mut extra: Vec<String> = self.cfg.profiles[self.cfg.active_profile]
            .rules
            .iter()
            .filter(|r| !r.device_filter.is_empty() && f.contains(&r.device_filter.to_lowercase()))
            .map(|r| r.trigger.clone())
            .chain(self.assign_open.clone())
            .filter(|k| !listed.contains(k))
            .collect();
        extra.sort();
        extra.dedup();
        if !extra.is_empty() {
            sections.insert(0, ("Other", extra));
        }

        let search = self.assign_search.to_lowercase();
        let macro_names: Vec<String> = self.cfg.macros.iter().map(|m| m.name.clone()).collect();
        let row_w = ui.available_width().min(620.0);
        let mut pending: Option<(String, Option<Action>)> = None;
        let mut capture = None;
        let mut any = false;

        for (title, keys) in &sections {
            let rows: Vec<&String> = keys
                .iter()
                .filter(|k| {
                    let lbl = key_label(d, k).to_lowercase();
                    (search.is_empty()
                        || lbl.contains(&search)
                        || k.to_lowercase().contains(&search))
                        && (!self.assign_changed_only
                            || self.device_rule(d, k).is_some()
                            || self.global_rule(k).is_some())
                })
                .collect();
            if rows.is_empty() {
                continue;
            }
            any = true;
            ui.add_space(6.0);
            ui.label(RichText::new(*title).strong().color(c.muted));
            ui.add_space(2.0);
            for k in rows {
                let dev = self.device_rule(d, k);
                let glob = self.global_rule(k);
                let rules = &self.cfg.profiles[self.cfg.active_profile].rules;
                let (func, changed, note) = match (dev, glob) {
                    (Some(i), _) if rules[i].enabled => (describe(&rules[i].action), true, ""),
                    (_, Some(i)) => (describe(&rules[i].action), true, "  (all devices)"),
                    _ => ("Default".to_string(), false, ""),
                };
                let open = self.assign_open.as_deref() == Some(k.as_str());
                let v = ui.visuals().clone();
                let frame = egui::Frame::new()
                    .inner_margin(egui::Margin::symmetric(10, 3))
                    .corner_radius(8.0)
                    .fill(if open {
                        v.faint_bg_color
                    } else {
                        Color32::TRANSPARENT
                    })
                    .stroke(if open {
                        Stroke::new(1.0, v.widgets.noninteractive.bg_stroke.color)
                    } else {
                        Stroke::NONE
                    });
                let fr = frame.show(ui, |ui| {
                    ui.set_width(row_w);
                    let row = ui.horizontal(|ui| {
                        let (lr, _) = ui.allocate_exact_size(vec2(180.0, 20.0), Sense::hover());
                        ui.painter().text(
                            lr.left_center(),
                            egui::Align2::LEFT_CENTER,
                            key_label(d, k),
                            egui::FontId::proportional(14.0),
                            v.strong_text_color(),
                        );
                        ui.label(RichText::new(format!("{func}{note}")).color(if changed {
                            c.accent
                        } else {
                            c.muted
                        }));
                        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                            ui.label(
                                RichText::new(if open { "Close" } else { "Edit" })
                                    .small()
                                    .color(c.muted),
                            );
                        });
                    });
                    let hit = ui.interact(
                        row.response.rect,
                        egui::Id::new(("arow", k)),
                        Sense::click(),
                    );
                    if hit.hovered() && !open {
                        ui.painter().rect_filled(
                            row.response.rect.expand(4.0),
                            6.0,
                            v.widgets.hovered.weak_bg_fill.gamma_multiply(0.35),
                        );
                    }
                    if hit
                        .on_hover_cursor(egui::CursorIcon::PointingHand)
                        .clicked()
                    {
                        self.assign_open = if open { None } else { Some(k.clone()) };
                    }
                    if open {
                        ui.add_space(6.0);
                        self.assignment_editor(ui, d, k, &macro_names, &mut pending, &mut capture);
                    }
                });
                if open && self.assign_scroll {
                    ui.scroll_to_rect(fr.response.rect, Some(egui::Align::Center));
                    self.assign_scroll = false;
                }
            }
        }
        if !any {
            ui.label(RichText::new("Nothing matches.").color(c.muted));
        }
        if let Some((k, a)) = pending {
            self.set_assignment(d, &k, a);
        }
        if let Some(t) = capture {
            self.begin_capture(t);
        }
    }

    fn assignment_editor(
        &self,
        ui: &mut egui::Ui,
        d: &PhysDevice,
        k: &str,
        macro_names: &[String],
        pending: &mut Option<(String, Option<Action>)>,
        capture: &mut Option<CaptureTarget>,
    ) {
        let c = self.colors;
        let idx = self.device_rule(d, k);
        let rule = idx.map(|i| &self.cfg.profiles[self.cfg.active_profile].rules[i]);
        let cur = rule.map(|r| action_kind(&r.action)).unwrap_or("Default");
        ui.horizontal_wrapped(|ui| {
            for (label, a) in [
                ("Default", None),
                (
                    "Another key",
                    Some(Action::Key {
                        key: "KEY_ESC".into(),
                    }),
                ),
                (
                    "Shortcut",
                    Some(Action::Combo {
                        keys: vec!["KEY_LEFTCTRL".into(), "KEY_C".into()],
                    }),
                ),
                (
                    "Macro",
                    Some(Action::Macro {
                        name: macro_names.first().cloned().unwrap_or_default(),
                    }),
                ),
                ("Autoclicker on/off", Some(Action::ToggleAutoclicker)),
                ("Disabled", Some(Action::Disabled)),
            ] {
                if ui
                    .add(egui::Button::new(label).selected(cur == label))
                    .clicked()
                    && cur != label
                {
                    *pending = Some((k.to_string(), a));
                }
            }
        });
        let Some(i) = idx else {
            if let Some(g) = self.global_rule(k) {
                ui.label(
                    RichText::new(format!(
                        "A rule for all devices makes this key do: {}. Choose an option above to override it here.",
                        describe(&self.cfg.profiles[self.cfg.active_profile].rules[g].action)
                    ))
                    .color(c.muted)
                    .small(),
                );
            }
            return;
        };
        let rule = &self.cfg.profiles[self.cfg.active_profile].rules[i];
        ui.add_space(4.0);
        ui.horizontal(|ui| match &rule.action {
            Action::Key { key } => {
                ui.label("Sends");
                let cap = self.capture == Some(CaptureTarget::RuleKey(i));
                let mut kk = key.clone();
                if key_button(ui, ("rk", i), &mut kk, cap) {
                    *capture = Some(CaptureTarget::RuleKey(i));
                }
                if kk != *key {
                    *pending = Some((k.to_string(), Some(Action::Key { key: kk })));
                }
            }
            Action::Combo { keys } => {
                ui.label("Sends");
                let mut ks = keys.clone();
                let mut del = None;
                for (j, kk) in ks.iter().enumerate() {
                    if j > 0 {
                        ui.label("+");
                    }
                    if ui
                        .button(pretty_key(kk))
                        .on_hover_text("Click to remove")
                        .clicked()
                    {
                        del = Some(j);
                    }
                }
                if let Some(j) = del {
                    ks.remove(j);
                    *pending = Some((k.to_string(), Some(Action::Combo { keys: ks })));
                }
                let lbl = if self.capture == Some(CaptureTarget::ComboAdd(i)) {
                    "Press…"
                } else {
                    "+ key"
                };
                if ui.button(lbl).clicked() {
                    *capture = Some(CaptureTarget::ComboAdd(i));
                }
            }
            Action::Macro { name } => {
                if macro_names.is_empty() {
                    ui.label(
                        RichText::new("No macros yet — create one in Settings → Macros.")
                            .color(c.muted),
                    );
                } else {
                    ui.label("Runs");
                    let mut n = name.clone();
                    egui::ComboBox::from_id_salt(("mac", i))
                        .selected_text(n.as_str())
                        .show_ui(ui, |ui| {
                            for m in macro_names {
                                ui.selectable_value(&mut n, m.clone(), m);
                            }
                        });
                    if n != *name {
                        *pending = Some((k.to_string(), Some(Action::Macro { name: n })));
                    }
                }
            }
            Action::ToggleAutoclicker => {
                ui.label(
                    RichText::new("Starts/stops the autoclicker (Settings → Autoclicker).")
                        .color(c.muted),
                );
            }
            Action::Disabled => {
                ui.label(RichText::new("This key does nothing.").color(c.muted));
            }
        });
    }

    fn ui_sensitivity(&mut self, ui: &mut egui::Ui, d: &PhysDevice) {
        let c = self.colors;
        self.managed_banner(ui, d);
        // Use the first pointer node's settings as the source of truth, apply to all.
        let ptr_nodes: Vec<(String, String)> = d
            .nodes
            .iter()
            .filter(|n| {
                matches!(
                    n.kind,
                    devices::DeviceKind::Mouse | devices::DeviceKind::Combo
                )
            })
            .map(|n| (n.name.clone(), n.phys.clone()))
            .collect();
        let cur = self
            .cfg
            .devices
            .iter()
            .find(|s| ptr_nodes.iter().any(|(n, p)| *n == s.name && *p == s.phys))
            .cloned()
            .unwrap_or_default();
        let (mut sp, mut sc, mut inv) = (cur.pointer_speed, cur.scroll_speed, cur.invert_scroll);
        let mut changed = false;
        egui::Grid::new("sens")
            .num_columns(2)
            .spacing([24.0, 18.0])
            .show(ui, |ui| {
                ui.label(RichText::new("Pointer speed").strong());
                changed |= ui
                    .add(
                        egui::Slider::new(&mut sp, 0.1..=5.0)
                            .custom_formatter(|v, _| format!("{v:.2}×")),
                    )
                    .changed();
                ui.end_row();
                ui.label(RichText::new("Scroll speed").strong());
                changed |= ui
                    .add(
                        egui::Slider::new(&mut sc, 0.25..=5.0)
                            .custom_formatter(|v, _| format!("{v:.2}×")),
                    )
                    .changed();
                ui.end_row();
                ui.label(RichText::new("Natural scrolling").strong());
                changed |= toggle(ui, &mut inv).changed();
                ui.end_row();
            });
        if ui.button("Reset").clicked() {
            (sp, sc, inv) = (1.0, 1.0, false);
            changed = true;
        }
        if changed {
            self.set_phys_enabled(d, true);
            for s in self.cfg.devices.iter_mut() {
                if ptr_nodes.iter().any(|(n, p)| *n == s.name && *p == s.phys) {
                    s.pointer_speed = sp;
                    s.scroll_speed = sc;
                    s.invert_scroll = inv;
                }
            }
        }
        ui.add_space(12.0);
        ui.label(
            RichText::new(
                "Hardware DPI and polling rate are under Settings → Advanced (needs libratbag).",
            )
            .color(c.muted)
            .small(),
        );
    }

    // ── settings ──
    pub(super) fn ui_settings(&mut self, ui: &mut egui::Ui, page: SettingsPage) {
        let c = self.colors;
        egui::Panel::left("settings_rail")
            .resizable(false)
            .exact_size(190.0)
            .show(ui, |ui| {
                ui.add_space(8.0);
                ui.label(RichText::new("Settings").size(20.0).strong());
                ui.add_space(12.0);
                for s in SettingsPage::ALL {
                    let sel = s == page;
                    if ui
                        .add(
                            egui::Button::new(RichText::new(s.label()).size(15.0))
                                .selected(sel)
                                .frame(sel)
                                .min_size(vec2(170.0, 32.0)),
                        )
                        .clicked()
                    {
                        self.view = View::Settings(s);
                    }
                }
            });
        egui::CentralPanel::default().show(ui, |ui| {
            egui::ScrollArea::vertical().auto_shrink([false, false]).show(ui, |ui| {
                ui.add_space(8.0);
                match page {
                    SettingsPage::General => {
                        ui.label(RichText::new("General").size(22.0).strong());
                        ui.add_space(10.0);
                        egui::Grid::new("gen").num_columns(2).spacing([24.0, 14.0]).show(ui, |ui| {
                            ui.label("Turn on when InputForge opens");
                            toggle(ui, &mut self.cfg.autostart_engine);
                            ui.end_row();
                            ui.label("Start at login (in the system tray)")
                                .on_hover_text("Runs InputForge in the background with a tray icon, so assignments and lighting work without the window open.");
                            let mut l = crate::tray::autostart_enabled();
                            if toggle(ui, &mut l).changed() {
                                if let Err(e) = crate::tray::sync_autostart(l, self.cfg.lighting.apply_on_launch) {
                                    self.error = Some(format!("Autostart: {e}"));
                                }
                            }
                            ui.end_row();
                            ui.label("Restore lighting at login");
                            let mut a = self.cfg.lighting.apply_on_launch;
                            if toggle(ui, &mut a).changed() {
                                self.cfg.lighting.apply_on_launch = a;
                                if let Err(e) = crate::tray::sync_autostart(crate::tray::autostart_enabled(), a) {
                                    self.error = Some(format!("Autostart: {e}"));
                                }
                            }
                            ui.end_row();
                            ui.label("Closing the window");
                            ui.label(RichText::new(if self.has_tray() {
                                "keeps InputForge running in the tray (Quit from the tray menu)"
                            } else {
                                "quits InputForge (no system tray found)"
                            }).color(self.colors.muted));
                            ui.end_row();
                            ui.label("Appearance");
                            egui::ComboBox::from_id_salt("theme")
                                .selected_text(self.cfg.theme.label())
                                .show_ui(ui, |ui| {
                                    for m in [ThemeMode::System, ThemeMode::Dark, ThemeMode::Light] {
                                        ui.selectable_value(&mut self.cfg.theme, m, m.label());
                                    }
                                });
                            ui.end_row();
                            ui.label("Use system font");
                            toggle(ui, &mut self.cfg.system_font);
                            ui.end_row();
                        });
                        ui.add_space(16.0);
                        ui.label(RichText::new("Profiles").size(16.0).strong());
                        ui.add_space(4.0);
                        let p = self.cfg.active_profile;
                        ui.horizontal(|ui| {
                            ui.label("Name of current profile:");
                            ui.text_edit_singleline(&mut self.cfg.profiles[p].name);
                            if self.cfg.profiles.len() > 1 && ui.button("Delete").clicked() {
                                self.cfg.profiles.remove(p);
                                self.cfg.active_profile = 0;
                            }
                        });
                        ui.add_space(16.0);
                        ui.label(
                            RichText::new("Emergency off: hold Left Ctrl + Right Ctrl and press Esc.")
                                .color(c.muted),
                        );
                        ui.label(RichText::new(format!("Theme detected from {}", self.theme_source)).color(c.muted).small());
                    }
                    SettingsPage::Macros => {
                        ui.label(RichText::new("Macros").size(22.0).strong());
                        ui.label(RichText::new("Create a macro here, then assign it to a key on a device's Assignments page.").color(c.muted));
                        ui.add_space(8.0);
                        self.ui_macros(ui);
                    }
                    SettingsPage::Autoclicker => {
                        ui.label(RichText::new("Autoclicker").size(22.0).strong());
                        ui.add_space(8.0);
                        self.ui_autoclicker(ui);
                    }
                    SettingsPage::Advanced => {
                        ui.label(RichText::new("Advanced").size(22.0).strong());
                        ui.add_space(8.0);
                        egui::CollapsingHeader::new("All input devices").show(ui, |ui| self.ui_devices(ui));
                        egui::CollapsingHeader::new("Hardware DPI / polling (libratbag) and OpenRGB")
                            .show(ui, |ui| self.ui_hardware(ui));
                        egui::CollapsingHeader::new("Activity log").show(ui, |ui| self.ui_log(ui));
                        egui::CollapsingHeader::new("All assignments (every device)").show(ui, |ui| self.ui_remap(ui));
                    }
                }
            });
        });
    }
}

/// What a rule does, in plain words.
fn describe(a: &Action) -> String {
    match a {
        Action::Key { key } => pretty_key(key),
        Action::Combo { keys } => keys
            .iter()
            .map(|k| pretty_key(k))
            .collect::<Vec<_>>()
            .join(" + "),
        Action::Macro { name } => format!("Macro: {name}"),
        Action::ToggleAutoclicker => "Autoclicker on/off".into(),
        Action::Disabled => "Disabled".into(),
    }
}

/// Label for a key in this device's list (mouse side keypads say "Side button N").
fn key_label(d: &PhysDevice, k: &str) -> String {
    if !d.is_keyboard {
        let side = [
            "KEY_1",
            "KEY_2",
            "KEY_3",
            "KEY_4",
            "KEY_5",
            "KEY_6",
            "KEY_7",
            "KEY_8",
            "KEY_9",
            "KEY_0",
            "KEY_MINUS",
            "KEY_EQUAL",
        ];
        if let Some(i) = side.iter().position(|s| *s == k) {
            return format!("Side button {}", i + 1);
        }
    }
    pretty_key(k)
}

/// The keys/buttons a device actually has, grouped like the physical device.
fn key_sections(d: &PhysDevice) -> Vec<(&'static str, Vec<String>)> {
    let has = |code: u16| d.nodes.iter().any(|n| n.keys.contains(&code));
    let name = |c: u16| crate::keys::key_name(evdev::KeyCode::new(c));
    let pick = |codes: &[u16]| -> Vec<String> {
        codes
            .iter()
            .copied()
            .filter(|c| has(*c))
            .map(name)
            .collect()
    };
    let mut out: Vec<(&'static str, Vec<String>)> = vec![];
    if d.is_keyboard {
        let letters: Vec<u16> = "QWERTYUIOPASDFGHJKLZXCVBNM"
            .chars()
            .map(|ch| crate::keys::parse_key(&format!("KEY_{ch}")).unwrap().code())
            .collect();
        let mut letters_sorted = letters.clone();
        letters_sorted.sort_by_key(|c| name(*c));
        out.push(("Letters", pick(&letters_sorted)));
        out.push(("Numbers", pick(&[2, 3, 4, 5, 6, 7, 8, 9, 10, 11])));
        out.push((
            "Function keys",
            pick(
                &(59..=68)
                    .chain([87, 88])
                    .chain(183..=194)
                    .collect::<Vec<_>>(),
            ),
        ));
        out.push(("Modifiers", pick(&[29, 42, 56, 125, 97, 54, 100, 126, 58])));
        out.push((
            "Typing & punctuation",
            pick(&[
                1, 15, 28, 57, 14, 41, 12, 13, 26, 27, 43, 39, 40, 51, 52, 53, 86, 127,
            ]),
        ));
        out.push((
            "Navigation",
            pick(&[
                110, 111, 102, 107, 104, 109, 103, 108, 105, 106, 99, 70, 119,
            ]),
        ));
        out.push((
            "Numpad",
            pick(&[
                69, 98, 55, 74, 78, 96, 83, 82, 79, 80, 81, 75, 76, 77, 71, 72, 73,
            ]),
        ));
        out.push(("Media", pick(&[164, 163, 165, 166, 113, 114, 115])));
    } else {
        out.push((
            "Buttons",
            pick(&[0x110, 0x111, 0x112, 0x113, 0x114, 0x115, 0x116, 0x117]),
        ));
        out.push((
            "Side buttons",
            pick(&[2, 3, 4, 5, 6, 7, 8, 9, 10, 11, 12, 13]),
        ));
    }
    out.retain(|(_, v)| !v.is_empty());
    out
}

pub fn view_from_str(v: &str) -> Option<View> {
    let parts: Vec<&str> = v.split(':').collect();
    match parts.as_slice() {
        ["settings"] => Some(View::Settings(SettingsPage::General)),
        ["settings", "macros"] => Some(View::Settings(SettingsPage::Macros)),
        ["settings", "advanced"] => Some(View::Settings(SettingsPage::Advanced)),
        ["device", vid, pid, sec] => Some(View::Device(
            format!("{vid}:{pid}"),
            match *sec {
                "lighting" => Section::Lighting,
                "sensitivity" => Section::Sensitivity,
                _ => Section::Assignments,
            },
        )),
        _ => None,
    }
}

fn on_and_paused(enabled: bool, running: bool) -> bool {
    enabled && !running
}

fn action_kind(a: &Action) -> &'static str {
    match a {
        Action::Key { .. } => "Another key",
        Action::Combo { .. } => "Shortcut",
        Action::Macro { .. } => "Macro",
        Action::ToggleAutoclicker => "Autoclicker on/off",
        Action::Disabled => "Do nothing",
    }
}

#[cfg(test)]
mod tests {
    use super::pretty_key;
    #[test]
    fn pretty() {
        assert_eq!(pretty_key("BTN_SIDE"), "Back button");
        assert_eq!(pretty_key("KEY_A"), "A");
        assert_eq!(pretty_key("KEY_F13"), "F13");
        assert_eq!(pretty_key("KEY_SCROLLLOCK"), "Scroll Lock");
        assert_eq!(pretty_key("KEY_KP7"), "Numpad 7");
        assert_eq!(pretty_key("KEY_PLAYPAUSE"), "Play / Pause");
    }
}
