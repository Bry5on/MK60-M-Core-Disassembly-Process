//! Serial bridge framing - must stay in sync with firmware/src/protocol.h (bench/protocol.md).
//! Frame: SYNC(0x55) | CMD | LEN(u16 LE) | PAYLOAD | CRC16(LE, CCITT-FALSE over CMD+LEN+PAYLOAD)

pub const SYNC: u8 = 0x55;

// host -> dongle
pub const CMD_PING: u8 = 0x01;
pub const CMD_CAN_OPEN: u8 = 0x10;
pub const CMD_CAN_CLOSE: u8 = 0x11;
#[allow(dead_code)] // protocol-complete: fw accepts-all, host filters in software
pub const CMD_CAN_FILTER: u8 = 0x12;
pub const CMD_CAN_TX: u8 = 0x13;
pub const CMD_CAN_RESET: u8 = 0x14; // on-demand TWAI re-init
pub const CMD_KLINE_OPEN: u8 = 0x20; // [baud u32][fmt u8 optional: 0=8E1 default, 1=8N1, 2=8O1]
pub const CMD_KLINE_CLOSE: u8 = 0x21;
pub const CMD_KLINE_TX: u8 = 0x22;
#[allow(dead_code)] // reserved: K-line inter-byte / P-timing config (fw stub)
pub const CMD_KLINE_CFG: u8 = 0x23;
pub const CMD_KLINE_RESET: u8 = 0x24; // hard re-init of the K-line UART (clears an RX desync)
pub const CMD_KLINE_FASTINIT: u8 = 0x25; // ISO-14230 fast-init wake-up on K-line
pub const CMD_KLINE_KNOCK: u8 = 0x26; // [sel u8][gap_ms u8 opt=18][sel_reps u8 opt=1] do the mfg knock
                                      // @ 01 02 <sel> with precise on-device timing (FW >= 0x0015)
pub const CMD_SPI_OPEN: u8 = 0x30; // init bit-bang SPI master (25LC040), payload [half_us u8 optional]
pub const CMD_SPI_XFER: u8 = 0x31; // one CS-framed SPI transaction, payload = raw bytes to clock out
pub const CMD_RELAY_SET: u8 = 0x40; // [state u8] drive EE MOSFET: 1=relay ON (sink IN), 0=OFF (FW >= 0x0012)
pub const CMD_ADC_READ: u8 = 0x41; // read A5 ignition feedback -> EVT_ADC [mv u16] (FW >= 0x0012)
pub const CMD_12_V_IN_READ: u8 = 0x42; // read the main-connector 12V input (ANALOG_0) -> EVT_12_V_IN [mv u16] (FW >= 0x0013)

// dongle -> host
pub const EVT_INFO: u8 = 0x81;
pub const EVT_CAN_RX: u8 = 0x90;
pub const EVT_CAN_STATE: u8 = 0x91; // [state u8] 0x00=running/recovered, 0x01=bus-off/recovering (FW 0x000D)
pub const EVT_KLINE_RX: u8 = 0xA0;
pub const EVT_SPI_RX: u8 = 0xB0; // MISO bytes from one CMD_SPI_XFER (same count as sent)
pub const EVT_ADC: u8 = 0xC0; // [mv u16] actual LINE millivolts at A5 (divider applied on-device since FW 0x0014)
pub const EVT_12_V_IN: u8 = 0xC1; // [mv u16] actual LINE millivolts at the main 12V input (ANALOG_0, on-device divider since 0x0014)
pub const EVT_RELAY_TRIP: u8 = 0xC2; // [mv u16] firmware auto-dropped the relay: 12V fell to this (line mV) while ON (FW >= 0x0014)
pub const EVT_ERROR: u8 = 0xE0;

pub fn crc16(data: &[u8], mut crc: u16) -> u16 {
    for &b in data {
        crc ^= (b as u16) << 8;
        for _ in 0..8 {
            crc = if crc & 0x8000 != 0 { (crc << 1) ^ 0x1021 } else { crc << 1 };
        }
    }
    crc
}

pub fn encode(cmd: u8, payload: &[u8]) -> Vec<u8> {
    let len = payload.len() as u16;
    let mut v = vec![SYNC, cmd, (len & 0xFF) as u8, (len >> 8) as u8];
    v.extend_from_slice(payload);
    let c = crc16(payload, crc16(&v[1..4], 0xFFFF));
    v.push((c & 0xFF) as u8);
    v.push((c >> 8) as u8);
    v
}

#[derive(Debug, Clone)]
pub struct Frame {
    pub cmd: u8,
    pub payload: Vec<u8>,
}

/// Incremental, resyncing frame decoder.
#[derive(Default)]
pub struct Decoder {
    buf: std::collections::VecDeque<u8>,
}
impl Decoder {
    pub fn new() -> Self { Self::default() }
    pub fn push(&mut self, data: &[u8]) { self.buf.extend(data.iter().copied()); }

    pub fn next_frame(&mut self) -> Option<Frame> {
        loop {
            while let Some(&b) = self.buf.front() {
                if b == SYNC { break; }
                self.buf.pop_front();
            }
            if self.buf.len() < 4 { return None; }
            let b: Vec<u8> = self.buf.iter().take(6).copied().collect();
            let len = (b[2] as usize) | ((b[3] as usize) << 8);
            let total = 4 + len + 2;
            if self.buf.len() < total { return None; }
            let frame: Vec<u8> = self.buf.iter().take(total).copied().collect();
            let want = crc16(&frame[4..4 + len], crc16(&frame[1..4], 0xFFFF));
            let got = (frame[4 + len] as u16) | ((frame[5 + len] as u16) << 8);
            if want == got {
                self.buf.drain(0..total);
                return Some(Frame { cmd: frame[1], payload: frame[4..4 + len].to_vec() });
            }
            self.buf.pop_front(); // bad CRC: drop the SYNC and resync
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn roundtrip() {
        let enc = encode(CMD_CAN_TX, &[0x97, 0x07, 8, 1, 2, 3, 4, 5, 6, 7, 8]);
        let mut d = Decoder::new();
        d.push(&enc);
        let f = d.next_frame().unwrap();
        assert_eq!(f.cmd, CMD_CAN_TX);
        assert_eq!(f.payload[0], 0x97);
        assert!(d.next_frame().is_none());
    }
    #[test]
    fn resync_after_noise() {
        let mut d = Decoder::new();
        d.push(&[0x00, 0xFF, 0x55]); // junk then a stray SYNC
        d.push(&encode(CMD_PING, &[]));
        // the stray 0x55 fails CRC and is dropped; the real frame survives
        let f = d.next_frame().unwrap();
        assert_eq!(f.cmd, CMD_PING);
    }
}
