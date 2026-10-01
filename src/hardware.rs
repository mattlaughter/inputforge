//! Hardware-side device settings: DPI / polling rate / onboard LEDs via libratbag's
//! `ratbagd` (DBus), and per-LED RGB via the OpenRGB SDK server (TCP 6742).

use anyhow::{Context, Result, anyhow, bail};
use std::io::{Read, Write};
use std::net::TcpStream;
use std::time::Duration;
use zbus::blocking::{Connection, Proxy};
use zbus::zvariant::{OwnedObjectPath, OwnedValue, Value};

// ───────────────────────────── ratbagd ─────────────────────────────

const RB_DEST: &str = "org.freedesktop.ratbag1";

#[derive(Debug, Clone)]
pub struct RbResolution {
    pub path: OwnedObjectPath,
    pub dpi: u32,
    pub active: bool,
    pub disabled: bool,
}

#[derive(Debug, Clone)]
pub struct RbLed {
    pub path: OwnedObjectPath,
    pub mode: u32,
    pub modes: Vec<u32>,
    pub color: [u8; 3],
    pub brightness: u32,
}

#[derive(Debug, Clone)]
pub struct RbProfile {
    pub path: OwnedObjectPath,
    pub index: u32,
    pub active: bool,
    pub resolutions: Vec<RbResolution>,
    pub dpi_choices: Vec<u32>,
    pub report_rate: u32,
    pub report_rates: Vec<u32>,
    pub leds: Vec<RbLed>,
}

#[derive(Debug, Clone)]
pub struct RbDevice {
    pub path: OwnedObjectPath,
    pub name: String,
    pub model: String,
    pub profiles: Vec<RbProfile>,
}

pub fn led_mode_name(m: u32) -> &'static str {
    match m {
        0 => "Off",
        1 => "Solid",
        2 => "Cycle",
        3 => "Breathing",
        _ => "Unknown",
    }
}

fn proxy<'a>(
    conn: &'a Connection,
    path: &'a OwnedObjectPath,
    iface: &'static str,
) -> Result<Proxy<'a>> {
    Ok(Proxy::new(conn, RB_DEST, path.as_str(), iface)?)
}

fn dpi_from_value(v: &OwnedValue) -> u32 {
    match &**v {
        Value::U32(x) => *x,
        Value::Structure(s) => s
            .fields()
            .first()
            .and_then(|f| u32::try_from(f).ok())
            .unwrap_or(0),
        _ => 0,
    }
}

pub struct Ratbag {
    conn: Connection,
}

impl Ratbag {
    pub fn connect() -> Result<Self> {
        let conn = Connection::system().context("cannot connect to the system DBus")?;
        let rb = Self { conn };
        // Probe that ratbagd exists.
        let mgr = Proxy::new(
            &rb.conn,
            RB_DEST,
            "/org/freedesktop/ratbag1",
            "org.freedesktop.ratbag1.Manager",
        )?;
        mgr.get_property::<Vec<OwnedObjectPath>>("Devices").map_err(|e| {
            anyhow!("ratbagd not available ({e}). Install it: sudo pacman -S libratbag && sudo systemctl enable --now ratbagd")
        })?;
        Ok(rb)
    }

