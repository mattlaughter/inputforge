//! The remapping engine: grabs selected physical devices, transforms their events
//! according to the active profile, and re-emits them through uinput.

use evdev::{Device, EventSummary, EventType, InputEvent, KeyCode, RelativeAxisCode};
use std::collections::{HashMap, HashSet, VecDeque};
use std::os::fd::AsRawFd;
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::mpsc::{self, RecvTimeoutError, Sender};
use std::sync::{Arc, Mutex, RwLock};
use std::thread::JoinHandle;
use std::time::{Duration, Instant};

use crate::config::{Action, Config, DeviceSettings, Macro, MacroMode, Step};
use crate::devices::{self, VIRTUAL_PREFIX};
use crate::keys::{char_to_key, key_name, parse_key};
use crate::output::Output;

const LOG_CAP: usize = 200;

/// State shared between the engine threads and the GUI.
#[derive(Default)]
pub struct Shared {
    pub log: VecDeque<String>,
    pub status: String,
    pub grabbed: Vec<String>,
    /// When set, the next key press is captured here instead of being processed.
    pub capture_request: bool,
    pub captured: Option<String>,
    pub running: bool,
}

impl Shared {
    pub fn push_log(&mut self, s: impl Into<String>) {
        if self.log.len() >= LOG_CAP {
            self.log.pop_front();
        }
        self.log.push_back(s.into());
    }
}

pub type SharedRef = Arc<Mutex<Shared>>;
type OutRef = Arc<Mutex<Output>>;

pub struct AutoClickState {
    pub active: AtomicBool,
    pub clicks: AtomicU64,
}

pub struct Engine {
    stop: Arc<AtomicBool>,
    thread: Option<JoinHandle<()>>,
    pub config: Arc<RwLock<Config>>,
    pub autoclick: Arc<AutoClickState>,
    out: OutRef,
}

impl Engine {
    pub fn start(
        cfg: Config,
        shared: SharedRef,
        repaint: impl Fn() + Send + Sync + 'static,
    ) -> anyhow::Result<Self> {
        let out = Arc::new(Mutex::new(Output::new().map_err(|e| {
            anyhow::anyhow!("cannot create uinput device ({e}). Is /dev/uinput writable?")
        })?));
        let stop = Arc::new(AtomicBool::new(false));
        let config = Arc::new(RwLock::new(cfg));
        let autoclick = Arc::new(AutoClickState {
            active: AtomicBool::new(false),
            clicks: AtomicU64::new(0),
        });
        let repaint: Arc<dyn Fn() + Send + Sync> = Arc::new(repaint);
        let ctx = Ctx {
            stop: stop.clone(),
            config: config.clone(),
            shared: shared.clone(),
            out: out.clone(),
            autoclick: autoclick.clone(),
            repaint,
        };
        {
            let mut s = shared.lock().unwrap();
            s.running = true;
            s.status = "Running".into();
            s.push_log("Engine started");
        }
        let thread = std::thread::Builder::new()
            .name("inputforge-engine".into())
            .spawn(move || run(ctx))?;
        Ok(Self {
            stop,
            thread: Some(thread),
            config,
            autoclick,
            out,
        })
    }

    /// Hot-update rules/macros/speeds (device selection changes need a restart).
    pub fn update_config(&self, cfg: Config) {
        *self.config.write().unwrap() = cfg;
    }

    pub fn set_autoclicker(&self, on: bool) {
        set_autoclick(&self.autoclick, &self.config, &self.out, on);
    }

    pub fn is_finished(&self) -> bool {
        self.thread.as_ref().is_none_or(|t| t.is_finished())
    }

    pub fn stop(&mut self) {
        self.stop.store(true, Ordering::SeqCst);
        self.autoclick.active.store(false, Ordering::SeqCst);
        if let Some(t) = self.thread.take() {
            let _ = t.join();
        }
    }
}

