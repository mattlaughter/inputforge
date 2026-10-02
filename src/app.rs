//! egui front-end.

mod home;
mod sensitivity;
use home::View;
// Re-export for sibling UI panels (e.g. sensitivity).
pub(crate) use home::toggle;

use eframe::egui::{self, RichText};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use crate::config::{Action, Config, DeviceSettings, Macro, MacroMode, Profile, Rule, Step};
use crate::devices::{self, DeviceInfo};
use crate::engine::{self, Engine, Shared, SharedRef};
use crate::hardware::{self, OpenRgb, Ratbag, RbDevice, RgbController};
use crate::keys::all_key_names;
use crate::theme::{self, Applier, Semantic, SystemTheme, ThemeMode};

/// Which text field a key capture should be written into.
#[derive(Clone, Copy, PartialEq, Debug)]
enum CaptureTarget {
    RuleTrigger(usize),
    RuleKey(usize),
    ComboAdd(usize),
    StepKey(usize, usize),
    AutoHotkey,
    AutoButton,
    /// Assignments page: press a key to jump to it.
    FindKey,
}

pub struct App {
    cfg: Config,
    saved_cfg: Config,
    devices: Vec<DeviceInfo>,
    unreadable: usize,
    uinput_ok: bool,
    shared: SharedRef,
    engine: Option<Engine>,
    engine_devices: Vec<(String, String)>,
    capture: Option<CaptureTarget>,
    capture_started: std::time::Instant,
    selected_macro: usize,
    error: Option<String>,
    // Hardware tab
    ratbag: Option<Ratbag>,
    rb_devices: Vec<RbDevice>,
    rb_status: String,
    rb_edit_dpi: Vec<Vec<u32>>,
    openrgb_addr: String,
    openrgb: Option<OpenRgb>,
    rgb_ctrls: Vec<RgbController>,
    rgb_status: String,
    rgb_pick: Vec<[u8; 3]>,
    macro_test_status: Arc<Mutex<String>>,
    sys_theme: Arc<Mutex<Option<SystemTheme>>>,
    theme_applier: Applier,
    colors: Semantic,
    theme_source: String,
    lighting: crate::lighting::Lighting,
    light_found: Vec<crate::lighting::Found>,
    brush: [u8; 3],
    view: View,
    assign_open: Option<String>,
    assign_search: String,
    assign_changed_only: bool,
    assign_scroll: bool,
    last_scan: std::time::Instant,
    phys: Vec<crate::devices::PhysDevice>,
    // Tray / background service
    waker: crate::tray::Waker,
    cmds: std::sync::mpsc::Receiver<crate::tray::Cmd>,
    tray: Option<crate::tray::Handle>,
    tray_state: crate::tray::TrayState,
    want_quit: bool,
    window_ctx: Option<egui::Context>,
}

impl App {
    pub fn new(
        waker: crate::tray::Waker,
        cmds: std::sync::mpsc::Receiver<crate::tray::Cmd>,
    ) -> Self {
        let (cfg, error) = match Config::load() {
            Ok(c) => (c, None),
            Err(e) => (Config::default(), Some(format!("Config load failed: {e}"))),
        };
        // Apply the system theme synchronously on first frame to avoid a flash.
        let sys_theme = Arc::new(Mutex::new(Some(theme::detect(None))));
        {
            let w = waker.clone();
            theme::spawn_watcher(sys_theme.clone(), move || w.wake());
        }
        let mut app = Self {
            saved_cfg: cfg.clone(),
            cfg,
            devices: vec![],
            unreadable: 0,
            uinput_ok: devices::uinput_writable(),
            shared: Arc::new(Mutex::new(Shared::default())),
            engine: None,
            engine_devices: vec![],
            capture: None,
            capture_started: std::time::Instant::now(),
            selected_macro: 0,
            error,
            ratbag: None,
            rb_devices: vec![],
            rb_status: String::new(),
            rb_edit_dpi: vec![],
            openrgb_addr: "127.0.0.1:6742".into(),
            openrgb: None,
            rgb_ctrls: vec![],
            rgb_status: String::new(),
            rgb_pick: vec![],
            macro_test_status: Arc::new(Mutex::new(String::new())),
            sys_theme,
            theme_applier: Applier::default(),
            colors: Semantic::default(),
            theme_source: String::new(),
            lighting: {
                let w = waker.clone();
                crate::lighting::Lighting::new(move || w.wake())
            },
            light_found: crate::lighting::discover(),
            brush: [255, 80, 0],
            view: View::Home,
            assign_open: None,
            assign_search: String::new(),
            assign_changed_only: false,
            assign_scroll: false,
            last_scan: std::time::Instant::now(),
            phys: vec![],
            waker,
            cmds,
            tray: None,
            tray_state: Default::default(),
            want_quit: false,
            window_ctx: None,
        };
        if app.cfg.lighting.apply_on_launch {
            app.lighting.apply_keyboard(&app.cfg.lighting.keyboard);
            app.lighting.apply_mouse(&app.cfg.lighting.mouse);
        }
        app.rescan();
        // Debug/screenshot aid: INPUTFORGE_VIEW=device:046d:c33f:lighting | settings
        if let Ok(v) = std::env::var("INPUTFORGE_VIEW") {
            app.view = home::view_from_str(&v).unwrap_or(View::Home);
        }
        app.assign_open = std::env::var("INPUTFORGE_OPEN").ok();
        if app.cfg.autostart_engine {
            app.start_engine();
        }
        app
    }

    fn apply_theme(&mut self, ctx: &egui::Context) {
        let Some(t) = self.sys_theme.lock().unwrap().clone() else {
            return;
        };
        if let Some(sem) = self
            .theme_applier
            .apply(ctx, &t, self.cfg.theme, self.cfg.system_font)
        {
            self.colors = sem;
            let font = t
                .ui_font
                .as_ref()
                .filter(|_| self.cfg.system_font)
                .map(|f| format!(", font {} {}pt", f.family, f.points))
                .unwrap_or_default();
            self.theme_source = format!("{}{font}", t.source);
        }
    }

    fn rescan(&mut self) {
        let r = devices::scan();
        self.devices = r.devices;
        self.unreadable = r.unreadable;
        self.phys = devices::group(&self.devices);
        self.uinput_ok = devices::uinput_writable();
    }

    fn running(&self) -> bool {
        self.engine.as_ref().is_some_and(|e| !e.is_finished())
    }

    fn enabled_device_keys(&self) -> Vec<(String, String)> {
        let mut v: Vec<_> = self
            .cfg
            .devices
            .iter()
            .filter(|d| d.enabled)
            .map(|d| d.key())
            .collect();
        v.sort();
        v
    }

    fn start_engine(&mut self) {
        self.stop_engine();
        let w = self.waker.clone();
        match Engine::start(self.cfg.clone(), self.shared.clone(), move || w.wake()) {
            Ok(e) => {
                self.engine = Some(e);
                self.engine_devices = self.enabled_device_keys();
                self.error = None;
                // Keep System Settings' mouse options applying to grabbed mice.
                let mice: Vec<String> =
                    self.devices
                        .iter()
                        .filter(|d| {
                            d.kind == devices::DeviceKind::Mouse
                                && self.cfg.devices.iter().any(|c| {
                                    c.enabled && c.key() == (d.name.clone(), d.phys.clone())
                                })
                        })
                        .map(|d| d.name.clone())
                        .collect();
                if !mice.is_empty() {
                    let shared = self.shared.clone();
                    std::thread::spawn(move || {
                        if let Some(msg) = crate::desktop::mirror_pointer_settings(&mice) {
                            shared.lock().unwrap().push_log(msg);
                        }
                    });
                }
            }
            Err(e) => self.error = Some(e.to_string()),
        }
    }

