//! Firmware provisioning for the Gauge.S board - flash / backup / restore.
//!
//! This is NOT part of e46-mk60-tool-core (which talks to the ECU over our serial bridge). It drives
//! the ESP32 **ROM bootloader** via the pure-Rust `espflash` library, so the GUI can reflash the
//! dongle and image its existing firmware without any external toolchain.
//!
//! One uniform path for every board (classic-ESP32 V4 over FTDI, or ESP32-S3 V5 over USB-C): the
//! user puts the board in **download mode** (hold BOOT, tap RST, release), then we connect
//! sync-only (`NoReset` - no DTR/RTS) and auto-detect the chip. Our firmware drives USB as OTG/CDC
//! rather than the ROM USB-Serial-JTAG, so a *running* board can't be auto-reset into the
//! bootloader reliably; the manual entry is the robust choice and works identically on both buses.
//!
//! Backups are a full-flash image (content-agnostic - whatever is on the chip, byte for byte),
//! written to ~/.e46-mk60-tool/backups with a sidecar recording chip, size, hash, and a best-effort
//! label (our bench FW vs unknown/custom vs blank). Nothing is ever overwritten; restore writes a
//! chosen image back verbatim.

use anyhow::{anyhow, bail, Context, Result};
use espflash::connection::{Connection, ResetAfterOperation, ResetBeforeOperation};
use espflash::flasher::Flasher;
use espflash::image_format::Segment;
use espflash::target::{Chip, ProgressCallbacks};
use serialport::{FlowControl, SerialPortType, UsbPortInfo};
use sha2::{Digest, Sha256};
use std::borrow::Cow;
use std::path::{Path, PathBuf};

// ---- embedded firmware images (staged by build.rs: freshest of .pio/build or gui/firmware) ----
const BOOT_APP0: &[u8] = include_bytes!(concat!(env!("OUT_DIR"), "/boot_app0.bin"));
const V4_BOOTLOADER: &[u8] = include_bytes!(concat!(env!("OUT_DIR"), "/v4_bootloader.bin"));
const V4_PARTITIONS: &[u8] = include_bytes!(concat!(env!("OUT_DIR"), "/v4_partitions.bin"));
const V4_FIRMWARE: &[u8] = include_bytes!(concat!(env!("OUT_DIR"), "/v4_firmware.bin"));
const V5_BOOTLOADER: &[u8] = include_bytes!(concat!(env!("OUT_DIR"), "/v5_bootloader.bin"));
const V5_PARTITIONS: &[u8] = include_bytes!(concat!(env!("OUT_DIR"), "/v5_partitions.bin"));
const V5_FIRMWARE: &[u8] = include_bytes!(concat!(env!("OUT_DIR"), "/v5_firmware.bin"));

const APP_OFFSET: u32 = 0x10000; // app partition start on both chips

/// Absolute path to the PlatformIO build output, baked at compile time so it resolves regardless
/// of the runtime working directory (e.g. a double-clicked .app runs from `/`). Present on the dev
/// machine; absent on a distributed install, where the embedded images are used instead.
// Dev builds prefer a freshly-built image straight from the PlatformIO tree over the embedded copy.
// In a release/distributed binary that path doesn't exist on the target (and baking the build machine's
// absolute path would leak the username), so release uses the embedded firmware only - which make-app.sh
// refreshes from the same PlatformIO build at bundle time anyway.
#[cfg(debug_assertions)]
const PIO_BUILD: &str = concat!(env!("CARGO_MANIFEST_DIR"), "/../firmware/.pio/build");
#[cfg(not(debug_assertions))]
const PIO_BUILD: &str = "";

