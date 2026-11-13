# KWP SID 0x22 DID Analysis

As of 2026-11-12. Analysis notes for the MK60-M-Core-Disassembly-Process repository. Companion to `kwp_dispatch.md`.

## Summary

KWP service 0x22, ReadDataByCommonIdentifier, on the MK60E5 (handler `kwp_22_rd_common_id` at 0xB23CC) reads four EEPROM blocks and a handful of live values, and nothing else. It is a hard-coded switch over 17 data identifiers (DIDs): ten queue an EEPROM read and answer later through the KWP layer's deferred-reply handle, the rest return RAM or ROM values, and any unknown DID is tried as an F-CAN message id.

The EEPROM it reaches is the raw coding block (DID 0x3000 and its test stamp 0x3FFF), slices of the identification block (0x2500 to 0x2506), the three-byte Prüfstempel (0x1000) and one 11-byte record with no other consumer (0x1603). It does not reach the sensor zero-offset record, the valve calibrations or the tyre-pressure learning data; those are written by routines and loaded at boot, and a tester gets at them only through ReadMemoryByAddress (0x23) and WriteMemoryByAddress (0x3D). The closest thing to a steering parameter here is DID 0x3001, one coding byte that is also pushed to the steering-angle sensor.

The write side, 0x2E, mirrors five of the DIDs. Tracing the handler also exposed two recovery gaps in our Ghidra pipeline, which are being fixed separately.

## How the handler works

The dispatcher (`kwp_dispatch`, see `kwp_dispatch.md`) hands SID 0x22 to `kwp_22_rd_common_id` after its own gates: session mask 0x94, no security level, and a fixed request length of 3 (SID plus a two-byte DID). The handler then switches on the 16-bit DID. There is no DID table; every case is spelled out in code.

Three kinds of case exist:

1. **EEPROM reads.** The case queues a 12-byte job descriptor from the `eeprom_job_descriptors` area at 0xF31B4 through `nvm_queue_job`. The descriptor names a RAM buffer, a record id, a length, an offset within the record and a mode byte (1 = read, 2 = write). The reply is not sent at once: the handler sets the response length, stores the record id plus one in the 16-bit wait handle at 0x409436 and the layout code 3 at 0x4094D5, and returns. The KWP cycle at 0xB0430 polls the job every tick and assembles and sends the reply when it completes. If the five-slot job queue is full, the case answers 0x21 busyRepeatRequest.
2. **Live values.** The case copies RAM or ROM bytes into the response buffer at 0x40948C and sets `diag_resp_ready` itself. Some of these call helpers that gather data from other modules.
3. **Fallback.** Any DID not in the switch goes to the function at 0xC55C0, which walks `fcan_msg_table` (stride 0x17) comparing the DID with each entry's 32-bit message id. On a match the helper at 0x6D448 copies that frame's payload out of the CAN controller mailbox (0xDD0000 or 0xFA0000), and the reply is the payload plus three bytes. No match gives 0x12.

The read buffer is the same 0x40943E scratch area every EEPROM read in the KWP layer uses, and the deferred-reply mechanism is the one routine 0x0A of RoutineControl uses.

## DID table

EEPROM positions come from cmg5461's `EEPROM_MAP_7846816A.md`: a descriptor's 16-bit id is `(block << 4) | sub-id`, the block table at 0xD7424 places each block in the 2 KiB logical space, and the chip stores every byte inverted. Reply bytes include the three-byte positive header. Confidence: **high** = behaviour read from code; **medium** = address certain, meaning from the map or the owning module; **low** = inferred.

