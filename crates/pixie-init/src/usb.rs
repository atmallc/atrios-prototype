//! A USB link, so a computer can type requests to Pixie over the phone's
//! USB-C cable.
//!
//! The preferred link is a serial port (`/dev/ttyACM0` on Linux/macOS, a COM
//! port on Windows). Google's stock Pixel 2 kernel has no serial gadget, so
//! the fallback is USB Ethernet (CDC-NCM, which macOS and Linux support
//! without drivers): Pixie takes `10.55.0.1` and listens on TCP port 2323.

use std::fs;
use std::io;
use std::path::Path;

const GADGET: &str = "/sys/kernel/config/usb_gadget/pixie";

pub const PHONE_IP: [u8; 4] = [10, 55, 0, 1];
pub const HOST_IP: &str = "10.55.0.2";
pub const NET_PORT: u16 = 2323;

#[derive(Debug, PartialEq)]
pub enum Link {
    /// A serial line, readable at `/dev/ttyGS0`.
    Serial,
    /// A network interface that is already up with `PHONE_IP`.
    Network,
}

fn write(path: impl AsRef<Path>, value: &str) -> io::Result<()> {
    fs::write(path, value)
}

/// Sets up the gadget and binds it to the first USB device controller.
/// Tries a serial function first, then USB Ethernet.
pub fn start_gadget(step: &mut dyn FnMut(&str)) -> io::Result<Link> {
    step("usb: finding controller");
    let udc = fs::read_dir("/sys/class/udc")?
        .flatten()
        .map(|e| e.file_name().to_string_lossy().into_owned())
        .next()
        .ok_or_else(|| io::Error::new(io::ErrorKind::NotFound, "no USB device controller"))?;
    let g = Path::new(GADGET);
    step("usb: creating gadget");
    fs::create_dir_all(g)?;
    write(g.join("idVendor"), "0x1d6b")?; // Linux Foundation
    write(g.join("idProduct"), "0x0104")?; // multifunction composite gadget
    fs::create_dir_all(g.join("strings/0x409"))?;
    write(g.join("strings/0x409/manufacturer"), "Pixie")?;
    write(g.join("strings/0x409/product"), "Pixie")?;
    write(g.join("strings/0x409/serialnumber"), "pixie0")?;
    fs::create_dir_all(g.join("configs/c.1/strings/0x409"))?;
    write(g.join("configs/c.1/strings/0x409/configuration"), "pixie")?;

    let mut failures = Vec::new();
    for (function, link) in [("acm", Link::Serial), ("ncm", Link::Network)] {
        step(&format!("usb: adding {function}"));
        match add_function(g, function) {
            Ok(()) => {
                step(&format!("usb: binding {function} to {udc}"));
                write(g.join("UDC"), &udc)?;
                if link == Link::Network {
                    step("usb: configuring network");
                    bring_up_interface()?;
                }
                return Ok(link);
            }
            Err(e) => failures.push(format!("{function}: {e}")),
        }
    }
    Err(io::Error::other(failures.join("; ")))
}

fn add_function(g: &Path, function: &str) -> io::Result<()> {
    let name = format!("{function}.usb0");
    let dir = g.join("functions").join(&name);
    fs::create_dir_all(&dir)?;
    let link = g.join("configs/c.1").join(&name);
    if !link.exists() {
        if let Err(e) = std::os::unix::fs::symlink(&dir, &link) {
            let _ = fs::remove_dir(&dir);
            return Err(e);
        }
    }
    Ok(())
}

/// Waits for the gadget's network interface, gives it `PHONE_IP`, and brings it up.
fn bring_up_interface() -> io::Result<()> {
    let mut name = None;
    for _ in 0..50 {
        name = fs::read_dir("/sys/class/net")?
            .flatten()
            .map(|e| e.file_name().to_string_lossy().into_owned())
            .find(|n| n.starts_with("usb") || n.starts_with("ncm"));
        if name.is_some() {
            break;
        }
        std::thread::sleep(std::time::Duration::from_millis(100));
    }
    let name = name.ok_or_else(|| io::Error::new(io::ErrorKind::NotFound, "no usb network interface"))?;
    configure_interface(&name, PHONE_IP, [255, 255, 255, 0])
}

fn sockaddr_in(ip: [u8; 4]) -> libc::sockaddr_in {
    let mut addr: libc::sockaddr_in = unsafe { std::mem::zeroed() };
    addr.sin_family = libc::AF_INET as libc::sa_family_t;
    addr.sin_addr.s_addr = u32::from_ne_bytes(ip);
    addr
}

fn configure_interface(name: &str, ip: [u8; 4], mask: [u8; 4]) -> io::Result<()> {
    unsafe {
        let sock = libc::socket(libc::AF_INET, libc::SOCK_DGRAM, 0);
        if sock < 0 {
            return Err(io::Error::last_os_error());
        }
        let result = (|| {
            let mut req: libc::ifreq = std::mem::zeroed();
            for (dst, src) in req.ifr_name.iter_mut().zip(name.bytes().take(15)) {
                *dst = src as libc::c_char;
            }
            let set_addr = |req: &mut libc::ifreq, addr: [u8; 4]| {
                let sin = sockaddr_in(addr);
                std::ptr::copy_nonoverlapping(
                    &sin as *const _ as *const u8,
                    &mut req.ifr_ifru as *mut _ as *mut u8,
                    std::mem::size_of::<libc::sockaddr_in>(),
                );
            };
            let ioctl = |request: libc::c_ulong, req: &mut libc::ifreq| {
                if libc::ioctl(sock, request as _, req as *mut libc::ifreq) < 0 {
                    Err(io::Error::last_os_error())
                } else {
                    Ok(())
                }
            };
            set_addr(&mut req, ip);
            ioctl(libc::SIOCSIFADDR, &mut req)?;
            set_addr(&mut req, mask);
            ioctl(libc::SIOCSIFNETMASK, &mut req)?;
            ioctl(libc::SIOCGIFFLAGS, &mut req)?;
            req.ifr_ifru.ifru_flags |= (libc::IFF_UP | libc::IFF_RUNNING) as libc::c_short;
            ioctl(libc::SIOCSIFFLAGS, &mut req)
        })();
        libc::close(sock);
        result
    }
}