    fn stop_engine(&mut self) {
        if let Some(mut e) = self.engine.take() {
            e.stop();
        }
    }

    /// Persist and hot-apply config changes.
    fn sync_config(&mut self) {
        if self.cfg == self.saved_cfg {
            return;
        }
        if let Err(e) = self.cfg.save() {
            self.error = Some(format!("Save failed: {e}"));
        }
        self.saved_cfg = self.cfg.clone();
        if let Some(e) = &self.engine {
            if self.enabled_device_keys() != self.engine_devices {
                // Device selection changed: restart so grabs match.
                self.start_engine();
            } else {
                e.update_config(self.cfg.clone());
            }
        }
    }

    fn begin_capture(&mut self, target: CaptureTarget) {
        self.capture = Some(target);
        self.capture_started = std::time::Instant::now();
        let mut sh = self.shared.lock().unwrap();
        sh.captured = None;
        if self.running() {
            sh.capture_request = true;
        } else {
            drop(sh);
            let shared = self.shared.clone();
            std::thread::spawn(move || {
                let k = engine::capture_once(Duration::from_secs(8));
                let mut sh = shared.lock().unwrap();
                sh.captured = Some(k.unwrap_or_default());
            });
        }
    }

    fn poll_capture(&mut self) {
        let Some(target) = self.capture else { return };
        let got = self.shared.lock().unwrap().captured.take();
        let Some(key) = got else {
            if self.capture_started.elapsed() > Duration::from_secs(9) {
                self.capture = None;
                self.shared.lock().unwrap().capture_request = false;
            }
            return;
        };
        self.capture = None;
        if key.is_empty() {
            return;
        }
        let p = self.cfg.active_profile;
        let rules = &mut self.cfg.profiles[p].rules;
        match target {
            CaptureTarget::RuleTrigger(i) => {
                if let Some(r) = rules.get_mut(i) {
                    r.trigger = key;
                }
            }
            CaptureTarget::RuleKey(i) => {
                if let Some(Rule {
                    action: Action::Key { key: k },
                    ..
                }) = rules.get_mut(i)
                {
                    *k = key;
                }
            }
            CaptureTarget::ComboAdd(i) => {
                if let Some(Rule {
                    action: Action::Combo { keys },
                    ..
                }) = rules.get_mut(i)
                {
                    keys.push(key);
                }
            }
            CaptureTarget::StepKey(m, s) => {
                if let Some(step) = self.cfg.macros.get_mut(m).and_then(|m| m.steps.get_mut(s)) {
                    match step {
                        Step::Tap { key: k }
                        | Step::KeyDown { key: k }
                        | Step::KeyUp { key: k } => *k = key,
                        _ => {}
                    }
                }
            }
            CaptureTarget::AutoHotkey => self.cfg.autoclicker.hotkey = key,
            CaptureTarget::AutoButton => self.cfg.autoclicker.button = key,
            CaptureTarget::FindKey => {
                self.assign_search.clear();
                self.assign_open = Some(key);
                self.assign_scroll = true;
            }
        }
    }
}

/// A key name field: text box + dropdown + capture button.
fn key_field(
    ui: &mut egui::Ui,
    id: impl std::hash::Hash + std::fmt::Debug,
    value: &mut String,
    capturing: bool,
) -> bool /* capture clicked */ {
    let mut clicked = false;
    ui.horizontal(|ui| {
        let valid = crate::keys::parse_key(value).is_some();
        let te = egui::TextEdit::singleline(value).desired_width(140.0);
        let te = if valid {
            te
        } else {
            te.text_color(ui.visuals().error_fg_color)
        };
        ui.add(te);
        egui::ComboBox::from_id_salt(egui::Id::new(&id).with("combo"))
            .selected_text("▾")
            .width(24.0)
            .height(400.0)
            .show_ui(ui, |ui| {
                for n in all_key_names() {
                    ui.selectable_value(value, n.clone(), n);
                }
            });
        let label = if capturing { "… press a key" } else { "🎯" };
        if ui
            .button(label)
            .on_hover_text("Capture: press the key/button you want")
            .clicked()
        {
            clicked = true;
        }
    });
    clicked
}

/// The window: a thin eframe wrapper around the long-lived App.
pub struct Window(pub std::rc::Rc<std::cell::RefCell<App>>);

impl eframe::App for Window {
    fn ui(&mut self, ui: &mut egui::Ui, frame: &mut eframe::Frame) {
        self.0.borrow_mut().frame(ui, frame);
    }
}

impl App {
    fn frame(&mut self, ui: &mut egui::Ui, _frame: &mut eframe::Frame) {
        let c = self.colors;
        let ctx = ui.ctx().clone();
        self.attach(&ctx);
        self.service();
        self.apply_theme(&ctx);
        self.poll_capture();
        if self.capture.is_some() || self.running() {
            ctx.request_repaint_after(Duration::from_millis(100));
        }
        egui::Panel::top("top").show(ui, |ui| self.ui_topbar(ui, &ctx));

        if let Some(e) = self.error.clone() {
            egui::Panel::bottom("bottom").show(ui, |ui| {
                ui.horizontal(|ui| {
                    ui.label(RichText::new(format!("⚠ {e}")).color(c.negative));
                    if ui.small_button("Dismiss").clicked() {
                        self.error = None;
                    }
                });
            });
        }

        // Rescan devices every few seconds so hot-plugged devices appear.
        if self.last_scan.elapsed() > Duration::from_secs(3) && self.capture.is_none() {
            self.rescan();
            self.light_found = crate::lighting::discover();
            self.last_scan = std::time::Instant::now();
        }
        ctx.request_repaint_after(Duration::from_secs(3));

        egui::CentralPanel::default().show(ui, |ui| match self.view.clone() {
            View::Home => {
                egui::ScrollArea::vertical()
                    .auto_shrink([false, false])
                    .show(ui, |ui| self.ui_home(ui));
            }
            View::Device(id, sec) => self.ui_device(ui, &id, sec),
            View::Settings(p) => self.ui_settings(ui, p),
        });

        self.sync_config();
    }
}

impl App {
    /// A (new) window is showing this app: route wake-ups to it and re-apply
    /// theme/fonts, which are per-window egui state.
    fn attach(&mut self, ctx: &egui::Context) {
        if self.window_ctx.as_ref() == Some(ctx) {
            return;
        }
        self.window_ctx = Some(ctx.clone());
        self.waker.set(Some(ctx.clone()));
        self.theme_applier = Applier::default();
    }

    /// The window closed; keep running in the background.
    pub fn detach(&mut self) {
        self.window_ctx = None;
        self.waker.set(None);
        self.capture = None;
        self.sync_config();
        // The window's textures, fonts and layout caches were freed; hand the
        // memory back to the OS instead of keeping it in malloc's free lists.
        unsafe {
            libc::malloc_trim(0);
        }
    }

    pub fn has_tray(&self) -> bool {
        self.tray.as_ref().is_some_and(|t| !t.is_closed())
    }

    pub fn wants_quit(&self) -> bool {
        self.want_quit
    }

    pub fn start_tray(&mut self, tx: std::sync::mpsc::Sender<crate::tray::Cmd>) {
        self.tray_state = self.current_tray_state();
        self.tray = crate::tray::spawn(tx, self.waker.clone(), self.tray_state.clone());
    }

