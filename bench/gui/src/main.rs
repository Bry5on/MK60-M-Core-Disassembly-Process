//! E46 MK60 Tool - desktop UI (egui/eframe).
//!
//! A thin front end over `e46-mk60-tool-core`: every button runs the SAME `ops`/`bridge` code the
//! CLI uses. Actions run on a worker thread and stream `ops::Event`s (log / progress / CAN)
//! back here over a channel; the UI drains them each frame. No simulated logic.
//!
//! Until the MK60 harness exists, `ping`/`sniff` work against the bare dongle, and
//! `arm`/`probe`/`dump` run for real but return clean timeout errors (no ECU to answer) -
//! which is exactly the behaviour to debug against once the ECU is wired in.

// Windows: hide the console window for release builds (a GUI app shouldn't spawn a terminal), but
// keep it in debug builds so panics/logs are visible while developing. No-op on macOS/Linux.
#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

mod flash;

use eframe::egui;
use e46_mk60_tool_core::bridge::{Bridge, Info, LinkKind};
use e46_mk60_tool_core::ops::{self, Event};
use serialport::SerialPortType;
use std::collections::BTreeMap;
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::{channel, Receiver, Sender};
use std::sync::{Arc, Mutex, OnceLock};
use std::thread;
use std::time::{Duration, Instant};

fn main() -> eframe::Result<()> {
    #[allow(unused_mut)]
    let mut opts = eframe::NativeOptions {
        viewport: egui::ViewportBuilder::default()
            .with_inner_size([960.0, 640.0])
            .with_min_inner_size([780.0, 500.0])
            .with_title("E46 MK60 Tool"),
        ..Default::default()
    };
    // ARM64 Windows has no hardware OpenGL (only software GL 1.1), so the default glow renderer fails with
    // "egui_glow requires opengl 2.0+" and the window never opens. Use the wgpu backend (Direct3D 12) there.
    // Every other target keeps the known-good glow default. The wgpu feature is compiled only for this
    // target (see gui/Cargo.toml), so Renderer::Wgpu exists only under this exact cfg.
    #[cfg(all(target_os = "windows", target_arch = "aarch64"))]
    {
        opts.renderer = eframe::Renderer::Wgpu;
    }
    eframe::run_native("E46 MK60 Tool", opts, Box::new(|_cc| Ok(Box::<App>::default())))
}

/// Which action a worker is running, so the UI can update state when it finishes Ok.
#[derive(Clone, Copy, PartialEq)]
enum Action {
    Ping,
    Sniff,
    CanMonitor,
    KwpCmd,
    RestoreNvm,
    RelayOn,
    RelayOff,
    AdcRead,
    MfgMmioRead,
    MfgMmioSweep,
    MfgEndSession,
    MfgDump,
    DumpNvm,
    Flash,
    Backup,
    Restore,
}

/// A snapshot of live link status from the background poller.
#[derive(Clone, Default)]
struct StatusReading {
    connected: bool,
    fw: String,
    board: String,
    ign: Option<bool>,
    ign_mv: Option<u16>,
    batt: Option<bool>,
    batt_v: Option<f32>,
    relay_trip_mv: Option<u16>, // firmware auto-dropped the relay (12V lost), if seen this poll
}

/// Messages from a worker thread to the UI thread.
enum Msg {
    Event(Event),
    Info(Option<Info>),
    Status(StatusReading),
    DumpData(Vec<u8>),
    BackupSaved(PathBuf),
    Done(Result<(), String>),
    /// The keepalive session worker exited (channel closed or bus error); carries any error.
    KeepaliveEnded(Result<(), String>),
}

#[derive(Default)]
struct CanRow {
    dlc: u8,
    data: Vec<u8>,
    count: u32,
}

/// A `Sender<Msg>` that also wakes the egui UI on every send, so the UI repaints the instant a worker
/// reports progress instead of polling on a fast timer (that 30 fps poll was burning a whole CPU core during
/// a dump). The egui `Context` isn't available until the first frame, so it's filled in then via `set_ctx`;
/// sends before that simply don't repaint (the UI is already painting as it starts up).
#[derive(Clone)]
struct MsgTx {
    tx: Sender<Msg>,
    ctx: Arc<OnceLock<egui::Context>>,
}

impl MsgTx {
    fn send(&self, m: Msg) -> Result<(), std::sync::mpsc::SendError<Msg>> {
        let r = self.tx.send(m);
        if let Some(c) = self.ctx.get() {
            c.request_repaint(); // wake the UI thread so drain() runs and the screen updates
        }
        r
    }
    fn set_ctx(&self, ctx: &egui::Context) {
        let _ = self.ctx.set(ctx.clone()); // once; later frames are no-ops
    }
}

/// The one persistent serial connection, shared by every worker (actions, status poll, keepalive) behind a
/// Mutex. Opening the port fresh for each operation was the big Windows-side latency on initiation (a USB-CDC
/// open + DTR handshake per button press); holding one open Bridge and reusing it makes an action pay only
/// for the bytes it sends. The Mutex also serializes access (Windows COM ports are exclusive), which replaces
/// the old port_lock. The bridge is dropped (and reopened on the next use) when the settings change or an
/// operation fails, so a replug / reboot / port switch still recovers.
#[derive(Default)]
struct Conn {
    bridge: Option<Bridge>,
    key: Option<(Option<String>, u32, LinkKind)>, // (port, baud, link) the current bridge was opened with
}

impl Conn {
    /// Return the open bridge for these settings, opening (or reopening, if the settings changed) as needed.
    fn get(&mut self, port: Option<&str>, baud: u32, link: LinkKind) -> anyhow::Result<&mut Bridge> {
        let key = (port.map(str::to_string), baud, link);
        if self.key.as_ref() != Some(&key) {
            self.bridge = None; // different port/baud/link than what's open - start over
        }
        if self.bridge.is_none() {
            self.bridge = Some(Bridge::open_kind(port, baud, link)?);
            self.key = Some(key);
        }
        Ok(self.bridge.as_mut().expect("just opened"))
    }

    /// Close the connection so the next `get` opens a fresh one (after an error, a flash, or a port change).
    fn close(&mut self) {
        self.bridge = None;
        self.key = None;
    }
}

struct App {
    tx: MsgTx,
    rx: Receiver<Msg>,
    busy: bool,
    current: Option<Action>,

    port: String,
    baud: u32,
    link: LinkKind, // Gauge.S dongle (full features) or a K+DCAN cable (K-line only)
    auto_ign: bool, // K+DCAN mfg entry: time the knock to a sensed ignition edge (OBD pin 1)
    on_car: bool,   // Gauge.S mfg entry: on-car mode - don't drive the relay, knock on the sensed A5 edge
    ports: Vec<(String, Option<String>)>, // (port name, USB product)
    ports_at: Instant,

    connected: bool,
    fw: String,
    board: String,
    // Live monitors (refreshed by a background poller when idle). Some(true/false) = on/off, None = not
    // readable. Battery = the main-connector 12 V input (ANALOG_0, FW >= 0x0013); batt_v is its voltage.
    ign: Option<bool>,
    ign_mv: Option<u16>,
    batt: Option<bool>,
    batt_v: Option<f32>,
    status_inflight: bool, // a status-poll worker is currently holding the port
    status_at: Instant,    // last status poll
    kwp_bytes: String,
    // Seed->key calculator (offline): seed, the even sendKey subfn, and an optional per-image constant
    // (for the 0x7E/0x7F folds, read from 1A 94). Plus the 1A 94 response to decode into those constants.
    did_can: bool,
    mmio_addr: String,    // mfg mmio-read: one MMIO/register address (exec stub)
    mmio_width: u8,       // mfg mmio-read: access width 1/2/4 (non-M peripherals are 16-bit)
    mfg_keep_alive: bool, // mfg: reuse a live session across ops (skip the per-op ignition knock)
    mmio_sweep_start: String, // mfg mmio-sweep: first address (hex)
    mmio_sweep_end: String,   // mfg mmio-sweep: last address (hex, inclusive)
    mmio_sweep_step: String,  // mfg mmio-sweep: address stride (hex; blank = width)
    mmio_sweep_fast: bool,    // mfg mmio-sweep: fast 0xA4 byte read (no exec stub), byte-width only
    scan_ms: String, // duration (ms) for Sniff + Wake-decode
    nvm_restore_base: String, // base address for "Restore NVM from file"
    truncated_dump: bool, // Dump Binary: stop at the data top (skip the erased 0xFF tail) instead of the full window

    progress: Option<(u32, u32)>,
    dump_t0: Option<Instant>, // when the current Dump Binary (MfgDump) started, for the B/s + ETA readout
    last_dump_total: Option<u32>, // the real total of the most recent dump (auto-sized), for the idle diagram label
    saved_cable_lead: Option<u64>, // the cable lead persisted to disk, to avoid redundant re-writes
    saved_oncar_lead: Option<u64>, // the Gauge.S on-car settle persisted to disk, same idea

    can: BTreeMap<u16, CanRow>,
    log: Vec<String>,

    backups: Vec<flash::BackupEntry>,
    restore_sel: usize,
    naming: Option<PathBuf>, // backup awaiting a name (modal open)
    name_buf: String,
    repaint_until: Instant, // force repaints briefly after dialogs/ops (clears paint artifacts)
    cancel: Arc<AtomicBool>, // cooperative stop for the dump loop
    // The single persistent serial connection, shared and serialized across all workers (see Conn).
    conn: Arc<Mutex<Conn>>,
    // Live keepalive session: while Some, a dedicated worker holds the transport open and fires 3E every
    // ~2s; the KWP box routes sends into it. Dropping this sender stops the worker (releases the port).
    ka_cmd: Option<Sender<ops::KaCmd>>,
}

