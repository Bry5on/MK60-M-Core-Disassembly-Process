//! High-level bench operations - the single orchestration path shared by the CLI and GUI.
//!
//! Each long-running op takes an event [`Sink`] so the caller can render progress live:
//! the CLI prints the events, the GUI forwards them to its UI thread over a channel. All the
//! actual protocol work lives in [`crate::ccp`] / [`crate::kwp`] / [`crate::probe`]; this layer
//! only sequences it and reports what happened.

use crate::bridge::Bridge;
use crate::{kwp, mk60can, proto};
use anyhow::{anyhow, ensure, Result};
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::mpsc::{Receiver, RecvTimeoutError};
use std::time::{Duration, Instant};

/// A progress/log event emitted during an operation.
#[derive(Debug, Clone)]
pub enum Event {
    /// A human-readable line (seed/key, version, result, ...).
    Log(String),
    /// Dump progress, in bytes.
    Progress { done: u32, total: u32 },
    /// A CAN frame seen while sniffing.
    Can { id: u16, data: Vec<u8> },
}

/// Where [`Event`]s go. The CLI prints them; the GUI sends them to its render thread.
pub type Sink<'a> = &'a mut dyn FnMut(Event);

/// Process-wide cooperative cancel flag for the running op. Only one op runs at a time (the GUI gates
/// on `busy`, the CLI runs one command and exits), so a single global flag is enough: the UI sets it
/// from another thread and the long loops below poll [`cancel_requested`]. Reset it with
/// [`reset_cancel`] before each job.
static CANCEL: AtomicBool = AtomicBool::new(false);

/// The cable auto-ignition lead (ms) that last entered the mfg session. Sticky across `mfg_enter_cable`
/// calls within the process, so a re-entry (e.g. a dropped session mid-dump) tries the known-good lead first
/// instead of re-rolling the whole sweep. `u64::MAX` = none learned yet. A front end can persist/restore it
/// across runs via [`cable_lead_hint`]/[`set_cable_lead`] (where to store it is the front end's choice).
static LAST_GOOD_CABLE_LEAD_MS: AtomicU64 = AtomicU64::new(u64::MAX);

/// The last cable auto-ignition lead (ms) that entered, if one has this process; `None` if none learned yet.
/// A front end reads this to persist the value across runs.
pub fn cable_lead_hint() -> Option<u64> {
    match LAST_GOOD_CABLE_LEAD_MS.load(Ordering::Relaxed) {
        u64::MAX => None,
        v => Some(v),
    }
}

/// Seed the sticky cable lead (e.g. from a value a front end persisted). The sweep will still overwrite it if
/// it turns out not to enter on this car.
pub fn set_cable_lead(ms: u64) {
    LAST_GOOD_CABLE_LEAD_MS.store(ms, Ordering::Relaxed);
}

/// The Gauge.S ON-CAR settle (ms, after the sensed A5 edge before the knock) that last entered the mfg
/// session. Same idea as the cable lead: the dongle streams the knock from the PC, so on a high-latency USB
/// host (Windows) the fixed ~40 ms settle can put the selection byte outside the ~130 ms window; sweeping the
/// settle finds a value that lands it. Sticky across re-entries; `u64::MAX` = none learned. Persist/restore
/// via [`oncar_lead_hint`]/[`set_oncar_lead`].
static LAST_GOOD_ONCAR_LEAD_MS: AtomicU64 = AtomicU64::new(u64::MAX);

/// The last Gauge.S on-car settle (ms) that entered this process, if any. A front end reads this to persist it.
pub fn oncar_lead_hint() -> Option<u64> {
    match LAST_GOOD_ONCAR_LEAD_MS.load(Ordering::Relaxed) {
        u64::MAX => None,
        v => Some(v),
    }
}

/// Seed the sticky on-car settle (e.g. from a persisted value). The sweep overwrites it if it doesn't enter.
pub fn set_oncar_lead(ms: u64) {
    LAST_GOOD_ONCAR_LEAD_MS.store(ms, Ordering::Relaxed);
}

/// Ask the running op to stop at its next loop boundary (called from the UI thread).
pub fn request_cancel() {
    CANCEL.store(true, Ordering::Relaxed);
}

/// Clear the cancel flag. Call before starting any new op.
pub fn reset_cancel() {
    CANCEL.store(false, Ordering::Relaxed);
}

/// True once [`request_cancel`] has been called since the last [`reset_cancel`].
pub fn cancel_requested() -> bool {
    CANCEL.load(Ordering::Relaxed)
}

/// When set, session-based mfg ops leave the diagnostic session LIVE on success (K-line open, KL15/relay
/// still energised) instead of dropping the ignition at the end, and `mfg_enter` first PROBES for that live
/// session and skips the ignition cycle / knock when it answers. This lets a run of probes (e.g. an MMIO
/// sweep) reuse one entry instead of re-knocking per address. Default OFF = the original per-op behaviour
/// (knock fresh, drop KL15 at the end). The module still drops the session on its own ~14 s inactivity
/// timeout, so a reuse probe that goes unanswered simply falls through to a full knock - correctness never
/// depends on the session actually still being up. Toggle from a front end via [`set_keep_session_alive`];
/// power the module down with [`mfg_end_session`].
static KEEP_SESSION_ALIVE: AtomicBool = AtomicBool::new(false);

/// Turn session keep-alive/reuse on or off (see [`KEEP_SESSION_ALIVE`]).
pub fn set_keep_session_alive(on: bool) {
    KEEP_SESSION_ALIVE.store(on, Ordering::Relaxed);
}

/// True when session keep-alive/reuse is enabled.
pub fn keep_session_alive() -> bool {
    KEEP_SESSION_ALIVE.load(Ordering::Relaxed)
}

/// When reusing a kept-alive session, whether `mfg_try_reuse` must run the full K/L/K/O exec-capability
/// probe (the expensive descriptor-exec re-arm check). Default ON: every exec-based op NEEDS a session whose
/// descriptor-exec genuinely re-armed, so it must pay for that probe each reuse. A plain 0xA4 read (fast
/// mmio-sweep) only needs a LIVE LEVEL-3 session, never descriptor-exec, so it turns this OFF - reuse then
/// costs just the one-frame level-3 'f' ack instead of a whole exec sequence per address. That is the real
/// speedup of fast mode. Correctness is preserved: if the level-3 ack lies (half-dead session), the 0xA4 read
/// simply returns no-reply, fast mode advances, and the next address's reuse probe fails its ack and re-knocks.
static REUSE_EXEC_PROBE: AtomicBool = AtomicBool::new(true);

/// Turn the reuse-time exec-capability probe on or off (see [`REUSE_EXEC_PROBE`]).
pub fn set_reuse_exec_probe(on: bool) {
    REUSE_EXEC_PROBE.store(on, Ordering::Relaxed);
}

/// True when session reuse must prove descriptor-exec re-armed (the default; off = level-3 ack is enough).
pub fn reuse_exec_probe() -> bool {
    REUSE_EXEC_PROBE.load(Ordering::Relaxed)
}

/// Send an arbitrary KWP2000/UDS service request and show the decoded reply. You supply the raw service
/// bytes (e.g. `10 87`, `21 06`, `1A 94`, `27 FC <key>`); this builds the rest of the frame for you -
/// the BMW-FAST header + XOR checksum on K-line, or ISO-TP on CAN - sends it on the chosen transport,
/// opening and closing it around the one request. It does exactly what you type: no implicit session,
/// no auth, no retries. `can` picks the transport (K-line/E46 or CAN ISO-TP/E5). Returns the response
/// bytes (positive = request-SID + 0x40; `7F <sid> <nrc>` = negative; empty = no reply).
pub fn kwp_cmd(br: &mut Bridge, raw: &[u8], can: bool, sink: Sink) -> Result<Vec<u8>> {
    ensure!(!raw.is_empty(), "no request bytes - give at least a service id, e.g. `1A 94`");
    did_open(br, can)?;
    let resp = kwp_tx(br, can, raw);
    did_close(br, can);
    log_kwp_resp(raw, &resp, sink);
    Ok(resp)
}

/// Log a KWP request/response pair the way the raw sender does: positive (SID+0x40), negative (with the
/// NRC name), no-reply, or anything else. Shared by [`kwp_cmd`] and [`keepalive_run`].
fn log_kwp_resp(raw: &[u8], resp: &[u8], sink: Sink) {
    let req_hex = hexs(raw);
    match resp.first() {
        None => sink(Event::Log(format!("{req_hex} -> no reply."))),
        Some(0x7F) => {
            let nrc = resp.get(2).copied().unwrap_or(0);
            sink(Event::Log(format!(
                "{req_hex} -> {} (negative: {}).",
                hexs(resp),
                kwp::nrc_name(nrc)
            )));
        }
        Some(&sid) if !raw.is_empty() && sid == raw[0].wrapping_add(0x40) => {
            sink(Event::Log(format!("{req_hex} -> {} (positive).", hexs(resp))));
        }
        _ => sink(Event::Log(format!("{req_hex} -> {}.", hexs(resp)))),
    }
}

/// A command handed to a running [`keepalive_run`] session over its channel: raw KWP service bytes to
/// send on the held transport. Dropping the sender ends the session (closes the transport, releases the
/// port). There is deliberately no explicit stop variant - closing the channel is the stop.
pub enum KaCmd {
    /// Send these raw KWP service bytes on the held transport and log the reply.
    Send(Vec<u8>),
}

/// Hold a diagnostic session open: open the transport once and keep it up, firing `3E 01` TesterPresent
/// every ~2 s so an extended session (e.g. after `10 87`) does not revert to default on the KWP P3
/// timeout. User commands arrive on `rx` and are sent on the SAME open transport, so a seed from
/// `27 FD` stays valid while you compute and send the key with `27 FE <key>` - the whole reason the
/// keepalive exists. The keepalive tick is silent while the session holds; it logs only if a tick ever
/// comes back non-positive (the session was lost). Returns when the channel is closed (sender dropped)
/// or the bus errors. Caller owns the Bridge; one session per Bridge. BENCH ONLY.
pub fn keepalive_run(br: &mut Bridge, can: bool, rx: Receiver<KaCmd>, sink: Sink) -> Result<()> {
    did_open(br, can)?;
    sink(Event::Log(format!(
        "keepalive on ({}): holding the session with 3E every 2s. Send 10 87 to open the extended session.",
        if can { "CAN" } else { "K-line" }
    )));
    loop {
        match rx.recv_timeout(Duration::from_millis(2000)) {
            Ok(KaCmd::Send(raw)) => {
                if !raw.is_empty() {
                    let resp = kwp_tx(br, can, &raw);
                    log_kwp_resp(&raw, &resp, sink);
                }
            }
            Err(RecvTimeoutError::Timeout) => {
                // Keepalive tick. Bare 3E (no subfunction): the E46 MK60 rejects 3E 01 with
                // subFunctionNotSupported, but answers a bare TesterPresent with a positive 7E.
                let resp = kwp_tx(br, can, &[0x3E]);
                if resp.first() != Some(&0x7E) {
                    let shown = if resp.is_empty() { "no reply".into() } else { hexs(&resp) };
                    sink(Event::Log(format!("keepalive: 3E -> {shown} (session may have dropped).")));
                }
            }
            Err(RecvTimeoutError::Disconnected) => break,
        }
    }
    did_close(br, can);
    sink(Event::Log("keepalive off: session released.".into()));
    Ok(())
}

fn hexs(b: &[u8]) -> String {
    b.iter().map(|x| format!("{x:02X}")).collect::<Vec<_>>().join(" ")
}

/// Open the transport for a 0x22 read/sweep: K-line fast-init (the E46 path), or the CAN bus in normal
/// mode for BMW ISO-TP (the E5/E9x path - diagnostics there are DCAN, 0x6F1/0x629).
fn did_open(br: &mut Bridge, can: bool) -> Result<()> {
    if can {
        br.can_open(false)
    } else {
        br.kline_open(9600)
    }
}

/// Close what [`did_open`] opened. K-line is closed; the CAN bus is left up (other ops reopen as needed).
fn did_close(br: &mut Bridge, can: bool) {
    if !can {
        br.kline_close().ok();
    }
}

/// Send raw UDS/KWP service bytes over the chosen transport (K-line BMW-FAST or CAN ISO-TP, DSC addr
/// 0x29) on an already-open transport, and return the response (service-id/NRC first), or empty on no
/// reply. The one place every transport-aware KWP op funnels its requests through.
fn kwp_tx(br: &mut Bridge, can: bool, raw: &[u8]) -> Vec<u8> {
    let t = if can { Transport::Can } else { Transport::Kline };
    uds_request(br, t, kwp::DSC_ADDR, raw).ok().flatten().unwrap_or_default()
}







/// Enter session 0x87 and run the 0x27 level-4 SecurityAccess unlock (non-fatal). K-line must be open.
fn open_session_unlock(br: &mut Bridge, sink: Sink) -> Result<()> {
    br.kline_tx(&kwp::start_session(kwp::SESSION_CCP))?;
    let s = kline_response(br)?;
    if s.first() == Some(&0x50) {
        sink(Event::Log("Entered diagnostic session 0x87.".into()));
    } else {
        sink(Event::Log(format!("Session 0x87 was refused ({s:02X?}). Continuing.")));
    }
    br.kline_tx(&kwp::request_seed_l4())?;
    let sr = kline_response(br)?;
    if sr.len() >= 4 && sr[0] == 0x67 && sr[1] == 0x07 {
        let seed = ((sr[2] as u16) << 8) | sr[3] as u16;
        if seed != 0 {
            br.kline_tx(&kwp::send_key_l4(kwp::key_level4(seed)))?;
            let kr = kline_response(br)?;
            if kr.first() == Some(&0x67) {
                sink(Event::Log(format!("Unlocked SecurityAccess (seed 0x{seed:04X}).")));
            } else {
                sink(Event::Log(format!("The key was rejected ({kr:02X?}).")));
            }
        } else {
            sink(Event::Log("Seed was 0x0000 (no level-4 grant here).".into()));
        }
    } else {
        sink(Event::Log(format!("No seed returned ({sr:02X?}).")));
    }
    Ok(())
}

/// One `0x23` read with retry on `7F 23 21` (busyRepeatRequest - the NVM job queue is busy, e.g.
/// draining writes from a recent ClearDTC). Returns the raw reply once it is no longer busy.
fn read_memory_retry(br: &mut Bridge, addr: u16, n: u8) -> Result<Vec<u8>> {
    // Two independent recovery budgets:
    //  - busy (7F 23 21): the NVM job queue is draining - just wait and re-send.
    //  - wedge: an EMPTY ("saw []") or CORRUPT/mis-framed reply = the dongle's K-line UART momentarily
    //    wedged/desynced. kline_response() returns Err in that case; recover with a HARD UART reset
    //    (CMD_KLINE_RESET, FW >= 0x0007) and re-send, so one transient hiccup can't kill a 512 B read.
    let mut busy = 0u32;
    let mut wedge = 0u32;
    loop {
        br.kline_tx(&kwp::read_memory(addr, n))?;
        match kline_response(br) {
            Ok(r) => {
                if r.first() == Some(&0x7F) && r.get(2) == Some(&0x21) {
                    busy += 1;
                    if busy > 25 {
                        anyhow::bail!("0x23 at 0x{addr:03X} stayed busy (7F 23 21) after {busy} retries - power-cycle the module to let the NVM queue drain.");
                    }
                    std::thread::sleep(Duration::from_millis(30));
                    continue;
                }
                return Ok(r); // a real reply (0x63 positive or a genuine NRC) - let the caller classify
            }
            Err(_) => {
                wedge += 1;
                if wedge > 8 {
                    anyhow::bail!("0x23 at 0x{addr:03X}: K-line stayed wedged/desynced (empty reads) after {wedge} UART resets - replug the Gauge.S USB-C or power-cycle the module.");
                }
                let _ = br.kline_reset(); // hard UART reset on the dongle; no-op on FW < 0x0007
                std::thread::sleep(Duration::from_millis(40));
                continue;
            }
        }
    }
}




/// Read EEPROM bytes `[lo, hi]` via service 0x23 (4 bytes/request). Assumes K-line open + unlocked.
/// Expects `63 <data...>`; errors on the first read that doesn't return 0x63.
fn eeprom_read_range(br: &mut Bridge, lo: u16, hi: u16, sink: Sink) -> Result<Vec<u8>> {
    let (start, end) = (lo as u32, hi as u32);
    let total = end - start + 1;
    let mut addr = start;
    let mut out = Vec::new();
    while addr <= end {
        if cancel_requested() {
            sink(Event::Log(format!("EEPROM read cancelled at 0x{addr:03X} ({} bytes read).", out.len())));
            break;
        }
        let n = 4u32.min(end - addr + 1) as u8;
        let r = read_memory_retry(br, addr as u16, n)?;
        match r.first() {
            Some(0x63) => out.extend_from_slice(&r[1..(1 + n as usize).min(r.len())]),
            // requestOutOfRange (0x31) - past the end of the readable EEPROM; stop cleanly.
            Some(0x7F) if r.get(2) == Some(&0x31) => break,
            _ => anyhow::bail!("0x23 read at 0x{addr:03X} failed: {r:02X?} (no access or wrong format)."),
        }
        addr += n as u32;
        // Drive the GUI progress bar, and drop a text line every 0x40 bytes (and at the end).
        let done = addr - start;
        sink(Event::Progress { done, total });
        if done % 0x40 == 0 || addr > end {
            sink(Event::Log(format!("Read 0x{done:03X}/0x{total:03X} bytes.")));
        }
    }
    Ok(out)
}










/// Capture raw K-line bytes (`EVT_KLINE_RX` payload) WITHOUT BMW-FAST parsing: accumulate until `idle`
/// of silence after the last byte, or `cap` total. Used for the non-framed streaming replies the
/// `$09` NVM-walk data subs produce (a long record walk, not a standard `B8`-framed response).
fn kline_capture_raw(br: &mut Bridge, idle: Duration, cap: Duration) -> Result<Vec<u8>> {
    let mut buf = Vec::new();
    let start = Instant::now();
    let mut last = Instant::now();
    loop {
        if let Some(f) = br.poll()? {
            if f.cmd == proto::EVT_KLINE_RX && !f.payload.is_empty() {
                buf.extend_from_slice(&f.payload);
                last = Instant::now();
            }
        }
        let now = Instant::now();
        if now.duration_since(start) >= cap {
            break;
        }
        if !buf.is_empty() && now.duration_since(last) >= idle {
            break;
        }
    }
    Ok(buf)
}

