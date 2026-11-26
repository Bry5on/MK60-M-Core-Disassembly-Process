//! KWP2000 over K-line (BMW-FAST) + the SecurityAccess seed->key transforms.
//! Verified against the 7846816A disassembly (FUN_000b1468) and the INPA trace.
//!
//! **Wire format (confirmed on the E46 MK60 bench, 2026-11-11):** 9600 **8E1** (even parity), and the
//! frame checksum is **XOR** of all preceding bytes - e.g. the read request `B8 29 F1 02 21 06 45`
//! (0x45 = B8^29^F1^02^21^06) got a valid `61 06 ...` reply. The old ADDITIVE checksum (from an INPA
//! trace) does NOT work and has been removed; parity is fixed at 8E1 in the firmware. This matches the
//! handmade0octopus/ds2 library (MIT) `forceKwp` path (src/DS2.cpp: `checksum ^= data[i]`).

pub const DSC_ADDR: u8 = 0x29; // target (DSC module)
pub const TESTER: u8 = 0xF1; // source (tester)

/// K-line frame checksum: XOR of every byte before the checksum. Confirmed on the E46 MK60 bench.
fn checksum(bytes: &[u8]) -> u8 {
    bytes.iter().fold(0u8, |a, &b| a ^ b)
}

/// BMW-FAST frame: B8 <target> <source> <len> <data...> <checksum>.
pub fn bmw_fast(target: u8, source: u8, data: &[u8]) -> Vec<u8> {
    let mut f = vec![0xB8, target, source, data.len() as u8];
    f.extend_from_slice(data);
    f.push(checksum(&f));
    f
}
/// A request to the DSC (target 0x29, source 0xF1).
pub fn req(data: &[u8]) -> Vec<u8> {
    bmw_fast(DSC_ADDR, TESTER, data)
}

/// Human-readable name for a KWP2000/UDS negative-response code (the byte after `7F <sid>`).
pub fn nrc_name(nrc: u8) -> &'static str {
    match nrc {
        0x10 => "generalReject",
        0x11 => "serviceNotSupported",
        0x12 => "subFunctionNotSupported",
        0x13 => "wrongLength/invalidFormat",
        0x21 => "busyRepeatRequest",
        0x22 => "conditionsNotCorrect",
        0x23 => "routineNotComplete",
        0x31 => "requestOutOfRange",
        0x33 => "securityAccessDenied",
        0x35 => "invalidKey",
        0x36 => "exceededNumberOfAttempts",
        0x37 => "requiredTimeDelayNotExpired",
        0x78 => "responsePending",
        _ => "?",
    }
}

// ---- SecurityAccess key (FUN_000b1468): key = bitrev16(seed) ^ ror16(seed, subfn+1) ----
pub fn bitrev16(x: u16) -> u16 { x.reverse_bits() }
pub fn ror16(x: u16, n: u32) -> u16 { x.rotate_right(n % 16) }
/// Level 4 (sendKey subfn 0x08 -> rotate by 9). This is the one the CCP arm needs.
pub fn key_level4(seed: u16) -> u16 { bitrev16(seed) ^ ror16(seed, 9) }
/// Level 2 (sendKey subfn 0x04 -> rotate by 5). The documented token-3/level-2 setter.
pub fn key_level2(seed: u16) -> u16 { bitrev16(seed) ^ ror16(seed, 5) }

fn bit(v: u16, n: u32) -> u16 { (v >> n) & 1 }