impl Default for App {
    fn default() -> Self {
        let (raw_tx, rx) = channel();
        let tx = MsgTx { tx: raw_tx, ctx: Arc::new(OnceLock::new()) };
        // Seed the sticky cable lead from disk so a known-good value survives relaunches (it's only a start
        // guess; the sweep re-learns on a different car).
        let saved_cable_lead = load_cable_lead();
        if let Some(ms) = saved_cable_lead {
            ops::set_cable_lead(ms);
        }
        let saved_oncar_lead = load_oncar_lead();
        if let Some(ms) = saved_oncar_lead {
            ops::set_oncar_lead(ms);
        }
        Self {
            tx,
            rx,
            busy: false,
            current: None,
            port: "auto".into(),
            baud: 921_600,
            link: load_interface(),
            auto_ign: true,
            on_car: true, // "Cycle Ignition Manually" - default on (safe: no relay needed)
            ports: scan_ports(),
            ports_at: Instant::now(),
            connected: false,
            fw: "-".into(),
            board: "-".into(),
            ign: None,
            ign_mv: None,
            batt: None,
            batt_v: None,
            status_inflight: false,
            status_at: Instant::now() - Duration::from_secs(10),
            kwp_bytes: "1A 94".into(),
            did_can: false,
            mmio_addr: "0x00FF8032".into(),
            mmio_width: 2,
            mfg_keep_alive: false,
            mmio_sweep_start: "0x00EF8600".into(),
            mmio_sweep_end: "0x00EF8F00".into(),
            mmio_sweep_step: "0x100".into(),
            mmio_sweep_fast: false,
            scan_ms: "3000".into(),
            nvm_restore_base: "0x000".into(),
            truncated_dump: false,
            progress: None,
            dump_t0: None,
            last_dump_total: None,
            saved_cable_lead,
            saved_oncar_lead,
            can: BTreeMap::new(),
            log: vec!["Ready. Connect a K+DCAN Cable or a Gauge.S.".into()],
            backups: flash::list_backups(),
            restore_sel: 0,
            naming: None,
            name_buf: String::new(),
            repaint_until: Instant::now(),
            cancel: Arc::new(AtomicBool::new(false)),
            ka_cmd: None,
            conn: Arc::new(Mutex::new(Conn::default())),
        }
    }
}

impl App {
    fn log(&mut self, s: impl Into<String>) {
        self.log.push(s.into());
    }

    /// `None` = auto-detect; otherwise the typed port name.
    fn port_opt(&self) -> Option<String> {
        let p = self.port.trim();
        if p.is_empty() || p.eq_ignore_ascii_case("auto") {
            None
        } else {
            Some(p.to_string())
        }
    }

    /// Spawn a worker that opens its own Bridge and runs `body`, reporting completion via Msg::Done.
    /// `body` may send extra messages (Info/Probe/DumpData) through the provided sender.
    /// Spawn a short-lived worker that reads live link status (connection + ignition/battery) and reports it
    /// via Msg::Status. Only called when idle (see `update`), so it doesn't contend with a running op.
    fn poll_status(&mut self) {
        self.status_inflight = true;
        let tx = self.tx.clone();
        let port = self.port_opt();
        let baud = self.baud;
        let link = self.link;
        let conn = self.conn.clone();
        thread::spawn(move || {
            let s = {
                let mut c = conn.lock().unwrap_or_else(|e| e.into_inner());
                let s = match c.get(port.as_deref(), baud, link) {
                    Ok(br) => read_status(br, link),
                    Err(_) => StatusReading::default(), // nothing on the port -> not connected
                };
                if !s.connected {
                    c.close(); // drop a dead/empty connection so the next poll reopens
                }
                s
            };
            let _ = tx.send(Msg::Status(s));
        });
    }

    fn start<F>(&mut self, action: Action, body: F)
    where
        F: FnOnce(&mut Bridge, &MsgTx) -> anyhow::Result<()> + Send + 'static,
    {
        if self.busy {
            return;
        }
        if self.ka_cmd.is_some() {
            self.log("Keepalive is holding the port - turn it off first.");
            return;
        }
        self.busy = true;
        self.current = Some(action);
        self.progress = None;
        self.cancel.store(false, Ordering::Relaxed);
        ops::reset_cancel();
        let tx = self.tx.clone();
        let port = self.port_opt();
        let baud = self.baud;
        let link = self.link;
        let conn = self.conn.clone();
        thread::spawn(move || {
            let r = (|| -> anyhow::Result<()> {
                // Reuse the persistent connection (opening it if this is the first use); the lock also makes
                // us wait for any in-flight status poll, which is now a quick read, not a fresh port open.
                let mut c = conn.lock().unwrap_or_else(|e| e.into_inner());
                let br = c.get(port.as_deref(), baud, link)?;
                let res = body(br, &tx);
                if res.is_err() {
                    c.close(); // the link may be dead after a failure - reopen fresh next time
                }
                res
            })();
            let _ = tx.send(Msg::Done(r.map_err(|e| e.to_string())));
        });
    }

    /// Spawn a firmware worker (flash/backup/restore). Opens NO Bridge - espflash owns the port.
    /// The port is the typed one, else auto-detected (which finds a board in download mode).
    fn start_flash<F>(&mut self, action: Action, body: F)
    where
        F: FnOnce(&str, &MsgTx) -> anyhow::Result<()> + Send + 'static,
    {
        if self.busy {
            return;
        }
        if self.ka_cmd.is_some() {
            self.log("Keepalive is holding the port - turn it off first.");
            return;
        }
        self.busy = true;
        self.current = Some(action);
        self.progress = None;
        self.cancel.store(false, Ordering::Relaxed);
        ops::reset_cancel();
        let tx = self.tx.clone();
        let port = self.port_opt();
        let conn = self.conn.clone();
        thread::spawn(move || {
            let r = (|| -> anyhow::Result<()> {
                // espflash opens the port itself, so close our persistent handle first (Windows won't let two
                // handles share a COM port) and hold the lock so nothing reopens it mid-flash. The flash
                // reboots the board, so we leave the connection closed - the next op reopens it fresh.
                let mut c = conn.lock().unwrap_or_else(|e| e.into_inner());
                c.close();
                let port = match port {
                    Some(p) => p,
                    None => flash::detect_port()?,
                };
                body(&port, &tx)
            })();
            let _ = tx.send(Msg::Done(r.map_err(|e| e.to_string())));
        });
    }

    /// Start the keepalive session worker: it opens its own Bridge, holds the chosen transport open, and
    /// fires 3E every ~2s so an extended session (e.g. after `10 87`) does not time out between the
    /// manual steps of an auth chain. While it runs, the KWP box routes sends into it (same open
    /// transport) and the other bus buttons are refused - the worker owns the port. NOT gated by `busy`,
    /// so the UI stays live; stopped by dropping the sender (see `stop_keepalive`).
    fn start_keepalive(&mut self) {
        if self.ka_cmd.is_some() || self.busy {
            return;
        }
        let (cmd_tx, cmd_rx) = channel::<ops::KaCmd>();
        self.ka_cmd = Some(cmd_tx);
        let tx = self.tx.clone();
        let port = self.port_opt();
        let baud = self.baud;
        let link = self.link;
        let can = self.did_can;
        let conn = self.conn.clone();
        thread::spawn(move || {
            let r = (|| -> anyhow::Result<()> {
                // Hold the connection lock for the whole keepalive session (it owns the port until stopped).
                // Actions refuse and the status poll is gated off while keepalive runs, so nothing else waits.
                let mut c = conn.lock().unwrap_or_else(|e| e.into_inner());
                let br = c.get(port.as_deref(), baud, link)?;
                let mut sink = |e: Event| {
                    let _ = tx.send(Msg::Event(e));
                };
                let res = ops::keepalive_run(br, can, cmd_rx, &mut sink);
                if res.is_err() {
                    c.close();
                }
                res
            })();
            let _ = tx.send(Msg::KeepaliveEnded(r.map_err(|e| e.to_string())));
        });
    }

    /// Stop the keepalive session: dropping the command sender closes the worker's channel, so it
    /// releases the transport and exits (reported back as `Msg::KeepaliveEnded`).
    fn stop_keepalive(&mut self) {
        self.ka_cmd = None;
    }

    /// Send raw KWP service bytes. If a keepalive session is holding the transport, route them into that
    /// worker (same open session - the one that keeps a seed valid between steps); otherwise fire a
    /// one-shot `KwpCmd` job that opens/closes the transport itself. Shared by the KWP box and the
    /// seed->key calculator's Send button.
    fn send_kwp(&mut self, raw: Vec<u8>) {
        if let Some(ka) = &self.ka_cmd {
            if ka.send(ops::KaCmd::Send(raw)).is_err() {
                self.ka_cmd = None;
                self.log("Keepalive worker is gone; send not delivered.");
            }
        } else {
            let can = self.did_can;
            self.start(Action::KwpCmd, move |br, tx| {
                let mut sink = |e: Event| {
                    let _ = tx.send(Msg::Event(e));
                };
                ops::kwp_cmd(br, &raw, can, &mut sink)?;
                Ok(())
            });
        }
    }

    fn refresh_backups(&mut self) {
        self.backups = flash::list_backups();
        if self.restore_sel >= self.backups.len() {
            self.restore_sel = 0;
        }
    }

    /// Flash button: back-up prompt -> (optional) explicit overwrite warning -> download-mode
    /// prompt -> worker (back up then flash, or flash alone).
    fn flash_clicked(&mut self) {
        use rfd::{MessageButtons as B, MessageDialog as D, MessageDialogResult as R, MessageLevel as L};
        self.kick_repaint();
        let backup_first = match D::new()
            .set_level(L::Warning)
            .set_title("Flash MK60 bench firmware")
            .set_description(
                "This replaces the firmware currently on the Gauge.S with the MK60 bench firmware.\n\n\
                 Back up the existing firmware first? (recommended - it may be the original Gauge.S firmware)",
            )
            .set_buttons(B::YesNoCancel)
            .show()
        {
            R::Yes => true,
            R::No => {
                if D::new()
                    .set_level(L::Warning)
                    .set_title("Overwrite WITHOUT a backup?")
                    .set_description(
                        "The firmware currently on the board will be permanently overwritten and NOT saved. \
                         This cannot be undone.",
                    )
                    .set_buttons(B::OkCancel)
                    .show()
                    != R::Ok
                {
                    return;
                }
                false
            }
            _ => return,
        };
        if !confirm_download_mode() {
            return;
        }
        self.log(if backup_first {
            "Backing up the current firmware, then flashing."
        } else {
            "Flashing without a backup."
        });
        self.start_flash(Action::Flash, move |port, tx| {
            let mut sink = flash_sink(tx);
            if backup_first {
                let (path, board) = flash::backup_then_flash(port, &mut sink)?;
                let _ = tx.send(Msg::Event(Event::Log(format!(
                    "Backed up to {} before flashing {}.",
                    path.display(),
                    board.label()
                ))));
            } else {
                flash::flash(port, &mut sink)?;
            }
            Ok(())
        });
    }

