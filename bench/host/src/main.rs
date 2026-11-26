//! E46 MK60 Tool (CLI front end). Drives a Gauge.S USB bridge: arm the CCP slave over
//! K-line, then probe/dump over CAN. All transport/protocol logic lives in `e46-mk60-tool-core`;
//! this crate is just argument parsing and command orchestration.

use anyhow::Result;
use clap::{Parser, Subcommand, ValueEnum};
use e46_mk60_tool_core::bridge::{Bridge, LinkKind};
use e46_mk60_tool_core::ops::{self, Event};
use std::time::Duration;

/// Which physical link drives the K-line.
#[derive(Copy, Clone, Debug, PartialEq, Eq, ValueEnum)]
enum Link {
    /// Gauge.S ESP32 dongle (full feature set: relay, ADC, CAN, SPI)
    Gauges,
    /// Generic K+DCAN FTDI cable (K-line only; no relay/ADC/CAN/SPI)
    Kdcan,
}

impl From<Link> for LinkKind {
    fn from(l: Link) -> Self {
        match l {
            Link::Gauges => LinkKind::GaugeS,
            Link::Kdcan => LinkKind::KDcan,
        }
    }
}

#[derive(Parser)]
#[command(about = "E46 MK60 Tool (via Gauge.S bridge or K+DCAN cable)")]
struct Cli {
    /// Serial port (auto-detected by USB VID if omitted; required for a K+DCAN cable)
    #[arg(long)]
    port: Option<String>,
    #[arg(long, default_value_t = 921600)]
    baud: u32,
    /// Physical link: the Gauge.S dongle (default) or a generic K+DCAN cable (K-line only).
    #[arg(long, value_enum, default_value_t = Link::Gauges, global = true)]
    link: Link,
    /// Hex-dump every frame in/out to stderr (debug the wire)
    #[arg(long)]
    raw: bool,
    /// Route KWP services over CAN ISO-TP (0x6F1/0x629, the E5/E9x DSC) instead of K-line (the E46).
    /// Honored by the transport-aware services: kwp, common-read/scan/sweep, ident-scan, read-faults.
    #[arg(long, global = true)]
    can: bool,
    /// Gauge.S ON-CAR mfg entry: don't drive the relay; watch A5 for the operator's ignition edge and knock.
    /// For dumping a module in the car where the key switches KL15. Ignored on a K+DCAN cable.
    #[arg(long, global = true)]
    on_car: bool,
    #[command(subcommand)]
    cmd: Cmd,
}

