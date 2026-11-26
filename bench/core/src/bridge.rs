//! Serial link to the Gauge.S bridge: port open/auto-detect + framed send/receive.
//! This is the ONLY OS-specific surface; everything above it is pure bytes.

use crate::proto::{self, Decoder, Frame};
use anyhow::{anyhow, Result};
use serialport::{ClearBuffer, DataBits, Parity, SerialPort, StopBits}; // SerialPort's supertraits io::Read/io::Write provide read()/write_all()
use std::time::{Duration, Instant};

/// Which physical link is driving the K-line.
///
/// - `GaugeS`: the Gauge.S ESP32 dongle, spoken over a framed USB-CDC protocol (CMD_*/EVT_*). This is the
///   full-featured path: relay-switched ignition, A5 ADC feedback, CAN, SPI.
/// - `KDcan`: a generic "K+DCAN" FTDI cable, where the serial port IS the K-line - we configure the UART
///   baud/parity directly and write/read raw bytes. No relay, no ADC, no CAN/SPI. Some cables wire OBD pin 1
///   to a modem status line, so we can sense ignition-on over USB; most don't, so entry falls back to
///   blasting the knock while the operator cycles the key.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LinkKind {
    GaugeS,
    KDcan,
}

impl LinkKind {
    pub fn is_cable(self) -> bool {
        matches!(self, LinkKind::KDcan)
    }
    pub fn label(self) -> &'static str {
        match self {
            LinkKind::GaugeS => "Gauge.S",
            LinkKind::KDcan => "K+DCAN cable",
        }
    }
}

/// The four RS-232 modem status inputs an FTDI exposes; a K+DCAN cable that wires OBD pin 1 routes the
/// ignition sense to one of these (which one varies by cable, so we read all four and let the caller pick).
#[derive(Debug, Clone, Copy, Default)]
pub struct ModemLines {
    pub cts: bool,
    pub dsr: bool,
    pub cd: bool,
    pub ri: bool,
}

impl ModemLines {
    /// Ignition (KL15) sense for an FTDI K+DCAN cable. On these cables **DSR and DCD track the key together**
    /// (both high = ignition on), **RI is stuck high** even unplugged, and **CTS is dead-low** - so an
    /// `any()`-style OR reads a permanent false "on" off the stuck RI. Empirically confirmed on a Bimmergeeks
    /// Pro (EdiabasLib firmware): cycling KL15 flips DSR+CD 1<->0 while CTS stays 0 and RI stays 1. Use
    /// `DSR || CD` (either, for cables that wire only one) and ignore RI/CTS.
    pub fn ignition(&self) -> bool {
        self.dsr || self.cd
    }
}

pub struct Bridge {
    port: Option<Box<dyn SerialPort>>,
    path: String,
    baud: u32,
    kind: LinkKind,
    dec: Decoder,
    trace: bool,
    /// Last CAN bus state the dongle reported via EVT_CAN_STATE (FW >= 0x000D): Some(0)=running,
    /// Some(1)=bus-off/recovering, None=never reported. Updated inside `poll()`.
    can_state: Option<u8>,
    /// An unconsumed EVT_CAN_STATE transition, for a CAN loop to log once. Drained by `take_can_state_event`.
    can_state_evt: Option<u8>,
    /// An unconsumed EVT_RELAY_TRIP (FW >= 0x0014): the firmware auto-dropped the relay because the 12 V
    /// supply fell away; carries the line millivolts it saw. Drained by `take_relay_trip`.
    relay_trip: Option<u16>,
    /// The dongle firmware version, learned from the last `ping()`. Gates features like the on-device knock
    /// (CMD_KLINE_KNOCK, FW >= 0x0015). `None` until the first successful ping.
    fw: Option<u16>,
}

/// Dongle identity from an EVT_INFO reply.
#[derive(Debug, Clone, Copy)]
pub struct Info {
    pub fw: u16,
    pub board: u8,
    /// `esp_reset_reason()` from the dongle (FW >= 0x0007; 0 on older firmware). See [`reset_reason_str`].
    pub reset_reason: u8,
}

/// Decode the ESP-IDF `esp_reset_reason_t` value the dongle reports in EVT_INFO.
pub fn reset_reason_str(r: u8) -> &'static str {
    match r {
        0 => "unknown",
        1 => "power-on",
        2 => "external",
        3 => "software",
        4 => "panic",
        5 => "interrupt-watchdog",
        6 => "task-watchdog",
        7 => "other-watchdog",
        8 => "deep-sleep",
        9 => "brownout",
        10 => "sdio",
        _ => "?",
    }
}