    pub fn devices(&self) -> Result<Vec<RbDevice>> {
        let mgr = Proxy::new(
            &self.conn,
            RB_DEST,
            "/org/freedesktop/ratbag1",
            "org.freedesktop.ratbag1.Manager",
        )?;
        let paths: Vec<OwnedObjectPath> = mgr.get_property("Devices")?;
        let mut out = vec![];
        for p in paths {
            let d = proxy(&self.conn, &p, "org.freedesktop.ratbag1.Device")?;
            let name: String = d.get_property("Name").unwrap_or_default();
            let model: String = d.get_property("Model").unwrap_or_default();
            let prof_paths: Vec<OwnedObjectPath> = d.get_property("Profiles").unwrap_or_default();
            let mut profiles = vec![];
            for pp in prof_paths {
                let pr = proxy(&self.conn, &pp, "org.freedesktop.ratbag1.Profile")?;
                if pr.get_property::<bool>("Disabled").unwrap_or(false) {
                    continue;
                }
                let mut resolutions = vec![];
                let mut dpi_choices = vec![];
                for rp in pr
                    .get_property::<Vec<OwnedObjectPath>>("Resolutions")
                    .unwrap_or_default()
                {
                    let r = proxy(&self.conn, &rp, "org.freedesktop.ratbag1.Resolution")?;
                    if dpi_choices.is_empty() {
                        dpi_choices = r
                            .get_property::<Vec<u32>>("Resolutions")
                            .unwrap_or_default();
                    }
                    resolutions.push(RbResolution {
                        dpi: r
                            .get_property::<OwnedValue>("Resolution")
                            .map(|v| dpi_from_value(&v))
                            .unwrap_or(0),
                        active: r.get_property("IsActive").unwrap_or(false),
                        disabled: r.get_property("IsDisabled").unwrap_or(false),
                        path: rp.clone(),
                    });
                }
                let mut leds = vec![];
                for lp in pr
                    .get_property::<Vec<OwnedObjectPath>>("Leds")
                    .unwrap_or_default()
                {
                    let l = proxy(&self.conn, &lp, "org.freedesktop.ratbag1.Led")?;
                    let (r, g, b): (u32, u32, u32) = l.get_property("Color").unwrap_or((0, 0, 0));
                    leds.push(RbLed {
                        mode: l.get_property("Mode").unwrap_or(0),
                        modes: l.get_property("Modes").unwrap_or_default(),
                        color: [r as u8, g as u8, b as u8],
                        brightness: l.get_property("Brightness").unwrap_or(255),
                        path: lp.clone(),
                    });
                }
                profiles.push(RbProfile {
                    index: pr.get_property("Index").unwrap_or(0),
                    active: pr.get_property("IsActive").unwrap_or(false),
                    report_rate: pr.get_property("ReportRate").unwrap_or(0),
                    report_rates: pr.get_property("ReportRates").unwrap_or_default(),
                    resolutions,
                    dpi_choices,
                    leds,
                    path: pp.clone(),
                });
            }
            out.push(RbDevice {
                path: p.clone(),
                name,
                model,
                profiles,
            });
        }
        Ok(out)
    }

    pub fn set_dpi(&self, res: &OwnedObjectPath, dpi: u32) -> Result<()> {
        let r = proxy(&self.conn, res, "org.freedesktop.ratbag1.Resolution")?;
        // Devices with separate X/Y DPI expose a (uu) struct; others a plain u.
        let current: OwnedValue = r.get_property("Resolution")?;
        let v: Value = match &*current {
            Value::Structure(_) => Value::from((dpi, dpi)),
            _ => Value::from(dpi),
        };
        r.set_property("Resolution", v)?;
        Ok(())
    }

    pub fn set_active_resolution(&self, res: &OwnedObjectPath) -> Result<()> {
        let r = proxy(&self.conn, res, "org.freedesktop.ratbag1.Resolution")?;
        r.call::<_, _, ()>("SetActive", &())?;
        Ok(())
    }

    pub fn set_report_rate(&self, profile: &OwnedObjectPath, hz: u32) -> Result<()> {
        proxy(&self.conn, profile, "org.freedesktop.ratbag1.Profile")?
            .set_property("ReportRate", hz)?;
        Ok(())
    }

    pub fn set_led(
        &self,
        led: &OwnedObjectPath,
        mode: u32,
        color: [u8; 3],
        brightness: u32,
    ) -> Result<()> {
        let l = proxy(&self.conn, led, "org.freedesktop.ratbag1.Led")?;
        l.set_property("Mode", mode)?;
        l.set_property("Color", (color[0] as u32, color[1] as u32, color[2] as u32))?;
        l.set_property("Brightness", brightness)?;
        Ok(())
    }

    pub fn commit(&self, dev: &OwnedObjectPath) -> Result<()> {
        proxy(&self.conn, dev, "org.freedesktop.ratbag1.Device")?
            .call::<_, _, ()>("Commit", &())?;
        Ok(())
    }
}

// ───────────────────────────── OpenRGB SDK ─────────────────────────────

const PKT_REQUEST_CONTROLLER_COUNT: u32 = 0;
const PKT_REQUEST_CONTROLLER_DATA: u32 = 1;
const PKT_REQUEST_PROTOCOL_VERSION: u32 = 40;
const PKT_SET_CLIENT_NAME: u32 = 50;
const PKT_UPDATELEDS: u32 = 1050;
const PKT_SETCUSTOMMODE: u32 = 1100;
/// Highest protocol we can parse. v4 is supported by OpenRGB 0.9 and 1.0.
const CLIENT_PROTOCOL: u32 = 4;

#[derive(Debug, Clone)]
pub struct RgbController {
    pub id: u32,
    pub name: String,
    pub vendor: String,
    pub num_leds: usize,
    pub zones: Vec<(String, u32)>,
    pub colors: Vec<[u8; 3]>,
}

pub struct OpenRgb {
    stream: TcpStream,
    protocol: u32,
}

/// Little-endian cursor over a packet body.
struct Rd<'a> {
    b: &'a [u8],
    i: usize,
}

