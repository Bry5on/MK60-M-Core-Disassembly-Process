// Gauge.S MK60 Tool bridge - a DUMB USB<->CAN/K-line pass-through. ALL protocol logic (BMW-FAST
// framing, seed/key, CCP, ISO-TP, the dump loop) lives on the PC host; the firmware only moves raw
// bytes/frames and offers on-demand resets. The main loop does NO CAN/TWAI work unless CAN is open, so
// a timing-sensitive half-duplex K-line transaction is never interleaved with TWAI servicing.
//
//   PC link : Serial  (UART0 FTDI on V4, native USB-CDC on V5) @ 921600
//   K-line  : Serial2 (SERIAL_KLINE_RX/TX) @ 9600 8E1 -> transceiver -> MK60 pin 2
//   CAN     : TWAI    (CAN_RX/CAN_TX) @ 500k           -> transceiver -> MK60 11/15
#include <Arduino.h>
#include "driver/twai.h"
#include "driver/uart.h"          // uart_flush_input() - hardware RX-FIFO reset for the K-line UART
#include "driver/periph_ctrl.h"   // periph_module_reset() - HARD reset of the K-line UART peripheral
#include "esp_task_wdt.h"         // one minimal last-resort watchdog (see setup())
#include "esp_system.h"           // esp_reset_reason() - reported in EVT_INFO
#include "config.h"
#include "protocol.h"

// Serial2 maps to UART2 on both the classic ESP32 (V4) and the ESP32-S3 (V5); the Arduino
// HardwareSerial for it installs the IDF driver on this port, so uart_flush_input() is valid here.
#define KLINE_UART_NUM 2

// ---------- CRC16/CCITT-FALSE (poly 0x1021, init 0xFFFF) ----------
static uint16_t crc16(const uint8_t* d, size_t n, uint16_t crc) {
  for (size_t i = 0; i < n; i++) {
    crc ^= (uint16_t)d[i] << 8;
    for (int b = 0; b < 8; b++) crc = (crc & 0x8000) ? (crc << 1) ^ 0x1021 : (crc << 1);
  }
  return crc;
}

// ---------- framed event -> PC ----------
// `droppable`: true ONLY for the high-volume broadcast stream (EVT_CAN_RX) - DROP the frame if the CDC
// TX buffer is full, so a CAN flood can never block the loop. false for the command REPLIES the host
// blocks on (EVT_INFO, EVT_KLINE_RX, EVT_ERROR): deliver them reliably, with a BOUNDED ~500 ms wait
// (feeding the watchdog) so even a pathological stall can't reach the watchdog timeout.
static void send_frame(uint8_t cmd, const uint8_t* p, uint16_t len, bool droppable) {
  uint16_t total = 4 + len + 2;
  if ((int)Serial.availableForWrite() < (int)total) {
    if (droppable) return;  // broadcast: drop and move on
    uint32_t t0 = millis();
    while ((int)Serial.availableForWrite() < (int)total) {
      if (millis() - t0 > 500) return;  // give up after 500 ms (still below the watchdog)
      esp_task_wdt_reset();
      delay(1);
    }
  }
  uint8_t hdr[4] = { PROTO_SYNC, cmd, (uint8_t)(len & 0xFF), (uint8_t)(len >> 8) };
  uint16_t c = crc16(hdr + 1, 3, 0xFFFF);  // CRC over CMD + LEN + PAYLOAD
  c = crc16(p, len, c);
  Serial.write(hdr, 4);
  if (len) Serial.write(p, len);
  uint8_t cb[2] = { (uint8_t)(c & 0xFF), (uint8_t)(c >> 8) };
  Serial.write(cb, 2);
}
static uint8_t boot_reason = 0;  // esp_reset_reason() captured once at boot
static void send_info() {
  // [fw_lo][fw_hi][board][caps=0][reset_reason]. Older hosts that read only the first 3-4 bytes are
  // unaffected. (The 0x000A phase byte was removed in the 0x000B slim-down.)
  uint8_t i[5] = { (uint8_t)(FW_VER & 0xFF), (uint8_t)(FW_VER >> 8), (uint8_t)(PCB_VER / 10), 0, boot_reason };
  send_frame(EVT_INFO, i, 5, false);  // reply - deliver reliably
}

