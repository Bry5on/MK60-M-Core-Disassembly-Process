//! MK60 CAN telemetry: the cluster/DME keepalive handshake that makes the module
//! start broadcasting, plus decoders for the frames it then transmits.
//!
//! ## Why a handshake
//! The MK60 stays **silent on CAN until it hears the rest of the car** - the
//! instrument cluster and the DME it expects to be installed alongside. At power-up
//! it emits a few remote (RTR) frames on 0x610 (ICL1, the cluster message) asking
//! "where's the cluster?"; if nobody answers it never leaves boot and never
//! broadcasts. Answering with a short burst of spoofed cluster/DME frames puts it
//! into normal running mode, after which it transmits its own telemetry: wheel
//! speeds (0x1F0), steering (0x1F5) and ASC1 / vehicle-speed (0x153).
//!
//! The burst only has to be sent **once per MK60 power cycle** - the module then
//! runs standalone. (Whether it also needs a slow periodic keepalive to stay awake
//! is UNVERIFIED on our bench; the reference tool only re-sends on each boot RTR.)
//!
//! ## What this gates - and what it does NOT
//! This handshake is about CAN **broadcast**. The K-line diagnostic channel (our
//! `arm`) and the gated CCP slave (our `dump`) are request/response and are believed
//! independent of it. UNVERIFIED whether a non-woken MK60 still answers CCP - resolve
//! on the bench; if a cold module ignores CCP, send the keepalive first.
//!
//! ## Provenance
//! Frame bytes and decode layouts are the empirically-derived set from
//! peterakimball/mk60_can_tool (MIT), an E46 MK60 CAN translator. Decoding only confirms
//! the module is *broadcasting* - on the bench the values are meaningless (no wheel /
//! steering / pressure sensors are connected, so the module emits fault/default readings),
//! so this validates presence, not magnitudes. Real values need the module in a car.

/// One keepalive frame: CAN id + 8 data bytes.
pub type Frame = (u16, [u8; 8]);

/// Build the five-frame cluster/DME keepalive burst, in send order.
/// 0x610 ICL1 (VIN bytes 0-4), 0x613/0x615 IKE (cluster), 0x316/0x329 DME.
///
/// ICL1 carries a VIN, but the MK60 does **not** validate it - both a zero VIN and a real
/// donor VIN woke the bench module (2026-11-11), so we send zeros.
pub fn keepalive_burst() -> [Frame; 5] {
    [
        (0x610, [0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00]),
        (0x613, [0x64, 0x0A, 0x39, 0x05, 0x00, 0x00, 0x00, 0x00]),
        (0x615, [0x00, 0x00, 0x14, 0x00, 0x00, 0x00, 0x00, 0x00]),
        (0x316, [0x01, 0x00, 0xD9, 0x1E, 0x00, 0x00, 0x00, 0x00]),
        (0x329, [0x11, 0x5B, 0xC9, 0x08, 0x01, 0x00, 0x00, 0x00]),
    ]
}

/// Spacing between keepalive frames (ms). The reference tool uses 5 ms.
pub const KEEPALIVE_GAP_MS: u64 = 5;

// Broadcast ids the MK60 transmits once woken.
pub const ID_ASC1: u16 = 0x153; // vehicle speed, DSC active, brake light, ASC torque
pub const ID_ASC2: u16 = 0x1F0; // four wheel speeds (4 x 12-bit)
pub const ID_LWS1: u16 = 0x1F5; // steering angle + rate

/// The broadcast ids we expect from a healthy, woken MK60 - used for the
/// "did the module come alive?" validation summary.
pub const EXPECTED: [u16; 3] = [ID_ASC1, ID_ASC2, ID_LWS1];

const WHEEL_KMH_PER_LSB: f32 = 0.0625;
const STEER_DEG_PER_LSB: f32 = 0.045;

/// A decoded, display-ready telemetry field.
#[derive(Debug, Clone, PartialEq)]
pub struct Signal {
    pub name: &'static str,
    pub value: String,
}

fn sig(name: &'static str, value: String) -> Signal {
    Signal { name, value }
}

/// 12-bit little-endian field: full low byte + low nibble of the next byte.
fn u12(lo: u8, hi: u8) -> u16 {
    (lo as u16) | (((hi & 0x0F) as u16) << 8)
}