    fn current_tray_state(&self) -> crate::tray::TrayState {
        crate::tray::TrayState {
            active: self.running(),
            profiles: self.cfg.profiles.iter().map(|p| p.name.clone()).collect(),
            profile: self.cfg.active_profile,
            autoclicker: self
                .engine
                .as_ref()
                .is_some_and(|e| e.autoclick.active.load(std::sync::atomic::Ordering::SeqCst)),
            devices: self.cfg.devices.iter().filter(|d| d.enabled).count(),
        }
    }

    /// Process tray/IPC commands and keep the tray in sync. Called every frame
    /// while a window is open and from the background loop otherwise.
    /// Returns true when the window should be (re)opened.
    pub fn service(&mut self) -> bool {
        // Engine may have stopped itself (emergency chord).
        if self.engine.as_ref().is_some_and(|e| e.is_finished()) {
            self.engine = None;
        }
        let mut show = false;
        while let Ok(cmd) = self.cmds.try_recv() {
            show |= self.handle(cmd);
        }
        self.sync_config();
        let st = self.current_tray_state();
        if st != self.tray_state {
            if let Some(t) = &self.tray {
                crate::tray::update(t, &st);
            }
            self.tray_state = st;
        }
        if let Some(ctx) = &self.window_ctx {
            if self.want_quit {
                ctx.send_viewport_cmd(egui::ViewportCommand::Close);
            } else if show {
                ctx.send_viewport_cmd(egui::ViewportCommand::Minimized(false));
                ctx.send_viewport_cmd(egui::ViewportCommand::Visible(true));
                ctx.send_viewport_cmd(egui::ViewportCommand::Focus);
            }
        }
        show
    }

    fn handle(&mut self, cmd: crate::tray::Cmd) -> bool {
        use crate::tray::Cmd;
        match cmd {
            Cmd::Show => return true,
            Cmd::Quit => self.want_quit = true,
            Cmd::SetActive(on) => {
                if on {
                    self.start_engine();
                } else {
                    self.stop_engine();
                }
                self.cfg.autostart_engine = on;
            }
            Cmd::SetProfile(i) if i < self.cfg.profiles.len() => self.cfg.active_profile = i,
            Cmd::SetProfile(_) => {}
            Cmd::ToggleAutoclicker => {
                if let Some(e) = &self.engine {
                    let on = e.autoclick.active.load(std::sync::atomic::Ordering::SeqCst);
                    e.set_autoclicker(!on);
                }
            }
        }
        false
    }

    /// Background mode (no window): block until a command arrives or `d`
    /// passes, then service. Returns true when the window should open.
    pub fn wait(&mut self, d: Duration) -> bool {
        let mut show = false;
        if let Ok(c) = self.cmds.recv_timeout(d) {
            show = self.handle(c);
        }
        self.service() || show
    }

    pub fn shutdown(&mut self) {
        self.stop_engine();
        self.sync_config();
        if let Some(t) = self.tray.take() {
            t.shutdown().wait();
        }
    }
}

impl App {
    fn ui_devices(&mut self, ui: &mut egui::Ui) {
        let c = self.colors;
        ui.horizontal(|ui| {
            if ui.button("🔄 Rescan").clicked() {
                self.rescan();
            }
            ui.label(format!("{} devices", self.devices.len()));
        });
        if !self.uinput_ok || self.unreadable > 0 {
            ui.group(|ui| {
                ui.label(RichText::new("Permissions").strong().color(c.warning));
                if !self.uinput_ok {
                    ui.label("• /dev/uinput is not writable — the engine can't create virtual devices.");
                }
                if self.unreadable > 0 {
                    ui.label(format!(
                        "• {} input devices are not accessible. That's normal: InputForge only gets access to a device when you turn it on (you'll be asked for your password once per device).",
                        self.unreadable
                    ));
                }
                let hint = devices::access_hint();
                if !hint.is_empty() {
                    ui.label(RichText::new(format!("• {hint}")).strong());
                }
            });
        }
        ui.add_space(6.0);
        ui.label("Enable a device to let InputForge grab it. Grabbed devices are exclusively owned by InputForge and re-emitted through a virtual keyboard/pointer, so remaps work in every app, Wayland or X11.");
        ui.add_space(6.0);

        let mut devs = self.devices.clone();
        devs.sort_by_key(|d| (!d.kind.grabbable(), d.name.clone()));
        for d in &devs {
            let idx = self
                .cfg
                .devices
                .iter()
                .position(|s| s.name == d.name && s.phys == d.phys);
            ui.group(|ui| {
                ui.horizontal(|ui| {
                    let mut enabled = idx.map(|i| self.cfg.devices[i].enabled).unwrap_or(false);
                    let resp =
                        ui.add_enabled(d.kind.grabbable(), egui::Checkbox::new(&mut enabled, ""));
                    if resp.changed()
                        && enabled
                        && !d.accessible
                        && let Err(e) = devices::request_access(&[(d.vendor, d.product)])
                    {
                        self.error = Some(format!("Can't use {}: {e}", d.name));
                        enabled = false;
                    }
                    if resp.changed() && (enabled || idx.is_some()) {
                        match idx {
                            Some(i) => self.cfg.devices[i].enabled = enabled,
                            None => self.cfg.devices.push(DeviceSettings {
                                name: d.name.clone(),
                                phys: d.phys.clone(),
                                enabled,
                                ..Default::default()
                            }),
                        }
                    }
                    ui.label(RichText::new(&d.name).strong());
                    ui.label(RichText::new(d.kind.label()).color(c.accent));
                    ui.label(
                        RichText::new(format!(
                            "{} · {:04x}:{:04x}",
                            d.path.display(),
                            d.vendor,
                            d.product
                        ))
                        .small()
                        .color(c.muted),
                    );
                });
                if let Some(i) = idx
                    && self.cfg.devices[i].enabled
                    && matches!(
                        d.kind,
                        devices::DeviceKind::Mouse | devices::DeviceKind::Combo
                    )
                {
                    let s = &mut self.cfg.devices[i];
                    ui.horizontal(|ui| {
                        ui.add(
                            egui::Slider::new(&mut s.pointer_speed, 0.1..=5.0)
                                .text("pointer speed ×"),
                        );
                        ui.add(
                            egui::Slider::new(&mut s.scroll_speed, 0.25..=5.0)
                                .text("scroll speed ×"),
                        );
                        ui.checkbox(&mut s.invert_scroll, "Invert scroll");
                    });
                }
            });
        }
    }