/// Space-separated hex, for the --raw wire trace.
fn hexs(b: &[u8]) -> String {
    b.iter().map(|x| format!("{x:02X}")).collect::<Vec<_>>().join(" ")
}

/// Open a serial port, retrying briefly when the OS reports it busy. On Windows a COM port opens
/// EXCLUSIVELY, so when the short-lived status poll (or any other holder) has it open, a concurrent
/// open fails with "Access is denied" (ERROR_ACCESS_DENIED) - it frees within a few hundred ms, so we
/// retry a handful of times. On macOS/Linux the port opens shared and this succeeds on the first try.
/// Non-busy errors (no device, bad path) return immediately - retrying those is pointless.
fn open_serial(path: &str, baud: u32, timeout: Duration) -> Result<Box<dyn SerialPort>> {
    const TRIES: u32 = 8;
    const GAP: Duration = Duration::from_millis(60);
    let mut last = String::new();
    for i in 0..TRIES {
        match serialport::new(path, baud).timeout(timeout).open() {
            Ok(p) => return Ok(p),
            Err(e) => {
                let msg = e.to_string();
                let busy = matches!(e.kind(), serialport::ErrorKind::Io(k) if k == std::io::ErrorKind::PermissionDenied)
                    || msg.to_lowercase().contains("denied")
                    || msg.to_lowercase().contains("busy");
                if !busy {
                    return Err(anyhow!("open {path} failed: {msg}"));
                }
                last = msg;
                if i + 1 < TRIES {
                    std::thread::sleep(GAP);
                }
            }
        }
    }
    Err(anyhow!(
        "open {path} failed: {last} - the port is held by another program (a second instance of this \
         tool, a serial terminal/monitor, or Device Manager). Close the other user of {path} and retry."
    ))
}

/// Assert DTR on a freshly-opened Gauge.S port. The ESP32-S3 (V5) talks over NATIVE USB-CDC with a
/// non-blocking write (`Serial.setTxTimeoutMs(0)`), so its TX is DROPPED whenever the CDC is not
/// "connected" - and the Arduino USB-CDC stack ties "connected" to the host's DTR line. macOS asserts
/// DTR on open, so it just works there; Windows opens with DTR deasserted, so the dongle's replies
/// vanish and every command reads back as "no reply". Asserting DTR (and deasserting RTS to a defined,
/// non-reset state) makes the dongle transmit. Best-effort: ignore drivers that don't expose the lines,
/// and it's a no-op for the K+DCAN cable (an FTDI, whose TX never gated on DTR). Harmless on the classic
/// FTDI Gauge.S too - a steady DTR level doesn't trip the auto-reset circuit.
fn assert_cdc_ready(port: &mut dyn SerialPort, kind: LinkKind) {
    if matches!(kind, LinkKind::GaugeS) {
        let _ = port.write_data_terminal_ready(true);
        let _ = port.write_request_to_send(false);
    }
}

impl Bridge {
    /// Open the Gauge.S dongle (framed USB-CDC link). Back-compat entry point; equivalent to
    /// `open_kind(path, baud, LinkKind::GaugeS)`.
    pub fn open(path: Option<&str>, baud: u32) -> Result<Self> {
        Self::open_kind(path, baud, LinkKind::GaugeS)
    }

    /// Open a link of the given kind. For `GaugeS` the serial `baud` is the USB-CDC framing rate (921600).
    /// For `KDcan` the serial baud IS the K-line baud, so we ignore the passed value and open at 10400 (a
    /// safe default that `kline_open*` overrides); the cable has no auto-detect by USB id yet, so pass an
    /// explicit `path`.
    pub fn open_kind(path: Option<&str>, baud: u32, kind: LinkKind) -> Result<Self> {
        let path = match path {
            Some(p) => p.to_string(),
            None => auto_detect_kind(kind)?,
        };
        let open_baud = match kind {
            LinkKind::GaugeS => baud,
            LinkKind::KDcan => 10400, // placeholder; kline_open_fmt sets the real K-line baud
        };
        let mut port = open_serial(&path, open_baud, Duration::from_millis(20))?;
        assert_cdc_ready(port.as_mut(), kind);
        Ok(Self {
            port: Some(port),
            path,
            baud: open_baud,
            kind,
            dec: Decoder::new(),
            trace: false,
            can_state: None,
            can_state_evt: None,
            relay_trip: None,
            fw: None,
        })
    }