    fn backup_clicked(&mut self) {
        self.kick_repaint();
        if !confirm_download_mode() {
            return;
        }
        self.log("Reading the full flash for backup.");
        self.start_flash(Action::Backup, |port, tx| {
            let mut sink = flash_sink(tx);
            let path = flash::backup(port, &mut sink)?;
            let _ = tx.send(Msg::BackupSaved(path));
            Ok(())
        });
    }

    /// Force a short burst of repaints - clears paint artifacts left when a native modal dialog
    /// covered the (idle) egui window and it didn't redraw the uncovered region.
    fn kick_repaint(&mut self) {
        self.repaint_until = Instant::now() + Duration::from_millis(600);
    }

    fn delete_selected_backup(&mut self) {
        let Some(entry) = self.backups.get(self.restore_sel) else {
            return;
        };
        let path = entry.path.clone();
        let name = entry.display();
        self.kick_repaint();
        let ok = rfd::MessageDialog::new()
            .set_level(rfd::MessageLevel::Warning)
            .set_title("Delete backup")
            .set_description(format!("Delete this backup permanently?\n\n{name}\n{}", path.display()))
            .set_buttons(rfd::MessageButtons::OkCancel)
            .show()
            == rfd::MessageDialogResult::Ok;
        if !ok {
            return;
        }
        match flash::delete_backup(&path) {
            Ok(()) => self.log(format!("Deleted backup {}.", path.display())),
            Err(e) => self.log(format!("Could not delete the backup. {e}")),
        }
        self.refresh_backups();
    }

    fn restore_clicked(&mut self) {
        use rfd::{MessageButtons as B, MessageDialog as D, MessageDialogResult as R, MessageLevel as L};
        self.kick_repaint();
        let Some(entry) = self.backups.get(self.restore_sel) else {
            return;
        };
        let path = entry.path.clone();
        let desc = format!(
            "This OVERWRITES the board with this backup:\n\n{}\n[{}]\n\n\
             The firmware currently on the board will be replaced. Continue?",
            path.display(),
            entry.label
        );
        if D::new()
            .set_level(L::Warning)
            .set_title("Restore firmware")
            .set_description(desc)
            .set_buttons(B::OkCancel)
            .show()
            != R::Ok
        {
            return;
        }
        if !confirm_download_mode() {
            return;
        }
        self.log(format!("Restoring firmware from {}.", path.display()));
        self.start_flash(Action::Restore, move |port, tx| {
            let mut sink = flash_sink(tx);
            flash::restore(port, &path, &mut sink)?;
            Ok(())
        });
    }

    /// Dump finished: pop the OS-native Save dialog (rfd - same call on every platform) and
    /// write the real bytes. Runs on the UI thread (native dialogs must).
    fn save_dump(&mut self, data: Vec<u8>) {
        if data.is_empty() {
            // Nothing came back (failed to enter, cancelled before any read, etc.) - don't prompt to save.
            self.log("Dump produced no data - nothing to save.");
            return;
        }
        self.log(format!("Dump complete. {} bytes read.", data.len()));
        match rfd::FileDialog::new()
            .set_title("Save CCP dump")
            .set_file_name("dump.bin")
            .add_filter("binary", &["bin"])
            .save_file()
        {
            Some(path) => match std::fs::write(&path, &data) {
                Ok(()) => self.log(format!("Wrote {} bytes to {}.", data.len(), path.display())),
                Err(e) => self.log(format!("Could not save the file. {e}")),
            },
            None => self.log("Save cancelled."),
        }
    }

    /// Drain everything the workers have sent since the last frame.
    fn drain(&mut self) {
        while let Ok(m) = self.rx.try_recv() {
            match m {
                Msg::Event(Event::Log(s)) => self.log(s),
                Msg::Event(Event::Progress { done, total }) => {
                    self.progress = Some((done, total));
                    self.last_dump_total = Some(total);
                }
                Msg::Event(Event::Can { id, data }) => {
                    let r = self.can.entry(id).or_default();
                    r.dlc = data.len() as u8;
                    r.data = data;
                    r.count += 1;
                }
                Msg::Status(s) => {
                    // Background poll result - update the live monitors. Don't log (it would spam).
                    self.status_inflight = false;
                    self.connected = s.connected;
                    if s.connected && (!s.fw.is_empty() || !s.board.is_empty()) {
                        self.fw = s.fw;
                        self.board = s.board;
                    }
                    self.ign = s.ign;
                    self.ign_mv = s.ign_mv;
                    self.batt = s.batt;
                    self.batt_v = s.batt_v;
                    if let Some(mv) = s.relay_trip_mv {
                        self.log(format!(
                            "Firmware auto-dropped the relay: 12 V supply fell to {:.1} V. Restore power before energising again.",
                            mv as f32 / 1000.0
                        ));
                    }
                }
                Msg::Info(info) => match info {
                    Some(i) if self.link.is_cable() => {
                        // board 0xCA = nothing answered the dongle handshake (a real cable / empty port).
                        // Any other board = a Gauge.S actually replied on this port -> wrong link selected.
                        if i.board == 0xCA {
                            self.connected = true;
                            self.fw = "-".into();
                            self.board = "cable".into();
                            self.log("K+DCAN cable port open (K-line only; no ignition control/CAN/SPI).");
                        } else {
                            self.connected = false;
                            self.fw = "-".into();
                            self.board = "cable?".into();
                            self.log(format!(
                                "A Gauge.S answered on this port (fw 0x{:04X}, board V{}). Switch the link to Gauge.S, or select the actual cable port.",
                                i.fw, i.board
                            ));
                        }
                    }
                    Some(i) => {
                        self.connected = true;
                        self.fw = format!("0x{:04X}", i.fw);
                        self.board = format!("V{}", i.board);
                        self.log(format!(
                            "Dongle is up. Firmware 0x{:04X}, board V{}, last reset: {}.",
                            i.fw,
                            i.board,
                            e46_mk60_tool_core::bridge::reset_reason_str(i.reset_reason)
                        ));
                    }
                    None => {
                        self.connected = false;
                        self.log("No reply from the dongle.");
                    }
                },
                Msg::DumpData(data) => self.save_dump(data),
                Msg::BackupSaved(path) => {
                    self.refresh_backups();
                    self.name_buf = String::new();
                    self.naming = Some(path); // open the "name this backup" modal
                }
                Msg::Done(res) => {
                    match &res {
                        Ok(()) => match self.current {
                            Some(Action::Flash | Action::Backup | Action::Restore) => {
                                self.refresh_backups()
                            }
                            _ => {}
                        },
                        Err(e) => self.log(format!("Error. {e}")),
                    }
                    self.busy = false;
                    self.current = None;
                    self.kick_repaint();
                }
                Msg::KeepaliveEnded(res) => {
                    if let Err(e) = res {
                        self.log(format!("Keepalive error. {e}"));
                    }
                    self.ka_cmd = None; // worker gone; reflect in the toggle
                    self.kick_repaint();
                }
            }
        }
    }

    /// Right-pane flash-dump progress diagram for the left-pane "Dump Binary" (MfgDump) flow: a custom
    /// address bar from 0x0 to the auto-detected data top, with the live fill, a position tick, percent,
    /// and a B/s + ETA readout. The end label tracks the real dump size (live while running, the last dump's
    /// size when idle, a sane default before the first dump). Idle when no dump is running.
    fn dump_diagram(&self, ui: &mut egui::Ui) {
        // Fallback end address before the first dump has reported a real total (auto-size discovers the
        // true top at dump time; this is just the initial bar scale).
        const FLASH: u32 = 0x40000;
        let running = self.current == Some(Action::MfgDump);
        ui.horizontal(|ui| {
            ui.label(egui::RichText::new("FLASH DUMP").weak().small());
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                let (txt, col) = if running {
                    ("running", egui::Color32::from_rgb(60, 200, 120))
                } else {
                    ("idle", egui::Color32::from_rgb(130, 130, 130))
                };
                ui.label(egui::RichText::new(txt).small().color(col));
            });
        });
        ui.add_space(4.0);

        // End address shown on the bar: live total while running, else the last dump's total, else default.
        let end = if running {
            self.progress.map(|(_, t)| t).or(self.last_dump_total).unwrap_or(FLASH)
        } else {
            self.last_dump_total.unwrap_or(FLASH)
        };
        let (done, total) = if running { self.progress.unwrap_or((0, end)) } else { (0, end) };
        let total = total.max(1);
        let frac = (done as f32 / total as f32).clamp(0.0, 1.0);

        // Custom-painted address bar.
        let h = 32.0;
        let (rect, _) = ui.allocate_exact_size(egui::vec2(ui.available_width(), h), egui::Sense::hover());
        let vis = ui.visuals();
        let rounding = egui::Rounding::same(6.0);
        let accent = if running {
            egui::Color32::from_rgb(60, 200, 120)
        } else {
            egui::Color32::from_rgb(90, 100, 110)
        };
        let p = ui.painter_at(rect);
        p.rect_filled(rect, rounding, vis.extreme_bg_color);
        if frac > 0.0 {
            let fill = egui::Rect::from_min_size(rect.min, egui::vec2(rect.width() * frac, h));
            p.rect_filled(fill, rounding, accent);
        }
        if running && frac > 0.0 && frac < 1.0 {
            let x = rect.min.x + rect.width() * frac;
            p.line_segment(
                [egui::pos2(x, rect.min.y), egui::pos2(x, rect.max.y)],
                egui::Stroke::new(1.5_f32, vis.strong_text_color()),
            );
        }
        p.text(
            rect.center(),
            egui::Align2::CENTER_CENTER,
            format!("{:.0}%", frac * 100.0),
            egui::FontId::proportional(14.0),
            vis.strong_text_color(),
        );

        // End labels.
        ui.horizontal(|ui| {
            ui.label(egui::RichText::new("0x00000").monospace().weak().small());
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                ui.label(egui::RichText::new(format!("0x{end:05X}")).monospace().weak().small());
            });
        });

        // Detail line.
        if running {
            let rate = self
                .dump_t0
                .map(|t0| {
                    let s = t0.elapsed().as_secs_f32();
                    if s > 0.5 { done as f32 / s } else { 0.0 }
                })
                .unwrap_or(0.0);
            let eta = if rate > 1.0 { ((total - done) as f32 / rate) as u64 } else { 0 };
            ui.label(
                egui::RichText::new(format!(
                    "0x{done:05X} / 0x{total:05X}   {done} / {total} bytes   {rate:.0} B/s   ETA {}m{:02}s",
                    eta / 60,
                    eta % 60
                ))
                .monospace()
                .small(),
            );
        } else {
            let hint = match self.last_dump_total {
                Some(t) => format!(
                    "Click \"Dump Binary\" (left) to re-read from 0x0 up to the detected data top (last: 0x{t:05X})."
                ),
                None => "Click \"Dump Binary\" (left) to read from 0x0 up to the auto-detected data top.".to_string(),
            };
            ui.label(egui::RichText::new(hint).weak().small());
        }
    }
}