impl<'a> Rd<'a> {
    fn take(&mut self, n: usize) -> Result<&'a [u8]> {
        if self.i + n > self.b.len() {
            bail!("truncated OpenRGB packet");
        }
        let s = &self.b[self.i..self.i + n];
        self.i += n;
        Ok(s)
    }
    fn u16(&mut self) -> Result<u16> {
        Ok(u16::from_le_bytes(self.take(2)?.try_into()?))
    }
    fn u32(&mut self) -> Result<u32> {
        Ok(u32::from_le_bytes(self.take(4)?.try_into()?))
    }
    fn string(&mut self) -> Result<String> {
        let n = self.u16()? as usize;
        let s = self.take(n)?;
        Ok(String::from_utf8_lossy(s)
            .trim_end_matches('\0')
            .to_string())
    }
}

impl OpenRgb {
    pub fn connect(addr: &str) -> Result<Self> {
        let sa = addr
            .parse()
            .or_else(|_| format!("{addr}:6742").parse())
            .context("bad OpenRGB address")?;
        let stream = TcpStream::connect_timeout(&sa, Duration::from_secs(2)).map_err(|e| {
            anyhow!("cannot reach OpenRGB SDK server at {sa} ({e}). Start it with: openrgb --server (or enable SDK Server in the OpenRGB GUI)")
        })?;
        stream.set_read_timeout(Some(Duration::from_secs(3)))?;
        let mut c = Self {
            stream,
            protocol: 0,
        };
        c.send(0, PKT_SET_CLIENT_NAME, b"InputForge\0")?;
        c.send(
            0,
            PKT_REQUEST_PROTOCOL_VERSION,
            &CLIENT_PROTOCOL.to_le_bytes(),
        )?;
        c.protocol = match c.recv_expect(PKT_REQUEST_PROTOCOL_VERSION) {
            Ok(b) if b.len() >= 4 => u32::from_le_bytes(b[..4].try_into()?).min(CLIENT_PROTOCOL),
            _ => 0,
        };
        Ok(c)
    }

    pub fn protocol(&self) -> u32 {
        self.protocol
    }

    fn send(&mut self, dev: u32, id: u32, data: &[u8]) -> Result<()> {
        let mut pkt = Vec::with_capacity(16 + data.len());
        pkt.extend_from_slice(b"ORGB");
        pkt.extend_from_slice(&dev.to_le_bytes());
        pkt.extend_from_slice(&id.to_le_bytes());
        pkt.extend_from_slice(&(data.len() as u32).to_le_bytes());
        pkt.extend_from_slice(data);
        self.stream.write_all(&pkt)?;
        Ok(())
    }

    /// Read packets until one with the expected id arrives (skipping notifications).
    fn recv_expect(&mut self, want: u32) -> Result<Vec<u8>> {
        for _ in 0..32 {
            let mut hdr = [0u8; 16];
            self.stream.read_exact(&mut hdr)?;
            if &hdr[..4] != b"ORGB" {
                bail!("bad OpenRGB magic");
            }
            let id = u32::from_le_bytes(hdr[8..12].try_into()?);
            let size = u32::from_le_bytes(hdr[12..16].try_into()?) as usize;
            if size > 64 * 1024 * 1024 {
                bail!("OpenRGB packet too large");
            }
            let mut body = vec![0u8; size];
            self.stream.read_exact(&mut body)?;
            if id == want {
                return Ok(body);
            }
        }
        bail!("no reply from OpenRGB for packet {want}")
    }

    pub fn controllers(&mut self) -> Result<Vec<RgbController>> {
        self.send(0, PKT_REQUEST_CONTROLLER_COUNT, &[])?;
        let b = self.recv_expect(PKT_REQUEST_CONTROLLER_COUNT)?;
        let count = u32::from_le_bytes(
            b.get(..4)
                .ok_or_else(|| anyhow!("short reply"))?
                .try_into()?,
        );
        let mut out = vec![];
        for id in 0..count {
            let req = if self.protocol > 0 {
                self.protocol.to_le_bytes().to_vec()
            } else {
                vec![]
            };
            self.send(id, PKT_REQUEST_CONTROLLER_DATA, &req)?;
            let body = self.recv_expect(PKT_REQUEST_CONTROLLER_DATA)?;
            out.push(parse_controller(id, &body, self.protocol)?);
        }
        Ok(out)
    }

    pub fn set_custom_mode(&mut self, id: u32) -> Result<()> {
        self.send(id, PKT_SETCUSTOMMODE, &[])
    }

