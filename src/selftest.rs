//! `inputforge --selftest`: end-to-end check of the engine using a fake source
//! device. Only the fake device is grabbed; harmless keys (F13–F17) are used.

use evdev::uinput::VirtualDevice;
use evdev::{AttributeSet, Device, EventSummary, EventType, InputEvent, KeyCode, RelativeAxisCode};
use std::os::fd::AsRawFd;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use crate::config::{Action, Config, DeviceSettings, Macro, MacroMode, Profile, Rule, Step};
use crate::engine::{Engine, Shared};

const SRC_NAME: &str = "IF-Selftest Source";
const SRC2_NAME: &str = "IF-Selftest Source B";

fn find(name: &str) -> anyhow::Result<Device> {
    find_new(name, &Default::default())
}

/// Find a device by name, ignoring nodes that already existed (e.g. the
/// virtual devices of an InputForge window that's already running).
fn find_new(
    name: &str,
    existing: &std::collections::HashSet<std::path::PathBuf>,
) -> anyhow::Result<Device> {
    let t0 = Instant::now();
    while t0.elapsed() < Duration::from_secs(3) {
        for (path, d) in evdev::enumerate() {
            if d.name() == Some(name) && !existing.contains(&path) {
                return Ok(d);
            }
        }
        std::thread::sleep(Duration::from_millis(50));
    }
    anyhow::bail!(
        "device '{name}' not found or not readable. {}",
        crate::devices::group_hint()
    )
}

fn drain(dev: &mut Device, wait: Duration) -> Vec<InputEvent> {
    let mut out = vec![];
    let end = Instant::now() + wait;
    while Instant::now() < end {
        let mut pfd = libc::pollfd {
            fd: dev.as_raw_fd(),
            events: libc::POLLIN,
            revents: 0,
        };
        if unsafe { libc::poll(&mut pfd, 1, 20) } > 0 {
            if let Ok(evs) = dev.fetch_events() {
                out.extend(evs.filter(|e| {
                    e.event_type() != EventType::SYNCHRONIZATION
                        && e.event_type() != EventType::MISC
                }));
            }
        }
    }
    out
}

fn keys_of(evs: &[InputEvent]) -> Vec<(String, i32)> {
    evs.iter()
        .filter_map(|e| match e.destructure() {
            EventSummary::Key(_, k, v) => Some((format!("{k:?}"), v)),
            _ => None,
        })
        .collect()
}

fn check(name: &str, ok: bool, detail: impl std::fmt::Debug) -> bool {
    println!(
        "{} {name}{}",
        if ok { "PASS" } else { "FAIL" },
        if ok {
            String::new()
        } else {
            format!(" — got {detail:?}")
        }
    );
    ok
}

