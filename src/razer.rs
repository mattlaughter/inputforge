//! Razer mouse hardware settings over hidraw (DPI).
//!
//! Supported: Razer Naga V2 HyperSpeed (receiver, 1532:00B4).
//!
//! Protocol follows OpenRazer's GPL driver (razercommon.c / razerchromacommon.c): a 90-byte
//! report sent as a HID *feature* report (USB SET_REPORT, type feature, report id 0) and the
//! answer read back with GET_REPORT. A plain `write()` sends an output report, which the mouse
//! silently ignores. The receiver uses transaction id 0x1F.

use anyhow::{Context, Result, bail};
use std::fs::{File, OpenOptions};
use std::os::fd::AsRawFd;
use std::time::Duration;

const RAZER_VENDOR: u16 = 0x1532;
const NAGA_V2_HYPERSPEED: u16 = 0x00B4;
const TRANSACTION_ID: u8 = 0x1F;
const VARSTORE: u8 = 0x01;
pub const DPI_MIN: u16 = 100;
pub const DPI_MAX: u16 = 30000;

const REPORT_LEN: usize = 90;
/// Status byte in a reply.
const STATUS_BUSY: u8 = 0x01;
const STATUS_OK: u8 = 0x02;

/// Build a report: [status, tid, remaining(2), proto, size, class, cmd, args[80], crc, 0].
fn report(class: u8, cmd: u8, size: u8, args: &[u8]) -> [u8; REPORT_LEN] {
    let mut r = [0u8; REPORT_LEN];
    r[1] = TRANSACTION_ID;
    r[5] = size;
    r[6] = class;
    r[7] = cmd;
    r[8..8 + args.len()].copy_from_slice(args);
    r[88] = r[2..88].iter().fold(0, |a, b| a ^ b);
    r
}

fn set_dpi_report(dpi: u16) -> [u8; REPORT_LEN] {
    let [hi, lo] = dpi.clamp(DPI_MIN, DPI_MAX).to_be_bytes();
    report(0x04, 0x05, 0x07, &[VARSTORE, hi, lo, hi, lo, 0, 0])
}

fn get_dpi_report() -> [u8; REPORT_LEN] {
    report(0x04, 0x85, 0x07, &[VARSTORE])
}

/// `_IOC(_IOC_READ|_IOC_WRITE, 'H', nr, len)`
fn hid_ioc(nr: u64, len: usize) -> u64 {
    (3 << 30) | ((len as u64) << 16) | ((b'H' as u64) << 8) | nr
}

fn feature(f: &File, nr: u64, buf: &mut [u8]) -> Result<()> {
    let r = unsafe { libc::ioctl(f.as_raw_fd(), hid_ioc(nr, buf.len()) as _, buf.as_mut_ptr()) };
    if r < 0 {
        bail!("{}", std::io::Error::last_os_error());
    }
    Ok(())
}

/// Send a report and return the reply (90 bytes, without the report id).
fn transfer(f: &File, req: &[u8; REPORT_LEN]) -> Result<[u8; REPORT_LEN]> {
    let mut buf = [0u8; REPORT_LEN + 1]; // byte 0 = report id 0
    buf[1..].copy_from_slice(req);
    feature(f, 0x06, &mut buf).context("HIDIOCSFEATURE")?;
    for _ in 0..10 {
        std::thread::sleep(Duration::from_millis(30));
        let mut out = [0u8; REPORT_LEN + 1];
        feature(f, 0x07, &mut out).context("HIDIOCGFEATURE")?;
        let mut reply = [0u8; REPORT_LEN];
        reply.copy_from_slice(&out[1..]);
        if reply[0] != STATUS_BUSY {
            if reply[0] != STATUS_OK {
                bail!("mouse rejected the command (status {:#04x})", reply[0]);
            }
            return Ok(reply);
        }
    }
    bail!("mouse stayed busy")
}

/// Interface number from a hidraw `HID_PHYS` like `usb-0000:0d:00.0-2/input0`.
fn phys_interface(phys: &str) -> Option<u8> {
    phys.rsplit('/').next()?.strip_prefix("input")?.parse().ok()
}

/// The control hidraw node (interface 0) of the Naga V2 HyperSpeed.
fn find_hidraw() -> Result<String> {
    let want = format!("HID_ID=0003:{RAZER_VENDOR:08X}:{NAGA_V2_HYPERSPEED:08X}");
    for e in std::fs::read_dir("/sys/class/hidraw")
        .context("cannot read /sys/class/hidraw")?
        .flatten()
    {
        let Ok(u) = std::fs::read_to_string(e.path().join("device/uevent")) else {
            continue;
        };
        let phys = u.lines().find_map(|l| l.strip_prefix("HID_PHYS="));
        if u.lines().any(|l| l == want) && phys.and_then(phys_interface) == Some(0) {
            return Ok(format!("/dev/{}", e.file_name().to_string_lossy()));
        }
    }
    bail!("Razer Naga V2 HyperSpeed not found")
}

fn open() -> Result<File> {
    let path = find_hidraw()?;
    OpenOptions::new()
        .read(true)
        .write(true)
        .open(&path)
        .with_context(|| format!("cannot open {path} (reinstall to get the udev rule)"))
}

/// Current DPI (X axis) as reported by the mouse.
pub fn naga_dpi() -> Result<u16> {
    let r = transfer(&open()?, &get_dpi_report())?;
    Ok(u16::from_be_bytes([r[9], r[10]]))
}

/// Set DPI and return the value the mouse reports afterwards.
pub fn set_naga_dpi(dpi: u16) -> Result<u16> {
    let f = open()?;
    transfer(&f, &set_dpi_report(dpi))?;
    let r = transfer(&f, &get_dpi_report())?;
    Ok(u16::from_be_bytes([r[9], r[10]]))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn set_dpi_layout() {
        let r = set_dpi_report(1600);
        assert_eq!(&r[..8], &[0, 0x1F, 0, 0, 0, 0x07, 0x04, 0x05]);
        assert_eq!(&r[8..13], &[VARSTORE, 0x06, 0x40, 0x06, 0x40]);
        assert_eq!(r[88], r[2..88].iter().fold(0, |a, b| a ^ b));
        assert_eq!(r[89], 0);
    }

    #[test]
    fn get_dpi_layout() {
        let r = get_dpi_report();
        assert_eq!(&r[5..9], &[0x07, 0x04, 0x85, VARSTORE]);
    }

    #[test]
    fn dpi_clamped() {
        assert_eq!(&set_dpi_report(50)[9..11], &DPI_MIN.to_be_bytes());
        assert_eq!(&set_dpi_report(40000)[9..11], &DPI_MAX.to_be_bytes());
    }

    #[test]
    fn feature_ioctl_numbers() {
        // Same values as the kernel's HIDIOCSFEATURE(91) / HIDIOCGFEATURE(91).
        assert_eq!(hid_ioc(0x06, 91), 0xC05B_4806);
        assert_eq!(hid_ioc(0x07, 91), 0xC05B_4807);
    }

    #[test]
    fn interface_from_phys() {
        assert_eq!(phys_interface("usb-0000:0d:00.0-2/input0"), Some(0));
        assert_eq!(phys_interface("usb-0000:0d:00.0-2/input2"), Some(2));
        assert_eq!(phys_interface("garbage"), None);
    }
}
