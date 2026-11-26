#pragma once
#include <stdint.h>
// Serial bridge protocol v0.1 - see bench/protocol.md. Must stay in sync with host/src/proto.rs.
// Frame: SYNC(0x55) | CMD | LEN(u16 LE) | PAYLOAD | CRC16(LE, CCITT-FALSE over CMD+LEN+PAYLOAD)

#define PROTO_SYNC 0x55
#define FW_VER     0x0015

// host -> dongle
#define CMD_PING        0x01
#define CMD_CAN_OPEN    0x10   // [bitrate u8 (0=500k)][mode u8 (0=normal,1=listen)]
#define CMD_CAN_CLOSE   0x11
#define CMD_CAN_FILTER  0x12   // [id u16][mask u16]  (fw accepts all; host filters)
#define CMD_CAN_TX      0x13   // [id u16][dlc u8][data...]
#define CMD_CAN_RESET   0x14   // (no payload) re-init TWAI in place (same mode). On-demand; added FW 0x000B.
#define CMD_KLINE_OPEN  0x20   // [baud u32][fmt u8 optional: 0=8E1(default) 1=8N1 2=8O1]
#define CMD_KLINE_CLOSE 0x21
#define CMD_KLINE_TX    0x22   // [bytes...]
#define CMD_KLINE_CFG   0x23   // [p1 u16][p4 u16][echo_suppress u8]
#define CMD_KLINE_RESET 0x24   // (no payload) re-init the K-line UART in place - recovers a desynced
                               // RX without a USB replug. Added FW 0x0005; host side updated separately.
#define CMD_KLINE_FASTINIT 0x25 // (no payload) ISO-14230 fast-init wake-up on K-line (LOW 25ms/HIGH 25ms)
                               // then UART ready for StartCommunication. Added FW 0x0008.
#define CMD_KLINE_KNOCK 0x26   // [sel u8][gap_ms u8 optional=18][sel_reps u8 optional=1] run the
                               // mfg_test_cycle knock `@ 01 02 <sel>` with PRECISE on-device timing. The
                               // host can't reliably hit the module's ~1-tick selection read-slot over USB
                               // (latency jitter, worst on Windows); pacing the TX locally lands it on any
                               // host. Echo + the module's ack/accept flow back as EVT_KLINE_RX. FW>=0x0015.
#define CMD_SPI_OPEN    0x30   // [half_us u8 (optional; 0/absent=default)] init the bit-bang SPI master on
                               // the V5 SPI pins (config.h SPI_CS/MISO/MOSI/SCK). Mode 0, MSB-first, CS
                               // idle-high and driven manually. half_us = microseconds per half clock.
                               // Added FW 0x000C. DUMB relay: no 25LC040 chip logic lives here.
#define CMD_SPI_XFER    0x31   // [bytes...] one full CS-framed SPI transaction: CS low, clock the bytes
                               // out on MOSI (MSB-first) while sampling MISO per bit, CS high. The host
                               // builds each 25LC040 command (opcode+addr+data). Reply: EVT_SPI_RX with the
                               // same byte count received. Added FW 0x000C.
#define CMD_RELAY_SET   0x40   // [state u8] drive the EE MOSFET (relay): 1 = ON (sink IN), 0 = OFF. FW>=0x0012.
#define CMD_ADC_READ    0x41   // (no payload) read A5 ignition-feedback -> EVT_ADC [mv u16]. FW>=0x0012.
#define CMD_12_V_IN_READ 0x42  // (no payload) read the main-connector 12V input (ANALOG_0/GPIO9) -> EVT_12_V_IN
                               // [mv u16] actual LINE millivolts (divider applied on-device). FW>=0x0013.

// dongle -> host
#define EVT_INFO        0x81   // [fw_ver u16][board u8][caps u8][reset_reason u8 (FW>=0x0007)]  (phase byte removed in 0x000B)
#define EVT_CAN_RX      0x90   // [id u16][dlc u8][data...][ts_us u32]
#define EVT_CAN_STATE   0x91   // [state u8] TWAI bus health: 0x00=running/recovered, 0x01=bus-off/recovering.
                               // Emitted on transitions only. Added FW 0x000D (bus-off auto-recovery).
#define EVT_KLINE_RX    0xA0   // [bytes...]
#define EVT_SPI_RX      0xB0   // [bytes...] MISO bytes from one CMD_SPI_XFER (same count as sent). Added FW 0x000C.
#define EVT_ADC         0xC0   // [mv u16] actual LINE millivolts at A5 (switched 12V; divider applied on-device). FW>=0x0012 (line mV since 0x0014).
#define EVT_12_V_IN     0xC1   // [mv u16] actual LINE millivolts at the main 12V input (ANALOG_0; divider on-device). FW>=0x0013 (line mV since 0x0014).
#define EVT_RELAY_TRIP  0xC2   // [mv u16] the firmware auto-dropped the relay: main 12V fell to this (line mV) while ON. FW>=0x0014.
#define EVT_ERROR       0xE0   // [code u8][detail...]