/// Prefer the freshest image on disk (`.pio/build`), else the embedded fallback.
fn resolve(disk: &str, embedded: &'static [u8]) -> Cow<'static, [u8]> {
    match std::fs::read(disk) {
        Ok(d) => Cow::Owned(d),
        Err(_) => Cow::Borrowed(embedded),
    }
}

/// Progress/log events, forwarded to the GUI's event channel.
pub enum FlashEvent {
    Log(String),
    Progress { done: usize, total: usize },
}
pub type Sink<'a> = &'a mut dyn FnMut(FlashEvent);

/// Gauge.S silicon and its flash layout.
#[derive(Clone, Copy, PartialEq, Eq)]
pub enum Board {
    V4Esp32,
    V5Esp32s3,
}

impl Board {
    fn from_chip(chip: Chip) -> Result<Self> {
        match chip {
            Chip::Esp32 => Ok(Board::V4Esp32),
            Chip::Esp32s3 => Ok(Board::V5Esp32s3),
            other => bail!("unsupported chip {other:?}; Gauge.S is ESP32 (V4) or ESP32-S3 (V5)"),
        }
    }
    pub fn label(self) -> &'static str {
        match self {
            Board::V4Esp32 => "V4 (ESP32)",
            Board::V5Esp32s3 => "V5 (ESP32-S3)",
        }
    }
    fn chip_name(self) -> &'static str {
        match self {
            Board::V4Esp32 => "ESP32",
            Board::V5Esp32s3 => "ESP32-S3",
        }
    }
    fn pio_env(self) -> &'static str {
        match self {
            Board::V4Esp32 => "gaugesV4",
            Board::V5Esp32s3 => "gaugesV5",
        }
    }
    /// On-disk `.pio/build` path for one of this board's build files.
    fn disk(self, file: &str) -> String {
        format!("{PIO_BUILD}/{}/{file}", self.pio_env())
    }
    /// True when a freshly-built image is present on disk (dev machine).
    fn has_disk_image(self) -> bool {
        std::path::Path::new(&self.disk("firmware.bin")).exists()
    }
    /// The app image, disk-preferred (so backups of a fresh build fingerprint correctly).
    fn app_firmware(self) -> Cow<'static, [u8]> {
        let embedded = match self {
            Board::V4Esp32 => V4_FIRMWARE,
            Board::V5Esp32s3 => V5_FIRMWARE,
        };
        resolve(&self.disk("firmware.bin"), embedded)
    }
    /// (offset, data) for every region flashed, in order. Each region is disk-preferred, embedded
    /// fallback. boot_app0 is a framework constant, so it's always the embedded copy.
    fn images(self) -> Vec<(u32, Cow<'static, [u8]>)> {
        let (bl_off, bl, pt, fw) = match self {
            // classic ESP32: bootloader at 0x1000
            Board::V4Esp32 => (0x1000, V4_BOOTLOADER, V4_PARTITIONS, V4_FIRMWARE),
            // ESP32-S3: bootloader at 0x0
            Board::V5Esp32s3 => (0x0, V5_BOOTLOADER, V5_PARTITIONS, V5_FIRMWARE),
        };
        vec![
            (bl_off, resolve(&self.disk("bootloader.bin"), bl)),
            (0x8000, resolve(&self.disk("partitions.bin"), pt)),
            (0xe000, Cow::Borrowed(BOOT_APP0)),
            (APP_OFFSET, resolve(&self.disk("firmware.bin"), fw)),
        ]
    }
}

// ---- connection ----

/// Open `port` and connect to the ROM bootloader (board must already be in download mode).
fn connect(port: &str, sink: Sink) -> Result<Flasher> {
    let info = usb_info(port);
    // Our running firmware presents a USB-CDC port named "Gauge_S ..."; the ROM bootloader is a
    // different interface. If we're looking at the firmware port, the board isn't in download mode.
    if info.product.as_deref().is_some_and(|p| p.contains("Gauge")) {
        bail!(
            "{port} is the running firmware's USB port (\"{}\"), not the ROM bootloader. Put the \
             board in download mode - hold BOOT, tap RST, release - and it reappears as a different port.",
            info.product.clone().unwrap_or_default()
        );
    }
    sink(FlashEvent::Log(format!("opening {port} ...")));
    let serial = serialport::new(port, 115_200)
        .flow_control(FlowControl::None)
        .open_native()
        .with_context(|| format!("failed to open {port}"))?;
    let conn = Connection::new(
        serial,
        info,
        ResetAfterOperation::HardReset,
        ResetBeforeOperation::NoReset, // sync-only; board is already in download mode
        115_200,
    );
    let flasher = Flasher::connect(conn, true, true, false, None, None).map_err(|e| {
        anyhow!("couldn't reach the bootloader ({e}). Is the board in download mode? (hold BOOT, tap RST, release)")
    })?;
    Ok(flasher)
}

