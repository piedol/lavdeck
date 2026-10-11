# DJI Mic Mobile RX — USB control protocol

What Lavdeck knows about talking to the DJI Mic Series Mobile RX (the USB‑C plug‑in receiver) over USB, and how it was confirmed. Everything here was observed on real hardware; nothing in this repository contains DJI code.

Tested setup: Mobile RX (reports itself as "DJI Mic Mini 2", firmware 02.03.11.00) with two DJI Mic Mini transmitters, macOS 26 on Apple Silicon. Other DJI receivers may differ.

> **Safety first.** The receiver also exposes a firmware‑update channel. Two of the commands below can put it into update mode or reset it. Lavdeck can only send the allowlisted commands in [Allowlist](#what-lavdeck-sends), and has no API for arbitrary frames.

## USB layout

USB ID `2ca3:4011`, full speed, product string `Wireless Mic Rx`.

| Interface | Class | Role |
|---|---|---|
| 0 | HID | Media keys (report `0x06`), telephony (report `0x07`), and two vendor reports (`0x55`, `0x41`) |
| 1, 3 | USB Audio | The 2‑channel 48 kHz microphone macOS sees |
| 6 | Vendor `0xFF`/`0xF0`, bulk IN `0x86` / OUT `0x06` | **DJI DUML control and status** |

Interface 6 is also attached by macOS's `accessoryd`. It can still be opened with IOUSBHost (no seize) or libusb, and both readers coexist. The receiver mixes iAP2 link‑sync bytes (`ff 55 02 00 ee 10`) into the stream; ignore them.

### HID report `0x55` is the firmware‑update service

Requests are `55 <cmd> …` on output report `0x55`; replies come back as `55 <type> …`.

| Cmd | Meaning | Lavdeck |
|---|---|---|
| `0x55` | Enter firmware update | **never sent** |
| `0x52` | Reboot | **never sent** |
| `0x59` | Connected transmitter count | read‑only |
| `0x5e` | Receiver version | read‑only |
| `0x6a` | Connected transmitter index | read‑only |

HID report `0x41` did not answer DUML in testing; control lives on interface 6.

### Transmitter button

A short press of a transmitter's link button arrives as a consumer **volume‑up** key (report `0x06`, bit 0). Both transmitters send the same event, so the Mac cannot tell which one was pressed. Lavdeck opens the HID device exclusively when "tap to record" is on, so the key stops changing the system volume.

## DUML v1 frames (interface 6)

```
0      0x55
1-2    length (10 bits, whole frame incl. CRC16) | version 1 << 10
3      CRC8 of bytes 0-2        (poly 0x31 reflected = 0x8C, init 0x77)
4      sender   (type | index << 5)
5      receiver (type | index << 5)
6-7    sequence (little endian)
8      command type (0x00 push, 0x40 request with ack, bit 7 = response)
9      command set
10     command id
11..   payload
n-2..  CRC16 of everything before it (poly 0x1021 reflected = 0x8408, init 0x3692), LE
```

Addresses: receiver `0x5a` (type 0x1a, index 2), host/app `0x02`. The receiver pushes status to `0x02` several times a second without being asked.

## Command set `0x5B` (wireless mic)

| ID | Name | Notes |
|---|---|---|
| `0x01` | set params | TLV settings, see below |
| `0x03` | status push | unsolicited, see below |
| `0x12` | get capability | read‑only |
| `0x35` | HID firmware update | **never sent** |

Also answered: general `0x00/0x01` get version (hardware name `DJI MIC MINI RX`).

### Status push `0x5B/0x03` (version 3)

Payload: `03, total length (u16 LE), records…`. Each record is `type (1), index (u32 LE: 0 = receiver, 1 = TX1, 2 = TX2), length (1), value`. Offsets below are from the record start `r`; the value starts at `r+6`.

| Type | Content |
|---|---|
| `0x01` | firmware (4 bytes, printed in reverse: `00 0b 03 02` → `02.03.11.00`) + serial (ASCII) |
| `0x02` | transmitter status bits, `w = u32 @ r+6` |
| `0x03` | receiver status, `h = u16 @ r+6` plus fixed‑offset bytes |
| `0x04` | Bluetooth ID suffix (ASCII) |
| `0x05` | level meter (1 byte, receiver units) |
| `0x06` | product name (ASCII) |

Transmitter `w` (type `0x02`): bit 2 mute, bits 5–6 noise‑cancel level, bit 8 noise cancel, bit 9 charging, **bits 10–12 battery (0–7, receiver estimate)**, signed gain at byte `r+0x0d`.

Receiver `h` (type `0x03`): bits 0–1 environment, bit 2 lossless, bits 3–4 noise‑cancel level, **bit 5 noise cancel**, bit 8 auto power‑off, **bits 9–11 channel mode**, bits 13–15 receiver battery. Signed **gain (dB)** at byte `r+0x08`. **Connected transmitter mask** at byte `r+0x1e` (bit 0 = TX1, bit 1 = TX2). Byte `r+0x22` bit 3 = attached over USB.

On the Mobile RX with Mic Mini transmitters, both transmitter records often carry identical values, so per‑transmitter battery and meter are best treated as approximate.

### Set params `0x5B/0x01`

Payload: `02 | target index (u32 LE, 0 = receiver) | setting id (u16 LE) | 01 | value`. The receiver answers with command type `0x80`, same sequence, payload = result (`0x00` ok, `0xff` rejected).

| Setting | ID | Values (verified) |
|---|---|---|
| Channel mode | `0x0008` | `0` mono, `2` stereo (`1` is rejected) |
| Receiver gain | `0x0039` | signed dB: −12, −6, 0, +6, +12 |
| Transmitter mute | `0x0009` | accepted but has **no audible effect** on this hardware |

> An older layout (`00 | index u8 | setting…`) for channel mode makes the Mobile RX drop off USB and reconnect in mono. Lavdeck never sends it.

### Capability `0x5B/0x12`

Request: `index (u32 LE) | capability id (u16 LE)` with `0` = all, `5` = channel modes, `6` = gain. Response: `total (u16) | (index u32 | length u16 | items)…`, item = `id (u16) | length (1) | value`. On the Mobile RX the gain range is −12…+12 dB in steps of 6, and the channel‑mode mask is `0x07`.

## What Lavdeck sends

Only these, built by `MicCommand` in `Sources/LavdeckKit/Commands.swift`:

- `0x00/0x01` get version
- `0x5B/0x12` get capability
- `0x5B/0x01` set channel mode (mono or stereo only)
- `0x5B/0x01` set receiver gain (−12…+12 dB)

## How this was worked out

By interoperability analysis: listening to the receiver's own USB traffic, reading which commands DJI's mobile app is built to send, and confirming every write on hardware against the receiver's status and recorded audio. If you have a different DJI receiver and want to help, the `lavdeck` command‑line tool (`lavdeck status --json`, `lavdeck caps`) is read‑only and a good place to start.