impl Drop for Engine {
    fn drop(&mut self) {
        self.stop();
    }
}

#[derive(Clone)]
struct Ctx {
    stop: Arc<AtomicBool>,
    config: Arc<RwLock<Config>>,
    shared: SharedRef,
    out: OutRef,
    autoclick: Arc<AutoClickState>,
    repaint: Arc<dyn Fn() + Send + Sync>,
}

impl Ctx {
    fn log(&self, s: impl Into<String>) {
        self.shared.lock().unwrap().push_log(s);
        (self.repaint)();
    }
}

enum Msg {
    Events { dev: usize, events: Vec<InputEvent> },
    Gone { dev: usize, err: String },
}

struct Source {
    settings: DeviceSettings,
    path: PathBuf,
    alive: bool,
    acc_x: f32,
    acc_y: f32,
    acc_wheel: f32,
    acc_wheel_hr: f32,
    acc_hwheel: f32,
    acc_hwheel_hr: f32,
    physically_held: HashSet<KeyCode>,
}

/// Per-trigger runtime state for macros.
struct MacroRun {
    cancel: Arc<AtomicBool>,
    handle: JoinHandle<()>,
}

fn run(ctx: Ctx) {
    let (tx, rx) = mpsc::channel::<Msg>();
    let mut sources: Vec<Source> = Vec::new();
    let mut readers: Vec<JoinHandle<()>> = Vec::new();
    attach_devices(&ctx, &tx, &mut sources, &mut readers);
    if sources.is_empty() {
        ctx.log("No devices enabled — enable at least one device on the Devices tab.");
    }

    let mut macros: HashMap<(usize, KeyCode), MacroRun> = HashMap::new();
    let mut last_rescan = Instant::now();

    loop {
        if ctx.stop.load(Ordering::SeqCst) {
            break;
        }
        match rx.recv_timeout(Duration::from_millis(100)) {
            Ok(Msg::Events { dev, events }) => {
                if let Some(stop) = handle_batch(&ctx, &mut sources, dev, events, &mut macros) {
                    if stop {
                        ctx.log("Emergency stop (LeftCtrl + RightCtrl + Esc)");
                        ctx.stop.store(true, Ordering::SeqCst);
                    }
                }
            }
            Ok(Msg::Gone { dev, err }) => {
                if let Some(s) = sources.get_mut(dev) {
                    s.alive = false;
                    s.physically_held.clear();
                    ctx.log(format!("Device disconnected: {} ({err})", s.settings.name));
                }
            }
            Err(RecvTimeoutError::Timeout) => {}
            Err(RecvTimeoutError::Disconnected) => break,
        }
        // Reap finished macros.
        macros.retain(|_, m| !m.handle.is_finished());
        // Hot-plug: periodically look for enabled devices that (re)appeared.
        if last_rescan.elapsed() > Duration::from_secs(2) {
            last_rescan = Instant::now();
            attach_devices(&ctx, &tx, &mut sources, &mut readers);
        }
    }

    // Shutdown.
    ctx.autoclick.active.store(false, Ordering::SeqCst);
    for (_, m) in macros.drain() {
        m.cancel.store(true, Ordering::SeqCst);
        let _ = m.handle.join();
    }
    drop(tx);
    for r in readers {
        let _ = r.join();
    }
    ctx.out.lock().unwrap().release_all();
    {
        let mut s = ctx.shared.lock().unwrap();
        s.running = false;
        s.grabbed.clear();
        s.status = "Stopped".into();
        s.push_log("Engine stopped — devices released");
    }
    (ctx.repaint)();
}