/// Drain and discard any stale K-line RX still queued from a PRIOR exchange, so the next capture starts
/// frame-aligned. A faulting/odd read can emit trailing bytes AFTER its capture window closed; those sit in
/// the host/driver buffer and, if not cleared, make the next read return the previous reply - one frame
/// behind. The fresh-knock entry flushes this implicitly (it re-opens/resets the line); the session-REUSE
/// path does not, so it must drain here. Polls until the stream goes idle for `idle`, or `cap` elapses.
fn kline_drain_stale(br: &mut Bridge, idle: Duration, cap: Duration) -> usize {
    let start = Instant::now();
    let mut last_rx = Instant::now();
    let mut dropped = 0usize;
    loop {
        if let Ok(Some(f)) = br.poll() {
            if f.cmd == proto::EVT_KLINE_RX && !f.payload.is_empty() {
                dropped += f.payload.len();
                last_rx = Instant::now();
            }
        }
        let now = Instant::now();
        if now.duration_since(start) >= cap {
            break;
        }
        if now.duration_since(last_rx) >= idle {
            break;
        }
    }
    dropped
}

/// Settle + clear any RX still pending RIGHT BEFORE a TX, so the following `kline_capture_raw` is frame-
/// aligned. `kline_capture_raw` returns as soon as the buffer is non-empty and then idle for its window; a
/// single leftover byte at capture-start therefore makes it return BEFORE our echo+reply arrive, leaving the
/// real reply buffered for the NEXT capture - a self-sustaining one-frame lag where every read echoes the
/// previous reply (the keep-alive reuse bug). Draining to idle before each TX removes that leftover so the
/// capture waits for THIS frame. Gated on keep-alive: the fresh-knock path (which re-opens/resets the line
/// and is already aligned) is left completely unchanged.
fn kline_settle_before_tx(br: &mut Bridge) {
    if keep_session_alive() {
        let _ = kline_drain_stale(br, Duration::from_millis(15), Duration::from_millis(120));
    }
}








/// Enumerate the full RoutineControl surface: send `31 <id> <param>` across `lo..=hi` WITHOUT
/// stopping, after a level-4 unlock. `7F 31 12` = absent (quiet); `71 ...` = the routine RAN; any
/// other `7F ...` = exists but a precondition rejected it. This maps the ids the earlier sweep never
/// reached (it stopped at the first that ran, 0x23). CAUTION: a routine that RUNS may be an
/// actuator/self-test - low-risk on a bench module with no hydraulics, but not strictly read-only.
/// Read the level-0x7F key constant from the 1A 94 ident appendix (block 0x14/0x15, after 90 00 0E).
fn read_7f_constant(br: &mut Bridge, can: bool) -> Result<u16> {
    let idr = kwp_tx(br, can, &[0x1A, 0x94]);
    ensure!(idr.first() == Some(&0x5A), "1A 94 read failed: {idr:02X?}.");
    let block = &idr[2..];
    let app = block
        .windows(3)
        .position(|w| w == [0x90, 0x00, 0x0E])
        .map(|p| p + 3)
        .ok_or_else(|| anyhow!("no 90 00 0E appendix in 1A 94."))?;
    ensure!(app + 5 < block.len(), "1A 94 appendix too short for the 0x7F constant.");
    Ok(((block[app + 4] as u16) << 8) | block[app + 5] as u16)
}

/// Unlock level 0x7F: 27 FD seed -> afb10 key (8-iter, constant direct - validated on hardware) ->
/// 27 FE. Returns whether the key was accepted. Assumes session 0x87 is open.
fn unlock_level_7f(br: &mut Bridge, can: bool, sink: Sink) -> Result<bool> {
    let c = read_7f_constant(br, can)?;
    let sr = kwp_tx(br, can, &[0x27, 0xFD]);
    ensure!(sr.first() == Some(&0x67) && sr.len() >= 4, "no 0x7F seed (27 FD): {sr:02X?}.");
    let seed = ((sr[2] as u16) << 8) | sr[3] as u16;
    // A zero seed means the level is already open (standard KWP). Sending a key against a zero/absent
    // challenge is rejected, so treat it as success - this makes repeated unlocks within one session
    // (e.g. the flag-region sweep's per-chunk re-unlock) idempotent.
    if seed == 0 {
        sink(Event::Log("Level 0x7F already unlocked.".into()));
        return Ok(true);
    }
    let key = kwp::key_calc_7f(seed, c, 8);
    let ok = kwp_tx(br, can, &[0x27, 0xFE, (key >> 8) as u8, (key & 0xFF) as u8]).first() == Some(&0x67);
    sink(Event::Log(if ok {
        format!("Unlocked level 0x7F (seed 0x{seed:04X}, key 0x{key:04X}, C=0x{c:04X}).")
    } else {
        "Level 0x7F unlock was rejected.".into()
    }));
    Ok(ok)
}




/// Dump the raw NVM window `$23 0x000..0x1FF` (the `$3D`-writable region, where the CCP-enable flag
/// must live if it is byte-settable) to a `.bin` under `bench/dumps/`, and log the hex so two
/// units can be diffed to locate the flag. Read-only. Reads via the level-4 unlock.
pub fn dump_nvm(br: &mut Bridge, sink: Sink) -> Result<()> {
    br.kline_open(9600)?;
    open_session_unlock(br, &mut *sink)?;
    sink(Event::Log("Reading raw NVM 0x000..0x1FF via 0x23 (format 3)...".into()));
    let data = eeprom_read_range(br, 0x000, 0x1FF, &mut *sink)?;
    br.kline_close()?;

    let dir = dumps_dir();
    let ts = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0);
    let path = format!("{dir}/nvm_{ts}.bin");
    std::fs::write(&path, &data)?;
    sink(Event::Log(format!("Saved {} bytes to {path}.", data.len())));
    for (i, chunk) in data.chunks(16).enumerate() {
        let hex = chunk.iter().map(|b| format!("{b:02X}")).collect::<Vec<_>>().join(" ");
        sink(Event::Log(format!("{:03X}: {hex}", i * 16)));
    }
    Ok(())
}

/// A5 line voltage. The firmware now reports ACTUAL line millivolts (it applies the 10k/2.2k divider since FW
/// 0x0014), so this is just mV -> V. Thresholds passed to the relay/mfg ops are likewise in line millivolts.
fn a5_volts(mv: u16) -> f32 {
    (mv as f32) / 1000.0
}

/// Main-connector 12 V input: line voltage from the firmware's line millivolts (divider applied on-device).
pub fn volts_12v_in(mv: u16) -> f32 {
    (mv as f32) / 1000.0
}
/// Supply is considered "present / relay-capable" at or above this (volts). Below it the relay coil can't
/// reliably pull in (it would buzz), so the relay controls are gated off. Matches the firmware's own
/// RELAY_MIN_12V_MV auto-cut threshold.
pub const PRESENT_12V_IN: f32 = 7.0;

/// Set the EE relay on/off (FW >= 0x0012). `on` = relay energised (the module's ignition is switched on).
pub fn relay_set(br: &mut Bridge, on: bool, sink: Sink) -> Result<()> {
    br.relay_set(on)?;
    sink(Event::Log(format!("Relay {}.", if on { "ON (ignition energised)" } else { "OFF" })));
    Ok(())
}

/// Energise the relay only when the 12 V supply is present, so a missing supply can't leave the relay
/// buzzing (the coil can't pull in on USB power alone and the gate chatters).
///
/// Preferred path (FW >= 0x0013): read the permanent input rail (ANALOG_0) BEFORE energising and refuse if
/// it's below `PRESENT_12V_IN` - no buzz at all. Fallback (older FW with no input-voltage read): energise,
/// then check the switched A5 feedback and drop the relay back off if nothing came up. Returns Ok(true) if
/// the relay was left on.
pub fn relay_on_checked(br: &mut Bridge, sink: Sink) -> Result<bool> {
    // Preemptive check on the permanent rail, if the firmware supports it.
    if let Ok(mv) = br.read_12v_in() {
        let v = volts_12v_in(mv);
        if v < PRESENT_12V_IN {
            sink(Event::Log(format!(
                "Relay NOT energised: no 12 V supply (input = {v:.1} V). Plug in / power the OBD (KL30) harness - the board on USB alone can't drive the relay (it would buzz)."
            )));
            return Ok(false);
        }
        br.relay_set(true)?;
        sink(Event::Log(format!("Relay ON (ignition energised). Supply {v:.1} V.")));
        return Ok(true);
    }
    // Fallback: energise and verify via the switched A5 feedback.
    br.relay_set(true)?;
    std::thread::sleep(Duration::from_millis(150)); // let the coil pull in and A5 settle
    let mv = br.adc_read().unwrap_or(0);
    const MIN_MV: u16 = 5000; // line mV; below ~5 V on the switched line the 12 V supply isn't present
    if mv < MIN_MV {
        br.relay_set(false)?;
        sink(Event::Log(format!(
            "Relay DISABLED: no 12 V on the switched line (A5 = {mv} mV ~= {:.1} V). Is the OBD/KL30 harness plugged in and powered? The board on USB alone can't energise the relay - it would just buzz.",
            a5_volts(mv)
        )));
        return Ok(false);
    }
    sink(Event::Log(format!("Relay ON (ignition energised). 12 V confirmed: A5 = {mv} mV ~= {:.1} V.", a5_volts(mv))));
    Ok(true)
}

/// Read the A5 ignition-feedback pin once (FW >= 0x0012).
pub fn adc_read(br: &mut Bridge, sink: Sink) -> Result<()> {
    let mv = br.adc_read()?;
    sink(Event::Log(format!("A5 = {mv} mV (line ~= {:.1} V via the 10k/2.2k divider).", a5_volts(mv))));
    Ok(())
}


/// Read `len` (<=4) NVM bytes at `addr` via the format-3 `$23` block read (with the busy-retry), and
/// return just the data bytes.
fn read_block(br: &mut Bridge, addr: u16, len: u8) -> Result<Vec<u8>> {
    let r = read_memory_retry(br, addr, len)?;
    if r.first() == Some(&0x63) && r.len() >= 1 + len as usize {
        Ok(r[1..1 + len as usize].to_vec())
    } else {
        Err(anyhow!("0x23 read at 0x{addr:03X} (len {len}) failed: {r:02X?}."))
    }
}

/// Single `$3D` byte write; Ok only on the 0x7D positive reply.
fn write_one_3d(br: &mut Bridge, addr: u16, value: u8) -> Result<()> {
    br.kline_tx(&kwp::write_memory(addr, value))?;
    let wr = kline_response(br)?;
    if wr.first() == Some(&0x7D) {
        Ok(())
    } else {
        Err(anyhow!("$3D write at 0x{addr:03X} returned {wr:02X?}"))
    }
}

/// Run a K-line transaction `f`; on failure (e.g. the Gauge.S CDC wedge, "saw []"), try to recover the
/// bridge in-process - [`Bridge::reconnect`] (drop + reopen the serial port) then re-establish the 0x7F
/// session - and retry, up to `tries` attempts total. Returns the op's value or the last error.
fn with_recovery<T>(
    br: &mut Bridge, sink: Sink, tries: u32, mut f: impl FnMut(&mut Bridge) -> Result<T>,
) -> Result<T> {
    let mut last = anyhow!("operation not attempted");
    for attempt in 0..tries {
        if attempt > 0 {
            // A failed transaction here is either a broken pipe (dongle rebooted) or a corrupted/desynced
            // reply - both recoverable. reestablish_7f does the full recovery (reconnect + CMD_KLINE_RESET
            // + re-unlock) within its ~30 s window.
            sink(Event::Log(format!(
                "Comms hiccup - recovering the link and retrying (attempt {}/{})...",
                attempt + 1,
                tries
            )));
            if let Err(e) = reestablish_7f(br, &mut *sink) {
                last = anyhow!("recovery failed: {e}");
                continue;
            }
        }
        match f(br) {
            Ok(v) => return Ok(v),
            Err(e) => last = e,
        }
    }
    Err(last)
}

/// Build the graceful "replug + resume" abort error for a wedged restore at `fail_addr`.
fn resume_err(e: anyhow::Error, fail_addr: u16, base: u16, path: &str) -> anyhow::Error {
    let done = if fail_addr > base {
        format!("Bytes 0x{base:03X}..0x{:03X} are already restored.", fail_addr.saturating_sub(1))
    } else {
        "No bytes were restored yet.".to_string()
    };
    anyhow!(
        "Restore stalled at 0x{fail_addr:03X} ({e}). {done} The Gauge.S link is wedged and an in-process \
         reconnect did not clear it. REPLUG the Gauge.S USB-C and re-run 'Restore NVM' with the SAME file \
         ({path}) - restore is idempotent: it skips the bytes already correct and resumes from 0x{fail_addr:03X}."
    )
}

/// Restore the ECU NVM from a backup `.bin`: for each 4-byte block, read the current NVM (format-3
/// `$23`) and `$3D`-write only the bytes that differ (skip-if-equal minimises EEPROM wear), then read
/// the whole range back and verify. GUARDS against a file that runs past the 512-byte writable window
/// (0x000..0x200) - nothing is written in that case. Logs progress every 64 bytes. Each K-line
/// transaction is wrapped in an in-process recovery (reconnect + re-unlock, up to 3 tries) to ride out
/// a Gauge.S CDC wedge; if recovery still fails it aborts cleanly with the last-good offset and a resume
/// hint - restore is idempotent, so replugging and re-running the SAME file finishes the job.
pub fn restore_nvm(br: &mut Bridge, path: &str, base: u16, sink: Sink) -> Result<()> {
    let file = std::fs::read(path).map_err(|e| anyhow!("could not read backup file {path}: {e}"))?;
    ensure!(!file.is_empty(), "backup file {path} is empty - nothing to restore.");
    let len = file.len();
    let end = base as usize + len;
    ensure!(
        end <= 0x200,
        "refusing to write: 0x{base:03X} + {len} = 0x{end:03X} runs past the 512B NVM window (0x000..0x200). Nothing was written."
    );
    sink(Event::Log(format!("Restoring {len} byte(s) from {path} to NVM 0x{base:03X}..0x{end:03X}.")));

    reestablish_7f(br, &mut *sink)?;

    // Write pass: read each 4-byte block, write only the differing bytes.
    let mut written = 0u32;
    let mut already = 0u32;
    let mut off = 0usize;
    while off < len {
        let blk = (len - off).min(4);
        let addr = base + off as u16;
        let cur = match with_recovery(br, &mut *sink, 3, |br| read_block(br, addr, blk as u8)) {
            Ok(c) => c,
            Err(e) => return Err(resume_err(e, addr, base, path)),
        };
        for j in 0..blk {
            let a = addr + j as u16;
            let want = file[off + j];
            if cur[j] == want {
                already += 1;
                continue; // skip-if-equal to minimise wear
            }
            if let Err(e) = with_recovery(br, &mut *sink, 3, |br| write_one_3d(br, a, want)) {
                return Err(resume_err(e, a, base, path));
            }
            written += 1;
        }
        off += blk;
        if off % 64 == 0 || off == len {
            sink(Event::Log(format!(
                "Restore 0x{:03X}/0x{end:03X} - {written} written, {already} already-correct.",
                base as usize + off
            )));
        }
    }

    // Verify pass: read the whole range back in blocks and compare to the file.
    let mut mismatches = 0u32;
    let mut off = 0usize;
    while off < len {
        let blk = (len - off).min(4);
        let addr = base + off as u16;
        let got = match with_recovery(br, &mut *sink, 3, |br| read_block(br, addr, blk as u8)) {
            Ok(g) => g,
            Err(e) => return Err(resume_err(e, addr, base, path)),
        };
        for j in 0..blk {
            if got[j] != file[off + j] {
                mismatches += 1;
                sink(Event::Log(format!(
                    "Verify MISMATCH at 0x{:03X}: file 0x{:02X}, NVM 0x{:02X}.",
                    addr + j as u16,
                    file[off + j],
                    got[j]
                )));
            }
        }
        off += blk;
    }
    br.kline_close().ok();

    if mismatches == 0 {
        sink(Event::Log(format!(
            "Restored {written} byte(s) ({already} already correct); verified 0x{base:03X}..0x{end:03X}."
        )));
        Ok(())
    } else {
        Err(anyhow!(
            "Restore wrote {written} byte(s) but verification found {mismatches} mismatch(es) in 0x{base:03X}..0x{end:03X} - re-run or check with 'eeprom-read'."
        ))
    }
}


/// Parse a hex-byte string (`AAF18C`, `AA F1 8C`, or `0xAAF18C`) into bytes. Whitespace and a leading
/// `0x` are ignored; the remaining hex digits must be an even count.
pub fn parse_hex_bytes(s: &str) -> Result<Vec<u8>, String> {
    let s = s.trim();
    let s = s.strip_prefix("0x").or_else(|| s.strip_prefix("0X")).unwrap_or(s);
    let cleaned: String = s.chars().filter(|c| c.is_ascii_hexdigit()).collect();
    if cleaned.is_empty() || cleaned.len() % 2 != 0 {
        return Err(format!("expected an even number of hex digits (e.g. AAF18C), got '{s}'"));
    }
    (0..cleaned.len())
        .step_by(2)
        .map(|i| u8::from_str_radix(&cleaned[i..i + 2], 16).map_err(|e| e.to_string()))
        .collect()
}








/// The NVM-dump directory. In a dev build it's the project-local `bench/dumps/` (resolved from core's
/// manifest dir so the printed path is clean and the same from the CLI, `cargo run`, or the bundled `.app`).
/// A release/distributed binary must NOT bake the build machine's manifest path (that leaks the username and
/// wouldn't exist on another machine), so it uses `~/.e46-mk60-tool/dumps` instead - alongside the backups dir.
/// Created if missing. Every dump writer (`dump_nvm`, the sweep's full snapshot, `nvm_walk_raw`,
/// `verify_record_write`) goes through it.
pub fn dumps_dir() -> String {
    #[cfg(debug_assertions)]
    let dir = {
        let manifest = std::path::Path::new(env!("CARGO_MANIFEST_DIR"));
        manifest.parent().unwrap_or(manifest).join("dumps")
    };
    #[cfg(not(debug_assertions))]
    let dir = data_dir()
        .map(|d| d.join("dumps"))
        .unwrap_or_else(|| std::path::PathBuf::from("dumps"));
    std::fs::create_dir_all(&dir).ok();
    dir.to_string_lossy().into_owned()
}