| DID | Source | Reply bytes | EEPROM (logical) | 0x2E write | Meaning | Confidence |
| --- | --- | --- | --- | --- | --- | --- |
| 0x1000 | block 11 (id 0xBB) +0x0E, 3 B | 6 | 0x1B7..0x1B9 | yes, 3 B | Prüfstempel, the test stamp | medium |
| 0x1001 | live, only when the coding bit `coding_field_read` is set | 27 | - | yes, with range checks (0..100, 0..31, value×100) | DDS/RPA tyre parameters for two axles, read from the wheel records at 0x40BF00 and 0x40BFC0 through the getters at 0x95D2C, 0x95C04, 0x95DB2 and 0x95CF6; 31000 and 32000 are invalid markers | medium |
| 0x1600 | constant | 4 | - | no | 00 00 00 03 | high |
| 0x1601 | live via the function at 0xC6EB8, selectors 1, 0 and 4 | 16 | - | no | steering-angle-sensor module data (the F-CAN 0x0C9 block); the callee is not yet recovered in the project | low |
| 0x1602 | live via 0xC71A6: a 12-byte checksummed string at 0x400967, then a 10-byte string received from the instrument cluster at 0x402E55 | 33 | - | no | identification strings | low |
| 0x1603 | block 47 (0x2F3) +0, 11 B into 0x409446 | 22 | 0x26F..0x279 | no | unknown; no other consumer in the image | medium |
| 0x2500 | block 24 (0x183) +0x26, 54 B | 57 | 0x348..0x37D | no | identification data, content unknown | medium |
| 0x2501 | block 24 +0x9C, 3 B, then fixed 00 00 00 0A | 7 | 0x3BE..0x3C0 | no | unknown | medium |
| 0x2502 | block 24 +0x6C, 21 B | 24 | 0x38E..0x3A2 | no | unknown | medium |
| 0x2503 | ROM 0xF56D8, 12 bytes copied three times | 39 | - | no | the software reference string `01818I000900`, the .0pa REFERENZ | high |
| 0x2504 | constant | 4 | - | no | four zero bytes | high |
| 0x2506 | block 24 +0x9F, 2 B | 5 | 0x3C1..0x3C2 | no | unknown | medium |
| 0x3000 | block 10 (0xA3) +0, 48 B | 51 | 0x400..0x42F | yes: 51-byte request, session 0x10, `coding_validate_write` | the raw coding bytes as the tester wrote them | high |
| 0x3001 | live via 0xC6EB8 selector 5 | 4 | 0x430 on write | yes, 1 B: block 10 +0x30, then command 0x30 to the steering-angle sensor through 0xC72EC | steering-sensor coding byte, loaded at boot to 0x402F4B | medium |
| 0x3FFF | block 10 +0x31, 2 B | 5 | 0x431..0x432 | yes, 2 B | coding test stamp (Codierprüfstempel) | medium |
| any other | 0xC55C0 matches the DID against the 32-bit message id of each `fcan_msg_table` entry; 0x6D448 copies the frame from the CAN controller mailbox | payload + 3 | - | no | raw F-CAN frame read by message id | medium |

The deferred-reply wait handles the EEPROM cases store are 0x84, 0xA4, 0xBC and 0xF4, each the descriptor id plus one, which is how the KWP cycle finds the job to poll.

## EEPROM blocks reached and not reached

The chip is a 2 KiB SPI EEPROM on QSPI chip-select 2, tiled into 27 populated blocks by the table at 0xD7424. SID 0x22 touches four of them:

| Block | Logical range | What SID 0x22 reads | Other users |
| --- | --- | --- | --- |
| 10, raw coding | 0x400..0x44A | 0x3000 (48 B), 0x3FFF (2 B), 0x3001 writes 0x430 | 0x2E writes; nothing else reads block 10, the control code runs from the unpacked image in block 7 |
| 24, identification | 0x322..0x3FF | 0x2500, 0x2501, 0x2502, 0x2506 | ReadEcuIdentification 0x1A views (VIN at 0x3A4, part numbers, dates), RoutineControl 0x07 and 0x0A around 0x323..0x335 |
| 11 | 0x1A9..0x1BA | 0x1000, the Prüfstempel at 0x1B7 | none found |
| 47 | 0x26F..0x280 | 0x1603, 11 bytes | none found |

It does not touch the blocks that hold the vehicle's learned and calibrated state: block 8, the sensor zero-offset record written by routines 0x20 to 0x24; block 16, the inlet and MCI valve calibrations written by routines 0x28 and 0x29; blocks 9 and 14, the DDS/RPA parameters and learned data; blocks 3, 5, 27 and 28, fault memory. Those are loaded at boot and written by their owning modules, and a tester reaches them only through ReadMemoryByAddress (0x23) and WriteMemoryByAddress (0x3D), which accept any logical address below 0x800 in four-byte steps.

## The write side: WriteDataByCommonIdentifier (0x2E)

`kwp_2e_wr_common_id` at 0xB2874 accepts five DIDs and refuses everything else with 0x12. Data is staged at `coding_write_buf` (0x403320) and written through the descriptors at `eeprom_job_descriptors` +0xA8 to +0xCC.

| DID | Request length | Gate | What is written |
| --- | --- | --- | --- |
| 0x1000 | 6 | none beyond the dispatcher | 3 bytes to block 11 +0x0E, the Prüfstempel |
| 0x1001 | 15 | second byte must be 1; axle selector 2 or 6 | tyre parameters into the wheel records through 0x95A20, 0x95B62 and 0x95C4E; values outside 0..100, 0..31 or the pressure range give 0x31 |
| 0x3000 | 51 | session 0x10; `coding_validate_write` must pass, else 0x31 | 48 coding bytes to block 10 +0; the validator also unpacks them into the active image in block 7 |
| 0x3001 | 4 | session 0x10 | 1 byte to block 10 +0x30, then command 0x30 with that byte to the steering-angle sensor (0xC72EC); 0x21 if the sensor interface refuses |
| 0x3FFF | 5 | none | 2 bytes to block 10 +0x31, the coding test stamp |