fn attach_devices(
    ctx: &Ctx,
    tx: &Sender<Msg>,
    sources: &mut Vec<Source>,
    readers: &mut Vec<JoinHandle<()>>,
) {
    let enabled: Vec<DeviceSettings> = ctx
        .config
        .read()
        .unwrap()
        .devices
        .iter()
        .filter(|d| d.enabled)
        .cloned()
        .collect();
    if enabled.is_empty() {
        return;
    }
    let scan = devices::scan();
    for info in scan.devices {
        if !info.kind.grabbable() || info.name.starts_with(VIRTUAL_PREFIX) {
            continue;
        }
        let Some(settings) = enabled
            .iter()
            .find(|d| d.name == info.name && d.phys == info.phys)
        else {
            continue;
        };
        if sources.iter().any(|s| s.alive && s.path == info.path) {
            continue;
        }
        let mut dev = match Device::open(&info.path) {
            Ok(d) => d,
            Err(e) => {
                ctx.log(format!("Cannot open {}: {e}", info.name));
                continue;
            }
        };
        // Wait until all physical keys are released before grabbing, otherwise the
        // compositor may see a key stuck down.
        let t0 = Instant::now();
        while dev
            .get_key_state()
            .map(|s| s.iter().next().is_some())
            .unwrap_or(false)
            && t0.elapsed() < Duration::from_secs(3)
        {
            std::thread::sleep(Duration::from_millis(20));
        }
        if let Err(e) = dev.grab() {
            ctx.log(format!(
                "Cannot grab {} ({}): {e}",
                info.name,
                info.path.display()
            ));
            continue;
        }
        let idx = sources.len();
        sources.push(Source {
            settings: settings.clone(),
            path: info.path.clone(),
            alive: true,
            acc_x: 0.0,
            acc_y: 0.0,
            acc_wheel: 0.0,
            acc_wheel_hr: 0.0,
            acc_hwheel: 0.0,
            acc_hwheel_hr: 0.0,
            physically_held: HashSet::new(),
        });
        let label = format!("{} ({})", info.name, info.path.display());
        ctx.shared.lock().unwrap().grabbed.push(label.clone());
        ctx.log(format!("Grabbed {label}"));
        let tx = tx.clone();
        let stop = ctx.stop.clone();
        readers.push(
            std::thread::Builder::new()
                .name(format!("reader-{idx}"))
                .spawn(move || reader(dev, idx, tx, stop))
                .expect("spawn reader"),
        );
    }
}

fn reader(mut dev: Device, idx: usize, tx: Sender<Msg>, stop: Arc<AtomicBool>) {
    let fd = dev.as_raw_fd();
    let mut batch = Vec::with_capacity(16);
    while !stop.load(Ordering::SeqCst) {
        let mut pfd = libc::pollfd {
            fd,
            events: libc::POLLIN,
            revents: 0,
        };
        let r = unsafe { libc::poll(&mut pfd, 1, 100) };
        if r < 0 {
            let e = std::io::Error::last_os_error();
            if e.kind() == std::io::ErrorKind::Interrupted {
                continue;
            }
            let _ = tx.send(Msg::Gone {
                dev: idx,
                err: e.to_string(),
            });
            return;
        }
        if r == 0 {
            continue;
        }
        if pfd.revents & (libc::POLLERR | libc::POLLHUP | libc::POLLNVAL) != 0 {
            let _ = tx.send(Msg::Gone {
                dev: idx,
                err: "device removed".into(),
            });
            return;
        }
        match dev.fetch_events() {
            Ok(evs) => {
                for ev in evs {
                    if ev.event_type() == EventType::SYNCHRONIZATION {
                        if !batch.is_empty() {
                            let events = std::mem::take(&mut batch);
                            if tx.send(Msg::Events { dev: idx, events }).is_err() {
                                return;
                            }
                        }
                    } else {
                        batch.push(ev);
                    }
                }
            }
            Err(e) if e.kind() == std::io::ErrorKind::WouldBlock => {}
            Err(e) => {
                let _ = tx.send(Msg::Gone {
                    dev: idx,
                    err: e.to_string(),
                });
                return;
            }
        }
    }
    let _ = dev.ungrab();
}