    fn ui_remap(&mut self, ui: &mut egui::Ui) {
        let c = self.colors;
        // Profile management
        ui.horizontal(|ui| {
            let p = self.cfg.active_profile;
            ui.label("Profile name:");
            ui.text_edit_singleline(&mut self.cfg.profiles[p].name);
            if ui.button("➕ New profile").clicked() {
                self.cfg.profiles.push(Profile {
                    name: format!("Profile {}", self.cfg.profiles.len() + 1),
                    rules: vec![],
                });
                self.cfg.active_profile = self.cfg.profiles.len() - 1;
            }
            if ui.button("⧉ Duplicate").clicked() {
                let mut np = self.cfg.profiles[p].clone();
                np.name += " copy";
                self.cfg.profiles.push(np);
                self.cfg.active_profile = self.cfg.profiles.len() - 1;
            }
            if self.cfg.profiles.len() > 1 && ui.button("🗑 Delete profile").clicked() {
                self.cfg.profiles.remove(p);
                self.cfg.active_profile = 0;
            }
        });
        ui.separator();
        if !self.running() {
            ui.label(
                RichText::new("Engine is stopped — rules take effect after you press ▶ Start.")
                    .color(c.warning),
            );
        }

        let macro_names: Vec<String> = self.cfg.macros.iter().map(|m| m.name.clone()).collect();
        let p = self.cfg.active_profile;
        let mut remove = None;
        let mut capture = None;
        let n = self.cfg.profiles[p].rules.len();
        for i in 0..n {
            let rule = &mut self.cfg.profiles[p].rules[i];
            ui.group(|ui| {
                ui.horizontal(|ui| {
                    ui.checkbox(&mut rule.enabled, "");
                    ui.label("When");
                    if key_field(
                        ui,
                        ("trig", i),
                        &mut rule.trigger,
                        self.capture == Some(CaptureTarget::RuleTrigger(i)),
                    ) {
                        capture = Some(CaptureTarget::RuleTrigger(i));
                    }
                    ui.label("is pressed →");
                    let cur = rule.action.label();
                    egui::ComboBox::from_id_salt(("act", i))
                        .selected_text(cur)
                        .show_ui(ui, |ui| {
                            let opts = [
                                Action::Key {
                                    key: "KEY_ESC".into(),
                                },
                                Action::Combo {
                                    keys: vec!["KEY_LEFTCTRL".into(), "KEY_C".into()],
                                },
                                Action::Macro {
                                    name: macro_names.first().cloned().unwrap_or_default(),
                                },
                                Action::Disabled,
                                Action::ToggleAutoclicker,
                            ];
                            for o in opts {
                                let l = o.label();
                                if ui.selectable_label(cur == l, l).clicked() && cur != l {
                                    rule.action = o;
                                }
                            }
                        });
                    if ui.button("🗑").clicked() {
                        remove = Some(i);
                    }
                });
                ui.horizontal(|ui| {
                    ui.add_space(24.0);
                    match &mut rule.action {
                        Action::Key { key } => {
                            ui.label("send");
                            if key_field(
                                ui,
                                ("rk", i),
                                key,
                                self.capture == Some(CaptureTarget::RuleKey(i)),
                            ) {
                                capture = Some(CaptureTarget::RuleKey(i));
                            }
                        }
                        Action::Combo { keys } => {
                            ui.label("send");
                            let mut del = None;
                            for (j, k) in keys.iter().enumerate() {
                                if j > 0 {
                                    ui.label("+");
                                }
                                if ui
                                    .button(k.trim_start_matches("KEY_"))
                                    .on_hover_text("click to remove")
                                    .clicked()
                                {
                                    del = Some(j);
                                }
                            }
                            if let Some(j) = del {
                                keys.remove(j);
                            }
                            let lbl = if self.capture == Some(CaptureTarget::ComboAdd(i)) {
                                "… press"
                            } else {
                                "➕ key"
                            };
                            if ui.button(lbl).clicked() {
                                capture = Some(CaptureTarget::ComboAdd(i));
                            }
                        }
                        Action::Macro { name } => {
                            ui.label("run");
                            egui::ComboBox::from_id_salt(("mac", i))
                                .selected_text(name.as_str())
                                .show_ui(ui, |ui| {
                                    for m in &macro_names {
                                        ui.selectable_value(name, m.clone(), m);
                                    }
                                });
                            if macro_names.is_empty() {
                                ui.label(
                                    RichText::new("(create a macro on the Macros tab)")
                                        .color(c.muted),
                                );
                            }
                        }
                        Action::Disabled => {
                            ui.label(RichText::new("key is swallowed").color(c.muted));
                        }
                        Action::ToggleAutoclicker => {
                            ui.label(RichText::new("starts/stops the autoclicker").color(c.muted));
                        }
                    }
                    ui.label("  only on device containing:");
                    ui.add(
                        egui::TextEdit::singleline(&mut rule.device_filter)
                            .hint_text("any")
                            .desired_width(120.0),
                    );
                });
            });
        }
        if let Some(i) = remove {
            self.cfg.profiles[p].rules.remove(i);
        }
        if let Some(t) = capture {
            self.begin_capture(t);
        }
        if ui.button("➕ Add rule").clicked() {
            self.cfg.profiles[p].rules.push(Rule::default());
        }
    }

    fn ui_macros(&mut self, ui: &mut egui::Ui) {
        let c = self.colors;
        ui.horizontal(|ui| {
            if ui.button("➕ New macro").clicked() {
                self.cfg.macros.push(Macro {
                    name: format!("macro{}", self.cfg.macros.len() + 1),
                    mode: MacroMode::Once,
                    steps: vec![],
                });
                self.selected_macro = self.cfg.macros.len() - 1;
            }
            for (i, m) in self.cfg.macros.iter().enumerate() {
                ui.selectable_value(&mut self.selected_macro, i, &m.name);
            }
        });
        ui.separator();
        let mi = self.selected_macro;
        if mi >= self.cfg.macros.len() {
            ui.label("No macro selected. Create one, then bind it to a key on the Remap tab with the \"Run macro\" action.");
            return;
        }
        let mut delete = false;
        let mut capture = None;
        let mut test = None;
        {
            let m = &mut self.cfg.macros[mi];
            ui.horizontal(|ui| {
                ui.label("Name:");
                ui.text_edit_singleline(&mut m.name);
                ui.label("Mode:");
                ui.selectable_value(&mut m.mode, MacroMode::Once, "Once per press");
                ui.selectable_value(&mut m.mode, MacroMode::WhileHeld, "Repeat while held");
                ui.selectable_value(&mut m.mode, MacroMode::Toggle, "Toggle loop");
            });
            ui.horizontal(|ui| {
                if ui.button("▶ Test (runs in 2s)").clicked() {
                    test = Some(m.clone());
                }
                ui.label(self.macro_test_status.lock().unwrap().as_str());
                if ui.button("🗑 Delete macro").clicked() {
                    delete = true;
                }
            });
            ui.add_space(6.0);
            let mut remove = None;
            let mut swap = None;
            let n = m.steps.len();
            for (s, step) in m.steps.iter_mut().enumerate() {
                ui.horizontal(|ui| {
                    ui.label(format!("{:>2}.", s + 1));
                    if ui.small_button("⬆").clicked() && s > 0 {
                        swap = Some((s, s - 1));
                    }
                    if ui.small_button("⬇").clicked() && s + 1 < n {
                        swap = Some((s, s + 1));
                    }
                    ui.label(RichText::new(step.label()).strong());
                    match step {
                        Step::Tap { key } | Step::KeyDown { key } | Step::KeyUp { key } => {
                            if key_field(
                                ui,
                                ("st", mi, s),
                                key,
                                self.capture == Some(CaptureTarget::StepKey(mi, s)),
                            ) {
                                capture = Some(CaptureTarget::StepKey(mi, s));
                            }
                        }
                        Step::Text { text } => {
                            ui.text_edit_singleline(text);
                        }
                        Step::Delay { ms } => {
                            ui.add(egui::DragValue::new(ms).range(0..=600_000).suffix(" ms"));
                        }
                        Step::MouseMove { dx, dy } => {
                            ui.label("dx");
                            ui.add(egui::DragValue::new(dx));
                            ui.label("dy");
                            ui.add(egui::DragValue::new(dy));
                        }
                        Step::Scroll { amount } => {
                            ui.add(
                                egui::DragValue::new(amount)
                                    .range(-50..=50)
                                    .suffix(" notches"),
                            );
                        }
                    }
                    if ui.small_button("🗑").clicked() {
                        remove = Some(s);
                    }
                });
            }
            if let Some(s) = remove {
                m.steps.remove(s);
            }
            if let Some((a, b)) = swap {
                m.steps.swap(a, b);
            }
            ui.add_space(6.0);
            ui.horizontal_wrapped(|ui| {
                ui.label("Add step:");
                let add: [(&str, Step); 7] = [
                    (
                        "Tap key",
                        Step::Tap {
                            key: "KEY_A".into(),
                        },
                    ),
                    (
                        "Key down",
                        Step::KeyDown {
                            key: "KEY_LEFTCTRL".into(),
                        },
                    ),
                    (
                        "Key up",
                        Step::KeyUp {
                            key: "KEY_LEFTCTRL".into(),
                        },
                    ),
                    (
                        "Type text",
                        Step::Text {
                            text: String::new(),
                        },
                    ),
                    ("Delay", Step::Delay { ms: 50 }),
                    ("Mouse move", Step::MouseMove { dx: 10, dy: 0 }),
                    ("Scroll", Step::Scroll { amount: 1 }),
                ];
                for (l, st) in add {
                    if ui.button(l).clicked() {
                        m.steps.push(st);
                    }
                }
                ui.label(
                    RichText::new("· Tip: Tap BTN_LEFT for a mouse click")
                        .small()
                        .color(c.muted),
                );
            });
        }
        if delete {
            self.cfg.macros.remove(mi);
            self.selected_macro = 0;
        }
        if let Some(t) = capture {
            self.begin_capture(t);
        }
        if let Some(m) = test {
            let st = self.macro_test_status.clone();
            *st.lock().unwrap() = "running in 2s…".into();
            std::thread::spawn(move || {
                std::thread::sleep(Duration::from_secs(2));
                let r = engine::test_macro(m);
                *st.lock().unwrap() = match r {
                    Ok(()) => "✔ done".into(),
                    Err(e) => format!("✖ {e}"),
                };
            });
        }
    }