The EEPROM writes defer their reply exactly as the reads do. `coding_validate_write` only validates when the state nibble `state_nibble_get(24, 1)` reads 4; otherwise it returns 0xFE and the handler stores the raw bytes without touching the active image, which cmg5461's map documents.

## The other EEPROM write paths: 0x3B and 0x3D, with 0x23

Three more services reach the EEPROM from the tester. Each has a tiny set of sub-identifiers.

**WriteDataByLocalIdentifier (0x3B, handler 0xB2338, security level 2)** accepts two local identifiers and refuses the rest with 0x12. Both writes are deferred through the wait handle like every other EEPROM job.

| Local id | Request | Writes | EEPROM (logical) | Notes |
| --- | --- | --- | --- | --- |
| 0x8F | 32 bytes: SID, LID, 30 data | descriptor +0x60: 30 B to block 0 +0 | 0x000..0x01D | the record ReadEcuIdentification 0x1A 0x8F returns; meaning unknown, 0x8F is not an ISO 14230 identifier |
| 0x90 | 9 bytes: SID, LID, 7 data | descriptor +0x6C: 7 B to block 24 +0x82 | 0x3A4..0x3AA | the short VIN, read back by 0x1A 0x90. This is the identifier the dispatcher exempts from the session and security gates, so the VIN can be written in any session without SecurityAccess |

**WriteMemoryByAddress (0x3D, handler 0xB324E, security level 0x7F, session 0x10 only)** writes exactly one byte per request. The request is SID, a three-byte address whose top byte must be zero and whose value must be below 0x800, memory type 3, length 1, and the byte; anything else gives 0x31 (0x12 for a memory type other than 3). The write goes through `nvm_request` synchronously, and the reply carries the address back. With the top security level this is raw access to the whole 2 KiB EEPROM, one byte at a time.

**ReadMemoryByAddress (0x23, handler 0xB3112, no security)** is the read counterpart and has two memory types. Type 3 reads 1 to 4 bytes at any logical address below 0x800, synchronously, which is how a bench dump of the chip would be taken. Type 7 reads one of two fixed 18-byte windows of block 24 by giving address 0x00 or 0x12: descriptor +0xFC at block offset 0x14 (logical 0x336, the VM identification table) and descriptor +0x108 at offset 0x81 (logical 0x3A3, the VIN and programming date). These replies are deferred.

## Open questions and recovery gaps

- [ ] **The block 24 slices.** DIDs 0x2500, 0x2501, 0x2502 and 0x2506 return 80 bytes of the identification block whose meaning is unknown; cmg5461's map marks them "?" too. A real tester log against a known car would settle them.
- [ ] **DID 0x1603.** Eleven bytes of block 47 with no other consumer in the image.
- [ ] **DIDs 0x1601 and 0x3001.** Both go through the function at 0xC6EB8, which is raw bytes in the project (recovery gap, below). Its selectors read the steering-angle-sensor module's state; which fields come back is unknown until it is recovered.
- [ ] **DID 0x1602.** The 12-byte string at 0x400967 is checksummed in RAM, the 10-byte string comes from the instrument cluster; whether either is the VIN is unverified.
- [ ] **Recovery gap 1.** The function at 0xC6EB8 (through about 0xC6FA0) is a prologue-less leaf called through a pool slot that the recovery never typed, so it was never disassembled and no verify gate sees it.
- [ ] **Recovery gap 2.** Eight prologue-less helpers at 0xD5474..0xD6734, targets of 36 `jsri` calls from drivers and interrupt handlers, sit above the auto-pinned data boundary (0xD5430) and were cleared and relabelled as table bases by `DataSectionClean`. Both gaps are being fixed in the pipeline; until then, any decompile that reaches those addresses shows `UNK_` or table labels.

## Method and provenance

All addresses and bytes are from the E9x M3 image 7846816A, project `projects/E9x_M3_7846816A`, read headlessly from snapshots with the decompiler's read-only flag on; nothing was written to the project. Descriptor bytes were decoded from the raw image. EEPROM block numbers, logical addresses and the meanings marked "map" come from cmg5461's `analysis/EEPROM_MAP_7846816A.md` in the mk60e5-firmware repository, which is a static analysis not yet checked against a real chip dump. Names in `FUN_` form or given as bare addresses are unnamed functions.