fn usb_info(port: &str) -> UsbPortInfo {
    if let Ok(ports) = serialport::available_ports() {
        for p in ports {
            if p.port_name == port {
                if let SerialPortType::UsbPort(u) = p.port_type {
                    return u;
                }
            }
        }
    }
    UsbPortInfo { vid: 0, pid: 0, serial_number: None, manufacturer: None, product: None }
}

/// A USB-serial port that could be a board in download mode. Prefers an Espressif/CP210x/CH340/FTDI
/// device; errors if none or if it can't decide.
pub fn detect_port() -> Result<String> {
    const VIDS: &[u16] = &[0x303a, 0x10c4, 0x1a86, 0x0403];
    let ports = serialport::available_ports().context("listing serial ports")?;
    let mut hits: Vec<String> = Vec::new();
    for p in ports {
        // macOS lists each USB device twice (/dev/cu.* and /dev/tty.*); keep the callout node.
        if p.port_name.starts_with("/dev/tty.") {
            continue;
        }
        if let SerialPortType::UsbPort(u) = &p.port_type {
            if VIDS.contains(&u.vid) {
                hits.push(p.port_name.clone());
            }
        }
    }
    match hits.as_slice() {
        [one] => Ok(one.clone()),
        [] => Err(anyhow!("no USB-serial port found - plug the board in (USB-C or FTDI)")),
        many => Err(anyhow!(
            "multiple USB-serial ports ({}) - pick one in the port field",
            many.join(", ")
        )),
    }
}

// ---- flash ----

/// Flash the matching MK60 bench firmware for the detected chip. Returns the board flashed.
pub fn flash(port: &str, sink: Sink) -> Result<Board> {
    let mut flasher = connect(port, &mut *sink)?;
    do_flash(&mut flasher, &mut *sink)
}

/// Back up the existing firmware, then flash ours - in a single bootloader session so the chip
/// never leaves download mode between the two. Returns (backup path, board flashed).
pub fn backup_then_flash(port: &str, sink: Sink) -> Result<(PathBuf, Board)> {
    let mut flasher = connect(port, &mut *sink)?;
    let path = do_backup(&mut flasher, &mut *sink)?;
    let board = do_flash(&mut flasher, &mut *sink)?;
    Ok((path, board))
}

fn do_flash(flasher: &mut Flasher, sink: Sink) -> Result<Board> {
    let board = Board::from_chip(flasher.chip())?;
    let images = board.images();
    let grand_total: usize = images.iter().map(|(_, d)| d.len()).sum();
    let source = if board.has_disk_image() { "live .pio/build" } else { "embedded snapshot" };
    sink(FlashEvent::Log(format!(
        "detected {}; flashing {} regions ({} KiB) from {source}...",
        board.label(),
        images.len(),
        grand_total / 1024
    )));
    let segments: Vec<Segment> = images
        .iter()
        .map(|(off, d)| Segment { addr: *off, data: Cow::Borrowed(d.as_ref()) })
        .collect();
    {
        let mut prog = Prog::new(&mut *sink, grand_total);
        flasher
            .write_bins_to_flash(&segments, &mut prog)
            .map_err(|e| anyhow!("flash write failed: {e}"))?;
    }
    sink(FlashEvent::Log("flash complete - board rebooting into MK60 bench firmware".into()));
    Ok(board)
}

// ---- backup ----

/// Full-flash backup to ~/.e46-mk60-tool/backups. Returns the image path. Content-agnostic.
pub fn backup(port: &str, sink: Sink) -> Result<PathBuf> {
    let mut flasher = connect(port, &mut *sink)?;
    let path = do_backup(&mut flasher, &mut *sink)?;
    flasher.connection().reset().ok(); // standalone backup: leave the board running again
    Ok(path)
}

