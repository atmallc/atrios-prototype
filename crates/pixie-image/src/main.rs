//! `pixie-image`: builds a Pixie boot image for the Pixel 2.
//!
//! ```text
//! pixie-image repack --stock boot.img --out pixie-boot.img [--init PATH]
//! pixie-image build  --kernel Image.lz4-dtb --out pixie-boot.img [--init PATH]
//! pixie-image initramfs --out initramfs.cpio.gz [--init PATH]
//! pixie-image info boot.img
//! ```
//!
//! `repack` takes the stock kernel from Google's factory `boot.img`; `build`
//! takes a kernel built from source. Both replace the ramdisk with Pixie.

mod bootimg;
mod cpio;
mod kernel;

use bootimg::BootImage;
use flate2::{write::GzEncoder, Compression};
use std::collections::HashMap;
use std::io::Write;
use std::process::ExitCode;

const DEFAULT_INIT: &str = "target/aarch64-unknown-linux-musl/release/pixie-init";

/// Pixel 2 command line (from the wahoo board config) plus Pixie's init.
const WALLEYE_CMDLINE: &str = "console=ttyMSM0,115200,n8 earlycon=msm_serial_dm,0xc1b0000 \
androidboot.console=ttyMSM0 lpm_levels.sleep_disabled=1 user_debug=31 msm_rtb.filter=0x37 \
ehci-hcd.park=3 service_locator.enable=1 swiotlb=2048 loop.max_part=7 raid=noautodetect rdinit=/init";

const USAGE: &str = "usage:
  pixie-image repack --stock boot.img --out pixie-boot.img [--init PATH]
  pixie-image build --kernel Image.lz4-dtb --out pixie-boot.img [--init PATH]
  pixie-image initramfs --out initramfs.cpio.gz [--init PATH]
  pixie-image info boot.img";

fn main() -> ExitCode {
    let args: Vec<String> = std::env::args().skip(1).collect();
    match run(&args) {
        Ok(()) => ExitCode::SUCCESS,
        Err(e) => {
            eprintln!("pixie-image: {e}");
            ExitCode::FAILURE
        }
    }
}

fn run(args: &[String]) -> Result<(), String> {
    let Some((command, rest)) = args.split_first() else {
        return Err(USAGE.into());
    };
    if command == "info" {
        let path = rest.first().ok_or(USAGE)?;
        return info(path);
    }
    let flags = parse_flags(rest)?;
    let get = |name: &str| {
        flags
            .get(name)
            .cloned()
            .ok_or(format!("missing --{name}\n{USAGE}"))
    };
    let init_path = flags
        .get("init")
        .cloned()
        .unwrap_or_else(|| DEFAULT_INIT.into());
    let out = get("out")?;

    let init = read(&init_path).map_err(|e| format!("{e}\nbuild it first: cargo build --release --target aarch64-unknown-linux-musl -p pixie-init"))?;
    let ramdisk = gzip(&cpio::pixie_initramfs(&init))?;

    match command.as_str() {
        "initramfs" => write(&out, &ramdisk),
        "build" => {
            let kernel = read(&get("kernel")?)?;
            kernel::Kernel::unpack(&kernel)?; // fail early on a wrong file
            let img = BootImage::walleye(kernel, ramdisk, WALLEYE_CMDLINE);
            write(&out, &img.to_bytes().map_err(|e| e.to_string())?)
        }
        "repack" => {
            let stock = BootImage::parse(&read(&get("stock")?)?).map_err(|e| e.to_string())?;
            let mut k = kernel::Kernel::unpack(&stock.kernel)?;
            let patched = k.disable_skip_initramfs();
            eprintln!(
                "kernel: {:?}, {} bytes, patched {patched} skip_initramfs",
                k.compression,
                k.image.len()
            );
            if patched == 0 {
                eprintln!("warning: no skip_initramfs found; the bootloader may still skip Pixie's ramdisk");
            }
            let mut img = stock;
            img.kernel = k.pack()?;
            img.ramdisk = ramdisk;
            if !String::from_utf8_lossy(&img.cmdline).contains("rdinit=") {
                img.cmdline.extend_from_slice(b" rdinit=/init");
            }
            write(&out, &img.to_bytes().map_err(|e| e.to_string())?)
        }
        _ => Err(USAGE.into()),
    }
}

fn info(path: &str) -> Result<(), String> {
    let img = BootImage::parse(&read(path)?).map_err(|e| e.to_string())?;
    println!("header version  {}", img.header_version);
    println!("page size       {}", img.page_size);
    println!(
        "kernel          {} bytes at {:#x}",
        img.kernel.len(),
        img.kernel_addr
    );
    println!(
        "ramdisk         {} bytes at {:#x}",
        img.ramdisk.len(),
        img.ramdisk_addr
    );
    println!("second          {} bytes", img.second.len());
    println!("tags            {:#x}", img.tags_addr);
    println!("cmdline         {}", String::from_utf8_lossy(&img.cmdline));
    match kernel::Kernel::unpack(&img.kernel) {
        Ok(k) => {
            let skip = k
                .image
                .windows(15)
                .filter(|w| w == b"skip_initramfs\0")
                .count();
            println!(
                "kernel format   {:?}, {} bytes unpacked, {} bytes of appended DTBs, skip_initramfs x{skip}",
                k.compression,
                k.image.len(),
                k.appended.len()
            );
        }
        Err(e) => println!("kernel format   {e}"),
    }
    Ok(())
}

fn parse_flags(args: &[String]) -> Result<HashMap<String, String>, String> {
    let mut flags = HashMap::new();
    let mut it = args.iter();
    while let Some(arg) = it.next() {
        let name = arg
            .strip_prefix("--")
            .ok_or(format!("unexpected argument {arg}\n{USAGE}"))?;
        let value = it.next().ok_or(format!("--{name} needs a value"))?;
        flags.insert(name.to_string(), value.clone());
    }
    Ok(flags)
}

fn gzip(data: &[u8]) -> Result<Vec<u8>, String> {
    let mut enc = GzEncoder::new(Vec::new(), Compression::best());
    enc.write_all(data).map_err(|e| e.to_string())?;
    enc.finish().map_err(|e| e.to_string())
}

fn read(path: &str) -> Result<Vec<u8>, String> {
    std::fs::read(path).map_err(|e| format!("{path}: {e}"))
}

fn write(path: &str, data: &[u8]) -> Result<(), String> {
    std::fs::write(path, data).map_err(|e| format!("{path}: {e}"))?;
    eprintln!("wrote {path} ({} bytes)", data.len());
    Ok(())
}