// ---------- CAN (TWAI) - a raw bridge: open / close / reset / tx / rx, with bus-off self-heal ----------
// The one auto-recovery the firmware runs (see can_service): when the module drops off the bus mid-probe
// the TWAI goes bus-off; instead of letting the error path starve loop() until the 8s watchdog reboots, we
// initiate recovery and suppress TX until the bus returns, and tell the host via EVT_CAN_STATE. This is
// hardware hygiene, not protocol logic - all BMW/CCP/ISO-TP decisions still live on the host.
static bool can_up = false;
static bool can_busoff = false;  // true between bus-off and recovery complete: TX suppressed
static uint8_t can_mode = 0;  // remembered for CMD_CAN_RESET (0=normal, 1=listen-only)
// Safely tear down the TWAI driver from ANY state. twai_stop()/twai_driver_uninstall() require a
// stoppable state; from BUS_OFF they fail (and uninstalling a driver mid-recovery was corrupting it and
// wedging the loop - the flag sweep's per-probe close landing during a power-cycle bus-off). So: if
// bus-off, initiate recovery first (completes in a few ms on an idle recessive bus), wait bounded for a
// stoppable state, THEN stop + uninstall. Self-healing: harmless if nothing is installed. Watchdog-fed.
static void can_teardown() {
  twai_status_info_t st;
  if (twai_get_status_info(&st) == ESP_OK && st.state == TWAI_STATE_BUS_OFF) {
    twai_initiate_recovery();  // BUS_OFF -> RECOVERING; cannot stop/uninstall straight from bus-off
  }
  for (int i = 0; i < 50; i++) {  // wait up to ~250 ms for RUNNING or STOPPED
    if (twai_get_status_info(&st) != ESP_OK) break;  // not installed -> nothing to wait for
    if (st.state == TWAI_STATE_RUNNING || st.state == TWAI_STATE_STOPPED) break;
    esp_task_wdt_reset();
    delay(5);
  }
  twai_stop();              // ESP_OK only from RUNNING; harmless error otherwise
  twai_driver_uninstall();  // from STOPPED; harmless error if not installed
  can_up = false;
  can_busoff = false;
}
static void can_open(uint8_t mode) {
  can_teardown();  // clear any leaked/half-torn driver (from a prior close during bus-off) before install
  can_mode = mode;
  twai_general_config_t g = TWAI_GENERAL_CONFIG_DEFAULT(
      (gpio_num_t)CAN_TX, (gpio_num_t)CAN_RX, mode ? TWAI_MODE_LISTEN_ONLY : TWAI_MODE_NORMAL);
  twai_timing_config_t t = TWAI_TIMING_CONFIG_500KBITS();   // 500k only
  twai_filter_config_t f = TWAI_FILTER_CONFIG_ACCEPT_ALL(); // host does the filtering
  if (twai_driver_install(&g, &t, &f) == ESP_OK && twai_start() == ESP_OK) {
    twai_reconfigure_alerts(TWAI_ALERT_BUS_OFF | TWAI_ALERT_BUS_RECOVERED, NULL);  // for can_service()
    can_busoff = false;
    can_up = true;
  } else {
    uint8_t e[1] = { 0x10 }; send_frame(EVT_ERROR, e, 1, false);
  }
}
// Bounded wait for the TX queue to reach the bus before stopping - twai_stop() ABORTS queued frames, so
// a transmit-then-close (the MK60 wake burst) would otherwise drop its last frames. On-demand only.
static void can_flush_tx() {
  twai_status_info_t st;
  for (int i = 0; i < 50; i++) {
    if (twai_get_status_info(&st) != ESP_OK || st.msgs_to_tx == 0) break;
    delay(2);
  }
}
static void can_close() {
  if (!can_up) return;
  can_flush_tx();
  can_teardown();  // state-safe: recovers from bus-off first so the uninstall can't corrupt the driver
}
static void can_reset() {  // host-driven on-demand TWAI re-init (CMD_CAN_RESET)
  if (!can_up) return;
  can_teardown();
  can_open(can_mode);
}
static void can_tx(uint16_t id, uint8_t dlc, const uint8_t* d) {
  if (can_busoff) return;   // bus in recovery - don't enqueue into a stopped controller
  twai_message_t m = {};
  m.flags = TWAI_MSG_FLAG_SS;  // SINGLE-SHOT: a non-ACKing bus (module off) drops the frame instead of
                               // retransmitting forever - that per-frame retry was the ISR-storm wedge.
  m.identifier = id; m.data_length_code = dlc;
  for (int i = 0; i < dlc && i < 8; i++) m.data[i] = d[i];
  twai_transmit(&m, 0);  // NON-BLOCKING: enqueue if there's room, else drop - never wait
}
// Bus-health service - detect bus-off, self-heal, report state. Called every loop while CAN is open.
// When the module drops off the bus (keyed off mid-probe) the TWAI enters bus-off; we initiate recovery
// and suppress TX (can_busoff) until the bus returns, then re-start. The host is told via EVT_CAN_STATE
// (0x01=bus-off/recovering, 0x00=running/recovered) so it sees the drop explicitly instead of silence.
static void can_service() {
  uint32_t alerts = 0;
  if (twai_read_alerts(&alerts, 0) != ESP_OK) return;  // non-blocking; TIMEOUT when nothing pending
  // EVT_CAN_STATE is DROPPABLE (true): it is an advisory notice, and during a bus-off thrash with the CDC
  // congested a non-droppable send would spin up to 500 ms per event and starve the loop (the wedge seen
  // on the CCP sweep). The local can_busoff flag below still gates TX immediately regardless of delivery.
  if (alerts & TWAI_ALERT_BUS_OFF) {
    can_busoff = true;
    twai_initiate_recovery();              // BUS_OFF -> RECOVERING (completes after 128x11 recessive bits)
    uint8_t s[1] = { 0x01 }; send_frame(EVT_CAN_STATE, s, 1, true);
  }
  if (alerts & TWAI_ALERT_BUS_RECOVERED) {
    twai_start();                          // STOPPED -> RUNNING
    can_busoff = false;
    uint8_t s[1] = { 0x00 }; send_frame(EVT_CAN_STATE, s, 1, true);
  }
}
// Drain received CAN frames to the host. Called ONLY when CAN is open; bounded so a noisy bus can't
// spin this loop and starve the rest of loop().
static void can_poll() {
  twai_message_t rx;
  int budget = 32;
  while (budget-- > 0 && twai_receive(&rx, 0) == ESP_OK) {
    uint8_t p[3 + 8 + 4]; uint16_t n = 0;
    p[n++] = rx.identifier & 0xFF; p[n++] = (rx.identifier >> 8) & 0xFF;
    uint8_t dlc = rx.data_length_code; if (dlc > 8) dlc = 8;
    p[n++] = dlc;
    for (int i = 0; i < dlc; i++) p[n++] = rx.data[i];
    uint32_t ts = micros();
    p[n++] = ts; p[n++] = ts >> 8; p[n++] = ts >> 16; p[n++] = ts >> 24;
    send_frame(EVT_CAN_RX, p, n, true);  // broadcast - droppable
  }
}

