//! Virtual output devices (uinput). Split into a keyboard and a pointer device so
//! libinput classifies each correctly.

use evdev::uinput::VirtualDevice;
use evdev::{AttributeSet, EventType, InputEvent, KeyCode, RelativeAxisCode};
use std::collections::HashSet;
use std::io;

use crate::keys::is_mouse_button;

pub struct Output {
    kbd: VirtualDevice,
    ptr: VirtualDevice,
    pressed: HashSet<KeyCode>,
}

fn keyboard_codes() -> impl Iterator<Item = u16> {
    // Skip 0x100..0x15f (misc/mouse/joystick/gamepad/digitizer buttons) and
    // 0x2c0+ (BTN_TRIGGER_HAPPY) so libinput doesn't think we're a joystick.
    (1u16..=0xff).chain(0x160..=0x2bf)
}

pub fn supports(k: KeyCode) -> bool {
    let c = k.code();
    is_mouse_button(k) || (1..=0xff).contains(&c) || (0x160..=0x2bf).contains(&c)
}

impl Output {
    pub fn new() -> io::Result<Self> {
        let keys: AttributeSet<KeyCode> = keyboard_codes().map(KeyCode::new).collect();
        let kbd = VirtualDevice::builder()?
            .name("InputForge Virtual Keyboard")
            .with_keys(&keys)?
            .build()?;

        let buttons: AttributeSet<KeyCode> = (0x110u16..=0x117).map(KeyCode::new).collect();
        let axes: AttributeSet<RelativeAxisCode> = [
            RelativeAxisCode::REL_X,
            RelativeAxisCode::REL_Y,
            RelativeAxisCode::REL_WHEEL,
            RelativeAxisCode::REL_HWHEEL,
            RelativeAxisCode::REL_WHEEL_HI_RES,
            RelativeAxisCode::REL_HWHEEL_HI_RES,
        ]
        .into_iter()
        .collect();
        let ptr = VirtualDevice::builder()?
            .name("InputForge Virtual Pointer")
            .with_keys(&buttons)?
            .with_relative_axes(&axes)?
            .build()?;
        Ok(Self {
            kbd,
            ptr,
            pressed: HashSet::new(),
        })
    }

    /// Emit a batch of events (without trailing SYN), routing each to the right device.
    pub fn emit(&mut self, events: &[InputEvent]) -> io::Result<()> {
        let mut kb = Vec::new();
        let mut pt = Vec::new();
        for ev in events {
            match ev.event_type() {
                EventType::KEY => {
                    let k = KeyCode::new(ev.code());
                    if !supports(k) {
                        continue;
                    }
                    match ev.value() {
                        0 => {
                            self.pressed.remove(&k);
                        }
                        1 => {
                            self.pressed.insert(k);
                        }
                        _ => {}
                    }
                    if is_mouse_button(k) {
                        pt.push(*ev)
                    } else {
                        kb.push(*ev)
                    }
                }
                EventType::RELATIVE => pt.push(*ev),
                _ => {}
            }
        }
        if !kb.is_empty() {
            self.kbd.emit(&kb)?;
        }
        if !pt.is_empty() {
            self.ptr.emit(&pt)?;
        }
        Ok(())
    }

    pub fn key(&mut self, k: KeyCode, value: i32) -> io::Result<()> {
        self.emit(&[InputEvent::new(EventType::KEY.0, k.code(), value)])
    }

    pub fn tap(&mut self, k: KeyCode) -> io::Result<()> {
        self.key(k, 1)?;
        self.key(k, 0)
    }

    pub fn scroll(&mut self, notches: i32) -> io::Result<()> {
        self.emit(&[
            InputEvent::new(
                EventType::RELATIVE.0,
                RelativeAxisCode::REL_WHEEL.0,
                notches,
            ),
            InputEvent::new(
                EventType::RELATIVE.0,
                RelativeAxisCode::REL_WHEEL_HI_RES.0,
                notches * 120,
            ),
        ])
    }

    pub fn release_all(&mut self) {
        let held: Vec<KeyCode> = self.pressed.iter().copied().collect();
        for k in held {
            let _ = self.key(k, 0);
        }
    }
}

impl Drop for Output {
    fn drop(&mut self) {
        self.release_all();
    }
}