fn do_backup(flasher: &mut Flasher, sink: Sink) -> Result<PathBuf> {
    let board = Board::from_chip(flasher.chip()).ok();
    let chip_name = board.map(|b| b.chip_name()).unwrap_or("unknown");

    let size = flasher
        .flash_detect()
        .map_err(|e| anyhow!("flash-size detect failed: {e}"))?
        .unwrap_or(espflash::flasher::FlashSize::_4Mb);
    flasher.set_flash_size(size);
    let bytes = size.size();

    let dir = backups_dir()?;
    std::fs::create_dir_all(&dir).with_context(|| format!("creating {}", dir.display()))?;
    let stamp = timestamp();
    let path = dir.join(format!("{}_{}.bin", chip_name.to_lowercase().replace('-', ""), stamp));

    sink(FlashEvent::Log(format!(
        "reading {} ({} MiB) from {chip_name} - this can take a bit...",
        human(bytes),
        bytes / (1024 * 1024)
    )));
    flasher
        .read_flash(0, bytes, 0x1000, 64, path.clone())
        .map_err(|e| anyhow!("flash read failed: {e}"))?;

    // Label + hash the image we just wrote.
    let data = std::fs::read(&path).with_context(|| format!("re-reading {}", path.display()))?;
    let sha = hex(&Sha256::digest(&data));
    let label = label_image(board, &data);
    write_sidecar(&path, chip_name, bytes, &stamp, &sha, &label)?;

    sink(FlashEvent::Log(format!(
        "backup saved: {} [{}]",
        path.display(),
        label
    )));
    Ok(path)
}

/// Classify a full-flash image: our bench FW, blank, or unknown/custom.
fn label_image(board: Option<Board>, data: &[u8]) -> String {
    if data.iter().all(|&b| b == 0xFF) {
        return "blank / read failed".into();
    }
    if let Some(b) = board {
        let fw = b.app_firmware();
        let app = data
            .get(APP_OFFSET as usize..APP_OFFSET as usize + fw.len())
            .unwrap_or(&[]);
        if app == fw.as_ref() {
            return "your bench FW (e46-mk60-tool)".into();
        }
    }
    "unknown / custom FW".into()
}

// ---- restore ----

/// Write a previously-captured full-flash image back to the board, verbatim at 0x0.
pub fn restore(port: &str, image: &Path, sink: Sink) -> Result<()> {
    let data = std::fs::read(image).with_context(|| format!("reading {}", image.display()))?;
    let mut flasher = connect(port, &mut *sink)?;
    // Guard against restoring an image for the wrong silicon.
    if let (Ok(board), Some(meta)) = (Board::from_chip(flasher.chip()), read_sidecar(image)) {
        if !meta.chip.is_empty() && meta.chip != board.chip_name() {
            bail!(
                "this backup is for {}, but the connected board is {} - refusing to restore",
                meta.chip,
                board.chip_name()
            );
        }
    }
    sink(FlashEvent::Log(format!(
        "restoring {} ({} KiB) verbatim...",
        image.file_name().and_then(|s| s.to_str()).unwrap_or("image"),
        data.len() / 1024
    )));
    {
        let mut prog = Prog::new(&mut *sink, data.len());
        let seg = [Segment { addr: 0, data: Cow::Borrowed(&data) }];
        flasher
            .write_bins_to_flash(&seg, &mut prog)
            .map_err(|e| anyhow!("restore write failed: {e}"))?;
    }
    sink(FlashEvent::Log("restore complete - board rebooting".into()));
    Ok(())
}

// ---- backup directory + sidecars ----

pub fn backups_dir() -> Result<PathBuf> {
    e46_mk60_tool_core::ops::data_dir()
        .map(|d| d.join("backups"))
        .ok_or_else(|| anyhow!("no HOME/USERPROFILE set"))
}

/// A backup image plus its sidecar metadata, for the restore picker.
pub struct BackupEntry {
    pub path: PathBuf,
    pub name: String,
    pub label: String,
    pub stamp: String,
}

impl BackupEntry {
    /// Name for the picker: the user's name if set, else the filename. The fingerprint label is
    /// shown at restore time (and in the sidecar), not cluttering every row here.
    pub fn display(&self) -> String {
        if self.name.is_empty() {
            self.path.file_stem().and_then(|s| s.to_str()).unwrap_or("backup").to_string()
        } else {
            self.name.clone()
        }
    }
}

struct Sidecar {
    chip: String,
    label: String,
    stamp: String,
    name: String,
}