/// The per-user data directory, `~/.e46-mk60-tool` - backups, the saved cable/on-car leads, the remembered
/// interface, and (release builds) NVM dumps. Resolves via `HOME`, then `USERPROFILE` (Windows, where HOME is
/// usually unset), so persistence works on every platform. `None` only if neither variable is set.
pub fn data_dir() -> Option<std::path::PathBuf> {
    std::env::var_os("HOME")
        .or_else(|| std::env::var_os("USERPROFILE"))
        .map(|h| std::path::PathBuf::from(h).join(".e46-mk60-tool"))
}





/// (Re)establish an unlocked (0x7F) K-line session, tolerating a module that is temporarily down
/// (mid power-cycle): open K-line, enter session 0x87, unlock 0x7F - retrying the whole sequence up to
/// 20 times, ~750 ms apart (~15 s total), so the user can finish cycling 2-3x. Logs the "not
/// responding" hint ONCE (not per attempt); the per-attempt unlock chatter is suppressed. Leaves the
/// K-line OPEN on success; returns Err only after the window is exhausted. The common (module-up) case
/// succeeds on the first attempt with no sleep and no extra logging.
fn reestablish_7f(br: &mut Bridge, sink: Sink) -> Result<()> {
    // Generous window (~30 s): a Task-Watchdog reboot + USB re-enumeration + the ECU coming back can
    // take ~20 s together. Each failed pass runs the FULL recovery - reconnect (rides a dongle reboot /
    // re-enumeration) then CMD_KLINE_RESET (hard UART reset, clears the leading-byte desync a plain
    // re-open can't) - before retrying open+session+unlock. The healthy case returns on the first pass
    // with no recovery overhead.
    let deadline = Instant::now() + Duration::from_secs(30);
    let mut warned = false;
    loop {
        if br.kline_open(9600).is_ok() {
            br.kline_tx(&kwp::start_session(kwp::SESSION_CCP)).ok();
            let _ = kline_response(br); // the 50 87 reply is optional; the unlock is the real gate
            let mut quiet = |_: Event| {}; // suppress unlock_level_7f's per-attempt log lines
            if unlock_level_7f(br, false, &mut quiet).unwrap_or(false) {
                return Ok(());
            }
            br.kline_close().ok();
        }
        if Instant::now() >= deadline {
            return Err(anyhow!("dongle/module did not recover within ~30 s - replug the Gauge.S USB-C and re-run."));
        }
        if !warned {
            sink(Event::Log(
                "Not responding (dongle reboot or K-line desync) - recovering. STOP power-cycling so recovery can finish...".into(),
            ));
            warned = true;
        }
        let _ = br.reconnect();   // polls up to ~10 s for the port to re-enumerate after a reboot
        let _ = br.kline_reset(); // CMD_KLINE_RESET - hard UART re-init on the dongle (clears a desync)
        std::thread::sleep(Duration::from_millis(250));
    }
}












// ======================= Bootloader comms: CAN ISO-TP + dual-transport probe =======================
// BMW ISO-TP (the E5 flasher variant): REQUEST on CAN id 0x6F1 with data[0] = ECU address (DSC 0x29)
// and the ISO-TP PCI+payload in data[1..]; RESPONSE on 0x629 (0x600+addr) with data[0] = 0xF1 (tester)
// and the ISO-TP in data[1..]. With the address occupying data[0], a Single Frame carries <=6 payload
// bytes, a First Frame 5, a Consecutive Frame 6.
const ISOTP_REQ_ID: u16 = 0x6F1;
const ISOTP_RESP_ID: u16 = 0x629;
const ISOTP_TESTER: u8 = 0xF1;

/// Read the next CAN frame on `want_id` within the deadline (returns its data bytes), else None.
fn next_can(br: &mut Bridge, want_id: u16, deadline: Instant) -> Result<Option<Vec<u8>>> {
    while Instant::now() < deadline {
        if let Some(f) = br.poll()? {
            if f.cmd == proto::EVT_CAN_RX && f.payload.len() >= 3 {
                let id = (f.payload[0] as u16) | ((f.payload[1] as u16) << 8);
                let dlc = f.payload[2] as usize;
                if id == want_id {
                    return Ok(Some(f.payload[3..(3 + dlc).min(f.payload.len())].to_vec()));
                }
            }
        }
    }
    Ok(None)
}

/// One UDS/KWP request to ECU `addr` over BMW CAN ISO-TP. Sends a Single Frame (request <=6 bytes) and
/// reassembles a single- or multi-frame response (sends Flow Control for a FirstFrame). Returns the UDS
/// response bytes, or None on timeout. The caller must have CAN open (normal mode).
fn isotp_request(br: &mut Bridge, addr: u8, req: &[u8]) -> Result<Option<Vec<u8>>> {
    ensure!(req.len() <= 6, "ISO-TP single-frame request limited to 6 bytes (got {}).", req.len());
    // Single Frame: [addr, 0x0L, payload...] padded to 8.
    let mut sf = vec![addr, req.len() as u8];
    sf.extend_from_slice(req);
    sf.resize(8, 0x00);
    br.can_tx(ISOTP_REQ_ID, &sf)?;

    let deadline = Instant::now() + Duration::from_millis(700);
    loop {
        let frame = match next_can(br, ISOTP_RESP_ID, deadline)? {
            Some(f) if f.first() == Some(&ISOTP_TESTER) && f.len() >= 2 => f,
            Some(_) => continue, // wrong tester byte / too short - keep looking
            None => return Ok(None),
        };
        let pci = frame[1];
        match pci >> 4 {
            0x0 => {
                // Single Frame: len in low nibble, payload in frame[2..2+len].
                let len = (pci & 0x0F) as usize;
                let end = (2 + len).min(frame.len());
                return Ok(Some(frame[2..end].to_vec()));
            }
            0x1 => {
                // First Frame: 12-bit total length, first 5 payload bytes in frame[3..8].
                let total = (((pci & 0x0F) as usize) << 8) | frame.get(2).copied().unwrap_or(0) as usize;
                let mut out: Vec<u8> = frame.get(3..).map(|s| s.to_vec()).unwrap_or_default();
                // Flow Control: [addr, 0x30 (ClearToSend), BS=0, STmin=0] padded.
                let mut fc = vec![addr, 0x30, 0x00, 0x00];
                fc.resize(8, 0x00);
                br.can_tx(ISOTP_REQ_ID, &fc)?;
                let cf_deadline = Instant::now() + Duration::from_millis(1500);
                while out.len() < total {
                    match next_can(br, ISOTP_RESP_ID, cf_deadline)? {
                        Some(f) if f.first() == Some(&ISOTP_TESTER) && f.len() >= 2 && (f[1] >> 4) == 0x2 => {
                            out.extend_from_slice(f.get(2..).unwrap_or(&[]));
                        }
                        Some(_) => continue, // not a Consecutive Frame - ignore
                        None => break,       // timed out mid-reassembly
                    }
                }
                out.truncate(total);
                return Ok(Some(out));
            }
            _ => continue, // FlowControl/stray ConsecutiveFrame without a FirstFrame - ignore
        }
    }
}

/// Which transport a UDS request goes over.
#[derive(Clone, Copy, PartialEq)]
enum Transport {
    Kline,
    Can,
}

/// Transport-agnostic "send a UDS/KWP service, get the response". K-line uses the existing BMW-FAST `B8`
/// framing; CAN uses BMW ISO-TP (0x6F1/0x629). The transport must already be open. Returns the response
/// UDS bytes (service-id/NRC first), or None on no reply.
fn uds_request(br: &mut Bridge, t: Transport, addr: u8, req: &[u8]) -> Result<Option<Vec<u8>>> {
    match t {
        Transport::Kline => {
            br.kline_tx(&kwp::req(req))?;
            Ok(kline_response(br).ok())
        }
        Transport::Can => isotp_request(br, addr, req),
    }
}












/// Build a framed mfg request `[len][payload][sum]` the way `mfg_frame_check` (0x7723e) validates it:
/// `len` = payload length (class + sub + params), and `sum = (len + sum(payload)) & 0xFF` (the firmware's
/// sum loop covers buffer[0..=len], i.e. it includes the length byte). Confirmed against `mfg_frame_tx`
/// (0x773bc), which builds replies as `[len][tag][payload][sum]` with the same sum.
fn mfg_build_frame(payload: &[u8]) -> Vec<u8> {
    let len = payload.len() as u8;
    let sum = payload.iter().fold(len, |a, &b| a.wrapping_add(b));
    let mut f = Vec::with_capacity(payload.len() + 2);
    f.push(len);
    f.extend_from_slice(payload);
    f.push(sum);
    f
}

/// One mfg entry attempt: cycle the ignition, knock `@ 01 02` at `first_delay` ms (paced ~18 ms/byte so each
/// byte clears the one-byte mailbox), send the selection `sel`, and listen. On acceptance into framed mode
/// ('j' = 0x6A, or the 'i' = 0x69 banner) it returns the captured reply and leaves the K-line OPEN and the
/// relay ON - the session is live for framed commands. Otherwise it closes the line, drops the relay, and
/// returns None. The proven entry is `first_delay` ~40 ms at 4800 8N1 with `sel` '0' (no baud switch).
fn mfg_enter_once(
    br: &mut Bridge,
    off_ms: u64,
    threshold_mv: u16,
    first_delay: u64,
    sel: u8,
    baud: u32,
    fmt: u8,
) -> Result<Option<Vec<u8>>> {
    br.relay_set(false)?;
    if br.kline_open_fmt(baud, fmt).is_err() {
        std::thread::sleep(Duration::from_millis(off_ms));
        return Ok(None);
    }
    std::thread::sleep(Duration::from_millis(off_ms));
    let t0 = Instant::now();
    br.relay_set(true)?;
    let ign_deadline = t0 + Duration::from_millis(2000);
    let mut up = false;
    loop {
        if cancel_requested() {
            break;
        }
        if let Ok(mv) = br.adc_read() {
            if mv >= threshold_mv {
                up = true;
                break;
            }
        }
        if Instant::now() >= ign_deadline {
            break;
        }
    }
    if !up || cancel_requested() {
        br.kline_close().ok();
        br.relay_set(false).ok();
        return Ok(None);
    }
    let dt = t0.elapsed().as_millis() as u64;
    if first_delay > dt {
        std::thread::sleep(Duration::from_millis(first_delay - dt));
    }
    const GAP: Duration = Duration::from_millis(18);
    // Knock: on FW >= 0x0015 this runs on the dongle (jitter-free, host-independent); otherwise it streams
    // the bytes from the host (the pre-0x0015 behaviour). Either way we get the captured window back.
    let rx = mfg_knock_window(br, sel, GAP);
    // Accepted into framed mode? 'j' (0x6A, selection accepted) or the 'i' (0x69) banner.
    if rx.iter().any(|&b| b == 0x6A || b == 0x69) {
        Ok(Some(rx)) // leave the K-line OPEN + relay ON - the session is live
    } else {
        br.kline_close().ok();
        br.relay_set(false)?;
        Ok(None)
    }
}



/// Send one already-built mfg frame on an OPEN session (does not enter or close), log the reply with our
/// half-duplex echo stripped, and return the RAW capture (pass it to [`mfg_parse_reply`] with the frame as
/// the echo). Used to send several frames in one session, e.g. the bootloader check.
fn mfg_tx_one(br: &mut Bridge, frame: &[u8], sink: Sink) -> Vec<u8> {
    let mut rx: Vec<u8> = Vec::new();
    kline_settle_before_tx(br); // reuse path: clear a leftover reply so this capture stays frame-aligned
    if br.kline_tx(frame).is_ok() {
        rx = kline_capture_raw(br, Duration::from_millis(50), Duration::from_millis(500)).unwrap_or_default();
    }
    let reply: Vec<u8> = if rx.starts_with(frame) { rx[frame.len()..].to_vec() } else { rx.clone() };
    let pascii: String =
        reply.iter().map(|&b| if (0x20..0x7f).contains(&b) { b as char } else { '.' }).collect();
    sink(Event::Log(format!(
        "  {} -> {} ({pascii})",
        hexs(frame),
        if reply.is_empty() { "(nothing)".into() } else { hexs(&reply) }
    )));
    rx
}

/// Like [`mfg_tx_one`] but SILENT: send a frame on an open session and return the raw capture without
/// logging each exchange. Used by the MMIO sweep, which does one compact line per address instead of the
/// full per-frame K/L/K/O trace.
fn mfg_tx_one_quiet(br: &mut Bridge, frame: &[u8]) -> Vec<u8> {
    kline_settle_before_tx(br); // reuse path: clear a leftover reply so this capture stays frame-aligned
    if br.kline_tx(frame).is_ok() {
        kline_capture_raw(br, Duration::from_millis(50), Duration::from_millis(500)).unwrap_or_default()
    } else {
        Vec::new()
    }
}




/// Parse a mfg reply `[len][marker][tag][payload][sum]` out of a raw capture (which starts with our frame
/// echo). Returns `(tag, payload)`. The reply framing was reverse-engineered on the bench: `len` counts the
/// marker + tag + payload, and `sum = (len + marker + tag + sum(payload)) & 0xFF`.
///
/// The marker is the `0xE4` family: the module sends `0xE4` on most replies but `0xE5` on others (bit 0 is a
/// toggle/flag it sets on some frames - bench-confirmed: an `0xE5` read reply checksums identically with the
/// marker byte included in the sum). We therefore accept any `marker & 0xFE == 0xE4`. Before this, every
/// `0xE5` reply failed to parse and cost a pointless relay re-entry even though the data arrived intact.
///
/// We match the marker at its expected position (tail[1], right after the length byte) first, then fall back
/// to scanning; the checksum verification rejects a false hit on an `0xE4`/`0xE5` data byte inside a payload.
fn mfg_parse_reply(rx: &[u8], echo: &[u8]) -> Option<(u8, Vec<u8>)> {
    // Skip our own echo at the front if present.
    let tail = if rx.starts_with(echo) { &rx[echo.len()..] } else { rx };
    let is_marker = |b: u8| b & 0xFE == 0xE4; // 0xE4 or 0xE5

    // Try each candidate marker position (expected tail[1] first, then any later 0xE4/0xE5), and return the
    // first whose length and checksum are self-consistent.
    let candidates = std::iter::once(1usize)
        .chain((0..tail.len()).filter(|&i| i != 1 && tail.get(i).copied().map(is_marker).unwrap_or(false)));
    for m in candidates {
        if m < 1 || !tail.get(m).copied().map(is_marker).unwrap_or(false) {
            continue;
        }
        let len = tail[m - 1] as usize; // counts marker + tag + payload
        if len < 2 {
            continue;
        }
        let tag = match tail.get(m + 1) {
            Some(&t) => t,
            None => continue,
        };
        let payload_len = len - 2;
        let start = m + 2;
        let payload = match tail.get(start..start + payload_len) {
            Some(p) => p.to_vec(),
            None => continue,
        };
        // Verify the checksum: sum over [len][marker][tag][payload] == the trailing sum byte.
        let frame_start = m - 1;
        let sum_idx = frame_start + 1 + len; // after the len byte + `len` body bytes
        let got = match tail.get(sum_idx) {
            Some(&b) => b,
            None => continue,
        };
        let calc = tail[frame_start..=frame_start + len].iter().fold(0u8, |a, &b| a.wrapping_add(b));
        if got == calc {
            return Some((tag, payload));
        }
    }
    None
}





// ---- Milestone 3: prove arbitrary code execution via the factory long-frame protocol (0xA6 K/L/K/O) ----
// Hardware run #1 proved that STATIC 0xA5 pokes of the level-3 descriptor buffer (0x4011A4+) do NOT
// persist: that buffer is live firmware-managed RAM (telemetry pointers/counters overwrite it between
// staging and the trigger). The factory path instead BUILDS the descriptor with the 0xA6 long-frame
// commands, which claim the buffer: K(init) -> L(append) -> K(finalize) -> O(exec). Pinned from the
// 820.3 non-M disassembly; full derivation in work/m3-klq-longframe-protocol.md.
//
//   K-init     A6 4B "00000000" "10020000"  -> [desc-3]='@'(0x40), length 0 (claims the buffer)
//   L-append   A6 4C 00 00 <stub>           -> raw bytes: desc[0]=0x0000, stub lands at desc+6
//   K-finalize A6 4B <P1> "00000000"        -> byte-sum checksum gate, then [desc-3]=0x80 (exec-ready)
//                  P1 = (sum16(stub) << 16) | (stub.len()+2)
//   O-exec     A6 4F                         -> mfg_rx_service: jsr desc + desc[0] + 6 = desc+6 = stub
//
// The stub runs at desc+6 (0x4011B0) with r2 = desc+2; it must write its marker to ABSOLUTE scratch
// OUTSIDE the descriptor buffer (writing (r2+8) would land in its own bytes and corrupt the return).
const EXEC_CTRL: u32 = 0x0040_11A7; // control byte = descriptor-3; '@'(0x40) filling -> 0x80 exec-ready
const EXEC_MARK: u32 = 0x0040_1400; // M1-proven writable scratch; the stub writes the marker byte here.
// NOTE: 0x401400 is the only address we've round-tripped on hardware (M1), but it falls within the
// level-3 descriptor buffer window (0x4011A4..0x401700). It's far above the staged stub (desc+6..),
// so the stub can't self-corrupt, but if the firmware's buffer machinery reaches it the marker could
// be disturbed. The pre-clear to 0x00 + before/after read distinguishes exec (0x5A) from no-exec
// (0x00) from clobber (other). If it proves unstable, move the marker above the buffer (0x401700+).
const EXEC_MARK_VAL: u8 = 0x5A; // distinctive marker the stub writes

/// The M-CORE stub, uploaded via the 'L' long-frame and executed at desc+6. It builds the absolute
/// address 0x00401400 and stores EXEC_MARK_VAL there, then returns via jmp r15. It uses only r3/r4/r5
/// (the handler saves/restores r13-r15, and the 0xA6 'O' handler only checks r2 != 0 afterward, which
/// the stub leaves untouched at desc+2). 14 bytes, big-endian 16-bit instructions:
///   bgeni r4,0x16    (0x3364)  r4 = 1<<22 = 0x400000
///   movi  r5,0x14    (0x6145)  r5 = 0x14
///   lsli  r5,0x8     (0x3C85)  r5 = 0x1400
///   or    r4,r5      (0x1E54)  r4 = 0x401400
///   movi  r3,0x5A    (0x65A3)  r3 = marker
///   st.b  r3,(r4,0)  (0xB304)  [0x401400] = 0x5A
///   jmp   r15        (0x00CF)
fn exec_stub() -> Vec<u8> {
    vec![0x33, 0x64, 0x61, 0x45, 0x3C, 0x85, 0x1E, 0x54, 0x65, 0xA3, 0xB3, 0x04, 0x00, 0xCF]
}