    /// Which physical link this is. Callers gate relay/CAN/SPI features and pick the mfg entry strategy on it.
    pub fn kind(&self) -> LinkKind {
        self.kind
    }

    /// The serial path this bridge was opened on (e.g. `COM3` on Windows, `/dev/cu.usbserial-*` on macOS).
    pub fn path(&self) -> &str {
        &self.path
    }

    /// Error for a feature that only the Gauge.S dongle provides (relay, A5 ADC, CAN, SPI). Keeps the
    /// message uniform when a K+DCAN cable is selected.
    fn require_gauges(&self, what: &str) -> Result<()> {
        if self.kind.is_cable() {
            return Err(anyhow!("{what} needs the Gauge.S dongle - not available over a K+DCAN cable."));
        }
        Ok(())
    }

    /// Enable hex-dumping of every frame in/out to stderr (the --raw flag).
    pub fn set_trace(&mut self, on: bool) {
        self.trace = on;
    }

    /// Recover the link, surviving a full dongle reboot. The firmware Task-Watchdog reboots the ESP32
    /// on a hang, which makes the USB-CDC port DISAPPEAR and re-enumerate a few seconds later (possibly
    /// under a different /dev name). Drop the handle, then POLL for up to ~10 s: prefer the original
    /// path, but if it has vanished, re-detect the dongle by USB id. Returns once a port re-opens;
    /// errors only if nothing comes back within the window (then the caller asks the user to replug).
    pub fn reconnect(&mut self) -> Result<()> {
        self.port = None; // drop the old handle so the OS frees the device
        std::thread::sleep(Duration::from_millis(500)); // let it drop off the bus after a reboot
        let deadline = Instant::now() + Duration::from_secs(10);
        loop {
            // Prefer the original path; if it's gone (re-enumerated), re-detect by USB id. auto_detect
            // may transiently fail (device not back yet, or briefly ambiguous) - fine, we retry.
            let candidate = if path_present(&self.path) {
                Some(self.path.clone())
            } else {
                auto_detect().ok()
            };
            if let Some(path) = candidate {
                if let Ok(mut port) = serialport::new(&path, self.baud).timeout(Duration::from_millis(20)).open() {
                    assert_cdc_ready(port.as_mut(), self.kind); // Windows: re-assert DTR so the dongle talks
                    self.port = Some(port);
                    self.path = path; // remember the (possibly new) name for next time
                    self.dec = Decoder::new(); // discard any half-decoded frame from before the reboot
                    return Ok(());
                }
            }
            if Instant::now() >= deadline {
                return Err(anyhow!("Gauge.S did not re-enumerate within 10 s - replug the USB-C and retry."));
            }
            std::thread::sleep(Duration::from_millis(300));
        }
    }

    /// Ping the dongle and read back its firmware/board identity (EVT_INFO).
    /// `Ok(None)` means no reply within the timeout.
    pub fn ping(&mut self) -> Result<Option<Info>> {
        if self.kind.is_cable() {
            // A dumb cable has no handshake to answer. Probe for a Gauge.S anyway: send the framed ping and
            // decode the RAW reply stream (poll() would wrap it as K-line RX). If a dongle answers, the user
            // picked the wrong link - return its real identity so the UI can say so instead of a false OK.
            // A USB-CDC dongle ignores the nominal baud, so this works even though we opened at the K-line
            // rate; a real FTDI cable with nothing listening simply never replies.
            let frame = proto::encode(proto::CMD_PING, &[]);
            if let Some(p) = self.port.as_deref_mut() {
                let _ = p.write_all(&frame);
                let _ = p.flush();
            }
            let mut dec = Decoder::new();
            let deadline = Instant::now() + Duration::from_millis(400);
            while Instant::now() < deadline {
                let mut b = [0u8; 256];
                match self.port.as_deref_mut().map(|p| p.read(&mut b)) {
                    Some(Ok(n)) if n > 0 => {
                        dec.push(&b[..n]);
                        while let Some(f) = dec.next_frame() {
                            if f.cmd == proto::EVT_INFO && f.payload.len() >= 3 {
                                return Ok(Some(Info {
                                    fw: ((f.payload[1] as u16) << 8) | f.payload[0] as u16,
                                    board: f.payload[2],
                                    reset_reason: f.payload.get(4).copied().unwrap_or(0),
                                }));
                            }
                        }
                    }
                    _ => {}
                }
            }
            // Nothing answered the dongle handshake - a real cable (or an empty port). Synthetic identity
            // (board 0xCA) tells the UI "cable, no dongle here".
            return Ok(Some(Info { fw: 0, board: 0xCA, reset_reason: 0 }));
        }
        self.send(proto::CMD_PING, &[])?;
        match self.wait_for(proto::EVT_INFO, Duration::from_millis(500))? {
            Some(f) if f.payload.len() >= 3 => {
                let fw = ((f.payload[1] as u16) << 8) | f.payload[0] as u16;
                self.fw = Some(fw); // cache for feature gating (e.g. the on-device knock)
                Ok(Some(Info {
                    fw,
                    board: f.payload[2],
                    reset_reason: f.payload.get(4).copied().unwrap_or(0), // FW >= 0x0007
                }))
            }
            _ => Ok(None),
        }
    }