/// Level 0x7E key (`diag_key_calc_7e`, the "afb9e" fold) XOR the per-image constant `c` (from the
/// `1A 94` ident appendix). From security-access.md. `acc_iters` is the accumulator loop count -
/// the doc's `for i in 0..12` is ambiguous (12 vs 13); by the 0x7F result the doc's `0..N` is
/// inclusive, so 13 is the likely value (0x7E is absent on the E46 units, so this fold is unexercised there).
pub fn key_calc_7e(seed: u16, c: u16, acc_iters: u32) -> u16 {
    let s = seed;
    let ns = !seed;
    let mut res: u16 = 0;
    for base in [0u32, 4, 8, 12] {
        res |= bit(s, base) << (base + 2);
        res |= bit(s, base + 2) << base;
        res |= bit(ns, base + 1) << (base + 1);
        res |= bit(ns, base + 3) << (base + 3);
    }
    res ^= ror16(s, 5);
    let res32 = res as u32;
    let mut acc: u32 = 0;
    for i in 0..acc_iters {
        acc ^= (res32 & (0xF << i)) << ((res32 >> i) & 0xF);
    }
    acc ^= acc >> 16;
    ((res32 ^ acc ^ c as u32) & 0xFFFF) as u16
}

/// Level 0x7F key (`diag_key_calc_7f`, the "afb10" fold) XOR the per-image constant `c` (from the
/// `1A 94` ident appendix). From security-access.md. `iters` is the fold's loop count - the doc's
/// `for i in 0..7` is ambiguous (7 vs 8); 8 covers all 16 bits and is hardware-validated (`unlock_level_7f`).
pub fn key_calc_7f(seed: u16, c: u16, iters: u32) -> u16 {
    let x = ror16(seed, 3);
    let nx = !x;
    let mut res: u16 = 0;
    for i in 0..iters {
        let a = 15 - i;
        let b = 7 - i;
        if bit(x, i) != bit(x, i + 8) {
            res |= bit(x, a) << b;
            res |= bit(x, b) << a;
        } else {
            res |= bit(nx, a) << a;
            res |= bit(nx, b) << b;
        }
    }
    res ^ c
}

/// Decode a `$1A 0x94` (SW-number / `SW_NR`) response into its software-number string and the two
/// public SecurityAccess constants it carries. The block is `<len> <sw-number string> 90 00 0E
/// <appendix>`; `diag_key_calc_7e`/`diag_key_calc_7f` read their constants out of that appendix. We
/// anchor on the `90 00 0E` marker (not a fixed block offset) so it survives a different SW-string
/// length: relative to the marker start `M`, the `0x7E` constant is bytes `M+4,M+6` and the `0x7F`
/// constant is `M+7,M+8` (matches the documented block offsets 0x11/0x13 and 0x14/0x15). Accepts the
/// raw KWP reply with or without the leading `5A 94` header. Returns `(sw_number, const_7e, const_7f)`,
/// or `None` if the marker/appendix isn't present. The constants are compiled into the flash image
/// (per-firmware, same across units on the same software), not per-unit secrets.
pub fn decode_1a94(resp: &[u8]) -> Option<(String, u16, u16)> {
    // Strip a leading 5A 94 positive-response header if the whole reply was pasted in.
    let block: &[u8] = match resp {
        [0x5A, 0x94, rest @ ..] => rest,
        _ => resp,
    };
    // Find the 90 00 0E appendix marker.
    let m = block.windows(3).position(|w| w == [0x90, 0x00, 0x0E])?;
    let hi7e = *block.get(m + 4)?;
    let lo7e = *block.get(m + 6)?;
    let hi7f = *block.get(m + 7)?;
    let lo7f = *block.get(m + 8)?;
    // SW-number string: length byte at block[0], then that many ASCII bytes (best-effort).
    let sw = match block.first() {
        Some(&len) if (len as usize) < m && block.len() > len as usize => {
            String::from_utf8_lossy(&block[1..=len as usize]).trim_end().to_string()
        }
        _ => String::new(),
    };
    Some((sw, u16::from_be_bytes([hi7e, lo7e]), u16::from_be_bytes([hi7f, lo7f])))
}

// ---- request builders ----
/// Diagnostic session for the CCP-enable chain. SecurityAccess (and thus the `31 31 01`
/// CCP-mode routine) is gated behind a non-default session by the KWP dispatcher's
/// per-service permission table - a cold `27 07` returns `7F 27 22` (conditionsNotCorrect).
///
/// 0x87 is the session the MK60 accepts and in which `0x27` unlocks. NOT 0x85/ECUProgrammingMode:
/// the 0x10 handler routes 0x85 to a path that requires security **level 5** (RAM 0x409545 == 5),
/// and no `0x27` sub-function grants above ~level 4 - so 0x85 is unreachable and returns
/// `7F 10 12`/`7F 10 22`. (From bench notes on the real E46 module.)
pub const SESSION_CCP: u8 = 0x87;
pub const SESSION_PROGRAMMING: u8 = 0x85;