/// P1 for the K-finalize frame: high halfword = 16-bit byte-sum of the stub (the checksum the finalize
/// validates over desc+6..), low halfword = declared length = stub.len()+2. (The append writes the
/// 2-byte offset header + stub with a 4-byte skipped reserved slot; the finalize's length check is
/// against declared+4 = accumulated, and the checksum range is exactly the stub bytes.)
fn exec_finalize_p1(stub: &[u8]) -> u32 {
    let sum16: u32 = stub.iter().map(|&b| b as u32).sum::<u32>() & 0xFFFF;
    let declared = stub.len() as u32 + 2;
    (sum16 << 16) | declared
}

/// Max stub bytes per `L` frame. The module rejects an oversized framed command (an L frame with a
/// 44-byte stub -> 50-byte wire frame got no reply; a 34-byte stub -> 40-byte frame worked). 24 keeps
/// each L frame small (wire <= ~30 bytes), well under the limit, and matches the 0xA5 'U' 24-byte chunk.
const EXEC_L_CHUNK: usize = 24;

/// The `L`-append frames for `stub`, chunked so no frame is oversized. The FIRST frame carries the 2-byte
/// offset halfword (00 00) then the first chunk; later frames carry raw stub bytes that append
/// contiguously (the descriptor length accumulates; the 4-byte reserved-slot skip only fires on the first).
fn exec_l_frames(stub: &[u8]) -> Vec<Vec<u8>> {
    let mut frames = Vec::new();
    let mut i = 0;
    let mut first = true;
    while first || i < stub.len() {
        let take = EXEC_L_CHUNK.min(stub.len() - i);
        let mut p = vec![0xA6u8, b'L'];
        if first {
            p.push(0x00);
            p.push(0x00); // offset halfword 0 -> jsr lands on desc+6
            first = false;
        }
        p.extend_from_slice(&stub[i..i + take]);
        frames.push(mfg_build_frame(&p));
        i += take;
    }
    frames
}

/// Build the frames of the K/L/K/O exec sequence for `stub`: K-init, the (chunked) L-append frames,
/// K-finalize, O-exec. Pure (no I/O) so it is unit-testable.
fn exec_frames(stub: &[u8]) -> (Vec<u8>, Vec<Vec<u8>>, Vec<u8>, Vec<u8>) {
    let k_init = {
        let mut p = vec![0xA6u8, b'K'];
        p.extend_from_slice(b"00000000"); // P1 (not checked on the '0' init slot)
        p.extend_from_slice(b"10020000"); // P2: char0='1' (nonzero), slot char1='0', table id "02"
        mfg_build_frame(&p)
    };
    let l_frames = exec_l_frames(stub);
    let k_final = {
        let mut p = vec![0xA6u8, b'K'];
        p.extend_from_slice(format!("{:08X}", exec_finalize_p1(stub)).as_bytes()); // P1 = len+checksum
        p.extend_from_slice(b"00000000"); // P2 char0='0' -> finalize branch (stack[0]==0)
        mfg_build_frame(&p)
    };
    let o_exec = mfg_build_frame(&[0xA6u8, b'O']);
    (k_init, l_frames, k_final, o_exec)
}

/// Build a `0xA5 'T'` raw-write frame (address as 8 hex chars, then the raw bytes) for an open session.
fn exec_write_frame(addr: u32, bytes: &[u8]) -> Vec<u8> {
    let mut p = vec![0xA5u8, b'T'];
    p.extend_from_slice(format!("{addr:08X}").as_bytes());
    p.extend_from_slice(bytes);
    mfg_build_frame(&p)
}


// ---- Flash-write sanity probe: does a PLAIN CPU store program flash? (expected: NO) ----
// Reuses the proven K/L/K/O exec path, but the uploaded stub, after writing the exec marker, attempts a
// plain `st.b` to an ERASED flash byte (0x3E8B4 = 0xFF). NOR flash needs the controller's unlock+program
// command sequence; a lone store should leave it 0xFF. If it changes, a plain store programs flash (a big
// surprise that would shortcut the whole flash-controller reverse). 0x3E8B4 is inside the boot-checksum
// range, but writing an erased byte there can at most clear bits; a failed checksum degrades ABS without
// bricking the diag door (see work/poc-append-byte-assessment.md), and we expect no change anyway.



// ---- Flash-controller register probe (M5a hw base-confirm): 0xA4 can't read MMIO (width-restricted
// block read -> no reply), so an uploaded stub does proper-width CPU loads of the flash registers into
// scratch RAM, which 0xA4 reads cleanly. Reads CHIPID (0xFF0006, halfword), MCR (0xF60000, word) and
// LMLR (0xF60004, word). Read-only; confirms the SC550080 H7Fa flash controller base on the non-M.
const REG_SCRATCH: u32 = 0x0040_1400; // s55 @+0, value @+4 (word), s66 @+8

/// Push `movi into,(byte&0x7F)` and, for a high byte, `bseti into,7` - so any 0..0xFF lands in `into`.
fn mcore_push_byte(out: &mut Vec<u16>, into: u8, byte: u8) {
    out.push(0x6000 | ((byte as u16 & 0x7F) << 4) | into as u16); // movi into, byte&0x7F
    if byte & 0x80 != 0 {
        out.push(0x3400 | (7u16 << 4) | into as u16); // bseti into, 7
    }
}

/// Emit M-CORE (big-endian halfword) code that loads the 32-bit `addr` into `reg`, using `tmp` as scratch.
/// General: builds the address byte by byte (MSB first), skipping leading zero bytes.
fn mcore_build_u32(reg: u8, tmp: u8, addr: u32) -> Vec<u8> {
    let mut w: Vec<u16> = Vec::new();
    let mut started = false;
    for &byte in &addr.to_be_bytes() {
        if !started {
            if byte == 0 {
                continue;
            }
            mcore_push_byte(&mut w, reg, byte);
            started = true;
        } else {
            w.push(0x3C00 | (8u16 << 4) | reg as u16); // lsli reg, 8
            if byte != 0 {
                mcore_push_byte(&mut w, tmp, byte);
                w.push(0x1E00 | ((tmp as u16) << 4) | reg as u16); // or reg, tmp
            }
        }
    }
    if !started {
        w.push(0x6000 | reg as u16); // movi reg, 0
    }
    w.iter().flat_map(|x| [(x >> 8) as u8, *x as u8]).collect()
}

/// Single-address MMIO read probe. Builds `addr` into r4, reads it at `width` (1=byte, 2=halfword,
/// 4=word), stores the value to scratch +4, with a sentinel before (ran) and after (read survived). One
/// address per run so a faulting/hanging probe is isolated. The non-M peripheral registers are 16-bit,
/// so a WORD read is the wrong width and FAULTS - halfword (2) is the right default. r2 untouched.
/// When `elevate` is set, inserts `mfcr r1,ss4` (0x10A1) after the zeroing block so the 818.3 privilege
/// gate raises S before the load - READ-ONLY (no flash program/erase).
///   r5 = 0x401400 ; sentinel 0x55 -> +0 ; ZERO +1..+8 ; [optional 10A1] ; build addr in r4 ; ld.X ;
///   st.X -> +4 ; sentinel 0x66 -> +8 ; jmp r15
/// The stub zeroes +1..+8 itself so the whole 9-byte result block is STUB-determined, not reliant on the
/// caller's scratch pre-clear - which does not reliably land on a REUSED (keep-alive) session, leaving
/// frame-fragment garbage in the don't-care bytes. With this, a fault reads a clean `55 00..00` and a mapped
/// read `55 00 00 00 <value> .. 66`, identical fresh or reused.
fn mmio_probe_stub(addr: u32, width: u8, elevate: bool) -> Vec<u8> {
    let mut s: Vec<u8> = vec![
        0x33, 0x65, 0x61, 0x46, 0x3C, 0x86, 0x1E, 0x65, // r5 = 0x401400
        0x65, 0x53, 0xB3, 0x05, // sentinel 0x55 -> +0
        // zero +1..+8 (r3=0): st.b +1/+2/+3, st.w +4..+7, st.b +8. The value/0x66 below overwrite what they
        // use; on a fault the ld traps and these zeros remain, giving a clean 55 00..00.
        0x60, 0x03, // movi r3, 0
        0xB3, 0x15, // st.b r3,(r5,1)
        0xB3, 0x25, // st.b r3,(r5,2)
        0xB3, 0x35, // st.b r3,(r5,3)
        0x93, 0x15, // st.w r3,(r5,1) -> zeros +4..+7
        0xB3, 0x85, // st.b r3,(r5,8)
    ];
    // Proven 818.3 user->supervisor gate: mfcr r1,ss4 (0x10A1). The privilege-violation handler accepts
    // only this insn, sets S in EPSR, bumps EPC, and rte's so the next insn runs supervisor. Safe if already
    // supervisor (mfcr just reads ss4 into r1; r1 is unused by the rest of this stub). Injected per probe so
    // we never assume S sticks across a session reuse / re-knock. READ-ONLY after this - no SGFM writes.
    if elevate {
        s.extend_from_slice(&[0x10, 0xA1]); // mfcr r1, ss4
    }
    s.extend_from_slice(&mcore_build_u32(4, 6, addr)); // r4 = addr (scratch r6)
    match width {
        4 => s.extend_from_slice(&[0x83, 0x04, 0x93, 0x15]), // ld.w r3,(r4,0) ; st.w r3,(r5,1)   -> +4 (word)
        1 => s.extend_from_slice(&[0xA3, 0x04, 0xB3, 0x45]), // ld.b r3,(r4,0) ; st.b r3,(r5,4)   -> +4 (byte)
        _ => s.extend_from_slice(&[0xC3, 0x04, 0xD3, 0x25]), // ld.h r3,(r4,0) ; st.h r3,(r5,2hw) -> +4 (halfword)
    }
    s.extend_from_slice(&[0x66, 0x63, 0xB3, 0x85]); // movi r3,0x66 ; st.b r3,(r5,8) -> +8 read-survived sentinel
    s.extend_from_slice(&[0x00, 0xCF]); // jmp r15
    s
}

/// Single-address MMIO read probe. Enters, raises to level 3, pre-clears scratch, runs the K/L/K/O exec
/// with a stub that reads `addr` (word unless `byte`), then reads back the value + sentinels. One address
/// per run so a faulting/hanging probe is isolated - hold the full dump; a hang needs a KL30 power cycle.
/// Use to hunt the non-M flash-controller registers (not in the app image) and to sanity-check known
/// peripherals (0xFF8032 reads 0x00D8). Read-only.
#[allow(clippy::too_many_arguments)]
pub fn mfg_mmio_read(
    br: &mut Bridge,
    off_ms: u64,
    threshold_mv: u16,
    first_delay: u64,
    attempts: u64,
    sel: u8,
    baud: u32,
    fmt: u8,
    auto_ign: bool,
    addr: u32,
    width: u8,
    elevate: bool,
    sink: Sink,
) -> Result<()> {
    let width = if matches!(width, 1 | 2 | 4) { width } else { 2 };
    let wname = match width {
        1 => "byte",
        4 => "word",
        _ => "halfword",
    };
    let stub = mmio_probe_stub(addr, width, elevate);
    let (k_init, l_frames, k_final, o_exec) = exec_frames(&stub);
    sink(Event::Log(format!(
        "mfg mmio-read 0x{addr:08X} ({wname}{}): stub {} (P1=0x{:08X}). Read-only; one address, isolated. A hang needs a KL30 power cycle.",
        if elevate { ", elevated 10A1" } else { "" },
        hexs(&stub),
        exec_finalize_p1(&stub)
    )));
    if mfg_enter(br, off_ms, threshold_mv, first_delay, attempts, sel, baud, fmt, auto_ign, sink)?.is_none() {
        br.relay_set(false).ok();
        sink(Event::Log("mfg mmio-read: could not enter the session.".into()));
        return Ok(());
    }
    std::thread::sleep(Duration::from_millis(200));

    let raise3 = mfg_build_frame(&[0xA3u8, b'3', 0x00, 0x00, b'1', b'6']);
    let rr = mfg_tx_one(br, &raise3, sink);
    match mfg_parse_reply(&rr, &raise3) {
        Some((0x66, p)) => sink(Event::Log(format!("  level raise: level {}", p.iter().map(|&b| b as char).collect::<String>()))),
        Some((t, _)) => sink(Event::Log(format!("  level raise: tag 0x{t:02X} (expected 'f'); level-3 may have been refused."))),
        None => sink(Event::Log("  level raise: no parseable reply; level-3 may have been refused.".into())),
    }

    // pre-clear scratch so stale data can't masquerade as a register value / sentinel
    let _ = mfg_tx_one(br, &exec_write_frame(REG_SCRATCH, &[0u8; 12]), sink);

    sink(Event::Log("  K-init / L-append / K-finalize:".into()));
    log_frame_reply(br, &k_init, sink);
    for lf in &l_frames {
        log_frame_reply(br, lf, sink);
    }
    log_frame_reply(br, &k_final, sink);
    if let Some((0x72, got)) = mfg_read_reply(br, EXEC_CTRL, 1) {
        sink(Event::Log(format!("  control byte @0x{EXEC_CTRL:08X}: {} ({})", hexs(&got), if got.first() == Some(&0x80) { "0x80 exec-ready" } else { "NOT 0x80" })));
    }

    sink(Event::Log("  O-exec (trigger jsr):".into()));
    let _ = mfg_tx_one(br, &o_exec, sink);
    std::thread::sleep(Duration::from_millis(100));
    let regs = mfg_read_reply(br, REG_SCRATCH, 9);

    mfg_session_teardown(br, sink);

    match &regs {
        Some((0x72, b)) if b.len() >= 9 => {
            sink(Event::Log(format!("  scratch 0x{REG_SCRATCH:08X} x9: {}", hexs(b))));
            let (ran, read_ok) = (b[0] == 0x55, b[8] == 0x66);
            if !ran {
                sink(Event::Log("  STUB DID NOT RUN: sentinel 0x55 absent at +0.".into()));
                return Ok(());
            }
            sink(Event::Log("  stub ran (0x55 @+0).".into()));
            if read_ok {
                let shown = match width {
                    1 => format!("0x{:02X} (byte)", b[4]),
                    4 => format!("0x{:08X} (word)", ((b[4] as u32) << 24) | ((b[5] as u32) << 16) | ((b[6] as u32) << 8) | b[7] as u32),
                    _ => format!("0x{:04X} (halfword)", ((b[4] as u16) << 8) | b[5] as u16),
                };
                sink(Event::Log(format!("  0x{addr:08X} = {shown}  [read OK - 0x66 @+8]. Address is MAPPED and readable.")));
            } else {
                sink(Event::Log(format!("  0x{addr:08X}: READ FAULTED ({wname}; 0x55 ran, no 0x66 @+8) - unmapped, OR the wrong width (non-M peripherals are 16-bit; a WORD read faults - retry as halfword/byte){}. Session survived (trapped, not hung).",
                    if elevate { " — elevate ran (10A1), so this is NOT a privilege wall" } else { ", OR supervisor-only (try elevated sweep)" }
                )));
            }
        }
        Some((0x72, b)) => sink(Event::Log(format!("  scratch read-back short ({} bytes): {}", b.len(), hexs(b)))),
        Some((t, b)) => sink(Event::Log(format!("  scratch read-back tag 0x{t:02X}: {}", hexs(b)))),
        None => sink(Event::Log(format!("  0x{addr:08X}: no scratch reply - the session dropped (a bad MMIO read can fault/HANG; if the module is wedged, power-cycle KL30 to recover).", ))),
    }
    Ok(())
}

/// Result of one quiet MMIO exec-read on an already-entered, live session.
enum MmioOutcome {
    Mapped(u32), // value, masked to width
    Faulted,     // stub ran (0x55) but no 0x66 - unmapped or supervisor-only or wrong width; session survived
    NoRun,       // 0x55 absent / odd reply - stub did not run (bad stage or session glitch)
    NoReply,     // no scratch reply at all - session dropped / possible HANG (needs KL30)
}

/// Stage + exec + read ONE MMIO probe on an open, level-3 session, SILENTLY (no per-frame logging). Pre-
/// clears scratch, runs the K/L/K/O exec with the width-specific read stub, reads back the 9-byte scratch,
/// and classifies it. Does not enter or tear down the session - the caller owns that. Used by the sweep.
fn mmio_probe_once(br: &mut Bridge, addr: u32, width: u8, elevate: bool) -> MmioOutcome {
    let stub = mmio_probe_stub(addr, width, elevate);
    let (k_init, l_frames, k_final, o_exec) = exec_frames(&stub);
    let _ = mfg_tx_one_quiet(br, &exec_write_frame(REG_SCRATCH, &[0u8; 12])); // pre-clear scratch
    let _ = mfg_tx_one_quiet(br, &k_init);
    for lf in &l_frames {
        let _ = mfg_tx_one_quiet(br, lf);
    }
    let _ = mfg_tx_one_quiet(br, &k_final);
    let _ = mfg_tx_one_quiet(br, &o_exec);
    std::thread::sleep(Duration::from_millis(100));
    match mfg_read_reply(br, REG_SCRATCH, 9) {
        Some((0x72, b)) if b.len() >= 9 => {
            if b[0] != 0x55 {
                MmioOutcome::NoRun
            } else if b[8] != 0x66 {
                MmioOutcome::Faulted
            } else {
                let v = match width {
                    1 => b[4] as u32,
                    4 => ((b[4] as u32) << 24) | ((b[5] as u32) << 16) | ((b[6] as u32) << 8) | b[7] as u32,
                    _ => ((b[4] as u32) << 8) | b[5] as u32,
                };
                MmioOutcome::Mapped(v)
            }
        }
        Some((0x72, _)) => MmioOutcome::NoRun,
        Some(_) => MmioOutcome::NoRun,
        None => MmioOutcome::NoReply,
    }
}

/// FAST byte probe: a single `0xA4 'U'` len-1 read on an open, live session - no exec stub, so it skips the
/// whole K/L/K/O descriptor dance and runs far faster than `mmio_probe_once` (the exec path's cost is that
/// dance, not the read). BYTE WIDTH ONLY: `0xA4` reads raw bytes, so halfword/word-restricted registers that
/// only answer a native-width access still need the exec path. Recovery mirrors `mfg_find_data_top`: on a
/// miss, dongle-side flush + one retry. Classification: tag 'r' with data = Mapped; any other tag = the
/// session is alive but the address is not readable (Faulted); no reply at all after the retry = NoReply (a
/// width-restricted hole, or a dropped/hung session - the sweep treats a run of these as a soft hang warning
/// rather than a hard stop, since for `0xA4` an unreadable address is normal, not a session death).
fn mmio_probe_once_a4(br: &mut Bridge, addr: u32) -> MmioOutcome {
    let reply = mfg_read_reply(br, addr, 1).or_else(|| {
        br.kline_reset().ok();
        std::thread::sleep(Duration::from_millis(10));
        mfg_read_reply(br, addr, 1)
    });
    match reply {
        Some((0x72, b)) if !b.is_empty() => MmioOutcome::Mapped(b[0] as u32),
        Some(_) => MmioOutcome::Faulted,
        None => MmioOutcome::NoReply,
    }
}

