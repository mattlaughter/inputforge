//! System tray (StatusNotifierItem), single-instance handling and the
//! "start at login" autostart entry.
//!
//! InputForge runs as one long-lived process: the engine and lighting live
//! independently of the window, which can be closed to the tray and reopened.

use std::sync::mpsc::Sender;
use std::sync::{Arc, Mutex};

/// Commands from the tray / a second instance to the main thread.
#[derive(Debug, Clone, PartialEq)]
pub enum Cmd {
    Show,
    SetActive(bool),
    SetProfile(usize),
    ToggleAutoclicker,
    Quit,
}

/// Wakes the GUI (if a window is open) so it processes commands promptly.
#[derive(Clone, Default)]
pub struct Waker(Arc<Mutex<Option<eframe::egui::Context>>>);

impl Waker {
    pub fn set(&self, ctx: Option<eframe::egui::Context>) {
        *self.0.lock().unwrap() = ctx;
    }
    pub fn wake(&self) {
        if let Some(c) = self.0.lock().unwrap().as_ref() {
            c.request_repaint();
        }
    }
}

/// What the tray shows; pushed from the app whenever it changes.
#[derive(Debug, Clone, PartialEq, Default)]
pub struct TrayState {
    pub active: bool,
    pub profiles: Vec<String>,
    pub profile: usize,
    pub autoclicker: bool,
    pub devices: usize,
}

pub struct Tray {
    state: TrayState,
    tx: Sender<Cmd>,
    waker: Waker,
    icon_active: Vec<ksni::Icon>,
    icon_paused: Vec<ksni::Icon>,
}

impl Tray {
    fn send(&self, c: Cmd) {
        let _ = self.tx.send(c);
        self.waker.wake();
    }
}

impl ksni::Tray for Tray {
    // Left click opens the window instead of the menu.
    const MENU_ON_ACTIVATE: bool = false;

    fn id(&self) -> String {
        "inputforge".into()
    }
    fn title(&self) -> String {
        "InputForge".into()
    }
    fn category(&self) -> ksni::Category {
        ksni::Category::Hardware
    }
    fn icon_pixmap(&self) -> Vec<ksni::Icon> {
        if self.state.active {
            self.icon_active.clone()
        } else {
            self.icon_paused.clone()
        }
    }
    fn tool_tip(&self) -> ksni::ToolTip {
        let s = &self.state;
        let mut desc = if s.active {
            format!(
                "Active — profile \"{}\"",
                s.profiles.get(s.profile).map(String::as_str).unwrap_or("?")
            )
        } else {
            "Paused — keyboard and mouse behave normally".into()
        };
        if s.autoclicker {
            desc.push_str("\nAutoclicker running");
        }
        ksni::ToolTip {
            title: "InputForge".into(),
            description: desc,
            ..Default::default()
        }
    }
    fn activate(&mut self, _x: i32, _y: i32) {
        self.send(Cmd::Show);
    }
    fn secondary_activate(&mut self, _x: i32, _y: i32) {
        let on = !self.state.active;
        self.send(Cmd::SetActive(on));
    }
    fn menu(&self) -> Vec<ksni::MenuItem<Self>> {
        use ksni::menu::*;
        let s = &self.state;
        let mut m: Vec<MenuItem<Self>> = vec![
            StandardItem {
                label: "Open InputForge".into(),
                icon_name: "inputforge".into(),
                activate: Box::new(|t: &mut Self| t.send(Cmd::Show)),
                ..Default::default()
            }
            .into(),
            MenuItem::Separator,
            CheckmarkItem {
                label: "Active".into(),
                checked: s.active,
                activate: Box::new(|t: &mut Self| {
                    let on = !t.state.active;
                    t.send(Cmd::SetActive(on))
                }),
                ..Default::default()
            }
            .into(),
        ];
        if s.profiles.len() > 1 {
            m.push(
                SubMenu {
                    label: "Profile".into(),
                    submenu: vec![
                        RadioGroup {
                            selected: s.profile,
                            select: Box::new(|t: &mut Self, i| t.send(Cmd::SetProfile(i))),
                            options: s
                                .profiles
                                .iter()
                                .map(|p| RadioItem {
                                    label: p.replace('_', "__"),
                                    ..Default::default()
                                })
                                .collect(),
                        }
                        .into(),
                    ],
                    ..Default::default()
                }
                .into(),
            );
        }
        m.push(
            CheckmarkItem {
                label: "Autoclicker".into(),
                checked: s.autoclicker,
                enabled: s.active,
                activate: Box::new(|t: &mut Self| t.send(Cmd::ToggleAutoclicker)),
                ..Default::default()
            }
            .into(),
        );
        m.push(MenuItem::Separator);
        m.push(
            StandardItem {
                label: "Quit".into(),
                icon_name: "application-exit".into(),
                activate: Box::new(|t: &mut Self| t.send(Cmd::Quit)),
                ..Default::default()
            }
            .into(),
        );
        m
    }
    fn watcher_offline(&self, _reason: ksni::OfflineReason) -> bool {
        // Keep the service alive; the panel may come back (plasmashell restart).
        true
    }
}