// ---------- K-line (Serial2) ----------
static bool kl_up = false;
static uint32_t kl_baud = 9600;  // remembered so a host-triggered reset re-inits at the same baud
// Remembered K-line wire format. Default 8E1 (KLINE_SERIAL_FORMAT = confirmed E46 KWP framing); the host
// can override per CMD_KLINE_OPEN byte 4 to try 8N1/8O1 for the mfg_test_cycle knock, whose config byte
// 0x08 in the disassembly reads as 8N1 (distinct from KWP's 8E1). Low-level UART config, not protocol.
static uint32_t kl_fmt = KLINE_SERIAL_FORMAT;
static uint32_t kl_fmt_from_code(uint8_t c) {
  switch (c) {
    case 1:  return SERIAL_8N1;  // none
    case 2:  return SERIAL_8O1;  // odd
    default: return SERIAL_8E1;  // 0 = even (default)
  }
}

// Full (re)install of the K-line UART: tear down any existing Serial2, HARD-reset the peripheral,
// (re)install with a larger RX ring, reset the RX FIFO. Clears a desynced RX (leading-byte mis-sampling
// observed as the `B8 29 F1 ...` echo read back as `60 95 16 ...`) that a driver-only end()+begin() can't.
// Wire format fixed at 8E1 (KLINE_SERIAL_FORMAT) - the confirmed E46 MK60 DS2 format.
static void kline_reinit() {
  if (kl_up) Serial2.end();
  periph_module_reset(PERIPH_UART2_MODULE);
  Serial2.setRxBufferSize(1024);  // must be called before begin()
  Serial2.begin(kl_baud, kl_fmt, SERIAL_KLINE_RX, SERIAL_KLINE_TX);
  uart_flush_input((uart_port_t)KLINE_UART_NUM);
  kl_up = true;
}
static void kline_open(uint32_t baud, uint32_t fmt) {
  kl_baud = baud;
  kl_fmt = fmt;
  kline_reinit();  // also the host-reconnect recovery path (re-sent CMD_KLINE_OPEN fully re-inits)
}
// ISO-14230 fast-init wake-up: detach the UART from the TX pin, bit-bang LOW 25 ms / HIGH 25 ms, then
// hand the pin back to the UART ready for the host's StartCommunication. Bounded ~50 ms, on-demand.
static void kline_fastinit() {
  if (kl_up) Serial2.end();
  pinMode(SERIAL_KLINE_TX, OUTPUT);
  digitalWrite(SERIAL_KLINE_TX, LOW);
  delay(25);                                     // TiniL
  digitalWrite(SERIAL_KLINE_TX, HIGH);
  delay(25);                                     // TiniH
  Serial2.setRxBufferSize(1024);
  Serial2.begin(kl_baud, kl_fmt, SERIAL_KLINE_RX, SERIAL_KLINE_TX);
  uart_flush_input((uart_port_t)KLINE_UART_NUM);
  kl_up = true;
}
// CMD_KLINE_KNOCK: run the mfg_test_cycle knock `@ 01 02 <sel>` with PRECISE on-device timing. The host
// can't hit the module's ~1-tick selection read-slot over USB (latency + OS-scheduler jitter, worst on
// Windows) - streaming the bytes from the PC seats the ack but the selection lands outside the ~130 ms
// state-2 slot. Doing the paced TX here - each byte flushed fully onto the wire, then a fixed gap (one
// module mailbox tick) - makes the timing deterministic on any host. The echo and the module's ack (0x64)
// and accept ('j' 0x6A / banner 'i' 0x69) flow back via kline_poll() -> EVT_KLINE_RX, which the host already
// captures. Blocks ~ (3+reps) * gap_ms (~70 ms) - fine for the 8 s task watchdog.
static void kline_knock(uint8_t sel, uint8_t gap_ms, uint8_t sel_reps) {
  if (!kl_up) return;
  if (gap_ms == 0) gap_ms = 18;   // the spacing that reliably earns the ack on the bench recipe
  if (sel_reps == 0) sel_reps = 1;
  uart_flush_input((uart_port_t)KLINE_UART_NUM);   // clean window before the knock
  const uint8_t knock[3] = { 0x40, 0x01, 0x02 };
  for (int i = 0; i < 3; i++) {
    Serial2.write(knock[i]);
    Serial2.flush();              // block until the byte is fully shifted onto the wire (~2 ms at 4800)
    delay(gap_ms);
  }
  for (int r = 0; r < sel_reps; r++) {
    Serial2.write(sel);
    Serial2.flush();
    delay(gap_ms);
  }
}
static void kline_poll() {
  // Cap each EVT_KLINE_RX at 192 bytes so one frame always fits the CDC TX ring (keeps drop-on-full in
  // send_frame meaningful); the host concatenates EVT_KLINE_RX payloads, so splitting is transparent.
  uint8_t p[192]; uint16_t n = 0;
  while (kl_up && Serial2.available() && n < sizeof(p)) p[n++] = (uint8_t)Serial2.read();
  if (n) send_frame(EVT_KLINE_RX, p, n, false);  // K-line reply the host waits on - deliver reliably
}