/// A mapped register is "notable" when it is neither all-zero nor all-ones for its width - i.e. it carries
/// some set bits that aren't just a fully-erased/empty reg. A flash MCR idle value (DONE set, PGM/ERS clear)
/// is notable; so are lock/status regs. This is only a hint to flag candidates in a sweep, not a claim.
fn mmio_notable(v: u32, width: u8) -> bool {
    let full: u32 = match width {
        1 => 0xFF,
        4 => 0xFFFF_FFFF,
        _ => 0xFFFF,
    };
    v != 0 && v != full
}

/// Sweep a RANGE of MMIO addresses as width-`width` reads, ONE exec per address, reusing a single live
/// session (keep-alive) so the whole range costs one ignition knock instead of one per address. Logs a
/// compact line per address and a summary of the MAPPED ones (flagging notable, non-0/non-FF values) at the
/// end. Stops on a HANG (no scratch reply) - that needs a KL30 power cycle - so a sweep is resumed from the
/// next address. Read-only: it only reads registers. Hunts the non-M flash-controller block the app never
/// touches. `step` is the address stride (defaults to the width); `end` is inclusive.
/// When `elevate` is true, each exec stub injects the proven `10A1` (mfcr r1,ss4) supervisor gate before the
/// load; `fast` is forced off (0xA4 cannot elevate). A FAULT then means wrong/protected map, not privilege.
#[allow(clippy::too_many_arguments)]
pub fn mfg_mmio_sweep(
    br: &mut Bridge,
    off_ms: u64,
    threshold_mv: u16,
    first_delay: u64,
    attempts: u64,
    sel: u8,
    baud: u32,
    fmt: u8,
    auto_ign: bool,
    start: u32,
    end: u32,
    step: u32,
    width: u8,
    fast: bool,
    elevate: bool,
    sink: Sink,
) -> Result<()> {
    // Fast mode is a single 0xA4 'U' byte read per address (no exec stub): far quicker, but byte-width only.
    // Elevated mode always uses the exec stub (0xA4 cannot inject 10A1), so elevate forces fast off.
    let fast = fast && !elevate;
    let width = if fast { 1 } else if matches!(width, 1 | 2 | 4) { width } else { 2 };
    let step = if step == 0 { width as u32 } else { step };
    let wname = match width {
        1 => "byte",
        4 => "word",
        _ => "halfword",
    };
    let fmt_val = |v: u32| match width {
        1 => format!("0x{v:02X}"),
        4 => format!("0x{v:08X}"),
        _ => format!("0x{v:04X}"),
    };
    // Soft hang warning after this many consecutive no-replies in fast mode (a real hang vs a big unmapped gap).
    const FAST_HANG_WARN: u32 = 32;
    let mut consec_noreply: u32 = 0;
    // Force keep-alive for the sweep so address N>1 reuses the session; restore the prior setting after.
    let prev_keep = keep_session_alive();
    set_keep_session_alive(true);
    // Skip the per-address reuse exec-probe for BOTH modes - it is the real cost and it is redundant here.
    // Fast reads with 0xA4 (no exec at all). Non-fast runs its own exec stub whose result block is fully
    // stub-written (0x55 sentinel + self-zeroed bytes), so a stale reused session surfaces as NoRun from the
    // op itself; we re-knock on that below instead of paying a whole second exec sequence to pre-check. The
    // probe stays ON for every other (single-shot) exec op via the global default. Restore after the sweep.
    let prev_exec_probe = reuse_exec_probe();
    set_reuse_exec_probe(false);
    let mode = if elevate {
        ", elevated 10A1 (supervisor before each ld)"
    } else if fast {
        ", fast 0xA4 read"
    } else {
        ""
    };
    sink(Event::Log(format!(
        "mfg mmio-sweep: 0x{start:08X}..=0x{end:08X} step 0x{step:X} ({wname}{mode}). One knock, session reused per address. {}",
        if fast {
            "An unreadable address just advances; cancel + KL30 if the module hangs (e.g. 0xEF8xxx)."
        } else if elevate {
            "Each probe injects mfcr r1,ss4 then a read-only ld. FAULT after elevate = wrong/protected map, not privilege. A HANG stops the sweep (KL30; resume from the next address)."
        } else {
            "A HANG stops the sweep (KL30 to recover, resume from the next address)."
        }
    )));
    let raise3 = mfg_build_frame(&[0xA3u8, b'3', 0x00, 0x00, b'1', b'6']);
    let mut mapped: Vec<(u32, u32)> = Vec::new();
    let mut hung_at: Option<u32> = None;
    let mut addr = start;
    // Per-address: true once we've already dropped+re-knocked this address after a NoRun, so a second NoRun
    // is a real glitch (skip) rather than another re-knock loop. Reset when we advance to a new address.
    let mut norun_reknocked = false;
    loop {
        if cancel_requested() {
            sink(Event::Log("mfg mmio-sweep: cancelled.".into()));
            break;
        }
        if mfg_enter(br, off_ms, threshold_mv, first_delay, attempts, sel, baud, fmt, auto_ign, sink)?.is_none() {
            sink(Event::Log(format!("  0x{addr:08X}: could not enter/reuse the session - stopping.")));
            break;
        }
        let _ = mfg_tx_one_quiet(br, &raise3); // ensure level 3 (idempotent; reuse already raised it)
        let outcome = if fast { mmio_probe_once_a4(br, addr) } else { mmio_probe_once(br, addr, width, elevate) };
        match outcome {
            MmioOutcome::Mapped(v) => {
                consec_noreply = 0;
                let notable = mmio_notable(v, width);
                sink(Event::Log(format!(
                    "  0x{addr:08X} = {}  MAPPED{}",
                    fmt_val(v),
                    if notable { "  <- notable (non-0/non-FF)" } else { "" }
                )));
                mapped.push((addr, v));
            }
            MmioOutcome::Faulted => {
                consec_noreply = 0;
                sink(Event::Log(format!(
                    "  0x{addr:08X}  FAULT{}",
                    if elevate {
                        " (unmapped/wrong width/protected — elevate ran, not a privilege wall)"
                    } else {
                        " (unmapped or supervisor-only)"
                    }
                )));
            }
            MmioOutcome::NoRun if !norun_reknocked => {
                // Stub did not run: on a REUSED session this means descriptor-exec went stale (the case the
                // old per-address exec-probe used to pre-catch). Drop the session so the next mfg_enter does a
                // full knock - the KL15 reboot re-arms exec cleanly - and retry THIS address once before
                // giving up on it. Without this a stale reuse would NoRun every address until the 14 s timeout.
                consec_noreply = 0;
                sink(Event::Log(format!("  0x{addr:08X}  stub did not run - dropping session to re-knock, retrying this address")));
                mfg_end_session(br, sink).ok();
                norun_reknocked = true;
                continue; // retry the same address (do not advance)
            }
            MmioOutcome::NoRun => {
                consec_noreply = 0;
                sink(Event::Log(format!("  0x{addr:08X}  stub did not run after a fresh knock (genuine glitch) - skipping")));
            }
            MmioOutcome::NoReply if fast => {
                // In fast (0xA4) mode an unreadable address is normal, not a session death, so keep going;
                // only warn once if a long run suggests the module actually hung.
                consec_noreply += 1;
                sink(Event::Log(format!("  0x{addr:08X}  no-reply (unreadable / width-restricted hole)")));
                if consec_noreply == FAST_HANG_WARN {
                    sink(Event::Log(format!(
                        "  (note: {FAST_HANG_WARN}+ consecutive no-replies - if the module has HUNG (e.g. 0xEF8xxx) cancel and KL30; otherwise this is just an unmapped stretch)"
                    )));
                }
            }
            MmioOutcome::NoReply => {
                sink(Event::Log(format!("  0x{addr:08X}  NO REPLY - possible HANG. Stopping. KL30 power-cycle, then resume from 0x{:08X}.", addr.wrapping_add(step))));
                hung_at = Some(addr);
                break;
            }
        }
        if addr >= end || end - addr < step {
            break; // reached the (inclusive) end, or the next step would pass it
        }
        norun_reknocked = false; // new address: allow one re-knock retry again
        addr += step;
    }
    // Power the module down on a clean finish; on a hang leave it as-is for the operator's KL30 cycle.
    if hung_at.is_none() {
        mfg_end_session(br, sink).ok();
    }
    set_keep_session_alive(prev_keep);
    set_reuse_exec_probe(prev_exec_probe);
    sink(Event::Log(format!("mfg mmio-sweep: done. {} mapped address(es) in range:", mapped.len())));
    for (a, v) in &mapped {
        sink(Event::Log(format!("    0x{a:08X} = {}{}", fmt_val(*v), if mmio_notable(*v, width) { "  (notable)" } else { "" })));
    }
    if let Some(a) = hung_at {
        sink(Event::Log(format!("  NOTE: stopped on a hang at 0x{a:08X}. KL30 power-cycle; resume with --start 0x{:08X}.", a.wrapping_add(step))));
    }
    Ok(())
}

/// Send one framed command on an open session and log its parsed reply (tag + ascii), for the K/L/K/O
/// steps. Returns nothing - the per-step read-backs carry the real verification.
fn log_frame_reply(br: &mut Bridge, frame: &[u8], sink: Sink) {
    let raw = mfg_tx_one(br, frame, sink);
    match mfg_parse_reply(&raw, frame) {
        Some((tag, p)) => {
            let a: String = p.iter().map(|&b| if (0x20..0x7f).contains(&b) { b as char } else { '.' }).collect();
            sink(Event::Log(format!("    reply tag 0x{tag:02X} ('{}') {} ({a})", if (0x20..0x7f).contains(&tag) { tag as char } else { '.' }, hexs(&p))));
        }
        None => sink(Event::Log("    no parseable reply".into())),
    }
}

/// Send a 0xA4 'U' read on an ALREADY-OPEN, live mfg session and return the parsed `(tag, data)` reply, or
/// None if there was no parseable reply (the session likely dropped). Tag 0x72 ('r') = a good read; ANY
/// other tag (e.g. 'c' negative) means the address wasn't readable but THE SESSION IS STILL ALIVE - the
/// caller must advance, not re-enter. Distinguishing these is what keeps a scan from looping forever on an
/// unreadable address.
fn mfg_read_reply(br: &mut Bridge, addr: u32, len: u8) -> Option<(u8, Vec<u8>)> {
    mfg_read_reply_raw(br, addr, len).0
}

/// Like `mfg_read_reply` but also returns the raw captured bytes, so the caller can log exactly what the
/// module sent (nothing / partial frame / error tag) when a read fails. Used by the diagnostic dump path.
fn mfg_read_reply_raw(br: &mut Bridge, addr: u32, len: u8) -> (Option<(u8, Vec<u8>)>, Vec<u8>) {
    let mut payload = vec![0xA4u8, b'U'];
    payload.extend_from_slice(format!("{addr:08X}").as_bytes());
    payload.extend_from_slice(format!("{:08X}", len as u32).as_bytes());
    let frame = mfg_build_frame(&payload);
    kline_settle_before_tx(br); // reuse path: clear a leftover reply so this read stays frame-aligned
    if br.kline_tx(&frame).is_err() {
        return (None, Vec::new());
    }
    let rx = kline_capture_raw(br, Duration::from_millis(40), Duration::from_millis(400)).unwrap_or_default();
    let parsed = mfg_parse_reply(&rx, &frame);
    (parsed, rx)
}


/// Enter the mfg session over a K+DCAN cable, which can neither switch nor (usually) sense the ignition.
///
/// Opens the K-line at 4800 8N1 and streams the knock `@ 01 02 <sel>` paced ~18 ms/byte (one byte per
/// mailbox tick), CONTINUOUSLY. Per the E46 recipe (nonm_mk60_image.md): the driver has no first-byte
/// deadline, only the ~130 ms window, and a wrong-position byte merely zeroes the match count - so a
/// continuous stream catches whichever window the operator opens by cycling the key. We never pause to
/// "listen": the reply ('j' 0x6A accepted / 'i' 0x69 banner) is caught in the next sequence's inter-byte
/// captures via a bounded rolling RX buffer. `0x64` is the knock-ack (close, not acceptance).
///
/// With `auto_ign` and a cable that wires OBD pin 1 to a modem line, we wait for the KL15 rising edge (the
/// window re-arms via a software re-boot ~1 tick after it) then burst-stream for ~0.8 s to cover the
/// window-open jitter, rather than firing once. The ignition state is primed from the current line so an
/// already-on key isn't read as a fresh edge. Returns the entry reply with the K-line left OPEN on success,
/// None on cancel. Cancellation is the global `cancel_requested()`.
fn mfg_enter_cable(br: &mut Bridge, _first_delay: u64, sel: u8, auto_ign: bool, sink: Sink) -> Result<Option<Vec<u8>>> {
    br.kline_open_fmt(4800, 1)?; // 4800 8N1
    // macOS: the VCP driver pins the FTDI latency timer at 16 ms, which makes the knock's selection byte miss
    // the module's ~130 ms window (echo round-trip ~13 ms). Drop it to 1 ms via an EP0 control transfer AFTER
    // the port is configured (the Apple driver blocks claiming an interface, but accepts this device-level
    // request) - confirmed to take the round-trip to ~3 ms. Best-effort: a failure just leaves the default.
    // Linux handles latency via sysfs and Windows via the FTDI driver, so this is macOS-only.
    #[cfg(target_os = "macos")]
    {
        let mut quiet = |_s: String| {};
        if crate::ftdi::set_latency_no_claim(1, &mut quiet).is_ok() {
            sink(Event::Log("  FTDI latency timer -> 1 ms (macOS EP0 trick; echo round-trip ~3 ms).".into()));
        }
    }
    // Windows: we can't change the latency timer under the VCP driver, but we CAN read it from the registry
    // and warn when it isn't 1 ms - that's the #1 cause of a cable knock that never seats on Windows.
    #[cfg(windows)]
    {
        let port = br.path().to_string();
        match crate::ftdi::latency_timer_for_port(&port) {
            Some(ms) if ms <= 2 => {
                sink(Event::Log(format!("  FTDI latency timer = {ms} ms on {port} (good for the cable knock).")));
            }
            Some(ms) => {
                sink(Event::Log(format!(
                    "  WARNING: FTDI latency timer = {ms} ms on {port}. The cable knock needs ~1 ms to seat in \
                     the module's ~130 ms window, so entry will likely fail. Fix it in Device Manager: Ports \
                     (COM & LPT) -> {port} -> Properties -> Port Settings -> Advanced -> Latency Timer -> 1, \
                     then unplug/replug the cable."
                )));
            }
            None => {
                sink(Event::Log(format!(
                    "  (couldn't read the FTDI latency timer for {port} from the registry; if cable entry is \
                     flaky, set it to 1 ms in Device Manager -> {port} -> Advanced -> Latency Timer.)"
                )));
            }
        }
    }
    const GAP: Duration = Duration::from_millis(18); // one-byte mailbox: >= 1 tick (15-20 ms), per the recipe

    if auto_ign {
        // Prime from the current line so an already-on key isn't read as a fresh edge.
        let mut last_high = br.modem_lines().map(|l| l.ignition()).unwrap_or(false);
        sink(Event::Log(if last_high {
            "mfg cable entry: auto-ignition on, but the ignition is ALREADY ON. Cycle it now (OFF, then ON) to arm the knock. Watching modem lines (CTS/DSR/CD/RI)...".into()
        } else {
            "mfg cable entry: auto-ignition on. Turn the ignition ON; the knock bursts on the sensed edge. Watching modem lines (CTS/DSR/CD/RI)...".to_string()
        }));
        let mut last_line_log = Instant::now() - Duration::from_secs(10);
        // SWEEP the lead across key cycles. The cable senses the edge off the FTDI modem line (DSR), which
        // lags the true KL15 edge by a variable amount - worse on a car where the key makes KL15 rise slowly -
        // so a fixed lead puts the @ 01 02 match at a variable spot in the ~130 ms window. Now that the
        // latency fix dropped the round-trip to ~3 ms the selection CAN seat, but only when the match lands
        // early enough. We can't measure the arm point, so try a different lead each cycle until one enters;
        // the log names the winner. (Pre-latency-fix this didn't help - the selection missed at every lead.)
        // STICKY: if a prior entry (this session) found a good lead, try it FIRST, then fall into the sweep -
        // so a re-entry mid-dump doesn't re-roll the whole sweep. The order wraps, so it keeps trying.
        const LEAD_SWEEP_MS: [u64; 8] = [30, 24, 18, 12, 6, 0, 27, 15];
        let mut order: Vec<u64> = Vec::with_capacity(LEAD_SWEEP_MS.len() + 1);
        let sticky = LAST_GOOD_CABLE_LEAD_MS.load(Ordering::Relaxed);
        if sticky != u64::MAX {
            order.push(sticky);
        }
        order.extend_from_slice(&LEAD_SWEEP_MS);
        let mut lead_i = 0usize;
        loop {
            if cancel_requested() {
                return Ok(None);
            }
            let lines = br.modem_lines().unwrap_or_default();
            let high = lines.ignition();
            if last_line_log.elapsed() >= Duration::from_secs(2) {
                let hint = if high { "ignition ON - cycle it OFF then ON to knock" } else { "waiting for ignition ON" };
                sink(Event::Log(format!(
                    "  ign lines: CTS={} DSR={} CD={} RI={} ({hint})",
                    lines.cts as u8, lines.dsr as u8, lines.cd as u8, lines.ri as u8
                )));
                last_line_log = Instant::now();
            }
            let rising = high && !last_high;
            last_high = high;
            if !rising {
                std::thread::sleep(Duration::from_millis(4));
                continue;
            }
            let from_sticky = lead_i == 0 && sticky != u64::MAX;
            let lead = order[lead_i % order.len()];
            lead_i += 1;
            let tag = if from_sticky { "last good" } else { "sweep" };
            sink(Event::Log(format!("  ignition edge sensed; +{lead} ms lead ({tag}), knock burst.")));
            if lead > 0 {
                std::thread::sleep(Duration::from_millis(lead));
            }
            let deadline = Instant::now() + Duration::from_millis(300);
            while Instant::now() < deadline {
                if cancel_requested() {
                    return Ok(None);
                }
                let seq = cable_stream_knock(br, sel, GAP);
                if seq.iter().any(|&b| b == 0x6A || b == 0x69) {
                    LAST_GOOD_CABLE_LEAD_MS.store(lead, Ordering::Relaxed); // remember it for next entry
                    sink(Event::Log(format!("mfg cable entry: ACCEPTED at +{lead} ms lead. Session live.")));
                    return Ok(Some(seq));
                }
                if seq.contains(&0x64) {
                    if let Some(win) = cable_resolve_ack(br, &seq, sink) {
                        LAST_GOOD_CABLE_LEAD_MS.store(lead, Ordering::Relaxed); // remember it for next entry
                        sink(Event::Log(format!("mfg cable entry: ACCEPTED (settle) at +{lead} ms lead. Session live.")));
                        return Ok(Some(win));
                    }
                    let _ = br.kline_reset(); // drain backlog so a stale ack doesn't accumulate
                }
            }
            sink(Event::Log(format!("  no acceptance at +{lead} ms; cycle the ignition again (sweep continues).")));
        }
    } else {
        sink(Event::Log(
            "mfg cable entry: streaming the knock at 4800 8N1 - cycle the ignition OFF then ON (repeatedly) and leave this running; on the knock-ack it STOPS and listens quietly for the accept.".into(),
        ));
        let mut seqs: u64 = 0;
        let mut acks: u64 = 0;
        loop {
            if cancel_requested() {
                return Ok(None);
            }
            let seq = cable_stream_knock(br, sel, GAP);
            seqs += 1;
            if seq.iter().any(|&b| b == 0x6A || b == 0x69) {
                // Caught inside the knock itself (rare) - the accept arrived before we could settle.
                sink(Event::Log(format!("mfg cable entry: ACCEPTED after {seqs} sequence(s). Session live.")));
                return Ok(Some(seq));
            }
            if seq.contains(&0x64) {
                // Knock matched -> state 2, window open NOW. The selection already trailed the knock, so STOP
                // blasting and go quiet - more knock bytes here clobber the module's reply (the old bug).
                acks += 1;
                if acks == 1 {
                    sink(Event::Log("  knock-ack 0x64 seen - selection already sent; going quiet to catch the accept (no more knock bytes).".into()));
                }
                if let Some(win) = cable_resolve_ack(br, &seq, sink) {
                    sink(Event::Log(format!("mfg cable entry: ACCEPTED after {seqs} sequence(s), {acks} ack(s). Session live.")));
                    return Ok(Some(win));
                }
                // A late-landing ack: the knock matched too near the end of the ~130 ms window for the
                // selection to beat the reset. It is roughly a coin-flip per ack where in the window the
                // knock lands - KEEP CYCLING, a later cycle lands early enough. (Pin-1 auto-ignition removes
                // the lottery: it times the knock to the edge like the Gauge.S path that enters 100%.)
                sink(Event::Log(format!(
                    "  ack #{acks} did not complete (late in the window). Cycle the ignition again - this is ~1-in-2 per ack; it is NOT stuck."
                )));
                let _ = br.kline_reset(); // drain any backlog so the next ack we react to is fresh, not seconds-stale
            }
            if seqs % 30 == 0 {
                sink(Event::Log(format!(
                    "  still streaming ({seqs} sequences, {acks} ack(s) so far); cycle the ignition OFF then ON."
                )));
            }
        }
    }
}