    /// The dongle firmware version from the last `ping()`, or `None` if we haven't pinged a dongle yet.
    pub fn fw(&self) -> Option<u16> {
        self.fw
    }

    /// Run the mfg knock `@ 01 02 <sel>` entirely on the dongle (CMD_KLINE_KNOCK, FW >= 0x0015), paced
    /// `gap_ms` per byte with the selection repeated `sel_reps` times. The paced TX happens on the ESP32 with
    /// no USB round-trip in the loop, so the selection lands in the module's read slot deterministically on
    /// any host. The echo + the module's ack/accept come back through `poll()` as K-line RX, exactly like a
    /// streamed knock - so the caller captures the window the same way. Gauge.S only.
    pub fn kline_knock(&mut self, sel: u8, gap_ms: u8, sel_reps: u8) -> Result<()> {
        self.require_gauges("the on-device knock")?;
        self.send(proto::CMD_KLINE_KNOCK, &[sel, gap_ms, sel_reps])
    }

    /// Hard-reset the dongle's K-line UART (CMD_KLINE_RESET) - clears an RX desync (leading-byte
    /// mis-sampling) that a plain re-open can't. Fire-and-forget; the dongle re-inits the UART in place.
    pub fn kline_reset(&mut self) -> Result<()> {
        if self.kind.is_cable() {
            // No UART state machine to re-init on the dongle; flush both serial buffers to clear a desync.
            if let Some(p) = self.port.as_deref_mut() {
                let _ = p.clear(ClearBuffer::All);
            }
            return Ok(());
        }
        self.send(proto::CMD_KLINE_RESET, &[])
    }

    /// Generate an ISO-14230 fast-init wake-up on the K-line (dongle drives LOW 25 ms / HIGH 25 ms, then
    /// hands the pin back to the UART). Fire-and-forget; follow immediately with a StartCommunication TX.
    pub fn kline_fastinit(&mut self) -> Result<()> {
        self.require_gauges("K-line fast-init")?;
        self.send(proto::CMD_KLINE_FASTINIT, &[])
    }

    pub fn send(&mut self, cmd: u8, payload: &[u8]) -> Result<()> {
        if self.trace {
            eprintln!("TX  cmd=0x{cmd:02X} [{}]", hexs(payload));
        }
        self.port
            .as_deref_mut()
            .ok_or_else(|| anyhow!("serial port is closed"))?
            .write_all(&proto::encode(cmd, payload))?;
        Ok(())
    }