/// Set (or clear) the user-facing name on a backup, stored in its sidecar.
pub fn set_backup_name(image: &Path, name: &str) -> Result<()> {
    let p = sidecar_path(image);
    let existing = std::fs::read_to_string(&p).unwrap_or_default();
    let mut lines: Vec<String> =
        existing.lines().filter(|l| !l.starts_with("name=")).map(str::to_string).collect();
    if !name.trim().is_empty() {
        lines.push(format!("name={}", name.trim()));
    }
    let mut body = lines.join("\n");
    body.push('\n');
    std::fs::write(&p, body).context("writing sidecar")?;
    Ok(())
}

/// Delete a backup image and its sidecar.
pub fn delete_backup(image: &Path) -> Result<()> {
    std::fs::remove_file(image).with_context(|| format!("deleting {}", image.display()))?;
    std::fs::remove_file(sidecar_path(image)).ok();
    Ok(())
}

/// Every backup image present, newest first.
pub fn list_backups() -> Vec<BackupEntry> {
    let Ok(dir) = backups_dir() else { return Vec::new() };
    let Ok(rd) = std::fs::read_dir(&dir) else { return Vec::new() };
    let mut out: Vec<BackupEntry> = Vec::new();
    for e in rd.flatten() {
        let path = e.path();
        if path.extension().and_then(|s| s.to_str()) != Some("bin") {
            continue;
        }
        let meta = read_sidecar(&path);
        out.push(BackupEntry {
            name: meta.as_ref().map(|m| m.name.clone()).unwrap_or_default(),
            label: meta
                .as_ref()
                .map(|m| m.label.clone())
                .unwrap_or_else(|| "unlabeled".into()),
            stamp: meta.map(|m| m.stamp).unwrap_or_default(),
            path,
        });
    }
    out.sort_by(|a, b| b.stamp.cmp(&a.stamp));
    out
}

fn sidecar_path(image: &Path) -> PathBuf {
    let mut p = image.to_path_buf();
    let mut name = p.file_name().unwrap_or_default().to_os_string();
    name.push(".meta");
    p.set_file_name(name);
    p
}

fn write_sidecar(
    image: &Path,
    chip: &str,
    flash_size: u32,
    stamp: &str,
    sha: &str,
    label: &str,
) -> Result<()> {
    let body = format!(
        "chip={chip}\nflash_size={flash_size}\ntime={stamp}\nsha256={sha}\nlabel={label}\n"
    );
    std::fs::write(sidecar_path(image), body).context("writing sidecar")?;
    Ok(())
}

fn read_sidecar(image: &Path) -> Option<Sidecar> {
    let text = std::fs::read_to_string(sidecar_path(image)).ok()?;
    let mut chip = String::new();
    let mut label = String::new();
    let mut stamp = String::new();
    let mut name = String::new();
    for line in text.lines() {
        if let Some((k, v)) = line.split_once('=') {
            match k {
                "chip" => chip = v.to_string(),
                "label" => label = v.to_string(),
                "time" => stamp = v.to_string(),
                "name" => name = v.to_string(),
                _ => {}
            }
        }
    }
    Some(Sidecar { chip, label, stamp, name })
}

// ---- helpers ----

/// Bridges espflash's per-segment progress into one cumulative bar over `grand_total`.
struct Prog<'a> {
    sink: Sink<'a>,
    grand_total: usize,
    base: usize,
    seg_total: usize,
}
impl<'a> Prog<'a> {
    fn new(sink: Sink<'a>, grand_total: usize) -> Self {
        Self { sink, grand_total, base: 0, seg_total: 0 }
    }
}
impl ProgressCallbacks for Prog<'_> {
    fn init(&mut self, _addr: u32, total: usize) {
        self.seg_total = total;
    }
    fn update(&mut self, current: usize) {
        (self.sink)(FlashEvent::Progress { done: self.base + current, total: self.grand_total });
    }
    fn verifying(&mut self) {
        (self.sink)(FlashEvent::Log("verifying...".into()));
    }
    fn finish(&mut self, _skipped: bool) {
        self.base += self.seg_total;
        (self.sink)(FlashEvent::Progress { done: self.base, total: self.grand_total });
    }
}

fn timestamp() -> String {
    // Local wall-clock isn't available without a dep; use seconds-since-epoch, sortable.
    let secs = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0);
    format!("{secs}")
}

fn human(bytes: u32) -> String {
    format!("{} B", bytes)
}

fn hex(bytes: &[u8]) -> String {
    bytes.iter().map(|b| format!("{b:02x}")).collect()
}