fn scale(acc: &mut f32, value: i32, factor: f32) -> i32 {
    if factor == 1.0 {
        return value;
    }
    *acc += value as f32 * factor;
    let whole = acc.trunc();
    *acc -= whole;
    whole as i32
}

/// True when `key` (just pressed) completes Left Ctrl + Right Ctrl + Esc, with the keys held on
/// any combination of sources.
fn emergency_chord(sources: &[Source], key: KeyCode) -> bool {
    key == KeyCode::KEY_ESC
        && sources
            .iter()
            .any(|s| s.physically_held.contains(&KeyCode::KEY_LEFTCTRL))
        && sources
            .iter()
            .any(|s| s.physically_held.contains(&KeyCode::KEY_RIGHTCTRL))
}

/// Returns Some(true) when the emergency-stop chord was pressed.
fn handle_batch(
    ctx: &Ctx,
    sources: &mut [Source],
    dev: usize,
    events: Vec<InputEvent>,
    macros: &mut HashMap<(usize, KeyCode), MacroRun>,
) -> Option<bool> {
    // Track physically held keys first, across every grabbed source, so the chord works even
    // when the modifiers and Esc arrive on different event nodes of the same device.
    let mut chord = false;
    sources.get(dev)?;
    for ev in &events {
        if let EventSummary::Key(_, key, value) = ev.destructure() {
            match value {
                1 => {
                    sources[dev].physically_held.insert(key);
                }
                0 => {
                    sources[dev].physically_held.remove(&key);
                }
                _ => {}
            }
            chord |= value == 1 && emergency_chord(sources, key);
        }
    }
    if chord {
        return Some(true);
    }
    let src = sources.get_mut(dev)?;
    let cfg = ctx.config.read().unwrap().clone();
    let mut out_events: Vec<InputEvent> = Vec::with_capacity(events.len());
    let mut deferred: Vec<Box<dyn FnOnce(&mut Output)>> = Vec::new();

    for ev in events {
        match ev.destructure() {
            EventSummary::Key(_, key, value) => {
                // Key capture for the GUI's "press a key" buttons.
                if value == 1 {
                    let mut sh = ctx.shared.lock().unwrap();
                    if sh.capture_request {
                        sh.capture_request = false;
                        sh.captured = Some(key_name(key));
                        drop(sh);
                        (ctx.repaint)();
                        continue;
                    }
                }
                // Autoclicker hotkey.
                if parse_key(&cfg.autoclicker.hotkey) == Some(key) {
                    if value == 1 {
                        let on = !ctx.autoclick.active.load(Ordering::SeqCst);
                        set_autoclick(&ctx.autoclick, &ctx.config, &ctx.out, on);
                        ctx.log(format!("Autoclicker {}", if on { "ON" } else { "OFF" }));
                    }
                    continue;
                }

                let dev_name = src.settings.name.to_lowercase();
                let matches =
                    |r: &&crate::config::Rule| r.enabled && parse_key(&r.trigger) == Some(key);
                let rule = cfg
                    .active_rules()
                    .iter()
                    .filter(matches)
                    .find(|r| {
                        !r.device_filter.is_empty()
                            && dev_name.contains(&r.device_filter.to_lowercase())
                    })
                    .or_else(|| {
                        cfg.active_rules()
                            .iter()
                            .filter(matches)
                            .find(|r| r.device_filter.is_empty())
                    });
                let Some(rule) = rule else {
                    out_events.push(ev);
                    continue;
                };
                match &rule.action {
                    Action::Key { key: target } => {
                        if let Some(t) = parse_key(target) {
                            out_events.push(InputEvent::new(EventType::KEY.0, t.code(), value));
                        }
                    }
                    Action::Combo { keys } => {
                        let ks: Vec<KeyCode> = keys.iter().filter_map(|k| parse_key(k)).collect();
                        match value {
                            1 => {
                                for k in &ks {
                                    out_events.push(InputEvent::new(EventType::KEY.0, k.code(), 1));
                                }
                            }
                            0 => {
                                for k in ks.iter().rev() {
                                    out_events.push(InputEvent::new(EventType::KEY.0, k.code(), 0));
                                }
                            }
                            _ => {}
                        }
                    }
                    Action::Disabled => {}
                    Action::ToggleAutoclicker => {
                        if value == 1 {
                            let on = !ctx.autoclick.active.load(Ordering::SeqCst);
                            set_autoclick(&ctx.autoclick, &ctx.config, &ctx.out, on);
                            ctx.log(format!("Autoclicker {}", if on { "ON" } else { "OFF" }));
                        }
                    }
                    Action::Macro { name } => {
                        let Some(m) = cfg.find_macro(name).cloned() else {
                            if value == 1 {
                                ctx.log(format!("Macro '{name}' not found"));
                            }
                            continue;
                        };
                        let slot = (dev, key);
                        let running = macros.get(&slot).is_some_and(|r| !r.handle.is_finished());
                        match (m.mode, value) {
                            (MacroMode::Once, 1) => {
                                if !running {
                                    macros.insert(slot, spawn_macro(ctx, m, false));
                                }
                            }
                            (MacroMode::WhileHeld, 1) => {
                                if !running {
                                    macros.insert(slot, spawn_macro(ctx, m, true));
                                }
                            }
                            (MacroMode::WhileHeld, 0) => {
                                if let Some(r) = macros.get(&slot) {
                                    r.cancel.store(true, Ordering::SeqCst);
                                }
                            }
                            (MacroMode::Toggle, 1) => {
                                if running {
                                    if let Some(r) = macros.get(&slot) {
                                        r.cancel.store(true, Ordering::SeqCst);
                                    }
                                    ctx.log(format!("Macro '{}' stopped", m.name));
                                } else {
                                    ctx.log(format!("Macro '{}' looping", m.name));
                                    macros.insert(slot, spawn_macro(ctx, m, true));
                                }
                            }
                            _ => {}
                        }
                    }
                }
            }
            EventSummary::RelativeAxis(_, axis, value) => {
                let s = &src.settings;
                let inv = if s.invert_scroll { -1 } else { 1 };
                let v = match axis {
                    RelativeAxisCode::REL_X => scale(&mut src.acc_x, value, s.pointer_speed),
                    RelativeAxisCode::REL_Y => scale(&mut src.acc_y, value, s.pointer_speed),
                    RelativeAxisCode::REL_WHEEL => {
                        inv * scale(&mut src.acc_wheel, value, s.scroll_speed)
                    }
                    RelativeAxisCode::REL_WHEEL_HI_RES => {
                        inv * scale(&mut src.acc_wheel_hr, value, s.scroll_speed)
                    }
                    RelativeAxisCode::REL_HWHEEL => {
                        scale(&mut src.acc_hwheel, value, s.scroll_speed)
                    }
                    RelativeAxisCode::REL_HWHEEL_HI_RES => {
                        scale(&mut src.acc_hwheel_hr, value, s.scroll_speed)
                    }
                    _ => value,
                };
                if v != 0 {
                    out_events.push(InputEvent::new(EventType::RELATIVE.0, axis.0, v));
                }
            }
            // MSC_SCAN, LEDs, etc. are not forwarded.
            _ => {}
        }
    }

    if !out_events.is_empty() || !deferred.is_empty() {
        let mut out = ctx.out.lock().unwrap();
        if !out_events.is_empty() {
            if let Err(e) = out.emit(&out_events) {
                drop(out);
                ctx.log(format!("uinput write failed: {e}"));
                return None;
            }
        }
        for f in deferred.drain(..) {
            f(&mut out);
        }
    }
    None
}