/// Stream one knock: `@ 01 02` paced `gap` per byte (the spacing that reliably earns the ack), then the
/// selection byte sent EARLY and repeated at ~tick cadence. On the FTDI cable an 18 ms `02`->sel gap put the
/// selection a hair past the module's state-2 read slot EVERY time (ack seen, never accepted), where the
/// Gauge.S path - whose ESP32 transmits each byte immediately - lands the identical sel in time. Giving the
/// `02` only a ~1-tick tail and repeating the sel a few times at ~tick cadence covers the FTDI TX latency and
/// the tick-phase jitter, so one sel lands in the read slot. Extra sels after acceptance are harmless (framed
/// garbage). Returns every captured byte (echo + any reply).
/// Perform the mfg knock and return every captured byte (echo + ack 0x64 + any accept 'j'/'i'), so callers
/// resolve it the same way regardless of how it was sent. On a Gauge.S whose firmware supports it
/// (FW >= 0x0015), the knock runs ON THE DONGLE (CMD_KLINE_KNOCK) with precise local timing - no USB
/// round-trip in the paced loop, so the selection lands in the module's read slot on any host (this is what
/// makes Windows entry reliable). Otherwise - old firmware, or a K+DCAN cable (no microcontroller to run it) -
/// it streams the bytes from the host (`cable_stream_knock`), the best we can do without a dongle.
fn mfg_knock_window(br: &mut Bridge, sel: u8, gap: Duration) -> Vec<u8> {
    let dongle_knock = !br.kind().is_cable() && br.fw().map_or(false, |v| v >= 0x0015);
    if dongle_knock {
        let gap_ms = gap.as_millis().clamp(1, 255) as u8;
        if br.kline_knock(sel, gap_ms, 3).is_err() {
            return Vec::new();
        }
        // The dongle runs the paced knock (~(3+3) * gap_ms), buffering the K-line RX while its loop is busy,
        // then flushes it back; capture the whole window including the module's reply.
        kline_capture_raw(br, Duration::from_millis(60), Duration::from_millis(400)).unwrap_or_default()
    } else {
        cable_stream_knock(br, sel, gap)
    }
}

fn cable_stream_knock(br: &mut Bridge, sel: u8, gap: Duration) -> Vec<u8> {
    const SEL_GAP: Duration = Duration::from_millis(10); // ~1 tick: drains the 02, then lands the sel in time
    const SEL_REPEAT: usize = 3; // cover tick-phase jitter so a sel sits in the state-2 read slot
    let mut c = Vec::new();
    // Knock: 40, 01 at the ack-earning gap; the 02 gets only a ~1-tick tail so the sel follows promptly.
    for (i, &b) in [0x40u8, 0x01, 0x02].iter().enumerate() {
        if br.kline_tx(&[b]).is_err() {
            return c;
        }
        let cap = if i < 2 { gap } else { SEL_GAP };
        if let Ok(x) = kline_capture_raw(br, cap, cap) {
            c.extend_from_slice(&x);
        }
    }
    for _ in 0..SEL_REPEAT {
        if br.kline_tx(&[sel]).is_err() {
            break;
        }
        if let Ok(x) = kline_capture_raw(br, SEL_GAP, SEL_GAP) {
            c.extend_from_slice(&x);
        }
    }
    c
}

/// Resolve a knock-ack (0x64 = module in STATE 2, window open NOW). The selection byte already trailed the
/// knock, so STOP transmitting and listen quietly for up to ~300 ms - exactly the settle the proven Gauge.S
/// recipe (`mfg_enter_once`) uses to catch the 'j' accept + 'i' banner. Streaming more knock bytes here is
/// what made the blind blast get the ack but never the accept: the fresh `@ 01 02` lands on top of the
/// module's reply and knocks it back out of state 2/3. Logs the full raw window (knock + settle) so a refusal
/// is SEEN, not guessed: 0x6E 'n' = wrong selection digit, 0x63 'c' = refused, silence = the sel missed the
/// ~130 ms window. Returns Some(window) only on a real accept.
fn cable_resolve_ack(br: &mut Bridge, knock_seq: &[u8], sink: Sink) -> Option<Vec<u8>> {
    let mut window = knock_seq.to_vec();
    if let Ok(x) = kline_capture_raw(br, Duration::from_millis(30), Duration::from_millis(300)) {
        window.extend_from_slice(&x);
    }
    let ascii: String =
        window.iter().map(|&b| if (0x20..0x7f).contains(&b) { b as char } else { '.' }).collect();
    sink(Event::Log(format!("  ack window raw {} ({ascii})", hexs(&window))));
    if window.iter().any(|&b| b == 0x6A || b == 0x69) {
        return Some(window);
    }
    if window.contains(&0x6E) {
        sink(Event::Log("  -> 'n' (0x6E): selection digit refused. The knock is good; the SELECTION byte is wrong for diag entry - try a different digit.".into()));
    } else if window.contains(&0x63) {
        sink(Event::Log("  -> 'c' (0x63): selection refused (wrong phase/precondition).".into()));
    } else {
        sink(Event::Log("  -> no accept byte after the ack: the selection byte landed outside the module's read slot. The knock is heard (0x64) but the PC-streamed selection timing is jittering past the slot - on Windows this is USB-stack + OS scheduling jitter, not the FTDI latency timer (which can already be 1 ms). The lead sweep won't fix this; the robust fix is a dongle-side knock (Gauge.S). Flushing and re-arming.".into()));
    }
    None
}

/// Gauge.S ON-CAR entry: the car (not our relay) owns KL15, so we DON'T drive the relay - watch the A5 ADC
/// for the operator's ignition-on edge and knock `first_delay` ms after it, using the dongle's immediate,
/// jitter-free byte TX (the exact knock recipe that enters ~100% on the bench relay path). This sidesteps the
/// FTDI-cable problem entirely: the dongle sits ON the K-line with no USB UART between it and the wire, so the
/// selection byte lands in the state-2 window just like the relay path. Relay stays OFF throughout; the K-line
/// is left OPEN on success. Loops across ignition cycles until entry or cancel. None = cancelled.
fn mfg_enter_oncar(
    br: &mut Bridge,
    threshold_mv: u16,
    first_delay: u64, // quiet settle after the sensed A5 edge, before streaming the knock
    sel: u8,
    baud: u32,
    fmt: u8,
    sink: Sink,
) -> Result<Option<Vec<u8>>> {
    br.relay_set(false).ok(); // the car drives KL15, not us
    if br.kline_open_fmt(baud, fmt).is_err() {
        sink(Event::Log("mfg on-car entry: could not open the K-line on the dongle (CMD_KLINE_OPEN failed). Is the Gauge.S still responding? Try Ping.".into()));
        return Ok(None);
    }
    // Prime: if the key is already on, ask the operator to cycle it so we catch a real rising edge.
    let mut last_high = br.adc_read().map(|mv| mv >= threshold_mv).unwrap_or(false);
    sink(Event::Log(if last_high {
        "mfg on-car entry: ignition is ALREADY ON - cycle it (OFF, then ON) to arm the knock. Watching A5...".into()
    } else {
        "mfg on-car entry: turn the ignition ON; the dongle knocks on the sensed A5 edge. Watching A5...".to_string()
    }));
    const GAP: Duration = Duration::from_millis(18);
    // SWEEP the settle across ignition cycles. The dongle streams the knock from the PC, so on a high-latency
    // USB host (Windows) a single fixed settle can put the selection byte just outside the ~130 ms window - the
    // module acks the knock (0x64) but never accepts. We try the known-good settle first (the sticky disk value
    // if one was learned, else the caller's `first_delay`), then a spread around it, a different one each cycle,
    // until one enters. STICKY: the winner is remembered so a re-entry mid-dump tries it first, and the front
    // end can persist it. Shorter settles come first - USB latency delays the knock, so starting earlier tends
    // to recentre the selection.
    let order = oncar_lead_order(first_delay);
    let mut lead_i = 0usize;
    let mut last_log = Instant::now() - Duration::from_secs(10);
    loop {
        if cancel_requested() {
            br.kline_close().ok();
            return Ok(None);
        }
        let mv = br.adc_read().unwrap_or(0);
        let high = mv >= threshold_mv;
        if last_log.elapsed() >= Duration::from_secs(2) {
            let hint = if high { "ignition ON - cycle OFF then ON" } else { "waiting for ignition ON" };
            sink(Event::Log(format!("  A5 = {:.1} V ({hint})", a5_volts(mv))));
            last_log = Instant::now();
        }
        let rising = high && !last_high;
        last_high = high;
        if !rising {
            continue; // tight poll - the dongle's adc_read is fast and precise
        }
        let lead = order[lead_i % order.len()];
        lead_i += 1;
        // Quiet settle FIRST, then stream. The module software-reboots on the KL15 edge and needs ~1 tick +
        // init to bring its diagnostic state up before it will match the knock; knocking during that brings
        // nothing (and the early bytes can wedge its UART), which is why knocking immediately got zero acks.
        // So wait `lead` ms with the line quiet, THEN stream the knock across the rest of the window so we
        // don't have to hit one exact instant - the dongle's TX seats the selection on the ack.
        sink(Event::Log(format!("  ignition edge sensed (A5); +{lead} ms settle, then streaming the knock.")));
        if lead > 0 {
            std::thread::sleep(Duration::from_millis(lead));
        }
        let deadline = Instant::now() + Duration::from_millis(200);
        while Instant::now() < deadline {
            if cancel_requested() {
                br.kline_close().ok();
                return Ok(None);
            }
            let seq = mfg_knock_window(br, sel, GAP);
            if seq.iter().any(|&b| b == 0x6A || b == 0x69) {
                LAST_GOOD_ONCAR_LEAD_MS.store(lead, Ordering::Relaxed);
                sink(Event::Log(format!("mfg on-car entry: ACCEPTED at +{lead} ms settle. Session live.")));
                return Ok(Some(seq)); // K-line stays OPEN - session live
            }
            if seq.contains(&0x64) {
                if let Some(win) = cable_resolve_ack(br, &seq, sink) {
                    LAST_GOOD_ONCAR_LEAD_MS.store(lead, Ordering::Relaxed);
                    sink(Event::Log(format!("mfg on-car entry: ACCEPTED at +{lead} ms settle (settle). Session live.")));
                    return Ok(Some(win));
                }
                let _ = br.kline_reset(); // clear any desync before the next window
            }
        }
        sink(Event::Log(format!("  no acceptance at +{lead} ms; cycle the ignition again (next: +{} ms).", order[lead_i % order.len()])));
    }
}

/// Build the on-car settle sweep order: the sticky/known-good value first, then a spread around the caller's
/// default (shorter settles first). Deduplicated, preserving order.
fn oncar_lead_order(first_delay: u64) -> Vec<u64> {
    let base = first_delay;
    let mut order: Vec<u64> = Vec::new();
    let sticky = LAST_GOOD_ONCAR_LEAD_MS.load(Ordering::Relaxed);
    if sticky != u64::MAX {
        order.push(sticky);
    }
    for d in [
        base,
        base.saturating_sub(10),
        base + 10,
        base.saturating_sub(20),
        base + 20,
        base.saturating_sub(30),
        base + 30,
        base.saturating_sub(40),
        base + 40,
    ] {
        if !order.contains(&d) {
            order.push(d);
        }
    }
    order
}

/// Probe whether a diagnostic session is ALREADY live on the link, so `mfg_enter` can skip the ignition
/// cycle and knock. Opens the K-line at the session baud/fmt (re-opening an already-open port is harmless),
/// then sends the level-3 raise `A3 '3' .. '1' '6'` and checks for the 'f' (0x66) ack - which only a module
/// still in framed mode (i.e. still in session AND still powered) returns. On a hit the session is reused as
/// is, with no relay/ignition change; on a miss (no powered module, session timed out, or not in framed
/// mode) returns None and the caller does a full knock. Cheap: one frame, ~400 ms worst case. This is what
/// lets a run of keep-alive ops share a single entry.
fn mfg_try_reuse(br: &mut Bridge, baud: u32, fmt: u8, sink: Sink) -> Option<Vec<u8>> {
    if br.kline_open_fmt(baud, fmt).is_err() {
        return None;
    }
    // Clear any stale RX the prior op left queued before probing.
    let _ = kline_drain_stale(br, Duration::from_millis(40), Duration::from_millis(250));
    // (1) Must be at level 3 to write. Raise and require the 'f' (0x66) ack. A level-3 ack ALONE is not
    //     proof the session is usable - the module can still answer 'f' from a half-dead state - so it only
    //     gates the real test below.
    let raise3 = mfg_build_frame(&[0xA3u8, b'3', 0x00, 0x00, b'1', b'6']);
    let raw = mfg_tx_one(br, &raise3, sink);
    if !matches!(mfg_parse_reply(&raw, &raise3), Some((0x66, _))) {
        return None; // not even level 3 - definitely knock fresh
    }
    // (1b) READ-ONLY FAST PATH. A plain 0xA4 read needs only a live level-3 session, not descriptor-exec, so
    //      skip the whole exec-capability probe below. This is what makes fast mmio-sweep fast: reuse costs
    //      the one level-3 ack above, not a full K/L/K/O exec sequence per address. A half-dead session that
    //      lied with its 'f' ack just yields a no-reply on the read (caller advances, next reuse re-knocks).
    if !reuse_exec_probe() {
        sink(Event::Log("mfg entry: reuse OK - live level-3 session (exec probe skipped; read-only fast path), no ignition cycle.".into()));
        return Some(raw);
    }
    // (2) EXEC CAPABILITY PROBE - the one that actually gates reuse. The exec ops need the K/L/K/O DESCRIPTOR
    //     code-exec. A plain 0xA5 write DOES execute on a warm (reused) session, but the descriptor-exec may
    //     NOT re-arm without the module reboot that the knock's KL15 cycle triggers - so a plain-write probe
    //     passes while exec is still dead, and the op then silently returns stale/garbage scratch (the bug).
    //     So test EXEC itself: clear the marker to 0x00 (plain write - works on reuse), run the standard exec
    //     stub, and require the marker to flip to 0x5A. Only a session where descriptor-exec genuinely
    //     re-armed passes; otherwise we re-knock (reboot -> clean descriptor). EXEC_MARK == REG_SCRATCH, and
    //     the op pre-clears scratch before its own exec, so this probe does not disturb the op's result.
    let _ = mfg_tx_one_quiet(br, &exec_write_frame(EXEC_MARK, &[0x00]));
    let probe = exec_stub();
    let (p_init, p_l, p_final, p_exec) = exec_frames(&probe);
    let _ = mfg_tx_one_quiet(br, &p_init);
    for f in &p_l {
        let _ = mfg_tx_one_quiet(br, f);
    }
    let _ = mfg_tx_one_quiet(br, &p_final);
    let _ = mfg_tx_one_quiet(br, &p_exec);
    std::thread::sleep(Duration::from_millis(100));
    match mfg_read_reply(br, EXEC_MARK, 1) {
        Some((0x72, b)) if b.first() == Some(&EXEC_MARK_VAL) => {
            sink(Event::Log("mfg entry: reuse OK - descriptor exec re-armed on the live session (marker 0x5A), no ignition cycle.".into()));
            Some(raw)
        }
        Some((0x72, b)) => {
            sink(Event::Log(format!(
                "mfg entry: session NOT reusable - descriptor exec did not re-arm (marker {} != 0x5A; needs a reboot) - knocking fresh.",
                hexs(&b)
            )));
            None
        }
        _ => {
            sink(Event::Log("mfg entry: session not reusable (no exec-marker read-back) - knocking fresh.".into()));
            None
        }
    }
}

