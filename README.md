# electrolux-remote

M5StickC Plus2 firmware: press **Button A** to transmit the Electrolux AC **OFF** frame
over the built-in IR LED (GPIO19). Built on the [PaperUI](https://github.com/iac26/PaperUI)
Rust framework (`paperui` engine + widgets + the embedded-graphics adapter), plus the
`paperui-tft` board addon for this device — its `TftTheme` look and `ButtonReader` input —
the app-local `electrolux-ir` protocol crate, and the RMT `IrTx` transmitter (`src/ir.rs`).

> Depends on PaperUI via git deps (`github.com/iac26/PaperUI`); run `cargo update -p paperui
> -p paperui-tft` to pull the latest framework commit.

## Build
```bash
source ~/export-esp.sh
cargo +esp build -Zbuild-std=core --target xtensa-esp32-none-elf --release
```
Produces `target/xtensa-esp32-none-elf/release/electrolux-remote` (a valid Xtensa ELF).

Run the protocol unit tests on the host:
```bash
cargo test -p electrolux-ir --target x86_64-unknown-linux-gnu
```

## Flash (requires the device on USB + espflash)
```bash
cargo install espflash            # one-time
source ~/export-esp.sh
# With the StickC connected (CP210x/CH9102 serial port):
espflash flash --monitor \
  target/xtensa-esp32-none-elf/release/electrolux-remote
```

## On-hardware smoke test
1. On boot the TFT shows a focused "AC OFF" button on a dark background.
2. Point the top edge (IR LED) at the AC; press **Button A**.
3. The serial monitor prints `BtnA: sending AC OFF`; the AC turns off.
4. If nothing happens, work down this list:
   - Confirm a phone camera sees the IR LED flash on each Button A press (verifies the
     RMT carrier + GPIO19 wiring). If no flash → IR LED pin / RMT carrier issue.
   - If the LED flashes but the AC ignores it, scope-check the 38 kHz carrier
     (`carrier_high`/`carrier_low` = 13 ticks at the 1 MHz RMT channel clock; this is
     correct for the plain ESP32 per the TRM but is the one value unverified off-hardware).
   - If the screen is shifted/garbled, adjust `display_offset(52, 40)` / `display_size`
     (and consider `.orientation(...)` / `.invert_colors(...)`) in `src/main.rs`.

## Pin map (VERIFY against your board)
These are the commonly-documented M5StickC Plus2 pins; confirm against your unit's schematic:

| Function | GPIO |
|----------|------|
| TFT SCLK | 13 |
| TFT MOSI | 15 |
| TFT CS   | 5  |
| TFT DC   | 14 |
| TFT RST  | 12 |
| TFT Backlight | 27 |
| Button A | 37 (input-only, board pull-up) |
| Button B | 39 (input-only, board pull-up) |
| IR LED   | 19 |

## What is verified vs. not
- **Verified (host `cargo test`):** the Electrolux OFF frame is byte-exact
  (`[C3 E1 00 00 06 00 00 00 00 00 00 00 55]`) and the mark/space pulse encoding
  (MSB-first, header 8950/4530, bit mark 563, "1" 1690 / "0" 538, footer 563 + gap).
- **Verified (Xtensa compile):** the whole firmware type-checks and links against the
  real `esp-hal` 1.1 SPI/RMT/GPIO + `mipidsi` 0.9 APIs into a valid ESP32 ELF.
- **Needs hardware:** actual display output, the emitted carrier frequency, and the AC
  responding — exercise these with the smoke test above.