fn spawn_macro(ctx: &Ctx, m: Macro, repeat: bool) -> MacroRun {
    let cancel = Arc::new(AtomicBool::new(false));
    let c2 = cancel.clone();
    let out = ctx.out.clone();
    let stop = ctx.stop.clone();
    let handle = std::thread::spawn(move || {
        loop {
            run_steps(&m.steps, &out, &c2, &stop);
            if !repeat || c2.load(Ordering::SeqCst) || stop.load(Ordering::SeqCst) {
                break;
            }
            // Guard against empty / zero-delay loops pinning a CPU.
            if !m
                .steps
                .iter()
                .any(|s| matches!(s, Step::Delay { ms } if *ms > 0))
            {
                std::thread::sleep(Duration::from_millis(10));
            }
        }
    });
    MacroRun { cancel, handle }
}

/// Execute macro steps. Delays are interruptible by cancellation.
pub fn run_steps(steps: &[Step], out: &Mutex<Output>, cancel: &AtomicBool, stop: &AtomicBool) {
    let cancelled = || cancel.load(Ordering::SeqCst) || stop.load(Ordering::SeqCst);
    let mut held: Vec<KeyCode> = Vec::new();
    for step in steps {
        if cancelled() {
            break;
        }
        let mut o = out.lock().unwrap();
        let _ = match step {
            Step::Tap { key } => parse_key(key).map(|k| o.tap(k)).unwrap_or(Ok(())),
            Step::KeyDown { key } => parse_key(key)
                .map(|k| {
                    held.push(k);
                    o.key(k, 1)
                })
                .unwrap_or(Ok(())),
            Step::KeyUp { key } => parse_key(key)
                .map(|k| {
                    held.retain(|h| *h != k);
                    o.key(k, 0)
                })
                .unwrap_or(Ok(())),
            Step::Text { text } => {
                for c in text.chars() {
                    if let Some((k, shift)) = char_to_key(c) {
                        if shift {
                            let _ = o.key(KeyCode::KEY_LEFTSHIFT, 1);
                        }
                        let _ = o.tap(k);
                        if shift {
                            let _ = o.key(KeyCode::KEY_LEFTSHIFT, 0);
                        }
                        drop(o);
                        std::thread::sleep(Duration::from_millis(4));
                        o = out.lock().unwrap();
                    }
                }
                Ok(())
            }
            Step::MouseMove { dx, dy } => o.emit(&[
                InputEvent::new(EventType::RELATIVE.0, RelativeAxisCode::REL_X.0, *dx),
                InputEvent::new(EventType::RELATIVE.0, RelativeAxisCode::REL_Y.0, *dy),
            ]),
            Step::Scroll { amount } => o.scroll(*amount),
            Step::Delay { ms } => {
                drop(o);
                let end = Instant::now() + Duration::from_millis(*ms);
                while Instant::now() < end && !cancelled() {
                    std::thread::sleep((end - Instant::now()).min(Duration::from_millis(10)));
                }
                continue;
            }
        };
    }
    // Never leave keys stuck if a macro is cancelled mid-way.
    if !held.is_empty() {
        let mut o = out.lock().unwrap();
        for k in held {
            let _ = o.key(k, 0);
        }
    }
}