/// StartDiagnosticSession (service 0x10): enter diagnostic `session`. Positive reply = 0x50.
pub fn start_session(session: u8) -> Vec<u8> { req(&[0x10, session]) }
pub fn request_seed_l4() -> Vec<u8> { req(&[0x27, 0x07]) }
pub fn send_key_l4(key: u16) -> Vec<u8> {
    req(&[0x27, 0x08, (key >> 8) as u8, (key & 0xFF) as u8])
}
/// SecurityAccess requestSeed for an arbitrary (odd) sub-function. Reply `67 <subfn> <seedHi> <seedLo>`.
pub fn request_seed(subfn: u8) -> Vec<u8> { req(&[0x27, subfn]) }
/// SecurityAccess sendKey for an arbitrary (even) sub-function.
pub fn send_key(subfn: u8, key: u16) -> Vec<u8> {
    req(&[0x27, subfn, (key >> 8) as u8, (key & 0xFF) as u8])
}
/// The seed->key transform family (FUN_000b1468): `key = bitrev16(seed) ^ ror16(seed, subfn+1)`,
/// generalised over the sendKey sub-function. subfn 0x04 -> ror5 (level 2), 0x08 -> ror9 (level 4).
/// Higher levels, if the E46 has them, plausibly follow the same rule - worth trying first.
pub fn key_level(seed: u16, send_subfn: u8) -> u16 {
    bitrev16(seed) ^ ror16(seed, send_subfn as u32 + 1)
}
/// `31 23 01` activates the end-of-line / band test (`Bandendetest`) on the E46 MK60 - it sets the
/// test MODE (lights the ABS/DSC lamp, logs fault 5E00), it is NOT a CCP enable. `31 23 00` deactivates.
pub fn band_test(enable: bool) -> Vec<u8> { req(&[0x31, 0x23, enable as u8]) }
/// ClearDiagnosticInformation (service 0x14), all groups (`14 FF 00`). Positive reply = 0x54.
pub fn clear_dtc() -> Vec<u8> { req(&[0x14, 0xFF, 0x00]) }
/// ReadDiagnosticTroubleCodesByStatus (service 0x18): `18 00 FF 00` = all DTCs, any status.
/// Positive reply = 0x58 <count> <DTC records...>.
pub fn read_dtc() -> Vec<u8> { req(&[0x18, 0x00, 0xFF, 0x00]) }
/// ReadDataByLocalIdentifier (service 0x21): read the data record stored under `lid`.
/// Read-only - no SecurityAccess needed. On the MK60, LID 0x06 = brake pressure.
pub fn read_by_lid(lid: u8) -> Vec<u8> { req(&[0x21, lid]) }

/// ReadDataByCommonIdentifier (service 0x22): `22 <didHi> <didLo>`. Read-only; no SecurityAccess.
/// Positive reply = 0x62 <didHi> <didLo> <data...>. On the E5, DID 0x3000 = coding.
pub fn read_by_common_id(did: u16) -> Vec<u8> {
    req(&[0x22, (did >> 8) as u8, (did & 0xFF) as u8])
}

/// WriteDataByCommonIdentifier (service 0x2E): `2E <didHi> <didLo> <data...>`. Positive reply = 0x6E.
/// On the MK60 the write side accepts only the coding DIDs (0x1000/0x1001/0x3000/0x3001/0x3FFF); any
/// other DID is rejected 7F 2E 12. This writes PERSISTENT config - supply exactly the bytes intended.
pub fn write_by_common_id(did: u16, data: &[u8]) -> Vec<u8> {
    let mut v = vec![0x2E, (did >> 8) as u8, (did & 0xFF) as u8];
    v.extend_from_slice(data);
    req(&v)
}
/// ReadECUIdentification (service 0x1A): read the identification record `id`. Read-only.
/// The per-unit level-5 auth constants live in one of these records. Positive reply = 0x5A.
pub fn read_ecu_ident(id: u8) -> Vec<u8> { req(&[0x1A, id]) }