fn hexs(b: &[u8]) -> String {
    b.iter().map(|x| format!("{x:02X}")).collect::<Vec<_>>().join(" ")
}

/// USB serial ports only (skips Bluetooth / debug consoles), as (name, product).
fn scan_ports() -> Vec<(String, Option<String>)> {
    serialport::available_ports()
        .unwrap_or_default()
        .into_iter()
        // macOS lists each USB device twice (/dev/cu.* and /dev/tty.*); keep the callout node.
        .filter(|p| !p.port_name.starts_with("/dev/tty."))
        .filter_map(|p| match p.port_type {
            SerialPortType::UsbPort(u) => Some((p.port_name, u.product)),
            _ => None,
        })
        .collect()
}

/// Truncate with an ellipsis so long text can't grow a fixed-width widget past its container.
fn elide(s: &str, max: usize) -> String {
    if s.chars().count() <= max {
        s.to_string()
    } else {
        let mut t: String = s.chars().take(max.saturating_sub(3)).collect();
        t.push_str("...");
        t
    }
}

/// Combo's selected-text: "auto (detect)" or the chosen port's basename.
fn port_label(port: &str) -> String {
    let p = port.trim();
    if p.is_empty() || p.eq_ignore_ascii_case("auto") {
        "auto (detect)".into()
    } else {
        p.rsplit('/').next().unwrap_or(p).to_string()
    }
}

/// Where the last-selected interface is remembered across launches.
fn interface_state_path() -> Option<PathBuf> {
    e46_mk60_tool_core::ops::data_dir().map(|d| d.join("interface"))
}

/// Load the remembered interface, defaulting to the Gauge.S dongle.
fn load_interface() -> LinkKind {
    match interface_state_path().and_then(|p| std::fs::read_to_string(p).ok()) {
        Some(s) if s.trim() == "kdcan" => LinkKind::KDcan,
        _ => LinkKind::GaugeS,
    }
}

/// Remember the chosen interface for next launch (best-effort; a write failure is ignored).
fn save_interface(k: LinkKind) {
    if let Some(p) = interface_state_path() {
        if let Some(dir) = p.parent() {
            let _ = std::fs::create_dir_all(dir);
        }
        let _ = std::fs::write(p, if k.is_cable() { "kdcan" } else { "gauges" });
    }
}

/// Where the last known-good cable auto-ignition lead (ms) is remembered across launches.
fn cable_lead_path() -> Option<PathBuf> {
    e46_mk60_tool_core::ops::data_dir().map(|d| d.join("cable_lead"))
}

/// Load the persisted cable lead, if any. It's only a starting guess - the sweep re-learns on a new car.
fn load_cable_lead() -> Option<u64> {
    cable_lead_path()
        .and_then(|p| std::fs::read_to_string(p).ok())
        .and_then(|s| s.trim().parse::<u64>().ok())
        .filter(|&ms| ms <= 200) // sanity-bound a corrupt file
}

/// Persist the known-good cable lead for next launch (best-effort; a write failure is ignored).
fn save_cable_lead(ms: u64) {
    if let Some(p) = cable_lead_path() {
        if let Some(dir) = p.parent() {
            let _ = std::fs::create_dir_all(dir);
        }
        let _ = std::fs::write(p, ms.to_string());
    }
}

/// Where the last known-good Gauge.S on-car settle (ms) is remembered across launches.
fn oncar_lead_path() -> Option<PathBuf> {
    e46_mk60_tool_core::ops::data_dir().map(|d| d.join("oncar_lead"))
}

/// Load the persisted on-car settle, if any. Only a starting guess - the sweep re-learns per host/car.
fn load_oncar_lead() -> Option<u64> {
    oncar_lead_path()
        .and_then(|p| std::fs::read_to_string(p).ok())
        .and_then(|s| s.trim().parse::<u64>().ok())
        .filter(|&ms| ms <= 200) // sanity-bound a corrupt file
}

/// Persist the known-good on-car settle for next launch (best-effort; a write failure is ignored).
fn save_oncar_lead(ms: u64) {
    if let Some(p) = oncar_lead_path() {
        if let Some(dir) = p.parent() {
            let _ = std::fs::create_dir_all(dir);
        }
        let _ = std::fs::write(p, ms.to_string());
    }
}

/// Map a firmware-op event onto the UI channel (reusing the Log/Progress plumbing).
fn flash_sink(tx: &MsgTx) -> impl FnMut(flash::FlashEvent) + '_ {
    move |e| {
        let _ = tx.send(match e {
            flash::FlashEvent::Log(s) => Msg::Event(Event::Log(s)),
            flash::FlashEvent::Progress { done, total } => {
                Msg::Event(Event::Progress { done: done as u32, total: total as u32 })
            }
        });
    }
}

/// Modal prompt: the board must be in ROM download mode before any flash/backup/restore.
fn confirm_download_mode() -> bool {
    rfd::MessageDialog::new()
        .set_level(rfd::MessageLevel::Info)
        .set_title("Put the board in download mode")
        .set_description(
            "Hold the BOOT button, tap RST (or re-plug power) while holding, then release BOOT.\n\n\
             Both USB-C (V5) and FTDI (V4) need this. Click OK once the board is in download mode.",
        )
        .set_buttons(rfd::MessageButtons::OkCancel)
        .show()
        == rfd::MessageDialogResult::Ok
}

/// Confirm before cancelling the running job. Cooperative cancel: the op stops at its next loop
/// boundary, so a read already in flight finishes first.
fn confirm_cancel_job() -> bool {
    rfd::MessageDialog::new()
        .set_level(rfd::MessageLevel::Warning)
        .set_title("Cancel the running job?")
        .set_description(
            "The job will stop at its next safe step, not instantly - a read already in flight \
             finishes first. Any progress so far is kept.",
        )
        .set_buttons(rfd::MessageButtons::YesNo)
        .show()
        == rfd::MessageDialogResult::Yes
}


fn parse_hex(s: &str) -> Option<u32> {
    let t = s.trim().trim_start_matches("0x").trim_start_matches("0X");
    u32::from_str_radix(t, 16).ok()
}

/// Parse a dumps filename for a restore base + expected length.
/// `flagsweep_<LO>_<HI>_*.bin` -> (Some(0xLO), Some(HI-LO+1)); `nvm_*.bin` (512B dump) -> (Some(0x000), None).
fn parse_restore_base(fname: &str) -> (Option<u16>, Option<usize>) {
    let stem = fname.strip_suffix(".bin").unwrap_or(fname);
    let parts: Vec<&str> = stem.split('_').collect();
    if parts.first() == Some(&"flagsweep") && parts.len() >= 3 {
        if let (Ok(lo), Ok(hi)) =
            (u16::from_str_radix(parts[1], 16), u16::from_str_radix(parts[2], 16))
        {
            if hi >= lo {
                return (Some(lo), Some((hi - lo) as usize + 1));
            }
        }
    }
    if parts.first() == Some(&"nvm") {
        return (Some(0x000), None); // dump_nvm output is the full 512B NVM at base 0
    }
    (None, None)
}

/// True if the operator picked the primary (first) button of a two-button rfd dialog. On macOS/Linux rfd
/// shows the custom labels and returns `Custom(primary_label)`; on Windows it uses the native MessageBox,
/// which can't show custom text and returns `Ok`/`Yes` for the primary button instead - accept those too.
fn primary_chosen(result: rfd::MessageDialogResult, primary_label: &str) -> bool {
    use rfd::MessageDialogResult as R;
    match result {
        R::Custom(s) => s == primary_label,
        R::Ok | R::Yes => true,
        _ => false,
    }
}

/// Warn before turning OFF auto-respond-to-ignition on a cable. Returns true if the operator confirms.
fn confirm_disable_auto_ign() -> bool {
    let result = rfd::MessageDialog::new()
        .set_level(rfd::MessageLevel::Warning)
        .set_title("Turn off auto-respond to ignition?")
        .set_description(
            "Without a cable that accurately reports ignition on the OBD port, it is MUCH harder to hit the \
             knock window. Entry might take many, many key cycles and might never work.\n\nLeave this setting \
             on if your cable accurately reports ignition. Only turn it off if you've confirmed your cable \
             does not support this.",
        )
        .set_buttons(rfd::MessageButtons::OkCancelCustom("Turn Off".to_owned(), "Keep On".to_owned()))
        .show();
    primary_chosen(result, "Turn Off")
}

/// Warn before turning OFF "cycle ignition manually" on the Gauge.S. Unchecked = the dongle drives KL15
/// itself via a relay, which must be physically wired. Returns true if the operator confirms.
fn confirm_disable_cycle_ignition() -> bool {
    let result = rfd::MessageDialog::new()
        .set_level(rfd::MessageLevel::Warning)
        .set_title("Drive the ignition with a relay?")
        .set_description(
            "Unchecking this means the dongle switches the module's ignition itself, which needs a relay \
             wired to Gauge.S.\n\nThe relay's low-trigger input has to go to pin EE on Gauge.S and the \
             ignition feedback (switched 12 V) to pin A5. Without that relay the dongle can't cycle the \
             ignition.\n\nLeave this checked to cycle the key by hand on a live car.",
        )
        .set_buttons(rfd::MessageButtons::OkCancelCustom("Relay".to_owned(), "Manual".to_owned()))
        .show();
    primary_chosen(result, "Relay")
}