fn set_autoclick(
    state: &Arc<AutoClickState>,
    config: &Arc<RwLock<Config>>,
    out: &OutRef,
    on: bool,
) {
    if !on {
        state.active.store(false, Ordering::SeqCst);
        return;
    }
    if state.active.swap(true, Ordering::SeqCst) {
        return; // already running
    }
    state.clicks.store(0, Ordering::SeqCst);
    let state = state.clone();
    let config = config.clone();
    let out = out.clone();
    std::thread::spawn(move || {
        let mut next = Instant::now();
        while state.active.load(Ordering::SeqCst) {
            let ac = config.read().unwrap().autoclicker.clone();
            let Some(btn) = parse_key(&ac.button) else {
                break;
            };
            let _ = out.lock().unwrap().tap(btn);
            let n = state.clicks.fetch_add(1, Ordering::SeqCst) + 1;
            if ac.max_clicks > 0 && n >= ac.max_clicks {
                break;
            }
            let cps = ac.clicks_per_second.clamp(0.5, 100.0);
            next += Duration::from_secs_f32(1.0 / cps);
            let now = Instant::now();
            if next > now {
                std::thread::sleep(next - now);
            } else {
                next = now;
            }
        }
        state.active.store(false, Ordering::SeqCst);
    });
}

/// Capture the next key press from any readable device without grabbing. Used
/// when the engine isn't running.
pub fn capture_once(timeout: Duration) -> Option<String> {
    let (tx, rx) = mpsc::channel::<String>();
    let stop = Arc::new(AtomicBool::new(false));
    let mut threads = vec![];
    for info in devices::scan().devices {
        if !info.kind.grabbable() {
            continue;
        }
        let Ok(mut dev) = Device::open(&info.path) else {
            continue;
        };
        let tx = tx.clone();
        let stop = stop.clone();
        threads.push(std::thread::spawn(move || {
            let fd = dev.as_raw_fd();
            while !stop.load(Ordering::SeqCst) {
                let mut pfd = libc::pollfd {
                    fd,
                    events: libc::POLLIN,
                    revents: 0,
                };
                if unsafe { libc::poll(&mut pfd, 1, 100) } <= 0 {
                    continue;
                }
                let Ok(evs) = dev.fetch_events() else { return };
                for ev in evs {
                    if let EventSummary::Key(_, k, 1) = ev.destructure() {
                        let _ = tx.send(key_name(k));
                        return;
                    }
                }
            }
        }));
    }
    drop(tx);
    let r = rx.recv_timeout(timeout).ok();
    stop.store(true, Ordering::SeqCst);
    for t in threads {
        let _ = t.join();
    }
    r
}