    /// Return the next frame if one is ready, pulling from the port as needed.
    /// Firmware-side EVT_ERROR frames are reported to stderr and skipped, never returned.
    pub fn poll(&mut self) -> Result<Option<Frame>> {
        // K+DCAN cable: the serial port carries RAW K-line bytes (no CMD/EVT framing). Read whatever is
        // available and hand it back AS IF it were an EVT_KLINE_RX frame, so kline_capture_raw and every
        // other RX consumer above works byte-for-byte identically to the dongle path. CAN/ADC/SPI events
        // never arrive on a cable (those commands are refused up front), so K-line RX is all poll sees here.
        if self.kind.is_cable() {
            let mut b = [0u8; 1024];
            let n = match self.port.as_deref_mut() {
                Some(p) => p.read(&mut b),
                None => return Err(anyhow!("serial port is closed")),
            };
            return match n {
                Ok(n) if n > 0 => {
                    if self.trace {
                        eprintln!("RX  kline [{}]", hexs(&b[..n]));
                    }
                    Ok(Some(Frame { cmd: proto::EVT_KLINE_RX, payload: b[..n].to_vec() }))
                }
                Ok(_) => Ok(None),
                Err(e) if e.kind() == std::io::ErrorKind::TimedOut => Ok(None),
                Err(e) => Err(e.into()),
            };
        }
        loop {
            if let Some(f) = self.dec.next_frame() {
                if self.trace {
                    eprintln!("RX  cmd=0x{:02X} [{}]", f.cmd, hexs(&f.payload));
                }
                if f.cmd == proto::EVT_ERROR {
                    eprintln!(
                        "[dongle error] code 0x{:02X} detail {:02X?}",
                        f.payload.first().copied().unwrap_or(0),
                        f.payload.get(1..).unwrap_or(&[])
                    );
                    continue;
                }
                if f.cmd == proto::EVT_CAN_STATE {
                    // Bus-off auto-recovery state (FW >= 0x000D). Record it and skip - no frame-matching
                    // loop needs to see it; a CAN loop can log the transition via take_can_state_event().
                    let s = f.payload.first().copied().unwrap_or(0);
                    self.can_state = Some(s);
                    self.can_state_evt = Some(s);
                    if self.trace {
                        eprintln!("[dongle] CAN {}", if s == 0 { "bus recovered" } else { "bus-off, recovering" });
                    }
                    continue;
                }
                if f.cmd == proto::EVT_RELAY_TRIP {
                    // Firmware auto-dropped the relay (12 V lost while ON), FW >= 0x0014. Record for a UI to
                    // surface via take_relay_trip(); it isn't a reply any wait loop matches on.
                    let mv = if f.payload.len() >= 2 { u16::from_le_bytes([f.payload[0], f.payload[1]]) } else { 0 };
                    self.relay_trip = Some(mv);
                    if self.trace {
                        eprintln!("[dongle] relay auto-tripped at {mv} mV (12V lost)");
                    }
                    continue;
                }
                return Ok(Some(f));
            }
            let mut b = [0u8; 1024];
            let n = match self.port.as_deref_mut() {
                Some(p) => p.read(&mut b),
                None => return Err(anyhow!("serial port is closed")),
            };
            match n {
                Ok(n) if n > 0 => self.dec.push(&b[..n]),
                Ok(_) => return Ok(None),
                Err(e) if e.kind() == std::io::ErrorKind::TimedOut => return Ok(None),
                Err(e) => return Err(e.into()),
            }
        }
    }

    /// Block until a frame of `cmd` arrives or `timeout` elapses.
    pub fn wait_for(&mut self, cmd: u8, timeout: Duration) -> Result<Option<Frame>> {
        let deadline = Instant::now() + timeout;
        while Instant::now() < deadline {
            if let Some(f) = self.poll()? {
                if f.cmd == cmd {
                    return Ok(Some(f));
                }
            }
        }
        Ok(None)
    }

    /// Collect every CAN_RX frame seen within `window`. Used by the probe + sniffer.
    pub fn collect_can(&mut self, window: Duration) -> Result<Vec<(u16, Vec<u8>)>> {
        let deadline = Instant::now() + window;
        let mut out = Vec::new();
        while Instant::now() < deadline {
            if let Some(f) = self.poll()? {
                if f.cmd == proto::EVT_CAN_RX && f.payload.len() >= 3 {
                    let id = (f.payload[0] as u16) | ((f.payload[1] as u16) << 8);
                    let dlc = f.payload[2] as usize;
                    let data = f.payload[3..(3 + dlc).min(f.payload.len())].to_vec();
                    out.push((id, data));
                }
            }
        }
        Ok(out)
    }