// ---------- raw SPI master (bit-bang) - 25LC040 config-EEPROM read/write ----------
// DUMB relay: the firmware knows NOTHING about the 25LC040 command set. It only clocks whatever bytes the
// host hands it out on MOSI (MSB-first, SPI mode 0: SCK idles LOW, data sampled on the rising edge) while
// sampling MISO per bit, framed by one CS low..high pulse per CMD_SPI_XFER. The host builds every opcode /
// address / data byte and handles the A8-in-opcode addressing, page boundaries and WIP polling. Only built
// on boards that define the SPI pins (V5, PCB_VER > 40); V4 has no such header.
#ifdef SPI_SCK
static bool spi_up = false;
static uint8_t spi_half_us = 1;  // microseconds per half clock (~500 kHz by default; conservative)

static void spi_open(uint8_t half_us) {
  spi_half_us = half_us ? half_us : 1;
  pinMode(SPI_CS, OUTPUT);
  digitalWrite(SPI_CS, HIGH);     // CS idle high - host frames CS per transaction
  pinMode(SPI_SCK, OUTPUT);
  digitalWrite(SPI_SCK, LOW);     // mode 0: clock idles low
  pinMode(SPI_MOSI, OUTPUT);
  digitalWrite(SPI_MOSI, LOW);
  pinMode(SPI_MISO, INPUT);
  spi_up = true;
}