pub type Handle = ksni::blocking::Handle<Tray>;

/// Start the tray icon. Returns None if no tray host is available.
pub fn spawn(tx: Sender<Cmd>, waker: Waker, state: TrayState) -> Option<Handle> {
    use ksni::blocking::TrayMethods;
    let t = Tray {
        state,
        tx,
        waker,
        icon_active: vec![
            png_icon(include_bytes!("../assets/tray-active-32.png")),
            png_icon(include_bytes!("../assets/tray-active-64.png")),
        ],
        icon_paused: vec![
            png_icon(include_bytes!("../assets/tray-paused-32.png")),
            png_icon(include_bytes!("../assets/tray-paused-64.png")),
        ],
    };
    match t.spawn() {
        Ok(h) => Some(h),
        Err(e) => {
            eprintln!("inputforge: no system tray ({e})");
            None
        }
    }
}

pub fn update(h: &Handle, s: &TrayState) {
    let s = s.clone();
    h.update(move |t| t.state = s);
}

/// Decode a PNG into the ARGB32 (big-endian) pixmap SNI expects.
pub fn png_icon(bytes: &[u8]) -> ksni::Icon {
    let (w, h, rgba) = decode_png(bytes);
    let mut data = Vec::with_capacity(rgba.len());
    for p in rgba.chunks_exact(4) {
        data.extend_from_slice(&[p[3], p[0], p[1], p[2]]);
    }
    ksni::Icon {
        width: w as i32,
        height: h as i32,
        data,
    }
}

/// Decode a PNG to RGBA8.
pub fn decode_png(bytes: &[u8]) -> (u32, u32, Vec<u8>) {
    let mut dec = png::Decoder::new(std::io::Cursor::new(bytes));
    dec.set_transformations(
        png::Transformations::normalize_to_color8() | png::Transformations::ALPHA,
    );
    let mut r = dec.read_info().expect("valid embedded png");
    let mut buf = vec![0; r.output_buffer_size().expect("png size")];
    let info = r.next_frame(&mut buf).expect("png frame");
    buf.truncate(info.buffer_size());
    let rgba = match info.color_type {
        png::ColorType::Rgba => buf,
        png::ColorType::GrayscaleAlpha => buf
            .chunks_exact(2)
            .flat_map(|p| [p[0], p[0], p[0], p[1]])
            .collect(),
        png::ColorType::Rgb => buf
            .chunks_exact(3)
            .flat_map(|p| [p[0], p[1], p[2], 255])
            .collect(),
        png::ColorType::Grayscale => buf.iter().flat_map(|&g| [g, g, g, 255]).collect(),
        png::ColorType::Indexed => unreachable!("expanded by normalize_to_color8"),
    };
    (info.width, info.height, rgba)
}

/// Window icon for the title bar / taskbar.
pub fn window_icon() -> eframe::egui::IconData {
    let (width, height, rgba) = decode_png(include_bytes!("../assets/inputforge-256.png"));
    eframe::egui::IconData {
        rgba,
        width,
        height,
    }
}

// ───────────────────────── single instance ─────────────────────────

const BUS_NAME: &str = "io.github.InputForge";
const OBJ_PATH: &str = "/io/github/InputForge";