pub fn run() -> anyhow::Result<bool> {
    // 1. Fake physical device.
    let keys: AttributeSet<KeyCode> = [
        KeyCode::KEY_F20,
        KeyCode::KEY_Z,
        KeyCode::KEY_ENTER,
        KeyCode::KEY_F13,
        KeyCode::KEY_F15,
        KeyCode::KEY_F18,
        KeyCode::KEY_F19,
        KeyCode::BTN_LEFT,
        KeyCode::new(0x118),   // G600-style extra button (G9+)
        KeyCode::KEY_LEFTCTRL, // first half of the cross-node emergency chord
    ]
    .into_iter()
    .collect();
    let axes: AttributeSet<RelativeAxisCode> = [RelativeAxisCode::REL_X, RelativeAxisCode::REL_Y]
        .into_iter()
        .collect();
    let mut src = VirtualDevice::builder()?
        .name(SRC_NAME)
        .with_keys(&keys)?
        .with_relative_axes(&axes)?
        .build()?;
    let src_dev = find(SRC_NAME)?;
    let phys = src_dev.physical_path().unwrap_or("").to_string();
    drop(src_dev);

    // A second fake node, like the extra event nodes real Logitech devices expose.
    let keys2: AttributeSet<KeyCode> = [
        KeyCode::KEY_LEFTCTRL,
        KeyCode::KEY_RIGHTCTRL,
        KeyCode::KEY_ESC,
    ]
    .into_iter()
    .collect();
    let mut src2 = VirtualDevice::builder()?
        .name(SRC2_NAME)
        .with_keys(&keys2)?
        .build()?;
    let src2_dev = find(SRC2_NAME)?;
    let phys2 = src2_dev.physical_path().unwrap_or("").to_string();
    drop(src2_dev);

    // 2. Config: only the fake device enabled.
    let cfg = Config {
        devices: vec![
            DeviceSettings {
                name: SRC_NAME.into(),
                phys,
                enabled: true,
                pointer_speed: 2.0,
                ..Default::default()
            },
            DeviceSettings {
                name: SRC2_NAME.into(),
                phys: phys2,
                enabled: true,
                ..Default::default()
            },
        ],
        profiles: vec![Profile {
            name: "test".into(),
            rules: vec![
                Rule {
                    trigger: "KEY_F13".into(),
                    action: Action::Key {
                        key: "KEY_F14".into(),
                    },
                    ..Default::default()
                },
                Rule {
                    trigger: "KEY_F15".into(),
                    action: Action::Combo {
                        keys: vec!["KEY_LEFTSHIFT".into(), "KEY_F16".into()],
                    },
                    ..Default::default()
                },
                Rule {
                    trigger: "KEY_F18".into(),
                    action: Action::Macro { name: "m".into() },
                    ..Default::default()
                },
                Rule {
                    trigger: "KEY_F19".into(),
                    action: Action::Disabled,
                    ..Default::default()
                },
            ],
        }],
        macros: vec![Macro {
            name: "m".into(),
            mode: MacroMode::Once,
            steps: vec![
                Step::Tap {
                    key: "KEY_F17".into(),
                },
                Step::Delay { ms: 10 },
                Step::Tap {
                    key: "KEY_F17".into(),
                },
            ],
        }],
        ..Default::default()
    };

    let shared = Arc::new(Mutex::new(Shared::default()));
    let existing: std::collections::HashSet<_> = evdev::enumerate().map(|(p, _)| p).collect();
    let mut engine = Engine::start(cfg, shared.clone(), || {})?;
    let mut vk = find_new("InputForge Virtual Keyboard", &existing)?;
    let mut vp = find_new("InputForge Virtual Pointer", &existing)?;
    // Wait for the grab.
    let t0 = Instant::now();
    while shared.lock().unwrap().grabbed.len() < 2 && t0.elapsed() < Duration::from_secs(4) {
        std::thread::sleep(Duration::from_millis(20));
    }
    let grabbed = shared.lock().unwrap().grabbed.len() == 2;
    let mut all = check(
        "engine grabbed both fake devices",
        grabbed,
        shared.lock().unwrap().log.clone(),
    );
    drain(&mut vk, Duration::from_millis(100));

    let key = |src: &mut VirtualDevice, k: KeyCode, v: i32| {
        src.emit(&[InputEvent::new(EventType::KEY.0, k.code(), v)])
            .unwrap();
    };

    key(&mut src, KeyCode::KEY_F13, 1);
    key(&mut src, KeyCode::KEY_F13, 0);
    let got = keys_of(&drain(&mut vk, Duration::from_millis(200)));
    all &= check(
        "remap F13 → F14",
        got == [("KEY_F14".into(), 1), ("KEY_F14".into(), 0)],
        &got,
    );

    key(&mut src, KeyCode::KEY_F15, 1);
    key(&mut src, KeyCode::KEY_F15, 0);
    let got = keys_of(&drain(&mut vk, Duration::from_millis(200)));
    let want: Vec<(String, i32)> = vec![
        ("KEY_LEFTSHIFT".into(), 1),
        ("KEY_F16".into(), 1),
        ("KEY_F16".into(), 0),
        ("KEY_LEFTSHIFT".into(), 0),
    ];
    all &= check("combo F15 → Shift+F16", got == want, &got);

    key(&mut src, KeyCode::KEY_F18, 1);
    key(&mut src, KeyCode::KEY_F18, 0);
    let got = keys_of(&drain(&mut vk, Duration::from_millis(300)));
    let want: Vec<(String, i32)> = vec![
        ("KEY_F17".into(), 1),
        ("KEY_F17".into(), 0),
        ("KEY_F17".into(), 1),
        ("KEY_F17".into(), 0),
    ];
    all &= check("macro F18 → F17 ×2", got == want, &got);

    key(&mut src, KeyCode::KEY_F19, 1);
    key(&mut src, KeyCode::KEY_F19, 0);
    let got = keys_of(&drain(&mut vk, Duration::from_millis(200)));
    all &= check("disabled F19 swallowed", got.is_empty(), &got);

    key(&mut src, KeyCode::KEY_F20, 1);
    key(&mut src, KeyCode::KEY_F20, 0);
    let got = keys_of(&drain(&mut vk, Duration::from_millis(200)));
    all &= check(
        "passthrough F20",
        got == [("KEY_F20".into(), 1), ("KEY_F20".into(), 0)],
        &got,
    );

    src.emit(&[
        InputEvent::new(EventType::RELATIVE.0, RelativeAxisCode::REL_X.0, 5),
        InputEvent::new(EventType::RELATIVE.0, RelativeAxisCode::REL_Y.0, -3),
    ])?;
    let got: Vec<(u16, i32)> = drain(&mut vp, Duration::from_millis(200))
        .iter()
        .filter(|e| e.event_type() == EventType::RELATIVE)
        .map(|e| (e.code(), e.value()))
        .collect();
    all &= check(
        "pointer speed ×2 (5,-3 → 10,-6)",
        got == [(0, 10), (1, -6)],
        &got,
    );

    key(&mut src, KeyCode::new(0x118), 1);
    key(&mut src, KeyCode::new(0x118), 0);
    let got: Vec<(u16, i32)> = drain(&mut vp, Duration::from_millis(200))
        .iter()
        .filter(|e| e.event_type() == EventType::KEY)
        .map(|e| (e.code(), e.value()))
        .collect();
    all &= check(
        "extra mouse button 0x118 passes through",
        got == [(0x118, 1), (0x118, 0)],
        &got,
    );

    // Emergency chord split across two nodes: Left Ctrl on A, Right Ctrl + Esc on B.
    key(&mut src, KeyCode::KEY_LEFTCTRL, 1);
    key(&mut src2, KeyCode::KEY_RIGHTCTRL, 1);
    key(&mut src2, KeyCode::KEY_ESC, 1);
    let t0 = Instant::now();
    while !engine.is_finished() && t0.elapsed() < Duration::from_secs(2) {
        std::thread::sleep(Duration::from_millis(20));
    }
    all &= check(
        "emergency chord across two nodes stops the engine",
        engine.is_finished(),
        "engine still running",
    );

    engine.stop();
    // After stop, the device must be released (not grabbed).
    let mut d = find(SRC_NAME)?;
    let released = d.grab().is_ok();
    let _ = d.ungrab();
    all &= check("device released on stop", released, "grab failed");
    Ok(all)
}