// Clock one byte out/in, MSB-first, SPI mode 0. MOSI is set while SCK is low; the slave samples it on the
// rising edge, and the master samples MISO on that same rising edge.
static uint8_t spi_byte(uint8_t out) {
  uint8_t in = 0;
  for (int b = 7; b >= 0; b--) {
    digitalWrite(SPI_MOSI, (out >> b) & 1);
    delayMicroseconds(spi_half_us);
    digitalWrite(SPI_SCK, HIGH);                 // rising edge: slave samples MOSI, master samples MISO
    in = (uint8_t)((in << 1) | (digitalRead(SPI_MISO) & 1));
    delayMicroseconds(spi_half_us);
    digitalWrite(SPI_SCK, LOW);                  // falling edge
  }
  return in;
}

// One full CS-framed transaction. Transforms the buffer IN PLACE (rx replaces each tx byte) and returns it
// via EVT_SPI_RX, so no second large buffer is needed.
static void spi_xfer(uint8_t* buf, uint16_t len) {
  if (!spi_up) { uint8_t e[1] = { 0x30 }; send_frame(EVT_ERROR, e, 1, false); return; }
  digitalWrite(SPI_CS, LOW);
  delayMicroseconds(spi_half_us);
  for (uint16_t i = 0; i < len; i++) buf[i] = spi_byte(buf[i]);
  delayMicroseconds(spi_half_us);
  digitalWrite(SPI_CS, HIGH);
  send_frame(EVT_SPI_RX, buf, len, false);       // reply the host waits on - deliver reliably
}
#endif // SPI_SCK

#ifdef RELAY_EE
// ---------- relay + ignition/supply sense ----------
// The firmware owns the hardware: it converts the raw ADC pin millivolts to ACTUAL line millivolts (via the
// board dividers) before reporting them, and it self-protects the relay against a lost 12V supply.
static bool     relay_on_state = false;   // is the relay commanded ON?
static uint32_t relay_last_check_ms = 0;  // throttle the safety poll

static uint16_t a5_line_mv() { return (uint16_t)(analogReadMilliVolts(FEEDBACK_A5) * FEEDBACK_A5_DIV); }
#ifdef ANALOG_0
static uint16_t v12_in_line_mv() { return (uint16_t)(analogReadMilliVolts(ANALOG_0) * V12_IN_DIV); }
#endif

static void relay_set(bool on) {
  relay_on_state = on;
  digitalWrite(RELAY_EE, on ? HIGH : LOW);
}