/// Decode a known MK60 broadcast frame into labeled signals. Returns `None` for
/// ids we don't recognize or frames too short to decode - the caller can still
/// show the raw bytes.
pub fn decode(id: u16, d: &[u8]) -> Option<Vec<Signal>> {
    let b = |i: usize| d.get(i).copied().unwrap_or(0);
    match id {
        ID_ASC2 if d.len() >= 8 => {
            let w = |o: usize| u12(b(o), b(o + 1)) as f32 * WHEEL_KMH_PER_LSB;
            Some(vec![
                sig("FL", format!("{:.2} km/h", w(0))),
                sig("FR", format!("{:.2} km/h", w(2))),
                sig("RL", format!("{:.2} km/h", w(4))),
                sig("RR", format!("{:.2} km/h", w(6))),
            ])
        }
        ID_LWS1 if d.len() >= 2 => {
            let raw = (b(0) as u16) | ((b(1) as u16) << 8);
            let mag = (raw & 0x7FFF) as f32 * STEER_DEG_PER_LSB;
            let ang = if raw & 0x8000 != 0 && mag != 0.0 { -mag } else { mag };
            Some(vec![sig("steer", format!("{ang:+.1} deg"))])
        }
        ID_ASC1 if d.len() >= 4 => {
            // byte0: bit3 = DSC active, bit4 = brake pressed. bytes1-2: vehicle-speed
            // raw (endianness/scale unconfirmed - shown raw). byte3: ASC torque intervention.
            let speed_raw = (b(1) as u16) | ((b(2) as u16) << 8);
            Some(vec![
                sig("DSC", if b(0) & 0x08 != 0 { "active".into() } else { "off".into() }),
                sig("brake", if b(0) & 0x10 != 0 { "on".into() } else { "off".into() }),
                sig("spd", format!("0x{speed_raw:04X}")),
                sig("ASCtq", format!("{:.0} %", b(3) as f32 * 0.392)),
            ])
        }
        _ => None,
    }
}

/// Render a decoded frame as one compact line (`FL=0.00 km/h  FR=...`), or `None`
/// if the id isn't a known MK60 broadcast.
pub fn decode_line(id: u16, d: &[u8]) -> Option<String> {
    decode(id, d).map(|sigs| {
        sigs.iter()
            .map(|s| format!("{}={}", s.name, s.value))
            .collect::<Vec<_>>()
            .join("  ")
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn keepalive_burst_shape() {
        let burst = keepalive_burst();
        assert_eq!(burst.len(), 5);
        // ICL1 first (the RTR trigger id), zero VIN.
        assert_eq!(burst[0].0, 0x610);
        assert_eq!(burst[0].1, [0u8; 8]);
        // the expected-broadcast ids are never ones we transmit.
        for (id, _) in burst {
            assert!(!EXPECTED.contains(&id));
        }
    }

    #[test]
    fn wheel_speeds() {
        // FL raw 0x010 = 16 -> 16 * 0.0625 = 1.00 km/h; upper nibble of odd byte masked off.
        let d = [0x10, 0xF0, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00];
        let s = decode(ID_ASC2, &d).unwrap();
        assert_eq!(s[0].value, "1.00 km/h");
        assert_eq!(s[1].value, "0.00 km/h");
    }

    #[test]
    fn steering_sign() {
        // 0x8000 set -> negative, magnitude 0.
        assert_eq!(decode(ID_LWS1, &[0x00, 0x80]).unwrap()[0].value, "+0.0 deg");
        // raw 100 (0x64) -> 100 * 0.045 = 4.5 deg, positive.
        assert_eq!(decode(ID_LWS1, &[0x64, 0x00]).unwrap()[0].value, "+4.5 deg");
    }

    #[test]
    fn asc1_flags() {
        // byte0 = 0x18 -> DSC active (bit3) + brake (bit4).
        let s = decode(ID_ASC1, &[0x18, 0x34, 0x12, 0x00]).unwrap();
        assert_eq!(s[0].value, "active");
        assert_eq!(s[1].value, "on");
        assert_eq!(s[2].value, "0x1234");
    }

    #[test]
    fn unknown_id_is_none() {
        assert!(decode(0x600, &[0; 8]).is_none());
        assert!(decode_line(0x600, &[0; 8]).is_none());
    }
}