/// Warn before enabling a truncated dump. Returns true to enable truncation, false to keep full dumps.
fn confirm_enable_truncated_dump() -> bool {
    let result = rfd::MessageDialog::new()
        .set_level(rfd::MessageLevel::Warning)
        .set_title("Truncated dump?")
        .set_description(
            "Truncated Dump stops at the top of real data and skips the erased 0xFF tail of the flash window, \
             so it finishes faster.\n\nIf a module variant stores data in assumed 0xFF tail, a truncated dump \
             will miss it. Leave this off for a complete image of the whole flash window.",
        )
        .set_buttons(rfd::MessageButtons::OkCancelCustom("Truncate".to_owned(), "Full".to_owned()))
        .show();
    primary_chosen(result, "Truncate")
}

/// Read live link status for the top-bar monitors, over the shared persistent connection (the caller opens
/// it). Gauge.S: ping for identity + A5 ADC for ignition (battery/KL30 has no ADC -> None). K+DCAN cable:
/// port open = present; ignition from the pin-1 modem line (battery/pin-16 isn't exposed -> None).
fn read_status(br: &mut Bridge, link: LinkKind) -> StatusReading {
    let mut s = StatusReading::default();
    match link {
        LinkKind::GaugeS => {
            match br.ping() {
                Ok(Some(info)) => {
                    s.connected = true;
                    s.fw = format!("0x{:04X}", info.fw);
                    s.board = format!("V{}", info.board);
                }
                _ => return s,
            }
            if let Ok(mv) = br.adc_read() {
                s.ign_mv = Some(mv);
                s.ign = Some(mv >= 800); // ~>4.4 V on the line through the 10k/2.2k divider = ignition on
            }
            // Battery = the permanent 12 V input rail (ANALOG_0). Needs FW >= 0x0013; older FW errors -> None.
            if let Ok(mv) = br.read_12v_in() {
                let v = ops::volts_12v_in(mv);
                s.batt_v = Some(v);
                s.batt = Some(v >= ops::PRESENT_12V_IN);
            }
            // A firmware relay auto-trip (EVT_RELAY_TRIP) gets recorded by poll() during the reads above.
            s.relay_trip_mv = br.take_relay_trip();
        }
        LinkKind::KDcan => {
            s.connected = true; // the port opened -> a cable is present
            if let Ok(lines) = br.modem_lines() {
                s.ign = Some(lines.ignition()); // KL15 = DSR||CD (RI stuck high, CTS dead-low on these cables)
            }
            // KL30 isn't a distinct line on these cables (DSR+CD = ignition, RI stuck, CTS dead), so there's no
            // real voltage to read. Show Battery as "on" to mean the cable is present/powered (it opened), not
            // an actual KL30 measurement - rendered without a voltage. Ignition is the signal that matters.
            s.batt = Some(true);
        }
    }
    s
}

fn pill(ui: &mut egui::Ui, on: bool, label: &str) {
    let col = if on {
        egui::Color32::from_rgb(60, 200, 120)
    } else {
        egui::Color32::from_rgb(150, 150, 150)
    };
    ui.colored_label(col, label);
}

/// INPA-style status indicator for a RIGHT-TO-LEFT parent layout: adds value, dot, then label so it still
/// reads `Label: ● 13.2V` left-to-right (a nested left-to-right `horizontal` gets reversed by the parent).
/// `on` drives the dot colour (green/grey/dim); `text` is the value shown. `None` = not readable.
fn monitor_rtl(ui: &mut egui::Ui, label: &str, on: Option<bool>, text: &str) {
    let (sym, col) = match on {
        Some(true) => ("\u{25CF}", egui::Color32::from_rgb(60, 200, 120)),
        Some(false) => ("\u{25CB}", egui::Color32::from_rgb(150, 150, 150)),
        None => ("\u{25CB}", egui::Color32::from_rgb(120, 120, 120)),
    };
    // right_to_left parent: first-added lands rightmost, so add value, then dot, then label.
    ui.colored_label(col, egui::RichText::new(text).small());
    ui.colored_label(col, egui::RichText::new(sym).small());
    ui.label(egui::RichText::new(format!("{label}:")).small());
}