#[derive(Subcommand)]
enum Cmd {
    /// Ping the dongle, print firmware/board info
    Ping,
    /// Listen-only: print the unique CAN ids seen
    Sniff {
        #[arg(long, default_value_t = 3000)]
        ms: u64,
    },
    /// Monitor + decode the MK60's CAN telemetry (wheel speeds, steering, DSC/speed).
    CanMonitor {
        #[arg(long, default_value_t = 3000)]
        ms: u64,
        /// Send the keepalive handshake first (active TX) to wake a cold module
        #[arg(long)]
        wake: bool,
    },
    /// Send a raw KWP2000/UDS request; the tool frames + checksums it for you (e.g. `kwp "1A 94"`,
    /// `kwp "27 FC 12 34"`). --can routes over CAN ISO-TP (E5/E9x) instead of K-line (E46).
    Kwp {
        /// Service bytes in hex: `10 87`, `21 06`, `1A 94`, `27 FC <key>` (spaces/0x optional).
        bytes: String,
    },
    /// Restore ECU NVM from a backup .bin: write differing bytes then verify. Guards the 512B window.
    #[command(name = "restore-nvm")]
    RestoreNvm {
        #[arg(long)]
        file: String,
        #[arg(long, value_parser = parse_hex16, default_value = "0x000")]
        base: u16,
    },
    /// Dump raw NVM 0x000..0x1FF (0x23) to bench/dumps and print the hex, for cross-unit diffs.
    DumpNvm,
    /// Relay (EE MOSFET) ON - energise the module's switched ignition (FW >= 0x0012).
    #[command(name = "relay-on")]
    RelayOn,
    /// Relay (EE MOSFET) OFF (FW >= 0x0012).
    #[command(name = "relay-off")]
    RelayOff,
    /// Read the A5 ignition-feedback once: prints pin mV + the line voltage (FW >= 0x0012).
    #[command(name = "adc-read")]
    AdcRead,
    /// mfg mmio-read: read ONE MMIO/register address via an uploaded exec stub (word by default; --byte for a
    /// byte). One address per run so a faulting/hanging probe is isolated. Use to hunt the non-M flash
    /// controller (not in the app image) and to check peripherals (0xFF8032 = 0x00D8). Read-only; a hang
    /// needs a KL30 power cycle.
    #[command(name = "mfg-mmio-read")]
    MfgMmioRead {
        /// MMIO address to read (hex).
        #[arg(long, value_parser = parse_hex32)]
        addr: u32,
        /// Access width in bytes: 1, 2 (halfword, default - non-M peripherals are 16-bit), or 4 (word).
        #[arg(long, default_value_t = 2)]
        width: u8,
        #[arg(long, default_value_t = 500)]
        off_ms: u64,
        #[arg(long, default_value_t = 10000)]
        threshold_mv: u16,
        #[arg(long, default_value_t = 40)]
        first_delay: u64,
        #[arg(long, default_value_t = 4)]
        attempts: u64,
        /// Keep the mfg session live and reuse it (skip the ignition knock when a session is already up).
        /// Pays off across ops in one process / the GUI; end it with `mfg-end-session`.
        #[arg(long)]
        keep_alive: bool,
        /// Inject mfcr r1,ss4 (0x10A1) before the load so the 818.3 privilege gate elevates to supervisor.
        /// Read-only; a FAULT after elevate means wrong/protected map, not a privilege wall.
        #[arg(long, default_value_t = false)]
        elevate: bool,
    },
    /// mfg end-session: close the K-line and drop KL15/the relay to power the module down after a keep-alive
    /// run.
    #[command(name = "mfg-end-session")]
    MfgEndSession,
    /// mfg mmio-sweep: read a RANGE of MMIO addresses (--start..=--end, stride --step) as --width reads in
    /// ONE reused session - one ignition knock for the whole range. One compact line per address + a mapped
    /// summary. Stops on a hang (KL30 to recover, resume from the next address). Hunts the flash controller.
    /// Pass `--elevate` to inject mfcr r1,ss4 (0x10A1) before each read-only load (supervisor).
    #[command(name = "mfg-mmio-sweep")]
    MfgMmioSweep {
        /// First address (hex, inclusive).
        #[arg(long, value_parser = parse_hex32)]
        start: u32,
        /// Last address (hex, inclusive).
        #[arg(long, value_parser = parse_hex32)]
        end: u32,
        /// Address stride (hex). 0 (default) uses the access width.
        #[arg(long, default_value_t = 0, value_parser = parse_hex32)]
        step: u32,
        /// Access width in bytes: 1, 2 (halfword, default - non-M peripherals are 16-bit), or 4 (word).
        #[arg(long, default_value_t = 2)]
        width: u8,
        /// Fast mode: one 0xA4 'U' byte read per address (no exec stub) - far quicker, but byte-width only
        /// (forces --width 1) and an unreadable address just advances instead of hard-stopping.
        #[arg(long, default_value_t = false)]
        fast: bool,
        /// Inject mfcr r1,ss4 (0x10A1) before each load (forces exec stub; disables --fast). Read-only.
        /// FAULT after elevate = wrong/protected map. On 818.3, 0xD0001C and 0xF60000 already faulted this way.
        #[arg(long, default_value_t = false)]
        elevate: bool,
        #[arg(long, default_value_t = 500)]
        off_ms: u64,
        #[arg(long, default_value_t = 10000)]
        threshold_mv: u16,
        #[arg(long, default_value_t = 40)]
        first_delay: u64,
        #[arg(long, default_value_t = 4)]
        attempts: u64,
    },
    /// mfg dump: stream 0xA4 'U' reads from --addr for --len bytes into --out (.bin), in one persistent mfg
    /// session (re-entering on any drop). The firmware dump over the OBD K-line.
    #[command(name = "mfg-dump")]
    MfgDump {
        #[arg(long, value_parser = parse_hex32)]
        addr: u32,
        /// Bytes to dump. Ignored with --auto (the length is found by the backward scan).
        #[arg(long, default_value_t = 0, value_parser = parse_hex32)]
        len: u32,
        /// Auto-size: backward-scan from --ceiling for the top of real data, then dump --addr..top (so a
        /// flash larger than expected isn't truncated).
        #[arg(long)]
        auto: bool,
        /// Where the auto-size backward scan starts (hex). Default 0x100000 (clears the E5 main body ~0xF6A24
        /// and the E46 ZCP tail ~0x402F7).
        #[arg(long, default_value = "0x100000", value_parser = parse_hex32)]
        ceiling: u32,
        #[arg(long)]
        out: String,
        #[arg(long, default_value_t = 500)]
        off_ms: u64,
        #[arg(long, default_value_t = 10000)]
        threshold_mv: u16,
        #[arg(long, default_value_t = 40)]
        first_delay: u64,
        #[arg(long, default_value_t = 4)]
        attempts: u64,
        /// K+DCAN cable only: time the knock to a sensed ignition edge (OBD pin 1 -> FTDI modem line)
        /// instead of blasting while you cycle the key.
        #[arg(long)]
        auto_ign: bool,
    },
}