    // ---- convenience wrappers ----
    pub fn can_open(&mut self, listen: bool) -> Result<()> {
        self.require_gauges("CAN")?;
        self.send(proto::CMD_CAN_OPEN, &[0 /*500k*/, listen as u8])
    }
    pub fn can_tx(&mut self, id: u16, data: &[u8]) -> Result<()> {
        self.require_gauges("CAN")?;
        let mut p = vec![(id & 0xFF) as u8, (id >> 8) as u8, data.len() as u8];
        p.extend_from_slice(data);
        self.send(proto::CMD_CAN_TX, &p)
    }
    pub fn can_close(&mut self) -> Result<()> {
        if self.kind.is_cable() {
            return Ok(()); // nothing to close; keep cleanup paths quiet
        }
        self.send(proto::CMD_CAN_CLOSE, &[])
    }
    /// On-demand TWAI re-init on the dongle (CMD_CAN_RESET, FW >= 0x000B). Since FW 0x000D the firmware
    /// auto-recovers from bus-off on its own (can_service), so this is now only a manual full re-init for
    /// an unforeseen CAN controller wedge. No-op on older firmware.
    pub fn can_reset(&mut self) -> Result<()> {
        self.require_gauges("CAN")?;
        self.send(proto::CMD_CAN_RESET, &[])
    }
    /// Drain an unconsumed EVT_CAN_STATE transition (FW >= 0x000D) for a CAN loop to log once:
    /// `Some(0)` = bus recovered/running, `Some(1)` = bus-off/recovering, `None` = no new transition.
    pub fn take_can_state_event(&mut self) -> Option<u8> {
        self.can_state_evt.take()
    }
    /// Drain an unconsumed EVT_RELAY_TRIP (FW >= 0x0014): `Some(mv)` = the firmware auto-dropped the relay
    /// because the 12 V supply fell to `mv` line-millivolts while it was on; `None` = no trip seen. Observed
    /// opportunistically (whenever a poll reads the port), so it is best-effort, not guaranteed delivery.
    pub fn take_relay_trip(&mut self) -> Option<u16> {
        self.relay_trip.take()
    }
    /// True if the dongle last reported the CAN bus as off/recovering (FW >= 0x000D).
    pub fn can_bus_off(&self) -> bool {
        self.can_state == Some(1)
    }
    pub fn kline_open(&mut self, baud: u32) -> Result<()> {
        // Default wire format 8E1 (confirmed E46 KWP framing). Payload is baud only, which older firmware
        // (that ignores a 5th byte) also accepts - keeps every existing caller unchanged.
        self.kline_open_fmt(baud, 0)
    }
    /// Open the K-line UART at an explicit wire format: `fmt` 0=8E1, 1=8N1, 2=8O1.
    ///
    /// Gauge.S: CMD_KLINE_OPEN (the 5th fmt byte needs firmware >= 0x0011; older firmware ignores it and
    /// stays 8E1). K+DCAN cable: the serial port IS the K-line, so reconfigure its baud and parity directly.
    pub fn kline_open_fmt(&mut self, baud: u32, fmt: u8) -> Result<()> {
        if self.kind.is_cable() {
            let parity = match fmt {
                1 => Parity::None, // 8N1
                2 => Parity::Odd,  // 8O1
                _ => Parity::Even, // 8E1
            };
            let p = self.port.as_deref_mut().ok_or_else(|| anyhow!("serial port is closed"))?;
            p.set_baud_rate(baud)?;
            p.set_data_bits(DataBits::Eight)?;
            p.set_parity(parity)?;
            p.set_stop_bits(StopBits::One)?;
            let _ = p.clear(ClearBuffer::All); // drop anything buffered from the previous config
            self.baud = baud;
            return Ok(());
        }
        self.send(
            proto::CMD_KLINE_OPEN,
            &[baud as u8, (baud >> 8) as u8, (baud >> 16) as u8, (baud >> 24) as u8, fmt],
        )
    }
    pub fn kline_tx(&mut self, bytes: &[u8]) -> Result<()> {
        if self.kind.is_cable() {
            // Raw write straight onto the K-line; the half-duplex echo comes back through poll() just like
            // the dongle passes it through, so callers that strip the echo keep working.
            if self.trace {
                eprintln!("TX  kline [{}]", hexs(bytes));
            }
            let p = self.port.as_deref_mut().ok_or_else(|| anyhow!("serial port is closed"))?;
            p.write_all(bytes)?;
            p.flush()?;
            return Ok(());
        }
        self.send(proto::CMD_KLINE_TX, bytes)
    }
    pub fn kline_close(&mut self) -> Result<()> {
        if self.kind.is_cable() {
            // Keep the serial port open (re-opening the FTDI per read would be far too slow); a "close" just
            // means the logical K-line session ended. Nothing to do on the wire.
            return Ok(());
        }
        self.send(proto::CMD_KLINE_CLOSE, &[])
    }