/// Close out a session op. When keep-alive is OFF (default) this drops the ignition (KL15/relay off) and
/// closes the K-line, killing the session - the original per-op behaviour. When keep-alive is ON it leaves
/// the session LIVE (relay energised, K-line open) so the next op reuses it via [`mfg_try_reuse`] and skips
/// the knock. A kept-alive session is ended explicitly with [`mfg_end_session`] (or by the module's ~14 s
/// inactivity timeout). Every single-shot mfg op ends through this instead of dropping the relay directly.
fn mfg_session_teardown(br: &mut Bridge, sink: Sink) {
    if keep_session_alive() {
        sink(Event::Log("  session left LIVE for reuse (KL15 still on) - the next op skips the knock. Run 'mfg end-session' to power down.".into()));
    } else {
        br.kline_close().ok();
        br.relay_set(false).ok();
    }
}

/// End a kept-alive session: close the K-line and de-energise KL15/the relay. Use after a run of keep-alive
/// ops to power the module down. Safe to call with nothing open.
pub fn mfg_end_session(br: &mut Bridge, sink: Sink) -> Result<()> {
    br.kline_close().ok();
    br.relay_set(false).ok();
    sink(Event::Log("mfg: session ended - K-line closed, KL15/relay off.".into()));
    Ok(())
}

/// Enter the mfg session for EITHER link and leave the K-line OPEN (session live) on success, returning the
/// entry reply. Gauge.S: the relay-timed knock retried `attempts` times, OR (when `auto_ign`) the ON-CAR
/// A5-edge knock with no relay. K+DCAN cable: the ignition-cycle knock (blast, or edge-triggered when
/// `auto_ign`). None = no entry / cancelled. This is the single entry point every session-based mfg op
/// (read/frame/handshake/scan/dump) uses, so all of them work on both links. When keep-alive is enabled it
/// first tries [`mfg_try_reuse`] to reuse a still-live session and skip the knock entirely.
#[allow(clippy::too_many_arguments)]
fn mfg_enter(
    br: &mut Bridge,
    off_ms: u64,
    threshold_mv: u16,
    first_delay: u64,
    attempts: u64,
    sel: u8,
    baud: u32,
    fmt: u8,
    auto_ign: bool,
    sink: Sink,
) -> Result<Option<Vec<u8>>> {
    // Keep-alive: if a prior op left the session live (KL15 still on), reuse it and skip the knock entirely.
    // A miss here is free - we fall through to the normal entry below. Works on either link.
    if keep_session_alive() {
        if let Some(rx) = mfg_try_reuse(br, baud, fmt, sink) {
            return Ok(Some(rx));
        }
    }
    if br.kind().is_cable() {
        return mfg_enter_cable(br, first_delay, sel, auto_ign, sink);
    }
    // Gauge.S: learn the firmware version once (it gates the on-device knock) if we haven't pinged yet - the
    // CLI may jump straight to a dump without a status poll having cached it.
    if br.fw().is_none() {
        let _ = br.ping();
    }
    if auto_ign {
        // On-car: the car owns KL15; wait for the operator's ignition edge (A5) and knock. No relay.
        return mfg_enter_oncar(br, threshold_mv, first_delay, sel, baud, fmt, sink);
    }
    // Bench/relay path: the dongle drives KL15 via the wired relay. If "Cycle Ignition Manually" is CHECKED
    // in the GUI this branch isn't used (that's on-car, above) - so a silent timeout here usually means the
    // box is unchecked but no relay is wired (EE->trigger, A5<-switched 12 V).
    sink(Event::Log(format!("mfg entry: relay (bench) path - cycling KL15 via the wired relay, up to {attempts} attempt(s). (Check \"Cycle Ignition Manually\" for the on-car A5-edge knock instead.)")));
    for _ in 1..=attempts {
        if cancel_requested() {
            break;
        }
        if let Some(rx) = mfg_enter_once(br, off_ms, threshold_mv, first_delay, sel, baud, fmt)? {
            return Ok(Some(rx));
        }
    }
    br.relay_set(false).ok();
    Ok(None)
}

/// Dump `total` bytes from `addr` by streaming 0xA4 'U' reads within ONE mfg session: enter once, then read
/// 59-byte chunks back-to-back (each read reloads the inactivity budget, so the session persists). On any
/// read failure (checksum, session drop) it closes and RE-ENTERS, continuing from where it left off, so a
/// lost session only costs one re-knock. Reads are byte-exact (checksum-verified). Returns the assembled
/// bytes; cancellable. The ignition is dropped OFF at the end. This is the firmware dump over the OBD K-line.
pub fn mfg_dump(
    br: &mut Bridge,
    off_ms: u64,
    threshold_mv: u16,
    first_delay: u64,
    attempts: u64,
    addr: u32,
    total: u32,
    auto_ign: bool,
    // The session is already entered (state 6) and should be read straight away - skips the initial knock, so
    // the auto-size flow doesn't cost a second ignition cycle. Mid-dump re-entries still happen as needed.
    assume_live: bool,
    cancel: &std::sync::atomic::AtomicBool,
    sink: Sink,
) -> Result<Vec<u8>> {
    let cable = br.kind().is_cable();
    sink(Event::Log(format!(
        "mfg dump: 0x{addr:08X} .. 0x{:08X} ({total} bytes) via 0xA4 'U', 59-byte chunks in a persistent session.{}",
        addr.wrapping_add(total),
        if cable { " [K+DCAN cable]" } else { "" }
    )));
    let mut data: Vec<u8> = Vec::with_capacity(total as usize);
    let mut live = assume_live;
    let mut reentries = 0u32;
    let mut flush_recov = 0u32;
    let mut diag_logged = 0u32;
    const STATUS_EVERY: u32 = 0x1000; // emit a status line each time we cross a 4KB boundary
    let mut next_mark = STATUS_EVERY;
    let start = Instant::now();
    while (data.len() as u32) < total {
        if cancel.load(Ordering::Relaxed) || cancel_requested() {
            sink(Event::Log(format!("mfg dump: cancelled at {} bytes.", data.len())));
            break;
        }
        if !live {
            // Unified entry: Gauge.S relay knock, or the cable ignition-cycle knock (blast / auto-ignition).
            let ok = mfg_enter(br, off_ms, threshold_mv, first_delay, attempts, b'0', 4800, 1, auto_ign, sink)?.is_some();
            if !ok {
                sink(Event::Log(format!("mfg dump: could not (re)enter at 0x{:08X}; stopping with {} bytes.", addr + data.len() as u32, data.len())));
                break;
            }
            std::thread::sleep(Duration::from_millis(200)); // reach state 6
            live = true;
        }
        let pos = data.len() as u32;
        let want = (total - pos).min(59) as u8;
        // First read on the open session.
        let (mut parsed, raw1) = mfg_read_reply_raw(br, addr + pos, want);
        // On no 'r', try the cheap dongle-side flush + one retry before paying for a relay re-knock.
        let mut flushed = false;
        if !matches!(parsed, Some((0x72, _))) {
            flushed = true;
            br.kline_reset().ok();
            std::thread::sleep(Duration::from_millis(10));
            parsed = mfg_read_reply_raw(br, addr + pos, want).0;
        }
        match parsed {
            Some((0x72, d)) if !d.is_empty() => {
                let take = d.len().min(want as usize);
                data.extend_from_slice(&d[..take]);
                if flushed {
                    flush_recov += 1;
                }
                let done = data.len() as u32;
                sink(Event::Progress { done, total });
                // Periodic status line every STATUS_EVERY bytes (handles a chunk crossing several marks).
                if done >= next_mark {
                    let secs = start.elapsed().as_secs_f64();
                    let rate = done as f64 / secs.max(0.001);
                    let eta = ((total - done) as f64 / rate.max(0.001)) as u64;
                    sink(Event::Log(format!(
                        "mfg dump: 0x{:08X} | {done}/{total} bytes ({:.0}%) | {:.0} B/s | ETA {}m{:02}s | {reentries} re-entries, {flush_recov} flush-recov",
                        addr + done,
                        100.0 * done as f64 / total as f64,
                        rate,
                        eta / 60,
                        eta % 60,
                    )));
                    while next_mark <= done {
                        next_mark += STATUS_EVERY;
                    }
                }
            }
            other => {
                // Log the first few failures with the exact bytes + tag so we can see WHY (silent /
                // partial frame / error tag), then fall back to the relay re-knock.
                if diag_logged < 8 {
                    diag_logged += 1;
                    let tag = match &other {
                        Some((t, _)) => format!("tag 0x{t:02X} ('{}')", if (0x20..0x7f).contains(t) { *t as char } else { '.' }),
                        None => "no parseable reply".into(),
                    };
                    sink(Event::Log(format!(
                        "mfg dump: read 0x{:08X} FAILED ({tag}); retry-after-flush {}; raw1={}",
                        addr + pos,
                        if flushed { "also failed" } else { "n/a" },
                        hexs(&raw1)
                    )));
                }
                // Session dropped or a bad chunk - close, re-enter, retry this position.
                br.kline_close().ok();
                live = false;
                reentries += 1;
                if reentries % 4 == 1 {
                    sink(Event::Log(format!("mfg dump: re-entering at 0x{:08X} ({} bytes so far; {flush_recov} flush-recov).", addr + pos, pos)));
                }
                if reentries > total / 20 + 20 {
                    sink(Event::Log("mfg dump: too many re-entries; stopping. The session may be unstable at this address.".into()));
                    break;
                }
            }
        }
    }
    br.kline_close().ok();
    br.relay_set(false).ok(); // dump done - drop the ignition
    let secs = start.elapsed().as_secs_f64();
    sink(Event::Log(format!(
        "mfg dump done: {} / {total} bytes from 0x{addr:08X} in {secs:.0}s ({:.0} B/s), {reentries} relay re-entries, {flush_recov} flush-recoveries.",
        data.len(),
        data.len() as f64 / secs.max(0.001)
    )));
    Ok(data)
}

/// Map the readable memory: within one persistent mfg session, read a small sample at each `stride` step
/// from `start` to `end` and classify it (all-FF = erased/unmapped, all-00 = zero/unmapped, else DATA),
/// logging each region transition. This finds every readable window - application flash, the bootloader /
/// boot ROM (if not read-protected), RAM - so we know what to dump and don't crawl empty space. A flash
/// bootloader in a read-protected sector shows as FF. K-line only. Needs FW >= 0x0012.
#[allow(clippy::too_many_arguments)]
/// Find the top of real (non-FF) data by scanning BACKWARD from `ceiling` toward `floor` in `stride` steps,
/// sampling 16 bytes via `0xA4 'U'`. The first sample that is readable and not all-`0xFF` is the top of the
/// populated flash; we stop there (so we can begin dumping as soon as we see data) and return the exclusive
/// end = that address + `stride`, so the whole block incl. any erased tail is covered. Returns `None` if no
/// data is found down to `floor` (or on cancel / failure to enter).
///
/// Recovery matches the dump: on a read miss it does the cheap dongle-side flush (`kline_reset`) + one retry
/// and otherwise just keeps descending. It NEVER re-knocks - a re-knock on the car is an ignition cycle, and
/// a miss during a backward scan means "past the top of readable memory", not a dead session, so descending
/// is the right move. On success the session is left OPEN (`leave_open` via the `Some` return): the caller
/// dumps on the same session, so the whole auto-size flow costs a single entry.
#[allow(clippy::too_many_arguments)]
fn mfg_find_data_top(
    br: &mut Bridge,
    off_ms: u64,
    threshold_mv: u16,
    first_delay: u64,
    attempts: u64,
    auto_ign: bool,
    ceiling: u32,
    floor: u32,
    stride: u32,
    // Skip the initial knock when the caller already holds an open mfg session (e.g. after a boundary check).
    assume_live: bool,
    cancel: &std::sync::atomic::AtomicBool,
    sink: Sink,
) -> Result<Option<u32>> {
    let stride = stride.max(0x100);
    sink(Event::Log(format!(
        "auto-size: scanning backward from 0x{ceiling:08X} (step 0x{stride:X}, 16-byte samples) for the top of real data; the dump begins as soon as data is seen."
    )));
    // First sample is the stride-aligned address just below the ceiling.
    let mut addr = (ceiling / stride).saturating_mul(stride);
    addr = addr.saturating_sub(stride);
    // Enter once, up front (unless the caller already holds the session open). We never re-knock during the
    // scan (see the doc comment).
    if !assume_live {
        if mfg_enter(br, off_ms, threshold_mv, first_delay, attempts, b'0', 4800, 1, auto_ign, sink)?.is_none() {
            sink(Event::Log("auto-size: could not enter the session; aborting.".into()));
            return Ok(None);
        }
        std::thread::sleep(Duration::from_millis(200));
    }
    loop {
        if cancel.load(Ordering::Relaxed) || cancel_requested() {
            sink(Event::Log("auto-size: cancelled.".into()));
            br.kline_close().ok();
            return Ok(None);
        }
        if addr < floor {
            br.kline_close().ok();
            return Ok(None);
        }
        // Same recovery ladder as the dump: on a miss, flush (kline_reset) + one retry. No re-knock.
        let reply = mfg_read_reply(br, addr, 16).or_else(|| {
            br.kline_reset().ok();
            std::thread::sleep(Duration::from_millis(10));
            mfg_read_reply(br, addr, 16)
        });
        match reply {
            // Readable ('r') and not entirely erased -> this is the top of the data. Leave the session OPEN
            // so the caller dumps on it (no second entry = no second ignition cycle).
            Some((0x72, d)) if !d.is_empty() && !d.iter().all(|&b| b == 0xFF) => {
                let top = addr.saturating_add(stride);
                sink(Event::Log(format!("auto-size: data at 0x{addr:08X} -> top 0x{top:08X}.")));
                return Ok(Some(top));
            }
            // Readable but all-FF (erased), or not readable (past the top of mapped memory / a transient
            // stall the flush didn't clear) -> keep descending. Either way we do NOT re-knock.
            _ => {}
        }
        if addr % 0x10000 < stride {
            sink(Event::Log(format!("  auto-size: 0x{addr:08X} blank, descending...")));
        }
        addr = addr.saturating_sub(stride);
    }
}

/// Auto-sizing dump: find the top of the data with a backward scan (so the length is not a hardcoded guess),
/// then dump `start`..top. Avoids truncating a flash that is larger than expected (e.g. the ZCP's calibration
/// tail past 0x40000). `ceiling` is where the backward scan starts; `stride` its step (also the round-up).
#[allow(clippy::too_many_arguments)]
pub fn mfg_dump_auto(
    br: &mut Bridge,
    off_ms: u64,
    threshold_mv: u16,
    first_delay: u64,
    attempts: u64,
    start: u32,
    ceiling: u32,
    stride: u32,
    auto_ign: bool,
    cancel: &std::sync::atomic::AtomicBool,
    sink: Sink,
) -> Result<Vec<u8>> {
    let top = match mfg_find_data_top(
        br, off_ms, threshold_mv, first_delay, attempts, auto_ign, ceiling, start, stride, false, cancel, sink,
    )? {
        Some(t) if t > start => t,
        _ => {
            sink(Event::Log("auto-size: no data found above the start address; nothing dumped.".into()));
            return Ok(Vec::new());
        }
    };
    let total = top.saturating_sub(start);
    sink(Event::Log(format!(
        "auto-size: dumping 0x{start:08X}..0x{top:08X} ({total} bytes) on the same session (no re-entry)."
    )));
    // The scan left the session OPEN at the data top, so dump on it directly (assume_live = true): the whole
    // auto-size flow is a single ignition cycle, not two.
    mfg_dump(br, off_ms, threshold_mv, first_delay, attempts, start, total, auto_ign, true, cancel, sink)
}

/// Dump the E46 MK60 flash window. This tool is MK60-only: the flash maps 0x0..FW_TOP with a known, fixed top
/// for this family, and above FW_TOP is unmapped (reads come back no-reply). We hardcode the bound rather than
/// probe down from a high ceiling - a readability probe up there would be fooled by the 0x100000 ECC-parity
/// mirror of the flash. Flow: enter the mfg session ONCE, run a quick two-point boundary check (readable just
/// below FW_TOP, unmapped at FW_TOP) to confirm the bound and warn if this unit differs, then dump on the same
/// session (one ignition cycle):
///   - full (`truncated` = false): 0x0..FW_TOP including the erased 0xFF tail, so data in the upper flash of
///     some variant is never missed.
///   - truncated (`truncated` = true): stop at the top of real data (first non-FF below FW_TOP) - faster, but
///     skips the erased tail and could miss upper-region data.
#[allow(clippy::too_many_arguments)]
pub fn mfg_dump_fw(
    br: &mut Bridge,
    off_ms: u64,
    threshold_mv: u16,
    first_delay: u64,
    attempts: u64,
    truncated: bool,
    auto_ign: bool,
    cancel: &std::sync::atomic::AtomicBool,
    sink: Sink,
) -> Result<Vec<u8>> {
    const FW_TOP: u32 = 0x60000; // known E46 MK60 flash-window top; above it is unmapped (no-reply)
    const STRIDE: u32 = 0x1000;
    // Enter once; keep the session open for the boundary check AND the dump (one ignition cycle total).
    if mfg_enter(br, off_ms, threshold_mv, first_delay, attempts, b'0', 4800, 1, auto_ign, sink)?.is_none() {
        sink(Event::Log("dump: could not enter the mfg session; nothing dumped.".into()));
        return Ok(Vec::new());
    }
    std::thread::sleep(Duration::from_millis(200));
    // Quick boundary check against the known bound: the last block should read, FW_TOP itself should not.
    let below_ok = matches!(mfg_read_reply(br, FW_TOP - STRIDE, 16), Some((0x72, _)));
    let top_mapped = matches!(mfg_read_reply(br, FW_TOP, 16), Some((0x72, _)));
    br.kline_reset().ok(); // clear any half-duplex stall left by probing the unmapped top
    sink(Event::Log(format!(
        "dump: boundary check - 0x{:08X} {}, 0x{FW_TOP:08X} {}.",
        FW_TOP - STRIDE,
        if below_ok { "readable" } else { "no-reply" },
        if top_mapped { "READABLE" } else { "no-reply (expected)" },
    )));
    if !below_ok {
        sink(Event::Log(format!(
            "dump: WARNING - 0x{:08X} did not read; the flash window may be smaller than the known 0x{FW_TOP:08X} top.",
            FW_TOP - STRIDE
        )));
    }
    if top_mapped {
        sink(Event::Log(format!(
            "dump: WARNING - memory is mapped at 0x{FW_TOP:08X}, above the known E46 top; this variant may have more flash. Dumping to the known top only."
        )));
    }
    // Decide the dump top.
    let top = if truncated {
        // Stop at the top of real data. Backward scan from FW_TOP - far below the 0x100000 ECC window, so the
        // parity mirror can't fool it - and assume_live so we reuse the session we already entered.
        match mfg_find_data_top(
            br, off_ms, threshold_mv, first_delay, attempts, auto_ign, FW_TOP, 0, STRIDE, true, cancel, sink,
        )? {
            Some(t) if t > 0 => t.min(FW_TOP),
            _ => {
                sink(Event::Log("dump: no data found below the top; nothing dumped.".into()));
                br.kline_close().ok();
                return Ok(Vec::new());
            }
        }
    } else {
        FW_TOP
    };
    sink(Event::Log(format!(
        "dump: reading 0x00000000..0x{top:08X} ({top} bytes){} on the open session.",
        if truncated { " - truncated at the data top" } else { " - full flash window" },
    )));
    // assume_live = true: dump on the session we already entered (no second ignition cycle).
    mfg_dump(br, off_ms, threshold_mv, first_delay, attempts, 0, top, auto_ign, true, cancel, sink)
}







