# Pico Autofill

Embassy/Rust firmware for a standard Raspberry Pi Pico (RP2040, no wireless hardware required). It uses the built-in `BOOTSEL` button as both a configuration gesture and an autofill trigger.

## Behavior

- Single click: types the saved value as a USB keyboard after the 650 ms multi-click window closes.
- Double click: does nothing.
- Triple click: inserts a 32 KiB removable FAT12 volume named `PICO FILL`.
- Edit the only visible file, `AUTOFILL.TXT`, then save and eject/unmount the volume.
- Eject, synchronize-cache, or the first single click after unmount commits changed text to the Pico's final 4 KiB flash sector.
- The click input uses 30 ms debounce. A possible single click is delayed until it cannot be the beginning of a triple click, so a triple click never types text.

`AUTOFILL.TXT` is limited to the first 50 printable US-ASCII characters. A trailing CR/LF is removed, unsupported bytes are dropped, and excess text is truncated. The USB keyboard mapping assumes a US keyboard layout.

The flash record has a version marker and CRC. Invalid or erased flash loads as an empty value. The linker script reserves `0x001ff000..0x001fffff`, preventing firmware from overlapping that record.

## Build and flash

Install the target and UF2 converter if needed:

```sh
rustup target add thumbv6m-none-eabi
cargo install elf2uf2-rs
```

Build and create a UF2:

```sh
cargo build --release
elf2uf2-rs target/thumbv6m-none-eabi/release/pico-autofill pico-autofill.uf2
```

Hold `BOOTSEL` while plugging in the Pico, then copy `pico-autofill.uf2` to the `RPI-RP2` drive. With a debug probe, `cargo run --release` uses the configured `probe-rs` runner instead.

## Use

1. Plug in the flashed Pico normally.
2. Triple-click `BOOTSEL`. The host may need a second or two to notice the newly inserted medium.
3. Open `AUTOFILL.TXT`, replace its contents with one line of up to 50 printable ASCII characters, and save.
4. Eject or unmount `PICO FILL`.
5. Focus the destination field and single-click `BOOTSEL`.

On systems where unmount does not send the USB eject command, the next single click safely removes the already-unmounted medium, commits it, and then types the updated value.

## Test

The platform-independent click, FAT12, flash-record, text-filtering, and key-mapping logic has host tests:

```sh
cargo test --target x86_64-unknown-linux-gnu --lib
cargo clippy --release -- -D warnings
```

## Security and flash wear

The value is plaintext in flash and is intentionally emitted as keyboard input. Do not treat the Pico as encrypted secret storage, and only plug it into systems where keystroke injection is appropriate.

Flash is erased only when the mounted filesystem changed and the host syncs/ejects (or you click after unmount). Avoid continuously rewriting the file if flash endurance matters.

The example USB VID/PID is `1209:2040`; obtain and substitute identifiers appropriate for any distributed product.

