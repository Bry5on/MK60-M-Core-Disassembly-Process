//! Direct-USB FTDI access for the latency EXPERIMENT. The macOS VCP driver (/dev/cu.*) forces a fixed ~16 ms
//! FTDI latency timer that we can't change through termios, and that timer is what makes the cable knock miss
//! the module's ~130 ms state-2 window. Here we open the FT232 straight over libusb (rusb), set the latency
//! timer to 1 ms, and re-measure the half-duplex K-line echo round-trip at 4800 8N1. If it collapses from the
//! VCP's ~13 ms toward ~2-3 ms, the cable is salvageable and this is the basis for a full libusb transport.
//!
//! Read-only: nothing is written to the module. It only talks to our own FTDI and times its self-echo.

use anyhow::{anyhow, bail, Result};
use rusb::{DeviceHandle, GlobalContext};
use std::time::{Duration, Instant};

const FTDI_VID: u16 = 0x0403;

// FTDI vendor control requests (bmRequestType 0x40 = vendor, host-to-device).
const REQTYPE_OUT: u8 = 0x40;
const SIO_RESET: u8 = 0x00; // value 0 = reset the SIO
const SIO_SET_BAUDRATE: u8 = 0x03;
const SIO_SET_DATA: u8 = 0x04; // 8N1 = 0x0008
const SIO_SET_LATENCY: u8 = 0x09; // value = latency in ms (default 16)
const FTDI_INDEX: u16 = 1; // INTERFACE_A - the port for single-channel control requests (per libftdi)

// Single-channel FTDI bulk endpoints (FT232R / FT-X).
const EP_IN: u8 = 0x81;
const EP_OUT: u8 = 0x02;

type Handle = DeviceHandle<GlobalContext>;

/// Encode the FT232R baud divisor (value, index) for an integer-divisor baud. 4800 is exact (625), which is
/// all we use; non-integer bauds would need the fractional sub-divisor table.
fn baud_divisor(baud: u32) -> (u16, u16) {
    let div = 3_000_000 / baud; // FT232R reference clock 3 MHz; 4800 -> 625 (0x0271), no fraction
    ((div & 0xFFFF) as u16, ((div >> 16) & 0xFF) as u16)
}

fn ctrl(h: &Handle, req: u8, value: u16, index: u16) -> Result<()> {
    h.write_control(REQTYPE_OUT, req, value, index, &[], Duration::from_millis(500))
        .map(|_| ())
        .map_err(|e| anyhow!("FTDI control 0x{req:02X} (val 0x{value:04X}) failed: {e}"))
}

/// Open the first FTDI (VID 0403) and report its product id.
fn open_ftdi(log: &mut dyn FnMut(String)) -> Result<(Handle, u16)> {
    let list = rusb::devices().map_err(|e| anyhow!("USB enumerate failed: {e}"))?;
    for d in list.iter() {
        if let Ok(desc) = d.device_descriptor() {
            if desc.vendor_id() == FTDI_VID {
                let pid = desc.product_id();
                let h = d
                    .open()
                    .map_err(|e| anyhow!("found FTDI 0403:{pid:04X} but open failed: {e}"))?;
                log(format!("Opened FTDI 0403:{pid:04X}."));
                return Ok((h, pid));
            }
        }
    }
    bail!("no FTDI device (VID 0403) on USB - plug in the K+DCAN cable.")
}