// Safety tick (called from loop): while the relay is ON, if the main 12V input falls below RELAY_MIN_12V_MV
// the coil can't pull in and the gate chatters/buzzes - drop the relay on our own and tell the host. This is
// a hardware precaution that must not depend on the host being connected or polling. Throttled to ~20 ms.
static void relay_safety_tick() {
#ifdef ANALOG_0
  if (!relay_on_state) return;
  uint32_t now = millis();
  if (now - relay_last_check_ms < 20) return;
  relay_last_check_ms = now;
  uint16_t mv = v12_in_line_mv();
  if (mv < RELAY_MIN_12V_MV) {
    relay_set(false);
    uint8_t a[2] = { (uint8_t)(mv & 0xFF), (uint8_t)(mv >> 8) };
    send_frame(EVT_RELAY_TRIP, a, 2, false);
  }
#endif
}
#endif // RELAY_EE

// ---------- command dispatch ----------
static void handle_cmd(uint8_t cmd, const uint8_t* p, uint16_t len) {
  switch (cmd) {
    case CMD_PING:          send_info(); break;
    case CMD_CAN_OPEN:      if (len >= 2) can_open(p[1]); break;  // p[0]=bitrate (ignored, 500k), p[1]=mode
    case CMD_CAN_CLOSE:     can_close(); break;
    case CMD_CAN_FILTER:    /* accept-all in fw; host filters in software */ break;
    case CMD_CAN_TX:        if (len >= 3) can_tx(p[0] | (p[1] << 8), p[2], p + 3); break;
    case CMD_CAN_RESET:     can_reset(); break;
    case CMD_KLINE_OPEN:    if (len >= 4) kline_open((uint32_t)p[0] | (p[1]<<8) | (p[2]<<16) | ((uint32_t)p[3]<<24),
                                                       len >= 5 ? kl_fmt_from_code(p[4]) : (uint32_t)KLINE_SERIAL_FORMAT); break;
    case CMD_KLINE_CLOSE:   Serial2.end(); kl_up = false; break;
    case CMD_KLINE_TX:
      if (len && kl_up) {
        uart_flush_input((uart_port_t)KLINE_UART_NUM);  // drop stale/echo bytes before each transaction
        Serial2.write(p, len);
      }
      break;
    case CMD_KLINE_CFG:     /* reserved: inter-byte timing / echo-suppress */ break;
    case CMD_KLINE_RESET:   if (kl_up) kline_reinit(); break;  // host-triggered UART recovery (no replug)
    case CMD_KLINE_FASTINIT: kline_fastinit(); break;          // ISO-14230 fast-init wake-up (boot knock)
    case CMD_KLINE_KNOCK:   if (len >= 1 && kl_up) kline_knock(p[0], len >= 2 ? p[1] : 0, len >= 3 ? p[2] : 0); break;
#ifdef SPI_SCK
    case CMD_SPI_OPEN:      spi_open(len ? p[0] : 0); break;   // [half_us u8 optional]
    case CMD_SPI_XFER:      if (len) spi_xfer((uint8_t*)p, len); break;  // p aliases rxbuf (writable)
#else
    case CMD_SPI_OPEN:
    case CMD_SPI_XFER:      { uint8_t e[1] = { 0x31 }; send_frame(EVT_ERROR, e, 1, false); } break;  // no SPI header on this board
#endif
#ifdef RELAY_EE
    case CMD_RELAY_SET:     if (len >= 1) relay_set(p[0] != 0); break;                        // 1=relay ON (sink IN)
    case CMD_ADC_READ: {    uint16_t mv = a5_line_mv();                                       // actual line mV at A5
                            uint8_t a[2] = { (uint8_t)(mv & 0xFF), (uint8_t)(mv >> 8) };
                            send_frame(EVT_ADC, a, 2, false); } break;
  #ifdef ANALOG_0
    case CMD_12_V_IN_READ: {    uint16_t mv = v12_in_line_mv();                               // actual line mV at the 12V input
                            uint8_t a[2] = { (uint8_t)(mv & 0xFF), (uint8_t)(mv >> 8) };
                            send_frame(EVT_12_V_IN, a, 2, false); } break;
  #else
    case CMD_12_V_IN_READ:      { uint8_t e[1] = { 0x42 }; send_frame(EVT_ERROR, e, 1, false); } break;  // no input-voltage pin on this board
  #endif
#else
    case CMD_RELAY_SET:
    case CMD_ADC_READ:
    case CMD_12_V_IN_READ:      { uint8_t e[1] = { 0x40 }; send_frame(EVT_ERROR, e, 1, false); } break;  // no relay/ADC pins on this board
#endif
  }
}