/// NVM-walk service (`$09`): `09 <sub>`. The raw EEPROM job engine, gated at SecurityAccess level 4.
/// Subs 0x7F/0x8F toggle the walk flag; 0x01/0x02 query. Positive reply = 0x49. (0x9F is the E1/Z4
/// ECU-reset sub - never send it as a read probe.)
pub fn nvm_walk(sub: u8) -> Vec<u8> { req(&[0x09, sub]) }

/// ReadMemoryByAddress (service 0x23), BMW logical-NVM form: `23 00 <addrHi> <addrLo> 03 <len>`
/// (format 3 = logical EEPROM, len <= 4). Reads the EEPROM where coding + the CCP-enable bit live.
/// Positive reply = 0x63 <data...>.
pub fn read_memory(addr: u16, len: u8) -> Vec<u8> {
    req(&[0x23, 0x00, (addr >> 8) as u8, (addr & 0xFF) as u8, 0x03, len])
}

/// TesterPresent (service 0x3E), to keep a diagnostic session alive during a long sweep.
pub fn tester_present() -> Vec<u8> { req(&[0x3E, 0x01]) }

/// WriteMemoryByAddress (service 0x3D), one logical-NVM byte: `3D 00 <addrHi> <addrLo> 03 01 <data>`
/// - mirrors the format-3 `read_memory`. Needs SecurityAccess level 0x7F. Positive reply = 0x7D.
pub fn write_memory(addr: u16, data: u8) -> Vec<u8> {
    req(&[0x3D, 0x00, (addr >> 8) as u8, (addr & 0xFF) as u8, 0x03, 0x01, data])
}

// ---- level-5 Authentisierung (RoutineControl 0x31 routines 07/08) ----
/// 31 07 03 <uid>: RequestForAuthentication. Reply `71 07 <8-byte random>`.
pub fn auth_request_random(uid: &[u8]) -> Vec<u8> {
    let mut d = vec![0x31, 0x07, 0x03];
    d.extend_from_slice(uid);
    req(&d)
}
/// 31 08 <resp16>: ReleaseAuthentication. Reply `71 08 00` on success (hardware-verified).
pub fn auth_send_response(resp: &[u8]) -> Vec<u8> {
    let mut d = vec![0x31, 0x08];
    d.extend_from_slice(resp);
    req(&d)
}

/// Level-5 auth response = MD5(key || uid || w564 || zufall || key) (standard MD5, 16-byte output).
pub fn auth_response(key: &[u8], uid: &[u8], w564: &[u8], zufall: &[u8]) -> [u8; 16] {
    use md5::{Digest, Md5};
    let mut h = Md5::new();
    h.update(key);
    h.update(uid);
    h.update(w564);
    h.update(zufall);
    h.update(key);
    h.finalize().into()
}

/// The E5 level-5 auth constants (shared across M3/1M/335i; flash 0xF56C8). Per-variant on others.
pub const AUTH_KEY_E5: [u8; 16] = [
    0xF1, 0xCE, 0x23, 0xC6, 0xCD, 0x24, 0xC8, 0xB7, 0xB8, 0xB3, 0xC6, 0x53, 0xD9, 0x70, 0x7B, 0xAC,
];
pub const AUTH_UID: [u8; 4] = [0, 0, 0, 0]; // tester-chosen; matches the hardware-verified example
pub const AUTH_W564: [u8; 4] = [0xD1, 0xBB, 0xCD, 0x33]; // PER-UNIT (0x409564) - this is the verified
                                                         // example's value; real units differ, must be read