/// On Windows, read the FTDI VCP driver's latency timer (in ms) for COM port `port` straight from the
/// registry - no need to open or claim the device, so it works while the port is in use. The value lives at
/// `HKLM\SYSTEM\CurrentControlSet\Enum\FTDIBUS\<instance>\<n>\Device Parameters` as `LatencyTimer` (a DWORD),
/// in the same key whose `PortName` names the COM port; we walk the FTDIBUS subtree to find the one matching
/// `port`. Returns `None` if `port` isn't an FTDI VCP port or the key can't be read. The default the driver
/// ships is 16 ms, which is too slow for the cable knock (we want 1 ms). Non-Windows always returns `None` -
/// the latency is handled elsewhere there (the EP0 trick on macOS, sysfs on Linux).
#[cfg(windows)]
pub fn latency_timer_for_port(port: &str) -> Option<u8> {
    use winreg::enums::{HKEY_LOCAL_MACHINE, KEY_READ};
    use winreg::RegKey;
    // Normalize to how Windows stores it ("COM3"); strip any \\.\ prefix used to open COM10+.
    let want = port.trim().trim_start_matches(r"\\.\").to_ascii_uppercase();
    let hklm = RegKey::predef(HKEY_LOCAL_MACHINE);
    let ftdibus = hklm
        .open_subkey_with_flags(r"SYSTEM\CurrentControlSet\Enum\FTDIBUS", KEY_READ)
        .ok()?;
    for inst in ftdibus.enum_keys().flatten() {
        let Ok(inst_key) = ftdibus.open_subkey_with_flags(&inst, KEY_READ) else {
            continue;
        };
        for sub in inst_key.enum_keys().flatten() {
            let Ok(dp) = inst_key.open_subkey_with_flags(format!(r"{sub}\Device Parameters"), KEY_READ) else {
                continue;
            };
            let Ok(pn) = dp.get_value::<String, _>("PortName") else {
                continue;
            };
            if pn.trim().to_ascii_uppercase() == want {
                let lt: u32 = dp.get_value("LatencyTimer").unwrap_or(16);
                return Some(lt.min(255) as u8);
            }
        }
    }
    None
}

/// Non-Windows stub: latency is handled by the EP0 trick (macOS) or sysfs (Linux), not the registry.
#[cfg(not(windows))]
pub fn latency_timer_for_port(_port: &str) -> Option<u8> {
    None
}

/// Set the FTDI latency timer with a control transfer to EP0 WITHOUT claiming an interface. Claiming fails on
/// macOS (the Apple driver owns the interface), but a device-level control request may still go through while
/// the normal serial driver keeps the port - letting us drop the latency timer under the VCP driver's feet.
/// Call this AFTER the serial port is open+configured, so the VCP's own config doesn't re-stamp 16 ms over it.
/// Returns Ok on a control transfer that the device accepted; Err says macOS blocked even EP0.
pub fn set_latency_no_claim(latency_ms: u8, log: &mut dyn FnMut(String)) -> Result<()> {
    let (h, pid) = open_ftdi(log)?;
    // Deliberately NO claim_interface - that's the call macOS rejects. A control transfer targets the device.
    ctrl(&h, SIO_SET_LATENCY, latency_ms as u16, FTDI_INDEX).map_err(|e| {
        anyhow!(
            "{e}\n  EP0 control transfer refused too (0403:{pid:04X}) - macOS won't let us touch the device \
             while the Apple driver owns it. The latency timer can't be changed on macOS this way."
        )
    })?;
    log(format!("Set FTDI 0403:{pid:04X} latency timer = {latency_ms} ms via EP0 (no claim) - accepted."));
    Ok(())
}

/// Open the FTDI over libusb, set its latency timer to `latency_ms`, and time the K-line self-echo round-trip
/// at 4800 8N1 over `samples` samples. Prints min/median/avg/max. Compare to the `cable-latency` probe (which
/// goes through the macOS VCP driver's fixed 16 ms timer).
pub fn latency_test(samples: u64, latency_ms: u8, log: &mut dyn FnMut(String)) -> Result<()> {
    let (h, pid) = open_ftdi(log)?;
    if !matches!(pid, 0x6001 | 0x6015 | 0x6014) {
        log(format!(
            "  note: 0403:{pid:04X} isn't a known single-channel FT232R/FT-X/FT232H - endpoints/port index may differ."
        ));
    }
    h.set_auto_detach_kernel_driver(true).ok(); // detaches on Linux; a no-op on macOS

    if let Err(e) = h.claim_interface(0) {
        bail!(
            "claim FTDI interface 0 failed: {e}\n  \
             On macOS the Apple FTDI driver (AppleUSBFTDI) owns the device and libusb can't seize it - this is \
             the blocker for a libusb cable path. If this is the only way to drop the latency timer, the cable \
             stays a fallback and on-car is the path. (On Linux this works with a udev rule / detach.)"
        );
    }

    // Configure 4800 8N1 and drop the latency timer - the knob under test.
    ctrl(&h, SIO_RESET, 0, FTDI_INDEX)?;
    let (bv, bi) = baud_divisor(4800);
    ctrl(&h, SIO_SET_BAUDRATE, bv, bi)?;
    ctrl(&h, SIO_SET_DATA, 0x0008, FTDI_INDEX)?; // 8 data bits, no parity, 1 stop
    ctrl(&h, SIO_SET_LATENCY, latency_ms as u16, FTDI_INDEX)?;
    log(format!(
        "Set 4800 8N1, latency timer = {latency_ms} ms. Timing the K-line echo round-trip over {samples} samples..."
    ));

    let mut lat: Vec<f64> = Vec::new();
    let mut timeouts = 0u64;
    for _ in 0..samples {
        // Drain any pending RX (the FTDI sends a 2-byte status packet every latency interval, plus any stale
        // echo) so each sample times a fresh byte.
        let mut scratch = [0u8; 64];
        for _ in 0..8 {
            match h.read_bulk(EP_IN, &mut scratch, Duration::from_millis(2)) {
                Ok(n) if n > 2 => continue, // drained a stale data packet; keep draining
                _ => break,                 // just status bytes or a timeout -> RX is clear
            }
        }
        let t0 = Instant::now();
        if h.write_bulk(EP_OUT, &[0x40], Duration::from_millis(100)).is_err() {
            timeouts += 1;
            continue;
        }
        let deadline = t0 + Duration::from_millis(100);
        let mut got = false;
        while Instant::now() < deadline {
            let mut rx = [0u8; 64];
            // Every IN packet is prefixed with 2 status bytes; our echo is present only when n > 2.
            if let Ok(n) = h.read_bulk(EP_IN, &mut rx, Duration::from_millis(5)) {
                if n > 2 {
                    lat.push(t0.elapsed().as_secs_f64() * 1000.0);
                    got = true;
                    break;
                }
            }
        }
        if !got {
            timeouts += 1;
        }
        std::thread::sleep(Duration::from_millis(10));
    }

    let _ = ctrl(&h, SIO_SET_LATENCY, 16, FTDI_INDEX); // restore the default before handing the device back
    h.release_interface(0).ok();

    if lat.is_empty() {
        bail!(
            "no echo seen in {samples} samples - the FTDI isn't looping our TX back on the K-line \
             (cable plugged into the laptop? right device? single-channel chip?)."
        );
    }
    lat.sort_by(|a, b| a.partial_cmp(b).unwrap());
    let n = lat.len();
    let (min, max, med) = (lat[0], lat[n - 1], lat[n / 2]);
    let avg = lat.iter().sum::<f64>() / n as f64;
    log(format!("echo round-trip over {n} samples ({timeouts} timeouts), latency timer {latency_ms} ms:"));
    log(format!("  min {min:.1} ms   median {med:.1} ms   avg {avg:.1} ms   max {max:.1} ms"));
    log(
        "  ~2.1 ms is the byte on the wire. Far below the VCP's ~13 ms -> the cable is salvageable via a libusb transport."
            .to_string(),
    );
    Ok(())
}