    fn ui_autoclicker(&mut self, ui: &mut egui::Ui) {
        let c = self.colors;
        let mut capture = None;
        let ac = &mut self.cfg.autoclicker;
        egui::Grid::new("ac")
            .num_columns(2)
            .spacing([12.0, 8.0])
            .show(ui, |ui| {
                ui.label("Button to click");
                if key_field(
                    ui,
                    "acbtn",
                    &mut ac.button,
                    self.capture == Some(CaptureTarget::AutoButton),
                ) {
                    capture = Some(CaptureTarget::AutoButton);
                }
                ui.end_row();
                ui.label("Clicks per second");
                ui.add(egui::Slider::new(&mut ac.clicks_per_second, 0.5..=100.0).logarithmic(true));
                ui.end_row();
                ui.label("Stop after N clicks");
                ui.add(egui::DragValue::new(&mut ac.max_clicks).suffix(" (0 = never)"));
                ui.end_row();
                ui.label("Toggle hotkey");
                if key_field(
                    ui,
                    "achk",
                    &mut ac.hotkey,
                    self.capture == Some(CaptureTarget::AutoHotkey),
                ) {
                    capture = Some(CaptureTarget::AutoHotkey);
                }
                ui.end_row();
            });
        if let Some(t) = capture {
            self.begin_capture(t);
        }
        ui.add_space(10.0);
        ui.label("The hotkey works on any grabbed device while the engine is running. You can also bind \"Toggle autoclicker\" to any key/mouse button on the Remap tab.");
        ui.add_space(10.0);
        match &self.engine {
            Some(e) if !e.is_finished() => {
                let on = e.autoclick.active.load(std::sync::atomic::Ordering::SeqCst);
                let n = e.autoclick.clicks.load(std::sync::atomic::Ordering::SeqCst);
                ui.horizontal(|ui| {
                    if on {
                        ui.label(RichText::new(format!("Clicking — {n} clicks")).color(c.positive));
                        if ui.button("⏹ Stop").clicked() {
                            e.set_autoclicker(false);
                        }
                    } else {
                        ui.label(format!("Idle ({n} clicks last run)"));
                        if ui.button("▶ Start").clicked() {
                            e.set_autoclicker(true);
                        }
                    }
                });
            }
            _ => {
                ui.label(
                    RichText::new("Start the engine to use the autoclicker.").color(c.warning),
                );
            }
        }
    }

    fn lighting_status(&self, ui: &mut egui::Ui) {
        if let Some(f) = self.light_found.iter().find(|f| !f.accessible) {
            ui.label(
                RichText::new(format!(
                    "⚠ No permission to change lighting ({}). Run the installer again.",
                    f.path.display()
                ))
                .color(self.colors.warning),
            );
        }
        let status = self.lighting.status.lock().unwrap().clone();
        if status.contains('✖') {
            ui.label(RichText::new(status).color(self.colors.negative));
        }
    }

    fn swatches(ui: &mut egui::Ui, target: &mut [u8; 3]) {
        ui.horizontal(|ui| {
            egui::color_picker::color_edit_button_srgb(ui, target);
            ui.add_space(6.0);
            for p in PRESETS {
                let (r, resp) =
                    ui.allocate_exact_size(egui::vec2(26.0, 26.0), egui::Sense::click());
                let sel = *target == p;
                ui.painter().circle_filled(
                    r.center(),
                    11.0,
                    egui::Color32::from_rgb(p[0], p[1], p[2]),
                );
                if sel || resp.hovered() {
                    ui.painter().circle_stroke(
                        r.center(),
                        13.0,
                        egui::Stroke::new(2.0, ui.visuals().strong_text_color()),
                    );
                }
                if resp
                    .on_hover_cursor(egui::CursorIcon::PointingHand)
                    .clicked()
                {
                    *target = p;
                }
            }
        });
    }

    fn ui_kb_lighting(&mut self, ui: &mut egui::Ui, model: crate::lighting::Model) {
        use crate::lighting::{self as l, KbMode};
        let muted = self.colors.muted;
        self.lighting_status(ui);
        let before = self.cfg.lighting.keyboard.clone();
        let k = &mut self.cfg.lighting.keyboard;
        ui.horizontal(|ui| {
            for m in KbMode::ALL {
                let label = m.label();
                ui.add(
                    egui::Button::new(RichText::new(label).size(14.0))
                        .selected(k.mode == m)
                        .min_size(egui::vec2(90.0, 30.0)),
                )
                .clicked()
                .then(|| k.mode = m);
            }
        });
        ui.add_space(12.0);
        if matches!(k.mode, KbMode::Static | KbMode::Breathing) {
            Self::swatches(ui, &mut k.color);
            ui.add_space(10.0);
        }
        if matches!(k.mode, KbMode::Breathing | KbMode::Cycle) {
            ui.horizontal(|ui| {
                ui.label("Speed");
                // slider: left = slow, right = fast
                let mut speed = 21.0 - k.period_ms as f32 / 1000.0;
                if ui
                    .add(egui::Slider::new(&mut speed, 1.0..=20.0).show_value(false))
                    .changed()
                {
                    k.period_ms = ((21.0 - speed) * 1000.0) as u32;
                }
            });
            ui.add_space(10.0);
        }
        if k.mode == KbMode::PerKey {
            k.keys.resize(l::G815_LEDS.len(), k.color);
            ui.horizontal(|ui| {
                ui.label("Brush");
                Self::swatches(ui, &mut self.brush);
                if ui.button("Fill all").clicked() {
                    k.keys.iter_mut().for_each(|c| *c = self.brush);
                }
            });
            ui.label(
                RichText::new(
                    "Click or drag across keys to paint. Right-click a key to pick its color.",
                )
                .color(muted)
                .small(),
            );
            ui.add_space(6.0);
            keyboard_painter(ui, model, &mut k.keys, &mut self.brush);
        } else {
            // Preview
            let base = match k.mode {
                KbMode::Off => [30, 30, 30],
                _ => k.color,
            };
            let mut preview = vec![base; l::G815_LEDS.len()];
            let mut dummy = self.brush;
            ui.add_enabled_ui(false, |ui| {
                keyboard_painter(ui, model, &mut preview, &mut dummy)
            });
        }
        if self.cfg.lighting.keyboard != before {
            self.lighting.apply_keyboard(&self.cfg.lighting.keyboard);
        }
    }

