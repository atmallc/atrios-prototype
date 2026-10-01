//! A USB serial gadget, so a computer can type requests to Pixie over the
//! phone's USB-C cable. Shows up as /dev/ttyACM0 (Linux/macOS) or a COM port.

use std::fs;
use std::io;
use std::path::Path;

const GADGET: &str = "/sys/kernel/config/usb_gadget/pixie";

fn write(path: impl AsRef<Path>, value: &str) -> io::Result<()> {
    fs::write(path, value)
}

/// Sets up the gadget and binds it to the first USB device controller.
pub fn start_serial_gadget() -> io::Result<String> {
    let udc = fs::read_dir("/sys/class/udc")?
        .flatten()
        .map(|e| e.file_name().to_string_lossy().into_owned())
        .next()
        .ok_or_else(|| io::Error::new(io::ErrorKind::NotFound, "no USB device controller"))?;
    let g = Path::new(GADGET);
    fs::create_dir_all(g)?;
    write(g.join("idVendor"), "0x1d6b")?; // Linux Foundation
    write(g.join("idProduct"), "0x0104")?; // multifunction composite gadget
    fs::create_dir_all(g.join("strings/0x409"))?;
    write(g.join("strings/0x409/manufacturer"), "Pixie")?;
    write(g.join("strings/0x409/product"), "Pixie serial")?;
    write(g.join("strings/0x409/serialnumber"), "pixie0")?;
    fs::create_dir_all(g.join("functions/acm.usb0"))?;
    fs::create_dir_all(g.join("configs/c.1/strings/0x409"))?;
    write(g.join("configs/c.1/strings/0x409/configuration"), "serial")?;
    let link = g.join("configs/c.1/acm.usb0");
    if !link.exists() {
        std::os::unix::fs::symlink(g.join("functions/acm.usb0"), link)?;
    }
    write(g.join("UDC"), &udc)?;
    Ok(udc)
}