/// Find a BMW-FAST *response* frame (B8 F1 29 <len> <data...> <cksum>) in a K-line byte stream
/// (which also contains the echo of our own request). Returns the response data bytes.
pub fn parse_response(buf: &[u8]) -> Option<Vec<u8>> {
    let mut i = 0;
    while i + 5 <= buf.len() {
        if buf[i] == 0xB8 && buf[i + 1] == TESTER && buf[i + 2] == DSC_ADDR {
            let len = buf[i + 3] as usize;
            let total = 4 + len + 1;
            if i + total <= buf.len() {
                let frame = &buf[i..i + total];
                if checksum(&frame[..frame.len() - 1]) == *frame.last().unwrap() {
                    return Some(frame[4..4 + len].to_vec());
                }
            }
        }
        i += 1;
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn parse_resp() {
        // echo of our request, then response: B8 F1 29 03 67 07 12 34 <sum>  (seed 0x1234)
        let resp = bmw_fast(TESTER, DSC_ADDR, &[0x67, 0x07, 0x12, 0x34]);
        let mut stream = tester_present(); // some echo noise
        stream.extend_from_slice(&resp);
        let data = parse_response(&stream).unwrap();
        assert_eq!(data, vec![0x67, 0x07, 0x12, 0x34]);
    }
    #[test]
    fn key_7f_vector() {
        // Hardware-validated on the non-M E46 MK60 (MK60E1-class): 1A 94 constant C=0x0F30,
        // seed 0x3B61 -> key 0x4733 accepted (0x67). Pins the afb10 fold at 8 iterations, C direct.
        assert_eq!(key_calc_7f(0x3B61, 0x0F30, 8), 0x4733);
    }
    #[test]
    fn decode_1a94_vector() {
        // Documented ident block (security-access.md "Ident block at 0x8354"):
        // 0C "AV_REL00270 " 90 00 0E 83 46 B8 14 90 DC ... -> 0x7F constant 0x90DC.
        let mut raw = vec![0x5A, 0x94, 0x0C];
        raw.extend_from_slice(b"AV_REL00270 ");
        raw.extend_from_slice(&[0x90, 0x00, 0x0E, 0x83, 0x46, 0xB8, 0x14, 0x90, 0xDC]);
        raw.extend_from_slice(&[0x00, 0x03, 0x00, 0x00, 0x00, 0x82, 0x01, 0x01]);
        let (sw, c7e, c7f) = decode_1a94(&raw).unwrap();
        assert_eq!(sw, "AV_REL00270");
        assert_eq!(c7e, 0x4614);
        assert_eq!(c7f, 0x90DC);
        // Header-less (block pasted without 5A 94) decodes the same.
        assert_eq!(decode_1a94(&raw[2..]).unwrap().2, 0x90DC);
    }
    #[test]
    fn key_vectors() {
        // from the disassembly + reference impl
        assert_eq!(key_level4(0x1234), 0x3641);
        assert_eq!(key_level4(0xABCD), 0x5500);
        assert_eq!(key_level4(0x0001), 0x8080);
        assert_eq!(key_level4(0x8000), 0x0041);
    }
    #[test]
    fn checksum_xor() {
        // XOR, from the Gauge.S MK60 config / bench reply: B8 29 F1 02 21 06 -> 0x45
        assert_eq!(*bmw_fast(0x29, 0xF1, &[0x21, 0x06]).last().unwrap(), 0x45);
    }
    #[test]
    fn auth_vector() {
        // FACTS.md hardware-verified live example: uid=00000000, w564=D1BBCD33,
        // zufall=1d76ead8f2276b03 -> resp=9c1a3704d2686c63625a69e8e76ee175.
        let resp = auth_response(
            &AUTH_KEY_E5,
            &[0x00, 0x00, 0x00, 0x00],
            &[0xD1, 0xBB, 0xCD, 0x33],
            &[0x1d, 0x76, 0xea, 0xd8, 0xf2, 0x27, 0x6b, 0x03],
        );
        assert_eq!(
            resp,
            [
                0x9c, 0x1a, 0x37, 0x04, 0xd2, 0x68, 0x6c, 0x63, 0x62, 0x5a, 0x69, 0xe8, 0xe7, 0x6e,
                0xe1, 0x75
            ]
        );
    }
}