    fn ui_mouse_lighting(&mut self, ui: &mut egui::Ui) {
        use crate::lighting::MouseMode;
        let muted = self.colors.muted;
        self.lighting_status(ui);
        let before = self.cfg.lighting.mouse.clone();
        let kb_color = self.cfg.lighting.keyboard.color;
        let m = &mut self.cfg.lighting.mouse;
        ui.horizontal(|ui| {
            for md in MouseMode::ALL {
                let label = md.label();
                ui.add(
                    egui::Button::new(RichText::new(label).size(14.0))
                        .selected(m.mode == md)
                        .min_size(egui::vec2(90.0, 30.0)),
                )
                .clicked()
                .then(|| m.mode = md);
            }
        });
        ui.add_space(12.0);
        if matches!(m.mode, MouseMode::Static | MouseMode::Breathing) {
            Self::swatches(ui, &mut m.color);
            if ui.button("Match keyboard").clicked() {
                m.color = kb_color;
            }
            ui.add_space(10.0);
        }
        if matches!(m.mode, MouseMode::Breathing | MouseMode::Cycle) {
            ui.horizontal(|ui| {
                ui.label("Speed");
                let mut speed = 16 - m.period_s as i32;
                if ui
                    .add(egui::Slider::new(&mut speed, 1..=15).show_value(false))
                    .changed()
                {
                    m.period_s = (16 - speed) as u8;
                }
            });
        }
        ui.add_space(12.0);
        ui.label(
            RichText::new("The G-shift button's color is stored on the mouse itself.")
                .color(muted)
                .small(),
        );
        if self.cfg.lighting.mouse != before {
            self.lighting.apply_mouse(&self.cfg.lighting.mouse);
        }
    }

    fn ui_hardware(&mut self, ui: &mut egui::Ui) {
        let c = self.colors;
        // ── ratbagd ──
        ui.heading("Mouse DPI / polling rate / onboard LEDs (libratbag)");
        ui.horizontal(|ui| {
            if ui.button("🔌 Connect to ratbagd").clicked() {
                match Ratbag::connect().and_then(|r| r.devices().map(|d| (r, d))) {
                    Ok((r, d)) => {
                        self.rb_status = format!("{} device(s) supported by ratbagd", d.len());
                        self.rb_edit_dpi = d
                            .iter()
                            .map(|dev| {
                                dev.profiles
                                    .iter()
                                    .find(|p| p.active)
                                    .or(dev.profiles.first())
                                    .map(|p| p.resolutions.iter().map(|r| r.dpi).collect())
                                    .unwrap_or_default()
                            })
                            .collect();
                        self.rb_devices = d;
                        self.ratbag = Some(r);
                    }
                    Err(e) => self.rb_status = e.to_string(),
                }
            }
            ui.label(&self.rb_status);
        });
        let mut action: Option<Box<dyn FnOnce(&Ratbag) -> anyhow::Result<()>>> = None;
        for (di, dev) in self.rb_devices.iter().enumerate() {
            ui.group(|ui| {
                ui.label(RichText::new(format!("{}  ({})", dev.name, dev.model)).strong());
                let Some(prof) = dev
                    .profiles
                    .iter()
                    .find(|p| p.active)
                    .or(dev.profiles.first())
                else {
                    ui.label("No profiles");
                    return;
                };
                ui.label(format!("Onboard profile {}", prof.index));
                for (ri, res) in prof.resolutions.iter().enumerate() {
                    if res.disabled {
                        continue;
                    }
                    ui.horizontal(|ui| {
                        if ui
                            .radio(res.active, "")
                            .on_hover_text("make active DPI stage")
                            .clicked()
                        {
                            let p = res.path.clone();
                            let d = dev.path.clone();
                            action = Some(Box::new(move |r| {
                                r.set_active_resolution(&p)?;
                                r.commit(&d)
                            }));
                        }
                        ui.label(format!("Stage {ri}:"));
                        let dpi = &mut self.rb_edit_dpi[di][ri];
                        if prof.dpi_choices.is_empty() {
                            ui.add(
                                egui::DragValue::new(dpi)
                                    .range(100..=30000)
                                    .speed(50)
                                    .suffix(" DPI"),
                            );
                        } else {
                            egui::ComboBox::from_id_salt(("dpi", di, ri))
                                .selected_text(format!("{dpi} DPI"))
                                .height(300.0)
                                .show_ui(ui, |ui| {
                                    for c in &prof.dpi_choices {
                                        ui.selectable_value(dpi, *c, c.to_string());
                                    }
                                });
                        }
                        if *dpi != res.dpi && ui.button("Apply").clicked() {
                            let (p, d, v) = (res.path.clone(), dev.path.clone(), *dpi);
                            action = Some(Box::new(move |r| {
                                r.set_dpi(&p, v)?;
                                r.commit(&d)
                            }));
                        }
                    });
                }
                if !prof.report_rates.is_empty() {
                    ui.horizontal(|ui| {
                        ui.label("Polling rate:");
                        for hz in &prof.report_rates {
                            if ui
                                .selectable_label(*hz == prof.report_rate, format!("{hz} Hz"))
                                .clicked()
                            {
                                let (p, d, v) = (prof.path.clone(), dev.path.clone(), *hz);
                                action = Some(Box::new(move |r| {
                                    r.set_report_rate(&p, v)?;
                                    r.commit(&d)
                                }));
                            }
                        }
                    });
                }
                for (li, led) in prof.leds.iter().enumerate() {
                    ui.horizontal(|ui| {
                        ui.label(format!("LED {li}:"));
                        for m in &led.modes {
                            if ui
                                .selectable_label(*m == led.mode, hardware::led_mode_name(*m))
                                .clicked()
                            {
                                let (p, d, m, c, b) = (
                                    led.path.clone(),
                                    dev.path.clone(),
                                    *m,
                                    led.color,
                                    led.brightness,
                                );
                                action = Some(Box::new(move |r| {
                                    r.set_led(&p, m, c, b)?;
                                    r.commit(&d)
                                }));
                            }
                        }
                        let mut c = led.color;
                        if ui.color_edit_button_srgb(&mut c).changed() {
                            let (p, d, m, b) = (
                                led.path.clone(),
                                dev.path.clone(),
                                led.mode.max(1),
                                led.brightness,
                            );
                            action = Some(Box::new(move |r| {
                                r.set_led(&p, m, c, b)?;
                                r.commit(&d)
                            }));
                        }
                    });
                }
            });
        }
        if let (Some(a), Some(rb)) = (action, &self.ratbag) {
            match a(rb).and_then(|_| rb.devices()) {
                Ok(d) => {
                    self.rb_status = "✔ written to device".into();
                    self.rb_devices = d;
                }
                Err(e) => self.rb_status = format!("✖ {e}"),
            }
        }

        ui.add_space(14.0);
        // ── OpenRGB ──
        ui.heading("RGB lighting (OpenRGB SDK)");
        ui.horizontal(|ui| {
            ui.label("Server:");
            ui.add(egui::TextEdit::singleline(&mut self.openrgb_addr).desired_width(140.0));
            if ui.button("🔌 Connect").clicked() {
                match OpenRgb::connect(&self.openrgb_addr)
                    .and_then(|mut c| c.controllers().map(|v| (c, v)))
                {
                    Ok((c, v)) => {
                        self.rgb_status =
                            format!("protocol v{}, {} controller(s)", c.protocol(), v.len());
                        self.rgb_pick = v
                            .iter()
                            .map(|c| c.colors.first().copied().unwrap_or([255, 255, 255]))
                            .collect();
                        self.rgb_ctrls = v;
                        self.openrgb = Some(c);
                    }
                    Err(e) => self.rgb_status = e.to_string(),
                }
            }
            ui.label(&self.rgb_status);
        });
        let mut send: Option<(u32, [u8; 3], usize)> = None;
        for (i, ctl) in self.rgb_ctrls.iter().enumerate() {
            ui.horizontal(|ui| {
                ui.label(RichText::new(&ctl.name).strong());
                ui.label(
                    RichText::new(format!(
                        "{} · {} LEDs · zones: {}",
                        ctl.vendor,
                        ctl.num_leds,
                        ctl.zones
                            .iter()
                            .map(|(n, k)| format!("{n}({k})"))
                            .collect::<Vec<_>>()
                            .join(", ")
                    ))
                    .color(c.muted),
                );
                ui.color_edit_button_srgb(&mut self.rgb_pick[i]);
                if ui.button("Apply to all LEDs").clicked() {
                    send = Some((ctl.id, self.rgb_pick[i], ctl.num_leds));
                }
                if ui.button("Off").clicked() {
                    send = Some((ctl.id, [0, 0, 0], ctl.num_leds));
                }
            });
        }
        if ui
            .add_enabled(
                !self.rgb_ctrls.is_empty(),
                egui::Button::new("Apply first color to every device"),
            )
            .clicked()
            && let Some(col) = self.rgb_pick.first().copied()
        {
            for c in &self.rgb_ctrls {
                if let Some(o) = &mut self.openrgb {
                    let _ = o
                        .set_custom_mode(c.id)
                        .and_then(|_| o.update_leds(c.id, &vec![col; c.num_leds]));
                }
            }
        }
        if let (Some((id, col, n)), Some(o)) = (send, &mut self.openrgb) {
            if let Err(e) = o
                .set_custom_mode(id)
                .and_then(|_| o.update_leds(id, &vec![col; n]))
            {
                self.rgb_status = format!("✖ {e} — reconnect");
                self.openrgb = None;
            }
        }
    }