struct Remote {
    tx: Sender<Cmd>,
    waker: Waker,
}

#[zbus::interface(name = "io.github.InputForge")]
impl Remote {
    fn show(&self) {
        let _ = self.tx.send(Cmd::Show);
        self.waker.wake();
    }
    fn quit(&self) {
        let _ = self.tx.send(Cmd::Quit);
        self.waker.wake();
    }
}

pub enum Instance {
    /// We are the only instance; keep this alive for the process lifetime.
    Primary(Option<zbus::blocking::Connection>),
    /// Another instance is running (and has been asked to show/quit).
    Secondary,
}

/// Claim the single-instance bus name. If another instance owns it, forward
/// `forward` (Show, or Quit) to it and return Secondary.
pub fn claim(tx: Sender<Cmd>, waker: Waker, forward: Option<&str>) -> Instance {
    let Ok(conn) = zbus::blocking::Connection::session() else {
        return Instance::Primary(None);
    };
    use zbus::fdo::RequestNameFlags as F;
    let _ = conn.object_server().at(OBJ_PATH, Remote { tx, waker });
    let forward_to_primary = |conn: &zbus::blocking::Connection| {
        if let Some(method) = forward {
            let _ = conn.call_method(Some(BUS_NAME), OBJ_PATH, Some(BUS_NAME), method, &());
        }
        Instance::Secondary
    };
    match conn.request_name_with_flags(BUS_NAME, F::DoNotQueue.into()) {
        Ok(zbus::fdo::RequestNameReply::PrimaryOwner)
        | Ok(zbus::fdo::RequestNameReply::AlreadyOwner) => Instance::Primary(Some(conn)),
        Ok(_) | Err(zbus::Error::NameTaken) => forward_to_primary(&conn),
        Err(e) => {
            eprintln!("inputforge: single-instance check failed ({e}); continuing");
            Instance::Primary(Some(conn))
        }
    }
}

// ───────────────────────── login autostart ─────────────────────────

fn autostart_file() -> std::path::PathBuf {
    dirs::config_dir()
        .unwrap_or_default()
        .join("autostart/inputforge.desktop")
}

/// Write/remove the login entries to match the settings. With "start at
/// login" the tray service starts (and restores lighting itself); otherwise
/// a lighting-only entry is used if lighting restore is on.
pub fn sync_autostart(start_at_login: bool, restore_lighting: bool) -> anyhow::Result<()> {
    let tray_file = autostart_file();
    if start_at_login {
        let exe = installed_exe();
        std::fs::create_dir_all(tray_file.parent().unwrap())?;
        std::fs::write(
            &tray_file,
            format!(
                "[Desktop Entry]\nType=Application\nName=InputForge\nComment=Keyboard & mouse control (tray)\nExec={exe} --background\nIcon=inputforge\nTerminal=false\nX-GNOME-Autostart-enabled=true\nX-KDE-autostart-phase=2\n"
            ),
        )?;
        crate::lighting::set_autostart(false)?;
    } else {
        if tray_file.exists() {
            std::fs::remove_file(&tray_file)?;
        }
        crate::lighting::set_autostart(restore_lighting)?;
    }
    Ok(())
}

pub fn autostart_enabled() -> bool {
    autostart_file().exists()
}

/// Prefer an installed binary (package: /usr/bin, script: /usr/local/bin) so
/// the entry survives `cargo clean`.
fn installed_exe() -> String {
    for p in ["/usr/bin/inputforge", "/usr/local/bin/inputforge"] {
        if std::path::Path::new(p).exists() {
            return p.into();
        }
    }
    std::env::current_exe()
        .map(|p| p.display().to_string())
        .unwrap_or_else(|_| "inputforge".into())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn icons_decode() {
        let i = png_icon(include_bytes!("../assets/tray-active-32.png"));
        assert_eq!((i.width, i.height), (32, 32));
        assert_eq!(i.data.len(), 32 * 32 * 4);
        assert!(i.data.chunks(4).any(|p| p[0] == 255), "has opaque pixels");
        let w = window_icon();
        assert_eq!((w.width, w.height), (256, 256));
    }
}