impl eframe::App for App {
    fn update(&mut self, ctx: &egui::Context, _f: &mut eframe::Frame) {
        self.tx.set_ctx(ctx); // once: lets worker messages wake the UI (see MsgTx)
        self.drain();
        // Persist a newly-learned cable lead (best-effort, only when it actually changed).
        let hint = ops::cable_lead_hint();
        if hint.is_some() && hint != self.saved_cable_lead {
            if let Some(ms) = hint {
                save_cable_lead(ms);
            }
            self.saved_cable_lead = hint;
        }
        // Same for a newly-learned Gauge.S on-car settle.
        let oncar_hint = ops::oncar_lead_hint();
        if oncar_hint.is_some() && oncar_hint != self.saved_oncar_lead {
            if let Some(ms) = oncar_hint {
                save_oncar_lead(ms);
            }
            self.saved_oncar_lead = oncar_hint;
        }
        // Track the Dump Binary (MfgDump) start time so the right-pane diagram can show B/s + ETA.
        if self.current == Some(Action::MfgDump) {
            self.dump_t0.get_or_insert_with(Instant::now);
        } else {
            self.dump_t0 = None;
        }
        if self.busy {
            // Event-driven: worker messages wake the UI via MsgTx, so we only need a slow fallback here to
            // keep the spinner/elapsed alive and catch a worker that goes quiet (~4 fps instead of 30).
            ctx.request_repaint_after(Duration::from_millis(250));
        } else {
            if self.ports_at.elapsed() >= Duration::from_millis(800) {
                // Keep the port list fresh (the board re-enumerates when it enters download mode).
                self.ports = scan_ports();
                self.ports_at = Instant::now();
            }
            // Live status poll (connection + ignition/battery) when idle. A short-lived worker opens the
            // port, reads, and reports, so the top-bar monitors refresh on their own - not just on Ping.
            // Gated on idle so it never contends with a running op or the keepalive.
            if self.ka_cmd.is_none() && !self.status_inflight
                && self.status_at.elapsed() >= Duration::from_millis(1500)
            {
                self.status_at = Instant::now();
                self.poll_status();
            }
            // Keep the UI ticking when idle so polling/port-scan continue without user input.
            ctx.request_repaint_after(Duration::from_millis(500));
        }

        egui::TopBottomPanel::top("bar").show(ctx, |ui| {
            ui.add_space(4.0);
            ui.horizontal(|ui| {
                ui.heading("E46 MK60 Tool");
                if self.busy {
                    ui.add(egui::Spinner::new());
                    if let Some((done, total)) = self.progress {
                        if total > 0 {
                            ui.label(
                                egui::RichText::new(format!("{}%", done as u64 * 100 / total as u64))
                                    .small()
                                    .weak(),
                            );
                        }
                    }
                    // Cancel button (cooperative - the flash ops own the port and can't be stopped, so
                    // only offer it for Bridge-based jobs). Confirms before signalling.
                    let cancellable =
                        !matches!(self.current, Some(Action::Flash | Action::Backup | Action::Restore));
                    if cancellable
                        && ui
                            .button("X")
                            .on_hover_text("Cancel the running job")
                            .clicked()
                        && confirm_cancel_job()
                    {
                        self.cancel.store(true, Ordering::Relaxed);
                        ops::request_cancel();
                        self.log("Cancelling - the job will stop at its next step.");
                    }
                }
                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                    // right_to_left: first-added is rightmost. Reads, left->right:
                    // [fw/board]   Ignition: ..   |   Connected
                    let label = if self.connected {
                        match self.link {
                            LinkKind::GaugeS => "Dongle Up",
                            LinkKind::KDcan => "Cable Connected",
                        }
                    } else {
                        "Not Connected"
                    };
                    pill(ui, self.connected, label);
                    ui.separator();
                    let ign_txt = match self.ign {
                        Some(true) => "on",
                        Some(false) => "off",
                        None => "n/a",
                    };
                    monitor_rtl(ui, "Ignition", self.ign, ign_txt);
                    ui.add_space(8.0);
                    let batt_txt = match (self.batt, self.batt_v) {
                        (Some(_), Some(v)) => format!("{v:.1}V"), // Gauge.S: real 12V-in reading
                        (Some(true), None) => "on".to_string(),   // cable: present (no KL30 voltage to read)
                        (Some(false), None) => "off".to_string(),
                        (None, _) => "n/a".to_string(),
                    };
                    monitor_rtl(ui, "Battery", self.batt, &batt_txt);
                    if self.connected && self.link == LinkKind::GaugeS {
                        ui.add_space(8.0);
                        ui.label(
                            egui::RichText::new(format!("fw {} board {}", self.fw, self.board))
                                .monospace()
                                .small(),
                        );
                    }
                });
            });
            ui.add_space(4.0);
        });

        egui::SidePanel::left("actions")
            .resizable(true)
            .default_width(200.0)
            .width_range(170.0..=560.0)
            .show(ctx, |ui| {
            // Free = not busy and keepalive not holding the port (the KWP box is the one exception - it
            // routes its sends into the keepalive worker instead of opening the port itself).
            let busy_free = !self.busy && self.ka_cmd.is_none();
            let cable = self.link.is_cable();
            // On a K+DCAN cable only the K-line path works: relay, A5 ADC, CAN, SPI, CCP and firmware flash
            // are unavailable, so those buttons use `idle` (disabled on a cable) while cable-capable K-line /
            // own-device actions use `idle_k`. The CAN KWP transport is also forced off below on a cable.
            let idle = busy_free && !cable;
            let idle_k = busy_free;
            // The "external ignition edge" flag the mfg ops take: on a cable it's auto-respond-to-ignition
            // (default on); on the Gauge.S it's on-car mode (default off = normal relay bench flow).
            let auto_ign = if cable { self.auto_ign } else { self.on_car };
            if cable {
                self.did_can = false; // KWP services must use K-line on a cable
            }
            // Scroll only when the controls overflow a short window; the bar auto-hides otherwise.
            egui::ScrollArea::vertical().auto_shrink([false, false]).show(ui, |ui| {
            let big = egui::vec2(ui.available_width(), 30.0);

            ui.add_space(8.0);
            ui.label(egui::RichText::new("CONNECTION").weak().small());
            // Interface selector: Gauge.S dongle (full features) or a generic K+DCAN FTDI cable (K-line only).
            // The choice is remembered across launches (saved to ~/.e46-mk60-tool/interface).
            ui.horizontal(|ui| {
                ui.label("interface");
                ui.add_enabled_ui(idle_k, |ui| {
                    egui::ComboBox::from_id_salt("iface_sel")
                        .selected_text(egui::RichText::new(self.link.label()).small())
                        .width(112.0)
                        .show_ui(ui, |ui| {
                            let mut picked = false;
                            picked |= ui.selectable_value(&mut self.link, LinkKind::GaugeS, "Gauge.S").clicked();
                            picked |= ui.selectable_value(&mut self.link, LinkKind::KDcan, "K+DCAN cable").clicked();
                            if picked {
                                save_interface(self.link); // remember it for next launch
                            }
                        });
                });
            });
            if cable {
                ui.label(
                    egui::RichText::new("cable: K-line only (no ignition control/CAN/SPI)")
                        .weak()
                        .small(),
                );
            }
            ui.horizontal(|ui| {
                ui.label("port");
                // Always selectable when not busy (even on a cable) - picking the right port is how you
                // disambiguate a Gauge.S and a cable that are both plugged in.
                ui.add_enabled_ui(idle_k, |ui| {
                    egui::ComboBox::from_id_salt("port_sel")
                        .selected_text(egui::RichText::new(port_label(&self.port)).small())
                        .width(112.0)
                        .show_ui(ui, |ui| {
                            ui.selectable_value(&mut self.port, "auto".to_string(), "auto (detect)");
                            for (name, product) in &self.ports {
                                let base = name.rsplit('/').next().unwrap_or(name);
                                let label = match product {
                                    Some(p) => format!("{base} - {p}"),
                                    None => base.to_string(),
                                };
                                ui.selectable_value(&mut self.port, name.clone(), label);
                            }
                            if self.ports.is_empty() {
                                ui.weak("(no USB ports)");
                            }
                        });
                });
            });
            ui.label(
                egui::RichText::new(if cable { "baud: K-line (per op)" } else { "baud 921600" })
                    .monospace()
                    .small(),
            );
            ui.add_space(6.0);
            if ui.add_enabled(idle_k, egui::Button::new("Ping").min_size(big)).clicked() {
                self.start(Action::Ping, |br, tx| {
                    let info = br.ping()?;
                    let _ = tx.send(Msg::Info(info));
                    Ok(())
                });
            }

            ui.add_space(12.0);
            ui.label(egui::RichText::new("CAN").weak().small());
            // Listen duration (ms) shared by Sniff and Wake + sniff + decode.
            ui.horizontal(|ui| {
                ui.label("ms");
                ui.add_enabled(
                    idle,
                    egui::TextEdit::singleline(&mut self.scan_ms)
                        .desired_width(60.0)
                        .font(egui::TextStyle::Monospace),
                );
            });
            let scan_ms = self.scan_ms.parse::<u64>().unwrap_or(3000);
            if ui.add_enabled(idle, egui::Button::new("Sniff").min_size(big)).clicked() {
                self.can.clear();
                self.log(format!("Sniffing for {scan_ms} ms."));
                let dur = Duration::from_millis(scan_ms);
                self.start(Action::Sniff, move |br, tx| {
                    let mut sink = |e: Event| {
                        let _ = tx.send(Msg::Event(e));
                    };
                    let ids = ops::sniff(br, dur, &mut sink)?;
                    let _ = tx.send(Msg::Event(Event::Log(format!("Saw {} IDs.", ids.len()))));
                    Ok(())
                });
            }
            // Wake the MK60 (handshake) so it broadcasts, then decode its telemetry.
            if ui.add_enabled(idle, egui::Button::new("Wake + sniff + decode").min_size(big)).clicked() {
                self.can.clear();
                self.log(format!("Waking the MK60, then decoding its telemetry for {scan_ms} ms."));
                let dur = Duration::from_millis(scan_ms);
                self.start(Action::CanMonitor, move |br, tx| {
                    let mut sink = |e: Event| {
                        let _ = tx.send(Msg::Event(e));
                    };
                    ops::can_monitor(br, dur, true, &mut sink)?;
                    Ok(())
                });
            }

            // ---- DUMP ----
            ui.add_space(12.0);
            ui.label(egui::RichText::new("DUMP").weak().small());
            // One click dumps the whole E46 MK60 flash window 0x0..0x60000 - no address/length input. Enters the
            // mfg session via the relay / on-car A5 edge (Gauge.S) or the knock / ignition-cycle (K+DCAN cable),
            // confirms the known top with a quick boundary check, then reads to 0x60000 (or the data top if
            // "Truncated Dump" is checked). MK60-only: the 0x60000 bound is fixed for this family.
            if ui.add_enabled(idle_k, egui::Button::new("Dump Binary").min_size(big))
                .on_hover_text("Dump the whole E46 MK60 flash window 0x0..0x60000 via 0xA4 'U' into a .bin, including the erased 0xFF tail (so nothing's missed). A quick boundary check confirms the known top first. Check \"Truncated Dump\" to stop at the data top instead. Progress shows in the right pane; cancel with the top-bar X.")
                .clicked()
            {
                let truncated = self.truncated_dump;
                self.log(if truncated {
                    "Dump Binary: boundary check, then dumping 0x0 to the data top (truncated). Cancel with the top-bar X."
                } else {
                    "Dump Binary: boundary check, then dumping the full flash window 0x0..0x60000. Cancel with the top-bar X."
                });
                let cancel = self.cancel.clone();
                self.start(Action::MfgDump, move |br, tx| {
                    let mut sink = |e: Event| { let _ = tx.send(Msg::Event(e)); };
                    let data = ops::mfg_dump_fw(br, 500, 10000, 40, 4, truncated, auto_ign, &cancel, &mut sink)?;
                    let _ = tx.send(Msg::DumpData(data));
                    Ok(())
                });
            }
            // Truncated Dump: stop at the top of real data (skip the erased 0xFF tail) - faster, but may miss
            // data in the upper flash on some variant. Enabling it pops a warning (native dialog, like the
            // other checkboxes); cancelling the warning reverts to a full-window dump.
            if ui.add_enabled(idle_k, egui::Checkbox::new(&mut self.truncated_dump, "Truncated Dump"))
                .on_hover_text("Stop at the top of real data instead of dumping the whole 0x0..0x60000 window - faster, but could miss data in the erased upper region on some module variant. Off (full window) is the safe default.")
                .changed()
                && self.truncated_dump
                && !confirm_enable_truncated_dump()
            {
                self.truncated_dump = false; // user cancelled the truncation warning - keep full-window dumps
            }
            // Gauge.S: cycle KL15 by hand (default) - the dongle times the knock off the A5 ignition edge, no
            // relay needed. Unchecking -> the dongle drives the ignition via a wired relay (confirm dialog).
            if !cable
                && ui.add_enabled(idle_k, egui::Checkbox::new(&mut self.on_car, "Cycle Ignition Manually"))
                    .on_hover_text("On a live car the key switches KL15; the dongle senses the ignition-on edge on A5 and knocks (no relay needed). Uncheck only if a relay is wired - low-trigger on EE, ignition feedback on A5 - to let the dongle drive the ignition itself.")
                    .changed()
                && !self.on_car
                && !confirm_disable_cycle_ignition()
            {
                self.on_car = true; // user cancelled the relay-mode warning - keep manual cycling on
            }
            // K+DCAN cable: auto-respond to a sensed ignition edge (OBD pin 1) instead of blasting the knock
            // while you cycle the key. Only shown on a cable; the Gauge.S uses the A5 ADC instead.
            if cable
                && ui.add_enabled(idle_k, egui::Checkbox::new(&mut self.auto_ign, "Auto-Respond to Ignition"))
                    .on_hover_text("If the cable wires OBD pin 1 to a modem line, time the knock to the sensed ignition edge. Leave off to blast the knock while you cycle the key. Watch the log's ign-line readout to tune.")
                    .changed()
                && !self.auto_ign
                && !confirm_disable_auto_ign()
            {
                self.auto_ign = true; // user cancelled the warning - keep it on
            }

            // ---- GAUGE.S FIRMWARE ----
            ui.add_space(12.0);
            ui.label(egui::RichText::new("GAUGE.S FIRMWARE").weak().small());
            if ui.add_enabled(idle, egui::Button::new("Flash firmware").min_size(big)).clicked() {
                self.flash_clicked();
            }
            if ui.add_enabled(idle, egui::Button::new("Back up FW").min_size(big)).clicked() {
                self.backup_clicked();
            }
            // Indented: these act on existing backups (a subset of "Back up FW"), not new actions.
            ui.indent("backups_group", |ui| {
                if self.backups.is_empty() {
                    ui.label(egui::RichText::new("no backups yet").weak().small());
                } else {
                    let n = self.backups.len();
                    let sel_text = self
                        .backups
                        .get(self.restore_sel)
                        .map(|b| b.display())
                        .unwrap_or_default();
                    egui::ComboBox::from_id_salt("restore_sel")
                        .width(ui.available_width())
                        // Elide the button text so a long name can't grow the combo past the panel.
                        .selected_text(egui::RichText::new(elide(&sel_text, 22)).small())
                        .show_ui(ui, |ui| {
                            // Bound the width AND force wrap mode, so long names wrap to multiple
                            // lines instead of growing the popup off-window.
                            ui.set_max_width(220.0);
                            ui.style_mut().wrap_mode = Some(egui::TextWrapMode::Wrap);
                            for i in 0..n {
                                let text = egui::RichText::new(self.backups[i].display()).small();
                                if ui.selectable_label(self.restore_sel == i, text).clicked() {
                                    self.restore_sel = i;
                                }
                            }
                        });
                    ui.horizontal(|ui| {
                        if ui.add_enabled(idle, egui::Button::new("Restore")).clicked() {
                            self.restore_clicked();
                        }
                        if ui.add_enabled(idle, egui::Button::new("Delete")).clicked() {
                            self.delete_selected_backup();
                        }
                    });
                }
            });

            // ---- ADVANCED (everything else; collapsed by default) ----
            // Use an UNINDENTED collapsing body: the default CollapsingHeader indents its content, which makes
            // the full-width buttons (and long checkbox labels) request indent + full width and force the whole
            // pane wider when expanded (egui grows a side panel to fit non-wrapping content). Unindented, the
            // body lays out exactly like the flat panel, so the pane width stays put.
            ui.add_space(12.0);
            let adv_id = ui.make_persistent_id("advanced_section");
            egui::collapsing_header::CollapsingState::load_with_default_open(ui.ctx(), adv_id, false)
                .show_header(ui, |ui| {
                    ui.label(egui::RichText::new("Advanced").strong());
                })
                .body_unindented(|ui| {

            ui.add_space(12.0);
            // Top-level KWP transport selector: governs every KWP service below - the generic Send KWP,
            // Read/Scan/Sweep common id (22), Read faults (17). Label on its own line + short toggle
            // buttons so the row stays narrow (can't overflow the left panel).
            ui.label(egui::RichText::new("KWP transport").weak().small());
            let ka_on = self.ka_cmd.is_some();
            // Transport is fixed while the keepalive holds a session open (can't switch bus mid-session).
            ui.add_enabled_ui(!ka_on, |ui| {
                ui.horizontal(|ui| {
                    ui.selectable_value(&mut self.did_can, false, "K-line")
                        .on_hover_text("E46 MK60 (DS2/KWP over K-line)");
                    // CAN ISO-TP needs the Gauge.S; a K+DCAN cable has no CAN path here.
                    ui.add_enabled_ui(!cable, |ui| {
                        ui.selectable_value(&mut self.did_can, true, "CAN")
                            .on_hover_text("E5/E9x MK60E5 (DCAN ISO-TP, 0x6F1/0x629)");
                    });
                });
            });
            // Generic KWP sender: type the service bytes, the tool frames + checksums the rest. Stays
            // usable while the keepalive holds the session - sends route into that worker (same open
            // transport), so a seed from 27 FD survives while you compute and send 27 FE <key>.
            let kwp_live = idle_k || ka_on;
            ui.horizontal(|ui| {
                ui.label("kwp");
                let w = (ui.available_width() - 8.0).max(60.0);
                ui.add_enabled(
                    kwp_live,
                    egui::TextEdit::singleline(&mut self.kwp_bytes)
                        .desired_width(w)
                        .hint_text("1A 94")
                        .font(egui::TextStyle::Monospace),
                );
            });
            if ui.add_enabled(kwp_live, egui::Button::new("Send KWP").min_size(big)).clicked() {
                match ops::parse_hex_bytes(&self.kwp_bytes) {
                    Ok(raw) => self.send_kwp(raw),
                    Err(e) => self.log(format!("bad KWP bytes: {e}")),
                }
            }
            // Keepalive toggle: hold the diagnostic session open (3E every ~2s) across the manual steps of
            // an auth chain (10 87 -> 27 FD -> 27 FE <key> -> 27 FB) so it doesn't revert on the ~5s P3
            // timeout. While on, the worker owns the port and the other bus buttons are disabled.
            let mut hold = ka_on;
            if ui
                .add_enabled(idle_k || ka_on, egui::Checkbox::new(&mut hold, "Hold session (3E keepalive)"))
                .on_hover_text(
                    "Fire TesterPresent (3E) every ~2s to keep a 10 87 session alive between manual \
                     commands - needed for seed/key chains. Other bus buttons are disabled while on.",
                )
                .changed()
            {
                if hold {
                    self.start_keepalive();
                } else {
                    self.stop_keepalive();
                }
            }

            ui.add_space(8.0);
            ui.separator();
            ui.add_space(4.0);
            ui.label(egui::RichText::new("Relay / ignition").weak().small());
            // Ignition-timing relay rig (EE drive + A5 feedback on ANALOG2). FW >= 0x0012.
            // Relay ON | Relay OFF share one row (both short, so they fit a narrow pane); "Read Ignition
            // Status" gets its own full-width row below. (A 3-in-a-row layout forced the whole left pane wide
            // because the long label couldn't wrap.) Relay ON is checked for 12 V (see relay_on_checked).
            // Energising the relay needs 12 V present (else the coil chatters/buzzes). Grey those controls
            // when the battery monitor says the supply is absent. Unknown (None, e.g. FW < 0x0013) stays
            // enabled - relay_on_checked then does the reactive A5 check as a backstop.
            let relay_ok = idle && self.batt != Some(false);
            ui.horizontal(|ui| {
                let w = ((ui.available_width() - ui.spacing().item_spacing.x) / 2.0).max(40.0);
                let bsize = egui::vec2(w, 30.0);
                if ui.add_enabled(relay_ok, egui::Button::new("Relay ON").min_size(bsize))
                    .on_hover_text("Energise the relay (ignition on). Needs 12 V on the board's main input.")
                    .clicked()
                {
                    self.start(Action::RelayOn, move |br, tx| {
                        let mut sink = |e: Event| { let _ = tx.send(Msg::Event(e)); };
                        ops::relay_on_checked(br, &mut sink).map(|_| ())
                    });
                }
                if ui.add_enabled(idle, egui::Button::new("Relay OFF").min_size(bsize)).clicked() {
                    self.start(Action::RelayOff, move |br, tx| {
                        let mut sink = |e: Event| { let _ = tx.send(Msg::Event(e)); };
                        ops::relay_set(br, false, &mut sink)
                    });
                }
            });
            if self.batt == Some(false) {
                ui.label(egui::RichText::new("relay disabled: no 12 V on the main input").weak().small());
            }
            if ui.add_enabled(idle, egui::Button::new("Read Ignition Status").min_size(big))
                .on_hover_text("Read the A5 ignition-feedback pin (Gauge.S only).")
                .clicked()
            {
                self.start(Action::AdcRead, move |br, tx| {
                    let mut sink = |e: Event| { let _ = tx.send(Msg::Event(e)); };
                    ops::adc_read(br, &mut sink)
                });
            }
            ui.add_space(8.0);
            ui.separator();
            ui.add_space(4.0);
            ui.label(egui::RichText::new("mfg MMIO").weak().small());
            // single-address MMIO read probe: a stub reads ONE register (word by default) into scratch and
            // reports it. One address per run so a faulting/hanging probe is isolated. Hunts the flash ctrl.
            ui.horizontal(|ui| {
                ui.label("read");
                ui.add_enabled(idle_k, egui::TextEdit::singleline(&mut self.mmio_addr).desired_width(90.0).font(egui::TextStyle::Monospace))
                    .on_hover_text("MMIO/register address to read via an exec stub. Sanity: 0xFF8032 (UART) = 0x00D8. Hunt the flash controller in the 0xEF8xxx gaps. A bad read can HANG (KL30 power-cycle to recover).");
                egui::ComboBox::from_id_salt("mmio_width")
                    .selected_text(match self.mmio_width { 1 => "byte", 4 => "word", _ => "halfword" })
                    .show_ui(ui, |ui| {
                        ui.selectable_value(&mut self.mmio_width, 2, "halfword");
                        ui.selectable_value(&mut self.mmio_width, 1, "byte");
                        ui.selectable_value(&mut self.mmio_width, 4, "word");
                    });
            });
            // mfg mmio-sweep inputs sit directly under the single-read address, so both actions share one
            // combined input group: the single read uses mmio+width, the range sweep uses start..=end + step.
            ui.horizontal(|ui| {
                ui.label("sweep");
                ui.add_enabled(idle_k, egui::TextEdit::singleline(&mut self.mmio_sweep_start).desired_width(80.0).font(egui::TextStyle::Monospace))
                    .on_hover_text("First address (hex, inclusive).");
                ui.label("..");
                ui.add_enabled(idle_k, egui::TextEdit::singleline(&mut self.mmio_sweep_end).desired_width(80.0).font(egui::TextStyle::Monospace))
                    .on_hover_text("Last address (hex, inclusive).");
                ui.label("step");
                ui.add_enabled(idle_k, egui::TextEdit::singleline(&mut self.mmio_sweep_step).desired_width(56.0).font(egui::TextStyle::Monospace))
                    .on_hover_text("Address stride (hex). Blank = access width.");
                ui.add_enabled(idle_k, egui::Checkbox::new(&mut self.mmio_sweep_fast, "fast (0xA4)"))
                    .on_hover_text("Fast mode: one 0xA4 'U' byte read per address (no exec stub) - far quicker, but byte-width only, and an unreadable address just advances instead of stopping.");
            });
            if ui.add_enabled(idle_k, egui::Button::new("mfg mmio-read (one reg)").min_size(big)).clicked() {
                let addr = parse_hex(&self.mmio_addr).unwrap_or(0x00FF_8032);
                let width = self.mmio_width;
                self.log(format!("mfg mmio-read: 0x{addr:08X} (w{width}) via exec stub. Read-only; one address."));
                self.start(Action::MfgMmioRead, move |br, tx| {
                    let mut sink = |e: Event| { let _ = tx.send(Msg::Event(e)); };
                    ops::mfg_mmio_read(br, 500, 10000, 40, 4, b'0', 4800, 1, auto_ign, addr, width, &mut sink)
                });
            }
            // mfg mmio-sweep: read a RANGE of registers in ONE reused session (one knock) at the chosen width.
            // Compact per-address output + a mapped summary. Uses the same stub as mmio-read; keep-alive is
            // forced on for the sweep. A hang stops it (KL30 to recover, resume from the next address).
            if ui.add_enabled(idle_k, egui::Button::new("mfg mmio-sweep (range, one knock)").min_size(big))
                .on_hover_text("Read start..=end at the mmio width, one exec per address, reusing one session. A hang stops the sweep - KL30 power-cycle, then resume from the next address.")
                .clicked()
            {
                let start = parse_hex(&self.mmio_sweep_start).unwrap_or(0x00EF_8600);
                let end = parse_hex(&self.mmio_sweep_end).unwrap_or(0x00EF_8F00);
                let step = parse_hex(&self.mmio_sweep_step).unwrap_or(0);
                let fast = self.mmio_sweep_fast;
                let width = if fast { 1 } else { self.mmio_width };
                self.log(format!("mfg mmio-sweep: 0x{start:08X}..=0x{end:08X} step 0x{step:X} (w{width}{}). One knock, reused per address.", if fast { ", fast 0xA4" } else { "" }));
                self.start(Action::MfgMmioSweep, move |br, tx| {
                    let mut sink = |e: Event| { let _ = tx.send(Msg::Event(e)); };
                    ops::mfg_mmio_sweep(br, 500, 10000, 40, 4, b'0', 4800, 1, auto_ign, start, end, step, width, fast, &mut sink)
                });
            }
            // Session reuse: when checked, the ops leave the mfg session LIVE (KL15 on) instead of dropping it,
            // and each next op reuses it - skipping the ignition knock. The module still times the session out
            // after ~14 s idle, so a stale session just re-knocks. "end session" powers the module down. (A bad
            // read that HANGS the module still needs a KL30 cycle.) Sits below the mmio actions it governs.
            ui.horizontal(|ui| {
                if ui
                    .add_enabled(idle_k, egui::Checkbox::new(&mut self.mfg_keep_alive, "Keep session alive (reuse, skip re-knock)"))
                    .on_hover_text("Leave the mfg session live between ops and reuse it, so a run of probes needs one ignition knock instead of one per op. End it with the button to the right.")
                    .changed()
                {
                    ops::set_keep_session_alive(self.mfg_keep_alive);
                    self.log(if self.mfg_keep_alive {
                        "mfg: keep-session-alive ON - ops leave KL15 on and reuse the live session (one knock per run)."
                    } else {
                        "mfg: keep-session-alive OFF - ops knock fresh and drop KL15 at the end (original behaviour)."
                    });
                }
                if ui
                    .add_enabled(idle_k, egui::Button::new("end session (KL15 off)"))
                    .on_hover_text("Close the K-line and de-energise KL15/the relay to power the module down after a keep-alive run.")
                    .clicked()
                {
                    self.log("mfg end-session: closing the K-line and dropping KL15/relay.");
                    self.start(Action::MfgEndSession, move |br, tx| {
                        let mut sink = |e: Event| { let _ = tx.send(Msg::Event(e)); };
                        ops::mfg_end_session(br, &mut sink)
                    });
                }
            });
            ui.add_space(8.0);
            ui.separator();
            ui.add_space(4.0);
            ui.label(egui::RichText::new("NVM").weak().small());
            // Restore ECU NVM from a backup .bin (write differing bytes + verify). Base auto-fills from
            // the filename (flagsweep_<lo>_<hi>_* / nvm_*); the field overrides it.
            ui.horizontal(|ui| {
                ui.label(egui::RichText::new("base").small().weak());
                ui.add(egui::TextEdit::singleline(&mut self.nvm_restore_base).desired_width(52.0).font(egui::TextStyle::Monospace));
            });
            // Dump raw NVM 0x000..0x1FF to a file + hex log, for diffing CCP-enabled vs disabled units.
            if ui.add_enabled(idle, egui::Button::new("Dump NVM (0x23)").min_size(big)).clicked() {
                self.log("Dumping raw NVM 0x000..0x1FF to bench/dumps and logging the hex.");
                self.start(Action::DumpNvm, move |br, tx| {
                    let mut sink = |e: Event| {
                        let _ = tx.send(Msg::Event(e));
                    };
                    ops::dump_nvm(br, &mut sink)
                });
            }
            if ui.add_enabled(idle, egui::Button::new("Restore NVM from file").min_size(big)).clicked() {
                let dir = e46_mk60_tool_core::ops::dumps_dir();
                match rfd::FileDialog::new()
                    .set_title("Restore ECU NVM from .bin")
                    .set_directory(&dir)
                    .add_filter("binary", &["bin"])
                    .pick_file()
                {
                    Some(path) => {
                        let fname = path.file_name().and_then(|s| s.to_str()).unwrap_or("").to_string();
                        let (mut fname_base, expected_len) = parse_restore_base(&fname);
                        // Any full 512-byte backup restores at base 0x000 (covers nvm_*, flagsweep_full_*,
                        // or any other whole-NVM dump), regardless of name.
                        if fname_base.is_none()
                            && std::fs::metadata(&path).map(|m| m.len()).unwrap_or(0) == 0x200
                        {
                            fname_base = Some(0x000);
                        }
                        // The field wins when the user set it to something other than the default;
                        // otherwise prefill it from the filename.
                        let field_default = {
                            let t = self.nvm_restore_base.trim();
                            t.is_empty() || parse_hex(t) == Some(0)
                        };
                        let base: u16 = if !field_default {
                            let b = parse_hex(&self.nvm_restore_base).unwrap_or(0) as u16;
                            if let Some(fb) = fname_base {
                                if fb != b {
                                    self.log(format!("Note: filename suggests base 0x{fb:03X} but the base field says 0x{b:03X}; using the field."));
                                }
                            }
                            b
                        } else if let Some(fb) = fname_base {
                            self.nvm_restore_base = format!("0x{fb:03X}");
                            fb
                        } else {
                            0
                        };
                        if let (Some(exp), Ok(meta)) = (expected_len, std::fs::metadata(&path)) {
                            let actual = meta.len() as usize;
                            if actual != exp {
                                self.log(format!("Warning: filename implies {exp} byte(s) but the file is {actual} byte(s)."));
                            }
                        }
                        let p = path.display().to_string();
                        self.log(format!("Restoring NVM from {p} at base 0x{base:03X}."));
                        self.start(Action::RestoreNvm, move |br, tx| {
                            let mut sink = |e: Event| {
                                let _ = tx.send(Msg::Event(e));
                            };
                            ops::restore_nvm(br, &p, base, &mut sink)
                        });
                    }
                    None => self.log("Restore cancelled (no file picked)."),
                }
            }

            }); // close Advanced (collapsible)
            }); // close left-pane ScrollArea
        });

        egui::TopBottomPanel::bottom("log").resizable(true).default_height(172.0).show(ctx, |ui| {
            ui.add_space(4.0);
            ui.horizontal(|ui| {
                ui.label(egui::RichText::new("LOG").weak().small());
                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                    if ui.small_button("clear").clicked() {
                        self.log.clear();
                    }
                });
            });
            egui::ScrollArea::vertical().auto_shrink([false, false]).stick_to_bottom(true).show(ui, |ui| {
                // One selectable text block: click-drag selects across lines to copy; wheel scrolls.
                ui.add(
                    egui::Label::new(egui::RichText::new(self.log.join("\n")).monospace())
                        .selectable(true),
                );
            });
        });

        egui::CentralPanel::default().show(ctx, |ui| {
            // Outer scroll: the whole pane scrolls only when it overflows a short window.
            egui::ScrollArea::vertical().auto_shrink([false, false]).show(ui, |ui| {
            // Flash-dump progress diagram for the left-pane "Dump Binary" (MfgDump) flow.
            ui.add_space(6.0);
            self.dump_diagram(ui);

            ui.add_space(8.0);
            ui.separator();

            // CAN monitor
            ui.horizontal(|ui| {
                ui.label(egui::RichText::new("CAN MONITOR").weak().small());
                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                    ui.label(egui::RichText::new(format!("{} ids", self.can.len())).weak().small());
                });
            });
            // CAN table flows inside the central pane's outer scroll (no nested scroller).
                egui::Grid::new("can").striped(true).num_columns(4).spacing([18.0, 4.0]).show(ui, |ui| {
                    ui.strong("ID");
                    ui.strong("DLC");
                    ui.strong("Data");
                    ui.strong("Count");
                    ui.end_row();
                    if self.can.is_empty() {
                        ui.weak("-");
                        ui.weak("");
                        ui.weak("run Sniff");
                        ui.weak("");
                        ui.end_row();
                    }
                    for (id, r) in &self.can {
                        ui.monospace(format!("0x{id:03X}"));
                        ui.monospace(format!("{}", r.dlc));
                        ui.monospace(hexs(&r.data));
                        ui.monospace(format!("{}", r.count));
                        ui.end_row();
                    }
                });
            });
        });

        // "Name this backup" modal - shown after a standalone backup completes.
        if let Some(path) = self.naming.clone() {
            let mut decision: Option<bool> = None; // Some(true)=save, Some(false)=skip
            egui::Window::new("Name this backup")
                .collapsible(false)
                .resizable(false)
                .anchor(egui::Align2::CENTER_CENTER, [0.0, 0.0])
                .show(ctx, |ui| {
                    ui.label(
                        egui::RichText::new(
                            path.file_name().and_then(|s| s.to_str()).unwrap_or("backup"),
                        )
                        .monospace()
                        .small(),
                    );
                    ui.add_space(4.0);
                    let edit = ui.add(
                        egui::TextEdit::singleline(&mut self.name_buf)
                            .hint_text("e.g. stock Gauge.S firmware")
                            .desired_width(240.0),
                    );
                    let entered =
                        edit.lost_focus() && ui.input(|i| i.key_pressed(egui::Key::Enter));
                    ui.add_space(6.0);
                    ui.horizontal(|ui| {
                        if ui.button("Save name").clicked() || entered {
                            decision = Some(true);
                        }
                        if ui.button("Skip").clicked() {
                            decision = Some(false);
                        }
                    });
                });
            if let Some(save) = decision {
                if save {
                    if let Err(e) = flash::set_backup_name(&path, &self.name_buf) {
                        self.log(format!("Could not rename the backup. {e}"));
                    }
                    self.refresh_backups();
                }
                self.naming = None;
            }
        }

        // Repaint briefly after dialogs/ops to clear any uncleared region a native modal left.
        if !self.busy && Instant::now() < self.repaint_until {
            ctx.request_repaint();
        }
    }
}