    fn ui_log(&mut self, ui: &mut egui::Ui) {
        let sh = self.shared.lock().unwrap();
        ui.label(
            RichText::new(format!(
                "Status: {}",
                if sh.status.is_empty() {
                    "Stopped"
                } else {
                    &sh.status
                }
            ))
            .strong(),
        );
        if !sh.grabbed.is_empty() {
            ui.label("Grabbed devices:");
            for g in &sh.grabbed {
                ui.label(format!("  • {g}"));
            }
        }
        ui.separator();
        for l in sh.log.iter().rev() {
            ui.monospace(l);
        }
    }
}

const PRESETS: [[u8; 3]; 8] = [
    [255, 255, 255],
    [255, 0, 0],
    [255, 100, 0],
    [255, 220, 0],
    [0, 255, 60],
    [0, 200, 255],
    [40, 60, 255],
    [190, 0, 255],
];

/// Physical G815 layout: (led index into G815_LEDS, x, y, w, h) in key units.
fn g815_layout() -> &'static [(usize, f32, f32, f32, f32)] {
    use std::sync::OnceLock;
    static L: OnceLock<Vec<(usize, f32, f32, f32, f32)>> = OnceLock::new();
    L.get_or_init(|| {
        let mut v = vec![];
        let x0 = 1.5; // G-key column on the left
        let mut k = |led: usize, x: f32, y: f32, w: f32, h: f32| v.push((led, x0 + x, y, w, h));
        let letter = |c: char| (c as u8 - b'A') as usize;
        let digit = |d: u8| if d == 0 { 35 } else { 25 + d as usize };
        // top strip: logo, brightness, media
        k(110, 7.0, 0.0, 1.0, 1.0);
        k(111, 15.25, 0.0, 1.0, 1.0);
        for (i, led) in [106, 107, 108, 109].into_iter().enumerate() {
            k(led, 18.5 + i as f32, 0.0, 1.0, 1.0);
        }
        // function row
        let y = 1.25;
        k(37, 0.0, y, 1.0, 1.0);
        for i in 0..12 {
            let gap = (i / 4) as f32 * 0.5;
            k(54 + i, 2.0 + i as f32 + gap, y, 1.0, 1.0);
        }
        for i in 0..3 {
            k(66 + i, 15.25 + i as f32, y, 1.0, 1.0);
        }
        // number row
        let y = 2.5;
        k(49, 0.0, y, 1.0, 1.0);
        for d in 1..=10u8 {
            k(digit(d % 10), d as f32, y, 1.0, 1.0);
        }
        k(41, 11.0, y, 1.0, 1.0);
        k(42, 12.0, y, 1.0, 1.0);
        k(38, 13.0, y, 2.0, 1.0);
        for i in 0..3 {
            k(69 + i, 15.25 + i as f32, y, 1.0, 1.0);
        }
        for (i, led) in [79, 80, 81, 82].into_iter().enumerate() {
            k(led, 18.5 + i as f32, y, 1.0, 1.0);
        }
        // tab row
        let y = 3.5;
        k(39, 0.0, y, 1.5, 1.0);
        for (i, c) in "QWERTYUIOP".chars().enumerate() {
            k(letter(c), 1.5 + i as f32, y, 1.0, 1.0);
        }
        k(43, 11.5, y, 1.0, 1.0);
        k(44, 12.5, y, 1.0, 1.0);
        k(45, 13.5, y, 1.5, 1.0);
        for i in 0..3 {
            k(72 + i, 15.25 + i as f32, y, 1.0, 1.0);
        }
        for i in 0..3 {
            k(91 + i, 18.5 + i as f32, y, 1.0, 1.0); // KP7-9
        }
        k(83, 21.5, y, 1.0, 2.0); // KP+
        // caps row
        let y = 4.5;
        k(53, 0.0, y, 1.75, 1.0);
        for (i, c) in "ASDFGHJKL".chars().enumerate() {
            k(letter(c), 1.75 + i as f32, y, 1.0, 1.0);
        }
        k(47, 10.75, y, 1.0, 1.0);
        k(48, 11.75, y, 1.0, 1.0);
        k(36, 12.75, y, 2.25, 1.0);
        for i in 0..3 {
            k(88 + i, 18.5 + i as f32, y, 1.0, 1.0); // KP4-6
        }
        // shift row
        let y = 5.5;
        k(99, 0.0, y, 2.25, 1.0);
        for (i, c) in "ZXCVBNM".chars().enumerate() {
            k(letter(c), 2.25 + i as f32, y, 1.0, 1.0);
        }
        k(50, 9.25, y, 1.0, 1.0);
        k(51, 10.25, y, 1.0, 1.0);
        k(52, 11.25, y, 1.0, 1.0);
        k(103, 12.25, y, 2.75, 1.0);
        k(78, 16.25, y, 1.0, 1.0); // Up
        for i in 0..3 {
            k(85 + i, 18.5 + i as f32, y, 1.0, 1.0); // KP1-3
        }
        k(84, 21.5, y, 1.0, 2.0); // KP Enter
        // bottom row
        let y = 6.5;
        for (led, x, w) in [
            (98, 0.0, 1.25),
            (101, 1.25, 1.25),
            (100, 2.5, 1.25),
            (40, 3.75, 6.25),
            (104, 10.0, 1.25),
            (105, 11.25, 1.25),
            (97, 12.5, 1.25),
            (102, 13.75, 1.25),
            (76, 15.25, 1.0),
            (77, 16.25, 1.0),
            (75, 17.25, 1.0),
            (94, 18.5, 2.0),
            (95, 20.5, 1.0),
        ] {
            k(led, x, y, w, 1.0);
        }
        // G-keys
        for i in 0..5 {
            v.push((112 + i, 0.0, 2.5 + i as f32, 1.0, 1.0));
        }
        v
    })
}

