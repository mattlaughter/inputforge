//! Razer mouse hardware settings over hidraw (DPI). Protocol from OpenRazer GPL drivers.
//!
//! Supported:
//!   * Razer Naga V2 HyperSpeed (1532:00B4) — DPI 100..30000 via razer_report.

use anyhow::{Context, Result, bail};
use std::fs::OpenOptions;
use std::io::Write;
use std::os::unix::fs::OpenOptionsExt;

const RAZER_VENDOR: u16 = 0x1532;
const NAGA_V2_HYPERSPEED: u16 = 0x00B4;

/// 90-byte Razer USB report (struct razer_report from openrazer).
#[repr(C, packed)]
struct RazerReport {
    status: u8,
    transaction_id: u8,
    remaining_packets: u16, // big-endian
    protocol_type: u8,
    data_size: u8,
    command_class: u8,
    command_id: u8,
    arguments: [u8; 80],
    crc: u8,
    reserved: u8,
}

impl RazerReport {
    fn new(command_class: u8, command_id: u8, data_size: u8) -> Self {
        Self {
            status: 0,
            transaction_id: 0,
            remaining_packets: 0,
            protocol_type: 0,
            data_size,
            command_class,
            command_id,
            arguments: [0; 80],
            crc: 0,
            reserved: 0,
        }
    }

    fn calculate_crc(&self) -> u8 {
        let bytes = unsafe {
            std::slice::from_raw_parts(self as *const _ as *const u8, std::mem::size_of::<Self>())
        };
        // XOR bytes 2..88 (skip status, transaction_id; stop before crc, reserved).
        bytes[2..88].iter().fold(0u8, |acc, &b| acc ^ b)
    }

    fn finalize(&mut self) {
        self.crc = self.calculate_crc();
    }

    fn as_bytes(&self) -> &[u8] {
        unsafe {
            std::slice::from_raw_parts(self as *const _ as *const u8, std::mem::size_of::<Self>())
        }
    }
}

/// Build a set_dpi_xy command (class 0x04, id 0x05).
fn set_dpi_xy_report(dpi_x: u16, dpi_y: u16) -> RazerReport {
    let dpi_x = dpi_x.clamp(100, 30000);
    let dpi_y = dpi_y.clamp(100, 30000);
    let mut r = RazerReport::new(0x04, 0x05, 0x07);
    // args[0] = varstore (NOSTORE=0 for Naga V2 HyperSpeed per openrazer).
    r.arguments[0] = 0x00;
    // args[1..5] = DPI X/Y as big-endian u16.
    r.arguments[1] = (dpi_x >> 8) as u8;
    r.arguments[2] = (dpi_x & 0xFF) as u8;
    r.arguments[3] = (dpi_y >> 8) as u8;
    r.arguments[4] = (dpi_y & 0xFF) as u8;
    r.arguments[5] = 0;
    r.arguments[6] = 0;
    r.finalize();
    r
}

/// Find the control hidraw node for a Razer device (interface 0, or the first if none match).
fn find_razer_hidraw(vendor: u16, product: u16) -> Result<String> {
    let rd = std::fs::read_dir("/sys/class/hidraw")
        .context("cannot read /sys/class/hidraw (hidraw kernel module not loaded?)")?;
    let mut candidates = vec![];
    for e in rd.flatten() {
        let uevent = e.path().join("device/uevent");
        let Ok(s) = std::fs::read_to_string(&uevent) else {
            continue;
        };
        let mut id = [0u16; 3];
        let mut iface = None;
        for line in s.lines() {
            if let Some(val) = line.strip_prefix("HID_ID=") {
                let parts: Vec<_> = val.split(':').collect();
                if parts.len() == 3 {
                    id[1] = u16::from_str_radix(parts[1], 16).unwrap_or(0);
                    id[2] = u16::from_str_radix(parts[2], 16).unwrap_or(0);
                }
            }
            if let Some(val) = line.strip_prefix("HID_PHYS=") {
                // usb-0000:0d:00.0-2/input0 → interface 0
                if let Some(last) = val.split('/').last() {
                    if let Some(n) = last.strip_prefix("input") {
                        iface = n.parse::<u8>().ok();
                    }
                }
            }
        }
        if id[1] == vendor && id[2] == product {
            let name = e.file_name().into_string().unwrap();
            candidates.push((name, iface));
        }
    }
    if candidates.is_empty() {
        bail!("Razer device {vendor:04x}:{product:04x} not found on hidraw");
    }
    // Prefer interface 0 (the control interface), else the first.
    let chosen = candidates
        .iter()
        .find(|(_, i)| *i == Some(0))
        .or_else(|| candidates.first())
        .unwrap();
    Ok(format!("/dev/{}", chosen.0))
}

/// Set DPI on a Razer Naga V2 HyperSpeed.
pub fn set_naga_v2_hyperspeed_dpi(dpi: u16) -> Result<()> {
    let path = find_razer_hidraw(RAZER_VENDOR, NAGA_V2_HYPERSPEED)?;
    let report = set_dpi_xy_report(dpi, dpi);
    let mut f = OpenOptions::new()
        .write(true)
        .custom_flags(libc::O_NONBLOCK)
        .open(&path)
        .with_context(|| format!("cannot open {path} (needs uaccess udev rule)"))?;
    f.write_all(report.as_bytes())
        .context("write razer_report failed")?;
    Ok(())
}

/// Diagnostic: check if the Naga V2 HyperSpeed is present.
pub fn naga_v2_hyperspeed_present() -> bool {
    find_razer_hidraw(RAZER_VENDOR, NAGA_V2_HYPERSPEED).is_ok()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn razer_report_size() {
        assert_eq!(std::mem::size_of::<RazerReport>(), 90);
    }

    #[test]
    fn set_dpi_xy_layout() {
        let r = set_dpi_xy_report(1600, 1600);
        assert_eq!(r.command_class, 0x04);
        assert_eq!(r.command_id, 0x05);
        assert_eq!(r.data_size, 0x07);
        assert_eq!(r.arguments[0], 0x00); // NOSTORE
        assert_eq!(r.arguments[1], 0x06); // 1600 >> 8
        assert_eq!(r.arguments[2], 0x40); // 1600 & 0xFF
        assert_eq!(r.arguments[3], 0x06);
        assert_eq!(r.arguments[4], 0x40);
        assert_eq!(r.crc, r.calculate_crc());
    }

    #[test]
    fn dpi_clamped() {
        let r = set_dpi_xy_report(50, 40000);
        let x = (r.arguments[1] as u16) << 8 | r.arguments[2] as u16;
        let y = (r.arguments[3] as u16) << 8 | r.arguments[4] as u16;
        assert_eq!(x, 100);
        assert_eq!(y, 30000);
    }
}
