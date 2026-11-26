//! Stage the firmware images the flasher embeds, preferring the live PlatformIO build output
//! (`../firmware/.pio/build`) and falling back to the committed snapshot in `firmware/`. Copies
//! into OUT_DIR so `include_bytes!` always picks up the freshest available image — on every
//! `cargo build`/`cargo run`, not just via make-app.sh. `cargo:rerun-if-changed` on each source
//! means a firmware rebuild re-triggers this automatically.

use std::path::{Path, PathBuf};

fn main() {
    let out = PathBuf::from(std::env::var("OUT_DIR").unwrap());
    let home = std::env::var("HOME").unwrap_or_default();
    let pio = Path::new("../firmware/.pio/build");
    let boot_app0 = format!(
        "{home}/.platformio/packages/framework-arduinoespressif32/tools/partitions/boot_app0.bin"
    );

    // (embedded name, preferred live source, committed fallback)
    let items: [(&str, PathBuf, &str); 7] = [
        ("boot_app0.bin", PathBuf::from(&boot_app0), "firmware/boot_app0.bin"),
        ("v4_bootloader.bin", pio.join("gaugesV4/bootloader.bin"), "firmware/v4/bootloader.bin"),
        ("v4_partitions.bin", pio.join("gaugesV4/partitions.bin"), "firmware/v4/partitions.bin"),
        ("v4_firmware.bin", pio.join("gaugesV4/firmware.bin"), "firmware/v4/firmware.bin"),
        ("v5_bootloader.bin", pio.join("gaugesV5/bootloader.bin"), "firmware/v5/bootloader.bin"),
        ("v5_partitions.bin", pio.join("gaugesV5/partitions.bin"), "firmware/v5/partitions.bin"),
        ("v5_firmware.bin", pio.join("gaugesV5/firmware.bin"), "firmware/v5/firmware.bin"),
    ];

    for (name, preferred, fallback) in &items {
        println!("cargo:rerun-if-changed={}", preferred.display());
        println!("cargo:rerun-if-changed={fallback}");
        let src = if preferred.exists() { preferred.clone() } else { PathBuf::from(fallback) };
        let data = std::fs::read(&src)
            .unwrap_or_else(|e| panic!("firmware image {}: {e}", src.display()));
        std::fs::write(out.join(name), data)
            .unwrap_or_else(|e| panic!("staging {name}: {e}"));
    }
}