    // ---- raw SPI master (25LC040 config-EEPROM) ----
    /// Initialize the dongle's bit-bang SPI master (CMD_SPI_OPEN, FW >= 0x000C). `half_us` is the
    /// microseconds per half clock (0 = firmware default, ~500 kHz). Mode 0, MSB-first, CS idle-high.
    pub fn spi_open(&mut self, half_us: u8) -> Result<()> {
        self.require_gauges("SPI")?;
        self.send(proto::CMD_SPI_OPEN, &[half_us])
    }
    /// Run one full CS-framed SPI transaction: clock `tx` out on MOSI while sampling MISO, and return the
    /// received bytes (same length as `tx`). The caller builds the whole 25LC040 command in `tx`.
    pub fn spi_xfer(&mut self, tx: &[u8]) -> Result<Vec<u8>> {
        self.require_gauges("SPI")?;
        self.send(proto::CMD_SPI_XFER, tx)?;
        match self.wait_for(proto::EVT_SPI_RX, Duration::from_millis(1000))? {
            Some(f) if f.payload.len() == tx.len() => Ok(f.payload),
            Some(f) => Err(anyhow!(
                "SPI xfer returned {} bytes, expected {} (dongle/firmware mismatch?)",
                f.payload.len(),
                tx.len()
            )),
            None => Err(anyhow!("no EVT_SPI_RX reply - is the SPI firmware (FW >= 0x000C) loaded?")),
        }
    }

    // ---- relay driver (EE MOSFET) + ignition feedback (A5 ADC), FW >= 0x0012 ----
    /// Drive the EE relay MOSFET: `on` = relay ON (the MOSFET sinks the relay IN low). No reply.
    pub fn relay_set(&mut self, on: bool) -> Result<()> {
        self.require_gauges("the ignition relay")?;
        self.send(proto::CMD_RELAY_SET, &[on as u8])
    }
    /// Read the A5 ignition-feedback pin. Returns the ACTUAL line millivolts (the firmware applies the board
    /// 10k/2.2k divider since FW 0x0014), i.e. the switched-12 V level - no host-side conversion needed.
    pub fn adc_read(&mut self) -> Result<u16> {
        self.require_gauges("the A5 ADC")?;
        self.send(proto::CMD_ADC_READ, &[])?;
        match self.wait_for(proto::EVT_ADC, Duration::from_millis(500))? {
            Some(f) if f.payload.len() >= 2 => Ok(u16::from_le_bytes([f.payload[0], f.payload[1]])),
            Some(f) => Err(anyhow!("EVT_ADC short payload: {:02X?}", f.payload)),
            None => Err(anyhow!("no EVT_ADC reply - is the relay/ADC firmware (FW >= 0x0012) loaded?")),
        }
    }

    /// Read the main-connector 12 V input sense (ANALOG_0 / GPIO9). Returns the ACTUAL line millivolts (the
    /// firmware applies the board divider since FW 0x0014) - the PERMANENT rail (battery/KL30), readable with
    /// the relay off, unlike A5 which only sees the switched 12 V. Needs firmware >= 0x0013; older firmware
    /// has no reply (returns an error).
    pub fn read_12v_in(&mut self) -> Result<u16> {
        self.require_gauges("the input-voltage ADC")?;
        self.send(proto::CMD_12_V_IN_READ, &[])?;
        // Short timeout: a supporting dongle replies in ~ms; on FW < 0x0013 there's no reply, and we don't
        // want the background status poll to stall 500 ms every cycle waiting for it.
        match self.wait_for(proto::EVT_12_V_IN, Duration::from_millis(200))? {
            Some(f) if f.payload.len() >= 2 => Ok(u16::from_le_bytes([f.payload[0], f.payload[1]])),
            Some(f) => Err(anyhow!("EVT_12_V_IN short payload: {:02X?}", f.payload)),
            None => Err(anyhow!("no EVT_12_V_IN reply - is the input-voltage firmware (FW >= 0x0013) loaded?")),
        }
    }

    // ---- K+DCAN cable ignition sense (OBD pin 1 -> FTDI modem status line, when the cable wires it) ----
    /// Read the FTDI's four RS-232 modem status inputs. A K+DCAN cable that brings out OBD pin 1 asserts one
    /// of these when the ignition (KL15) comes on - which one depends on the cable, so read all four and log
    /// them to find the one that tracks ignition. Cable only; errors on the Gauge.S path (use `adc_read`).
    pub fn modem_lines(&mut self) -> Result<ModemLines> {
        if !self.kind.is_cable() {
            return Err(anyhow!("modem_lines is for the K+DCAN cable; the Gauge.S uses adc_read for ignition."));
        }
        let p = self.port.as_deref_mut().ok_or_else(|| anyhow!("serial port is closed"))?;
        Ok(ModemLines {
            cts: p.read_clear_to_send().unwrap_or(false),
            dsr: p.read_data_set_ready().unwrap_or(false),
            cd: p.read_carrier_detect().unwrap_or(false),
            ri: p.read_ring_indicator().unwrap_or(false),
        })
    }