// ---------- PC frame parser ----------
static uint8_t  rxbuf[600];
static uint16_t rxexp, rxlen, rxcrc;
static uint8_t  rxstate, rxcmd;
static void pc_feed(uint8_t b) {
  switch (rxstate) {
    case 0: if (b == PROTO_SYNC) rxstate = 1; break;
    case 1: rxcmd = b; rxstate = 2; break;
    case 2: rxexp = b; rxstate = 3; break;
    case 3: rxexp |= (uint16_t)b << 8; rxlen = 0;
            rxstate = (rxexp <= sizeof(rxbuf)) ? (rxexp ? 4 : 5) : 0; break;
    case 4: rxbuf[rxlen++] = b; if (rxlen >= rxexp) rxstate = 5; break;
    case 5: rxcrc = b; rxstate = 6; break;
    case 6: {
      rxcrc |= (uint16_t)b << 8;
      uint8_t h[3] = { rxcmd, (uint8_t)(rxexp & 0xFF), (uint8_t)(rxexp >> 8) };
      uint16_t c = crc16(h, 3, 0xFFFF); c = crc16(rxbuf, rxexp, c);
      if (c == rxcrc) handle_cmd(rxcmd, rxbuf, rxexp);
      rxstate = 0;
    } break;
  }
}

#ifdef RELAY_EE
// Ignition-timing rig. EE MOSFET gate (RELAY_EE): HIGH = relay ON (sink the relay IN low). GPIO 3 is an
// S3 strapping pin, so drive it LOW here as early as possible so the relay starts and stays OFF. A5
// (FEEDBACK_A5) senses the switched 12V through the board's 10k/2.2k divider; 11dB attenuation covers the
// ~2.2V the divider yields at 12V.
static void relay_setup() {
  pinMode(RELAY_EE, OUTPUT);
  digitalWrite(RELAY_EE, LOW);
  analogSetPinAttenuation(FEEDBACK_A5, ADC_11db);
#ifdef ANALOG_0
  analogSetPinAttenuation(ANALOG_0, ADC_11db);  // main-connector 12V input sense (CMD_12_V_IN_READ)
#endif
}
#endif

void setup() {
  Serial.begin(921600);
  boot_reason = (uint8_t)esp_reset_reason();  // POWERON=replug, TASK_WDT=the last-resort fired, etc.
#ifdef RELAY_EE
  relay_setup();                              // relay OFF + feedback ADC ready, before anything else
#endif
#if ARDUINO_USB_CDC_ON_BOOT
  Serial.setTxTimeoutMs(0);  // V5 USB-CDC: fully non-blocking write()/availableForWrite() (see send_frame)
#endif
  // A minimal last-resort task watchdog. During a K-line-only session the loop does no CAN work and cannot
  // hang. A CAN bus-off (car keyed off mid-probe) is now handled by can_service() without a reboot - the
  // bridge stays up and the host is told via EVT_CAN_STATE. This watchdog remains only for a truly
  // unforeseen hang; if it ever fires it reboots the chip (~8 s) and the host reconnects.
  esp_task_wdt_init(8, true);
  esp_task_wdt_add(NULL);
  send_info();
}
void loop() {
  esp_task_wdt_reset();                                        // feed the last-resort watchdog
  while (Serial.available()) pc_feed((uint8_t)Serial.read());  // host commands
  if (can_up) { can_service(); can_poll(); }                   // GATED: zero TWAI work unless CAN is open
  kline_poll();                                                // K-line RX (no-op unless K-line is open)
#ifdef RELAY_EE
  relay_safety_tick();                                         // auto-drop the relay if the 12V supply is lost
#endif
}