// ---- CAN ISO-TP NVM primitives (for the E5 prog-request test; the E46 path stays on K-line above) ----







/// Listen-only CAN sniff for `dur`: emits a [`Event::Can`] per frame and returns the sorted,
/// de-duplicated set of ids seen.
pub fn sniff(br: &mut Bridge, dur: Duration, sink: Sink) -> Result<Vec<u16>> {
    br.can_open(true)?;
    let mut ids: Vec<u16> = Vec::new();
    let deadline = Instant::now() + dur;
    while Instant::now() < deadline {
        if let Some(f) = br.poll()? {
            if f.cmd == proto::EVT_CAN_RX && f.payload.len() >= 3 {
                let id = (f.payload[0] as u16) | ((f.payload[1] as u16) << 8);
                let dlc = f.payload[2] as usize;
                let data = f.payload[3..(3 + dlc).min(f.payload.len())].to_vec();
                if !ids.contains(&id) {
                    ids.push(id);
                }
                sink(Event::Can { id, data });
            }
        }
    }
    br.can_close().ok();
    ids.sort_unstable();
    ids.dedup();
    Ok(ids)
}

/// Send the cluster/DME keepalive burst (0x610/613/615/316/329) once, so a
/// freshly-booted MK60 leaves boot and starts broadcasting its telemetry. This is
/// **active TX on the bus**, not listen-only. Assumes CAN is already open in normal
/// mode; see [`mk60can`] for why this is needed and what it does/doesn't gate.
pub fn send_keepalive_burst(br: &mut Bridge, sink: Sink) -> Result<()> {
    for (id, data) in mk60can::keepalive_burst() {
        br.can_tx(id, &data)?;
        std::thread::sleep(Duration::from_millis(mk60can::KEEPALIVE_GAP_MS));
    }
    sink(Event::Log("Sent the MK60 keepalive burst.".into()));
    Ok(())
}


/// Monitor CAN for `dur`, decoding the MK60's broadcast telemetry (wheel speeds,
/// steering, ASC1/DSC/speed). When `wake` is set, first send the keepalive burst so
/// a cold module starts talking (requires normal mode / active TX); otherwise listen
/// only. Emits a throttled decoded line per known id plus every frame as [`Event::Can`],
/// and finishes with a validation summary of which expected ids were seen. Returns the
/// sorted, de-duplicated set of ids observed.
pub fn can_monitor(
    br: &mut Bridge,
    dur: Duration,
    wake: bool,
    sink: Sink,
) -> Result<Vec<u16>> {
    br.can_open(!wake)?; // wake => normal (TX capable); else listen-only
    if wake {
        send_keepalive_burst(br, sink)?;
    }

    let mut ids: Vec<u16> = Vec::new();
    let mut last_log: std::collections::HashMap<u16, Instant> = std::collections::HashMap::new();
    let throttle = Duration::from_millis(250);
    let deadline = Instant::now() + dur;
    while Instant::now() < deadline {
        if let Some(f) = br.poll()? {
            if f.cmd == proto::EVT_CAN_RX && f.payload.len() >= 3 {
                let id = (f.payload[0] as u16) | ((f.payload[1] as u16) << 8);
                let dlc = f.payload[2] as usize;
                let data = f.payload[3..(3 + dlc).min(f.payload.len())].to_vec();
                if !ids.contains(&id) {
                    ids.push(id);
                }
                sink(Event::Can { id, data: data.clone() });
                // Decoded, human-readable line - throttled per id so fast frames don't flood.
                if let Some(line) = mk60can::decode_line(id, &data) {
                    let now = Instant::now();
                    let due = last_log.get(&id).map_or(true, |t| now.duration_since(*t) >= throttle);
                    if due {
                        last_log.insert(id, now);
                        sink(Event::Log(format!("0x{id:03X}: {line}")));
                    }
                }
            }
        }
    }
    br.can_close().ok();
    ids.sort_unstable();
    ids.dedup();

    // Validation summary: did the MK60 broadcast what a healthy module should?
    let name = |id: u16| -> &'static str {
        match id {
            mk60can::ID_ASC1 => "speed and DSC",
            mk60can::ID_ASC2 => "wheel speeds",
            mk60can::ID_LWS1 => "steering",
            _ => "telemetry",
        }
    };
    let present: Vec<&str> = mk60can::EXPECTED.iter().filter(|id| ids.contains(id)).map(|id| name(*id)).collect();
    let missing: Vec<&str> = mk60can::EXPECTED.iter().filter(|id| !ids.contains(id)).map(|id| name(*id)).collect();
    let total = ids.len();
    let msg = if present.is_empty() {
        let hint = if wake {
            "Nothing received from the MK60. Check wiring and power to the unit."
        } else {
            "Nothing received from the MK60. Try the Wake button first."
        };
        format!("CAN: {total} IDs total. {hint}")
    } else if missing.is_empty() {
        format!("CAN: {total} IDs total. The MK60 is broadcasting all expected telemetry.")
    } else {
        format!(
            "CAN: {total} IDs total. The MK60 is broadcasting {}, but {} {} not.",
            present.join(", "),
            missing.join(" and "),
            if missing.len() == 1 { "is" } else { "are" }
        )
    };
    sink(Event::Log(msg));
    Ok(ids)
}



/// Collect K-line bytes (including our own echo) until a valid BMW-FAST response frame appears.
fn kline_response(br: &mut Bridge) -> Result<Vec<u8>> {
    kline_response_timeout(br, 300)
}

/// Like [`kline_response`] but with a caller-chosen timeout. The matrix sub-function sweeps use a short
/// one (the E46 answers `7F xx 12` within a few ms for an absent sub, so a long wait only hurts the rare
/// genuinely-silent sub) to keep a 256-entry sweep fast.
fn kline_response_timeout(br: &mut Bridge, ms: u64) -> Result<Vec<u8>> {
    let mut buf = Vec::new();
    let deadline = Instant::now() + Duration::from_millis(ms);
    while Instant::now() < deadline {
        if let Some(f) = br.poll()? {
            if f.cmd == proto::EVT_KLINE_RX {
                buf.extend_from_slice(&f.payload);
            }
        }
        if let Some(resp) = kwp::parse_response(&buf) {
            return Ok(resp);
        }
    }
    kwp::parse_response(&buf).ok_or_else(|| anyhow!("no K-line response (saw {:02X?})", buf))
}

#[cfg(test)]
mod tests {
    use super::{
        exec_finalize_p1, exec_frames, exec_stub, exec_write_frame, mcore_build_u32, mfg_build_frame,
        mfg_parse_reply, mmio_probe_stub, parse_hex_bytes, EXEC_CTRL, EXEC_MARK, REG_SCRATCH,
    };

    // Real bench capture (dump of 0x1BC36): echo of our 0xA4 'U' frame, then an 0xE5-marked 'r' reply that
    // must parse. Before the fix, the 0xE5 marker was unrecognized and this cost a relay re-entry.
    #[test]
    fn parse_reply_accepts_e5_marker() {
        let raw: Vec<u8> = vec![
            0x12, 0xA4, 0x55, 0x30, 0x30, 0x30, 0x31, 0x42, 0x43, 0x33, 0x36, 0x30, 0x30, 0x30, 0x30,
            0x30, 0x30, 0x33, 0x42, 0x4F, // echo
            0x28, 0xE5, 0x72, 0x36, 0xEE, 0xE8, 0x04, 0x64, 0xB7, 0xB7, 0x0D, 0x62, 0x7E, 0xF0, 0x03,
            0xA7, 0x1D, 0xAE, 0x2D, 0xB7, 0x0D, 0xBE, 0x1D, 0x12, 0xDE, 0x1C, 0xCE, 0x12, 0xE2, 0xFF,
            0xCE, 0xB2, 0x0E, 0xF0, 0x08, 0x12, 0xD2, 0x1C, 0xC2, 0xA7, 0x02, 0x3D, // 0xE5 reply
        ];
        // Reconstruct the exact echo frame we sent for 0x1BC36 x59.
        let mut payload = vec![0xA4u8, b'U'];
        payload.extend_from_slice(b"0001BC36");
        payload.extend_from_slice(b"0000003B");
        let echo = mfg_build_frame(&payload);
        let (tag, data) = mfg_parse_reply(&raw, &echo).expect("E5 reply must parse");
        assert_eq!(tag, 0x72); // 'r' read
        assert_eq!(data.len(), 38); // len 0x28 - marker - tag
        assert_eq!(data[0], 0x36);
        assert_eq!(*data.last().unwrap(), 0x02);
    }

    // The classic 0xE4 marker must still parse.
    #[test]
    fn parse_reply_accepts_e4_marker() {
        // [len=04][E4][tag='r'][AA BB][sum]; sum = (04+E4+72+AA+BB)&FF
        let body = [0x04u8, 0xE4, 0x72, 0xAA, 0xBB];
        let sum = body.iter().fold(0u8, |a, &b| a.wrapping_add(b));
        let mut raw = body.to_vec();
        raw.push(sum);
        let (tag, data) = mfg_parse_reply(&raw, &[]).expect("E4 reply must parse");
        assert_eq!(tag, 0x72);
        assert_eq!(data, vec![0xAA, 0xBB]);
    }

    #[test]
    fn hex_bytes_forms() {
        assert_eq!(parse_hex_bytes("AAF18C").unwrap(), vec![0xAA, 0xF1, 0x8C]);
        assert_eq!(parse_hex_bytes("AA F1 8C").unwrap(), vec![0xAA, 0xF1, 0x8C]);
        assert_eq!(parse_hex_bytes("0xAAF18C").unwrap(), vec![0xAA, 0xF1, 0x8C]);
        assert!(parse_hex_bytes("AAF").is_err()); // odd
        assert!(parse_hex_bytes("").is_err()); // empty
    }

    #[test]
    fn mmio_probe_build_and_stub() {
        // General 32-bit address builder: 0xFF8032 (the known-good UART reg).
        // bytes FF 80 32: load FF (movi 0x7F, bseti 7); lsli 8; load 80 (movi 0, bseti 7), or; lsli 8; or 0x32.
        let a = mcore_build_u32(4, 6, 0x00FF_8032);
        // movi r4,0x7F=0x67F4 ; bseti r4,7=0x3474 ; lsli r4,8=0x3C84 ; movi r6,0=0x6006 ; bseti r6,7=0x3476 ;
        // or r4,r6=0x1E64 ; lsli r4,8=0x3C84 ; movi r6,0x32=0x6326 ; or r4,r6=0x1E64
        assert_eq!(a, vec![0x67, 0xF4, 0x34, 0x74, 0x3C, 0x84, 0x60, 0x06, 0x34, 0x76, 0x1E, 0x64, 0x3C, 0x84, 0x63, 0x26, 0x1E, 0x64]);
        // a simple address exercising the no-high-bit path: 0x00401400.
        let b = mcore_build_u32(4, 6, 0x0040_1400);
        // bytes 40 14 00: movi r4,0x40 (0x6404) ; lsli r4,8 ; movi r6,0x14 ; or r4,r6 ; lsli r4,8
        assert_eq!(b, vec![0x64, 0x04, 0x3C, 0x84, 0x61, 0x46, 0x1E, 0x64, 0x3C, 0x84]);

        // The probe stub: scratch build, sentinel 0x55, zero +1..+8, address build, ld/st, sentinel 0x66, jmp.
        // halfword (default, 2) -> ld.h r3,(r4,0) ; st.h r3,(r5,2hw=+4).
        let stub = mmio_probe_stub(0x00FF_8032, 2, false);
        assert_eq!(REG_SCRATCH, 0x0040_1400);
        assert_eq!(&stub[..12], &[0x33, 0x65, 0x61, 0x46, 0x3C, 0x86, 0x1E, 0x65, 0x65, 0x53, 0xB3, 0x05]);
        // the 12-byte zeroing block: movi r3,0 ; st.b +1/+2/+3 ; st.w +4..+7 ; st.b +8.
        assert_eq!(&stub[12..24], &[0x60, 0x03, 0xB3, 0x15, 0xB3, 0x25, 0xB3, 0x35, 0x93, 0x15, 0xB3, 0x85]);
        // after the 18-byte address build (24 + 18 = 42): ld.h ; st.h.
        assert_eq!(&stub[42..46], &[0xC3, 0x04, 0xD3, 0x25]);
        assert_eq!(&stub[46..52], &[0x66, 0x63, 0xB3, 0x85, 0x00, 0xCF]);
        // word (4) -> ld.w/st.w ; byte (1) -> ld.b/st.b.
        assert_eq!(&mmio_probe_stub(0x00FF_8032, 4, false)[42..46], &[0x83, 0x04, 0x93, 0x15]);
        assert_eq!(&mmio_probe_stub(0x00FF_8032, 1, false)[42..46], &[0xA3, 0x04, 0xB3, 0x45]);

        // Elevated stub: same layout with 0x10A1 inserted after zeroing (before address build).
        let estub = mmio_probe_stub(0x00FF_8032, 2, true);
        assert_eq!(&estub[..24], &stub[..24]);
        assert_eq!(&estub[24..26], &[0x10, 0xA1]); // mfcr r1, ss4
        assert_eq!(&estub[26..44], &stub[24..42]); // address build
        assert_eq!(&estub[44..50], &[0xC3, 0x04, 0xD3, 0x25, 0x66, 0x63]); // ld.h/st.h + start of 0x66
        assert_eq!(estub.len(), stub.len() + 2);
        // Sanity halfword at a known-good user MMIO addr still ends with jmp r15.
        assert_eq!(estub[estub.len() - 2..], [0x00, 0xCF]);
    }

    // The RAM code-exec path (exec_stub + exec_frames) that the kept mmio-read/sweep stubs reuse: lock down
    // the stub bytes, the K/L/K/O descriptor framing, and the pre-clear write so a drift in either is caught.
    #[test]
    fn mfg_exec_layout() {
        // descriptor base: control byte sits at descriptor-3 (EXEC_CTRL). Scoped to the test since only the
        // framing layout, not the running code, references the descriptor address directly.
        const EXEC_DESC: u32 = 0x0040_11AA;

        let stub = exec_stub();
        // Stub: bgeni r4,0x16 / movi r5,0x14 / lsli r5,0x8 / or r4,r5 / movi r3,0x5A / st.b r3,(r4,0) / jmp r15
        assert_eq!(stub, vec![0x33, 0x64, 0x61, 0x45, 0x3C, 0x85, 0x1E, 0x54, 0x65, 0xA3, 0xB3, 0x04, 0x00, 0xCF]);
        assert_eq!(stub.len(), 14);
        // marker goes to absolute scratch (M1-proven writable), well clear of the stub bytes at desc+6.
        assert_eq!(EXEC_MARK, 0x0040_1400);
        assert!((0x0040_0000..=0x0040_17FF).contains(&EXEC_MARK)); // valid on-chip SRAM
        assert!(EXEC_MARK > EXEC_DESC + 0x20); // far above the staged stub (desc+6..+0x13)
        // control byte sits at descriptor-3.
        assert_eq!(EXEC_CTRL, EXEC_DESC - 3);
        assert_eq!((EXEC_CTRL, EXEC_DESC), (0x0040_11A7, 0x0040_11AA));
        // finalize P1 = (sum16(stub) << 16) | (len+2): sum = 0x04FE, len 14 -> declared 0x10.
        assert_eq!(exec_finalize_p1(&stub), 0x04FE_0010);

        // the frames: K-init / L-append(s) / K-finalize / O-exec.
        let (k_init, l_frames, k_final, o_exec) = exec_frames(&stub);
        // K-init: A6 'K' "00000000" "10020000"
        assert_eq!(k_init, mfg_build_frame(b"\xA6K0000000010020000"));
        // 14-byte stub fits in one L frame: A6 'L' 00 00 + stub (raw)
        assert_eq!(l_frames.len(), 1);
        let mut want_l = vec![0xA6u8, b'L', 0x00, 0x00];
        want_l.extend_from_slice(&stub);
        assert_eq!(l_frames[0], mfg_build_frame(&want_l));
        // K-finalize: A6 'K' "04FE0010" "00000000"
        assert_eq!(k_final, mfg_build_frame(b"\xA6K04FE001000000000"));
        // O-exec: A6 'O'
        assert_eq!(o_exec, mfg_build_frame(&[0xA6u8, b'O']));

        // frame integrity: [len][payload][sum], sum = (len + payload) & 0xFF. No frame oversized.
        for f in std::iter::once(&k_init).chain(l_frames.iter()).chain([&k_final, &o_exec]) {
            let len = f[0] as usize;
            assert_eq!(f.len(), len + 2);
            assert!(f.len() <= 44, "frame must stay small enough for the module");
            let sum = f[1..=len].iter().fold(len as u8, |a, &b| a.wrapping_add(b));
            assert_eq!(*f.last().unwrap(), sum);
        }
        // the pre-clear write is a 0xA5 'T' to absolute scratch, inside SRAM.
        let w = exec_write_frame(EXEC_MARK, &[0x00]);
        assert_eq!(w, mfg_build_frame(b"\xA5T00401400\x00"));
    }
}