    pub fn update_leds(&mut self, id: u32, colors: &[[u8; 3]]) -> Result<()> {
        let n = colors.len() as u16;
        let data_size = 4 + 2 + 4 * colors.len() as u32;
        let mut d = Vec::with_capacity(data_size as usize);
        d.extend_from_slice(&data_size.to_le_bytes());
        d.extend_from_slice(&n.to_le_bytes());
        for c in colors {
            d.extend_from_slice(&[c[0], c[1], c[2], 0]);
        }
        self.send(id, PKT_UPDATELEDS, &d)
    }
}

fn parse_controller(id: u32, body: &[u8], proto: u32) -> Result<RgbController> {
    let mut r = Rd { b: body, i: 0 };
    let _data_size = r.u32()?;
    let _type = r.u32()?;
    let name = r.string()?;
    let vendor = if proto >= 1 {
        r.string()?
    } else {
        String::new()
    };
    let _desc = r.string()?;
    let _version = r.string()?;
    let _serial = r.string()?;
    let _location = r.string()?;
    let num_modes = r.u16()?;
    let _active_mode = r.u32()?;
    for _ in 0..num_modes {
        r.string()?; // name
        r.u32()?; // value
        r.take(4 * 3)?; // flags, speed_min, speed_max
        if proto >= 3 {
            r.take(4 * 2)?; // brightness min/max
        }
        r.take(4 * 2)?; // colors min/max
        r.u32()?; // speed
        if proto >= 3 {
            r.u32()?; // brightness
        }
        r.take(4 * 2)?; // direction, color_mode
        let nc = r.u16()? as usize;
        r.take(4 * nc)?;
    }
    let num_zones = r.u16()?;
    let mut zones = vec![];
    for _ in 0..num_zones {
        let zname = r.string()?;
        r.u32()?; // type
        r.u32()?; // min
        r.u32()?; // max
        let count = r.u32()?;
        let mlen = r.u16()? as usize;
        r.take(mlen)?;
        if proto >= 4 {
            let nseg = r.u16()?;
            for _ in 0..nseg {
                r.string()?;
                r.take(4 * 3)?;
            }
        }
        if proto >= 5 {
            r.u32()?;
        }
        zones.push((zname, count));
    }
    let num_leds = r.u16()? as usize;
    for _ in 0..num_leds {
        r.string()?;
        r.u32()?;
    }
    let num_colors = r.u16()? as usize;
    let mut colors = Vec::with_capacity(num_colors);
    for _ in 0..num_colors {
        let c = r.take(4)?;
        colors.push([c[0], c[1], c[2]]);
    }
    Ok(RgbController {
        id,
        name,
        vendor,
        num_leds,
        zones,
        colors,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn s(v: &mut Vec<u8>, t: &str) {
        let b = format!("{t}\0");
        v.extend_from_slice(&(b.len() as u16).to_le_bytes());
        v.extend_from_slice(b.as_bytes());
    }

    #[test]
    fn parse_minimal_controller_v4() {
        let mut v = vec![];
        v.extend_from_slice(&0u32.to_le_bytes()); // data size
        v.extend_from_slice(&1u32.to_le_bytes()); // type
        for t in ["Kbd", "Logi", "desc", "1.0", "SN", "HID"] {
            s(&mut v, t);
        }
        v.extend_from_slice(&1u16.to_le_bytes()); // modes
        v.extend_from_slice(&0u32.to_le_bytes()); // active
        s(&mut v, "Direct");
        v.extend(std::iter::repeat_n(0u8, 4 * 12)); // value..color_mode
        v.extend_from_slice(&0u16.to_le_bytes()); // mode colors
        v.extend_from_slice(&1u16.to_le_bytes()); // zones
        s(&mut v, "Keys");
        v.extend(std::iter::repeat_n(0u8, 12));
        v.extend_from_slice(&2u32.to_le_bytes()); // count
        v.extend_from_slice(&0u16.to_le_bytes()); // matrix
        v.extend_from_slice(&0u16.to_le_bytes()); // segments
        v.extend_from_slice(&2u16.to_le_bytes()); // leds
        for n in ["A", "B"] {
            s(&mut v, n);
            v.extend_from_slice(&0u32.to_le_bytes());
        }
        v.extend_from_slice(&2u16.to_le_bytes());
        v.extend_from_slice(&[255, 0, 0, 0, 0, 255, 0, 0]);
        let c = parse_controller(3, &v, 4).unwrap();
        assert_eq!(c.name, "Kbd");
        assert_eq!(c.vendor, "Logi");
        assert_eq!(c.zones, vec![("Keys".to_string(), 2)]);
        assert_eq!(c.colors, vec![[255, 0, 0], [0, 255, 0]]);
    }
}