    /// Best-effort "is the ignition on?" over a K+DCAN cable: true if any modem status line is asserted.
    /// Returns `Ok(None)` if the cable exposes no usable line. Tune against `modem_lines` for a given cable.
    pub fn ignition_high(&mut self) -> Result<Option<bool>> {
        Ok(Some(self.modem_lines()?.ignition()))
    }
}

/// Is a serial port with this exact name currently enumerated? Used by `reconnect` to tell "still
/// there" from "re-enumerated under a new name" after a dongle reboot.
fn path_present(path: &str) -> bool {
    serialport::available_ports()
        .map(|ports| ports.iter().any(|p| p.port_name == path))
        .unwrap_or(false)
}

/// Enumerate USB serial ports, collapsing the macOS cu./tty. twins. macOS lists each USB serial device
/// TWICE: /dev/cu.NAME and /dev/tty.NAME (the callout vs dial-in node). Drop the tty. twin whenever its cu.
/// sibling is present, so one physical cable/dongle isn't miscounted as two ports.
fn usb_ports_deduped() -> Result<Vec<(String, serialport::UsbPortInfo)>> {
    let mut usb: Vec<(String, serialport::UsbPortInfo)> = serialport::available_ports()?
        .into_iter()
        .filter_map(|p| match p.port_type {
            serialport::SerialPortType::UsbPort(u) => Some((p.port_name, u)),
            _ => None,
        })
        .collect();
    let cu: std::collections::HashSet<String> =
        usb.iter().filter_map(|(n, _)| n.strip_prefix("/dev/cu.").map(str::to_string)).collect();
    usb.retain(|(n, _)| match n.strip_prefix("/dev/tty.") {
        Some(suffix) => !cu.contains(suffix),
        None => true,
    });
    Ok(usb)
}

/// True for a Gauge.S dongle: Espressif VID+PID or a "Gauge" product string.
fn is_gauges(u: &serialport::UsbPortInfo) -> bool {
    (u.vid == 0x303a && u.pid == 0x1001) || u.product.as_deref().is_some_and(|s| s.contains("Gauge"))
}

/// Auto-detect the port for `kind`.
///
/// Gauge.S: prefer the exact Gauge.S match, else the single generic USB-serial bridge (FTDI/CP210x/CH340/
/// Espressif). K+DCAN cable: prefer a generic bridge that is NOT a Gauge.S (so a dongle plugged in alongside
/// the cable is skipped, not picked). Either way, if the choice is ambiguous, refuse and ask for an explicit
/// port rather than returning the wrong device.
fn auto_detect_kind(kind: LinkKind) -> Result<String> {
    let usb = usb_ports_deduped()?;

    if kind == LinkKind::GaugeS {
        if let Some((name, _)) = usb.iter().find(|(_, u)| is_gauges(u)) {
            return Ok(name.clone());
        }
    }

    // Generic USB-serial bridge VIDs (FTDI / CP210x / CH340 / Espressif).
    const BRIDGE_VIDS: &[u16] = &[0x0403, 0x10c4, 0x1a86, 0x303a];
    let candidates: Vec<&String> = usb
        .iter()
        .filter(|(_, u)| BRIDGE_VIDS.contains(&u.vid))
        // For a cable, exclude the Gauge.S so it isn't chosen when both are plugged in.
        .filter(|(_, u)| kind != LinkKind::KDcan || !is_gauges(u))
        .map(|(name, _)| name)
        .collect();
    let what = if kind == LinkKind::KDcan { "K+DCAN cable" } else { "dongle" };
    match candidates.as_slice() {
        [one] => Ok((*one).clone()),
        [] => Err(anyhow!("no {what} auto-detected - pass --port <name>")),
        many => Err(anyhow!(
            "multiple USB-serial ports found ({}) - pass --port <name>",
            many.iter().map(|s| s.as_str()).collect::<Vec<_>>().join(", ")
        )),
    }
}

/// Back-compat: auto-detect a Gauge.S dongle.
fn auto_detect() -> Result<String> {
    auto_detect_kind(LinkKind::GaugeS)
}