/// The layout for one model. The G915 TKL has no numpad and no G-keys, so those are
/// dropped, the rest shifts left and its media keys move up to the right edge.
fn layout_for(model: crate::lighting::Model) -> Vec<(usize, f32, f32, f32, f32)> {
    let full = g815_layout();
    if model != crate::lighting::Model::G915Tkl {
        return full.to_vec();
    }
    full.iter()
        .filter(|(led, ..)| model.has_led(*led))
        .map(|&(led, x, y, w, h)| {
            let x = x - 1.5; // no G-key column
            let x = match led {
                106..=109 => x - 3.25, // media keys: 15.25..18.25
                111 => 14.25,          // brightness
                _ => x,
            };
            (led, x, y, w, h)
        })
        .collect()
}

fn layout_width(model: crate::lighting::Model) -> f32 {
    if model == crate::lighting::Model::G915Tkl {
        19.25
    } else {
        24.0
    }
}

/// Draw the G815 layout; click/drag paints with `brush`, right-click samples.
fn keyboard_painter(
    ui: &mut egui::Ui,
    model: crate::lighting::Model,
    keys: &mut [[u8; 3]],
    brush: &mut [u8; 3],
) {
    use crate::lighting::G815_LEDS;
    let layout = layout_for(model);
    let (units_w, units_h) = (layout_width(model), 7.5_f32);
    let pad_u = 0.35;
    let avail = ui.available_width().min(1200.0);
    let u = (avail / (units_w + 2.0 * pad_u)).clamp(20.0, 46.0);
    let size = egui::vec2(u * (units_w + 2.0 * pad_u), u * (units_h + 2.0 * pad_u));
    let (rect, resp) = ui.allocate_exact_size(size, egui::Sense::click_and_drag());
    let painter = ui.painter_at(rect);
    painter.rect_filled(rect, u * 0.4, egui::Color32::from_gray(24));
    painter.rect_stroke(
        rect,
        u * 0.4,
        egui::Stroke::new(1.0, egui::Color32::from_gray(70)),
        egui::StrokeKind::Inside,
    );
    let origin = rect.min + egui::vec2(u * pad_u, u * pad_u);
    let interactive = ui.is_enabled();
    let pointer = if interactive {
        resp.interact_pointer_pos().or(resp.hover_pos())
    } else {
        None
    };
    let fg = ui.visuals().strong_text_color();
    for &(led, x, y, w, h) in &layout {
        let kr =
            egui::Rect::from_min_size(origin + egui::vec2(x * u, y * u), egui::vec2(w * u, h * u))
                .shrink(u * 0.07);
        let hovered = pointer.is_some_and(|p| kr.contains(p));
        if hovered {
            if resp.dragged_by(egui::PointerButton::Primary)
                || resp.clicked_by(egui::PointerButton::Primary)
            {
                keys[led] = *brush;
            } else if resp.clicked_by(egui::PointerButton::Secondary) {
                *brush = keys[led];
            }
        }
        let [cr, cg, cb] = keys[led];
        let lit = egui::Color32::from_rgb(cr, cg, cb);
        // Dark keycap with the light shining through: cap + colored legend/glow.
        painter.rect_filled(kr, u * 0.14, egui::Color32::from_gray(38));
        painter.rect_filled(kr.shrink(u * 0.06), u * 0.1, lit.gamma_multiply(0.28));
        painter.rect_stroke(
            kr,
            u * 0.14,
            egui::Stroke::new(
                if hovered { 2.0 } else { 1.2 },
                if hovered {
                    fg
                } else {
                    lit.gamma_multiply(0.85)
                },
            ),
            egui::StrokeKind::Inside,
        );
        let label = G815_LEDS[led].0;
        let fs = (u * 0.34)
            .min(w * u * 0.9 / (label.chars().count() as f32 * 0.6))
            .max(7.0);
        // Brighten the legend so dark colors stay readable.
        let legend = if (cr as u32 + cg as u32 + cb as u32) < 90 {
            egui::Color32::from_gray(110)
        } else {
            lit.lerp_to_gamma(egui::Color32::WHITE, 0.35)
        };
        painter.text(
            kr.center(),
            egui::Align2::CENTER_CENTER,
            label,
            egui::FontId::proportional(fs),
            legend,
        );
    }
}

#[cfg(test)]
mod layout_tests {
    #[test]
    fn every_led_drawn_once_no_overlap() {
        let l = super::g815_layout();
        let mut seen = std::collections::HashSet::new();
        for &(led, ..) in l {
            assert!(seen.insert(led), "led {led} drawn twice");
        }
        // US G815 has no ISO keys (#, extra \); everything else must be present.
        let n = crate::lighting::G815_LEDS.len();
        let missing: Vec<_> = (0..n).filter(|i| !seen.contains(i)).collect();
        assert_eq!(missing, vec![46, 96]);
    }

    #[test]
    fn tkl_layout_has_no_numpad_or_gkeys_and_fits() {
        use crate::lighting::Model;
        let l = super::layout_for(Model::G915Tkl);
        let seen: std::collections::HashSet<_> = l.iter().map(|k| k.0).collect();
        for led in 0..crate::lighting::G815_LEDS.len() {
            assert_eq!(
                seen.contains(&led),
                Model::G915Tkl.has_led(led) && led != 46 && led != 96,
                "led {led}"
            );
        }
        let w = super::layout_width(Model::G915Tkl);
        for (i, a) in l.iter().enumerate() {
            assert!(
                a.1 >= 0.0 && a.1 + a.3 <= w + 0.01,
                "key {} out of bounds",
                a.0
            );
            for b in &l[i + 1..] {
                let ox = a.1 < b.1 + b.3 - 0.01 && b.1 < a.1 + a.3 - 0.01;
                let oy = a.2 < b.2 + b.4 - 0.01 && b.2 < a.2 + a.4 - 0.01;
                assert!(!(ox && oy), "keys {} and {} overlap", a.0, b.0);
            }
        }
        for (i, a) in l.iter().enumerate() {
            for b in &l[i + 1..] {
                let ox = a.1 < b.1 + b.3 - 0.01 && b.1 < a.1 + a.3 - 0.01;
                let oy = a.2 < b.2 + b.4 - 0.01 && b.2 < a.2 + a.4 - 0.01;
                assert!(!(ox && oy), "keys {} and {} overlap", a.0, b.0);
            }
        }
    }
}