fn parse_hex32(s: &str) -> Result<u32, std::num::ParseIntError> {
    u32::from_str_radix(s.trim_start_matches("0x"), 16)
}
fn parse_hex16(s: &str) -> Result<u16, std::num::ParseIntError> {
    u16::from_str_radix(s.trim_start_matches("0x"), 16)
}
fn main() -> Result<()> {
    let cli = Cli::parse();
    let mut br = Bridge::open_kind(cli.port.as_deref(), cli.baud, cli.link.into())?;
    let cable = cli.link == Link::Kdcan; // cable mfg ops auto-respond to the ignition edge by default
    // External-ignition-edge entry: the cable always edge-triggers; the Gauge.S does so only with --on-car
    // (otherwise it drives the relay). Fed to the mfg ops as their `auto_ign` argument.
    let ext_edge = cable || cli.on_car;
    br.set_trace(cli.raw);

    // CLI event sink: print log lines; progress/CAN are summarized per-command instead.
    let mut log = |e: Event| {
        if let Event::Log(s) = e {
            println!("{s}");
        }
    };

    let can = cli.can; // global KWP transport: false = K-line (E46), true = CAN ISO-TP (E5/E9x)
    match cli.cmd {
        Cmd::Ping => match br.ping()? {
            Some(info) => println!(
                "dongle up: fw 0x{:04X}, board V{}, last reset: {}",
                info.fw,
                info.board,
                e46_mk60_tool_core::bridge::reset_reason_str(info.reset_reason)
            ),
            None => println!("no reply from dongle"),
        },
        Cmd::Sniff { ms } => {
            let ids = ops::sniff(&mut br, Duration::from_millis(ms), &mut log)?;
            println!(
                "{} ids: {}",
                ids.len(),
                ids.iter().map(|i| format!("0x{i:03X}")).collect::<Vec<_>>().join(" ")
            );
        }
        Cmd::CanMonitor { ms, wake } => {
            ops::can_monitor(&mut br, Duration::from_millis(ms), wake, &mut log)?;
        }
        Cmd::Kwp { bytes } => {
            let raw = ops::parse_hex_bytes(&bytes).map_err(anyhow::Error::msg)?;
            ops::kwp_cmd(&mut br, &raw, can, &mut log)?;
        }
        Cmd::RestoreNvm { file, base } => ops::restore_nvm(&mut br, &file, base, &mut log)?,
        Cmd::RelayOn => ops::relay_set(&mut br, true, &mut log)?,
        Cmd::RelayOff => ops::relay_set(&mut br, false, &mut log)?,
        Cmd::AdcRead => ops::adc_read(&mut br, &mut log)?,
        Cmd::MfgMmioRead { addr, width, off_ms, threshold_mv, first_delay, attempts, keep_alive, elevate } => {
            ops::set_keep_session_alive(keep_alive);
            ops::mfg_mmio_read(&mut br, off_ms, threshold_mv, first_delay, attempts, b'0', 4800, 1, ext_edge, addr, width, elevate, &mut log)?
        }
        Cmd::MfgEndSession => {
            ops::mfg_end_session(&mut br, &mut log)?
        }
        Cmd::MfgMmioSweep { start, end, step, width, fast, elevate, off_ms, threshold_mv, first_delay, attempts } => {
            ops::mfg_mmio_sweep(&mut br, off_ms, threshold_mv, first_delay, attempts, b'0', 4800, 1, ext_edge, start, end, step, width, fast, elevate, &mut log)?
        }
        Cmd::MfgDump { addr, len, auto, ceiling, out, off_ms, threshold_mv, first_delay, attempts, auto_ign } => {
            let never = std::sync::atomic::AtomicBool::new(false);
            let ext = auto_ign || cli.on_car;
            let data = if auto {
                ops::mfg_dump_auto(&mut br, off_ms, threshold_mv, first_delay, attempts, addr, ceiling, 0x1000, ext, &never, &mut log)?
            } else {
                ops::mfg_dump(&mut br, off_ms, threshold_mv, first_delay, attempts, addr, len, ext, false, &never, &mut log)?
            };
            std::fs::write(&out, &data)?;
            log(Event::Log(format!("wrote {} bytes to {out}", data.len())));
        }
        Cmd::DumpNvm => ops::dump_nvm(&mut br, &mut log)?,
    }
    Ok(())
}