/// Run a macro once outside the engine (for the "Test" button).
pub fn test_macro(m: Macro) -> anyhow::Result<()> {
    let out = Mutex::new(Output::new()?);
    // Give the compositor a moment to pick up the new virtual devices.
    std::thread::sleep(Duration::from_millis(400));
    let no = AtomicBool::new(false);
    run_steps(&m.steps, &out, &no, &no);
    std::thread::sleep(Duration::from_millis(100));
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn source(held: &[KeyCode]) -> Source {
        Source {
            settings: DeviceSettings::default(),
            path: PathBuf::new(),
            alive: true,
            acc_x: 0.0,
            acc_y: 0.0,
            acc_wheel: 0.0,
            acc_wheel_hr: 0.0,
            acc_hwheel: 0.0,
            acc_hwheel_hr: 0.0,
            physically_held: held.iter().copied().collect(),
        }
    }

    #[test]
    fn chord_on_one_source() {
        let s = [source(&[KeyCode::KEY_LEFTCTRL, KeyCode::KEY_RIGHTCTRL])];
        assert!(emergency_chord(&s, KeyCode::KEY_ESC));
    }

    #[test]
    fn chord_split_across_sources() {
        let s = [
            source(&[KeyCode::KEY_LEFTCTRL]),
            source(&[KeyCode::KEY_RIGHTCTRL]),
            source(&[]),
        ];
        assert!(emergency_chord(&s, KeyCode::KEY_ESC));
    }

    #[test]
    fn chord_needs_both_ctrls_and_esc() {
        let s = [source(&[KeyCode::KEY_LEFTCTRL]), source(&[])];
        assert!(!emergency_chord(&s, KeyCode::KEY_ESC));
        let s = [source(&[KeyCode::KEY_LEFTCTRL, KeyCode::KEY_RIGHTCTRL])];
        assert!(!emergency_chord(&s, KeyCode::KEY_A));
    }

    #[test]
    fn scale_carries_fraction() {
        let mut acc = 0.0;
        assert_eq!(scale(&mut acc, 1, 0.5), 0);
        assert_eq!(scale(&mut acc, 1, 0.5), 1);
    }
}
