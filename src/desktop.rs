//! Keep desktop pointer settings working for grabbed mice.
//!
//! Once InputForge grabs a mouse, the compositor only sees the "InputForge
//! Virtual Pointer", so settings made for the real mouse in System Settings
//! (acceleration profile/speed, natural scroll, left-handed, scroll factor)
//! would silently stop applying. On KDE Plasma we copy them from the real
//! device to the virtual one through KWin's InputDevice D-Bus API.

use zbus::blocking::{Connection, Proxy};

const SVC: &str = "org.kde.KWin";
const IFACE: &str = "org.kde.KWin.InputDevice";
const VIRTUAL: &str = "InputForge Virtual Pointer";

/// Settings copied from the real mouse.
const PROPS_BOOL: &[&str] = &[
    "pointerAccelerationProfileFlat",
    "pointerAccelerationProfileAdaptive",
    "naturalScroll",
    "leftHanded",
    "middleEmulation",
];
const PROPS_F64: &[&str] = &["pointerAcceleration", "scrollFactor"];

fn device(conn: &Connection, sys: &str) -> Option<Proxy<'static>> {
    Proxy::new_owned(
        conn.clone(),
        SVC,
        format!("/org/kde/KWin/InputDevice/{sys}"),
        IFACE,
    )
    .ok()
}

fn pointers(conn: &Connection) -> Vec<(String, String)> {
    let Ok(mgr) = Proxy::new(
        conn,
        SVC,
        "/org/kde/KWin/InputDevice",
        "org.kde.KWin.InputDeviceManager",
    ) else {
        return vec![];
    };
    let Ok(list): zbus::Result<Vec<String>> = mgr.call("ListPointers", &()) else {
        return vec![];
    };
    list.into_iter()
        .filter_map(|sys| {
            let name: String = device(conn, &sys)?.get_property("name").ok()?;
            Some((sys, name))
        })
        .collect()
}

/// Copy the KDE pointer settings of the first of `mice` (device names, in
/// priority order) onto the virtual pointer. Returns a log line, or None when
/// not on KDE / nothing to do.
pub fn mirror_pointer_settings(mice: &[String]) -> Option<String> {
    let conn = Connection::session().ok()?;
    // The virtual pointer appears a moment after the engine starts.
    let mut ptrs = vec![];
    for _ in 0..20 {
        ptrs = pointers(&conn);
        if ptrs.iter().any(|(_, n)| n == VIRTUAL) {
            break;
        }
        std::thread::sleep(std::time::Duration::from_millis(100));
    }
    let virt = ptrs.iter().find(|(_, n)| n == VIRTUAL)?.0.clone();
    let (src_sys, src_name) = mice
        .iter()
        .find_map(|m| ptrs.iter().find(|(_, n)| n == m))?
        .clone();
    let src = device(&conn, &src_sys)?;
    let dst = device(&conn, &virt)?;
    let mut changed = vec![];
    for p in PROPS_BOOL {
        if let (Ok(a), Ok(b)) = (src.get_property::<bool>(p), dst.get_property::<bool>(p)) {
            if a != b && dst.set_property(p, a).is_ok() {
                changed.push(format!("{p}={a}"));
            }
        }
    }
    for p in PROPS_F64 {
        if let (Ok(a), Ok(b)) = (src.get_property::<f64>(p), dst.get_property::<f64>(p)) {
            if (a - b).abs() > 1e-6 && dst.set_property(p, a).is_ok() {
                changed.push(format!("{p}={a}"));
            }
        }
    }
    Some(if changed.is_empty() {
        format!("Pointer settings match {src_name}")
    } else {
        format!(
            "Applied {src_name}'s desktop pointer settings: {}",
            changed.join(", ")
        )
    })
}
