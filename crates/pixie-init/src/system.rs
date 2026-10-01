//! PID 1 duties: mount the kernel filesystems and reboot on request.

use std::ffi::CString;
use std::io;

fn mount(source: &str, target: &str, fstype: &str) -> io::Result<()> {
    let _ = std::fs::create_dir_all(target);
    let c = |s: &str| CString::new(s).unwrap();
    let (source, target, fstype) = (c(source), c(target), c(fstype));
    let rc = unsafe {
        libc::mount(
            source.as_ptr(),
            target.as_ptr(),
            fstype.as_ptr(),
            0,
            std::ptr::null(),
        )
    };
    if rc == 0 || io::Error::last_os_error().raw_os_error() == Some(libc::EBUSY) {
        Ok(())
    } else {
        Err(io::Error::last_os_error())
    }
}

/// Mounts /proc, /sys, /dev, configfs and a tmpfs for data. Returns what failed.
pub fn mount_all() -> Vec<String> {
    let mut failed = Vec::new();
    for (src, target, fstype) in [("proc", "/proc", "proc"), ("sysfs", "/sys", "sysfs")] {
        if let Err(e) = mount(src, target, fstype) {
            failed.push(format!("{target}: {e}"));
        }
    }
    // Stock Android kernels often lack devtmpfs; then build /dev from sysfs.
    if mount("devtmpfs", "/dev", "devtmpfs").is_err() {
        match mount("tmpfs", "/dev", "tmpfs") {
            Ok(()) => populate_dev(),
            Err(e) => failed.push(format!("/dev: {e}")),
        }
    }
    for (src, target, fstype) in [
        ("devpts", "/dev/pts", "devpts"),
        ("configfs", "/sys/kernel/config", "configfs"),
        ("tmpfs", "/tmp", "tmpfs"),
        ("tmpfs", "/data", "tmpfs"),
    ] {
        if let Err(e) = mount(src, target, fstype) {
            failed.push(format!("{target}: {e}"));
        }
    }
    failed
}

/// Creates device nodes for everything in /sys/class and /sys/block, the
/// way mdev does. Input devices go under /dev/input, framebuffers also under
/// /dev/graphics, matching Android's layout.
fn populate_dev() {
    let mut dirs: Vec<(std::path::PathBuf, bool)> = Vec::new();
    if let Ok(classes) = std::fs::read_dir("/sys/class") {
        dirs.extend(classes.flatten().map(|c| (c.path(), false)));
    }
    dirs.push(("/sys/block".into(), true));
    for (dir, block) in dirs {
        let Ok(entries) = std::fs::read_dir(&dir) else {
            continue;
        };
        for entry in entries.flatten() {
            let Ok(dev) = std::fs::read_to_string(entry.path().join("dev")) else {
                continue;
            };
            let Some((major, minor)) = parse_dev(&dev) else {
                continue;
            };
            let name = entry.file_name().to_string_lossy().into_owned();
            let class = dir
                .file_name()
                .map(|c| c.to_string_lossy().into_owned())
                .unwrap_or_default();
            let path = match class.as_str() {
                "input" => format!("/dev/input/{name}"),
                _ => format!("/dev/{name}"),
            };
            make_node(&path, block, major, minor);
            if class == "graphics" {
                make_node(&format!("/dev/graphics/{name}"), false, major, minor);
            }
        }
    }
}

pub fn parse_dev(s: &str) -> Option<(u32, u32)> {
    let (major, minor) = s.trim().split_once(':')?;
    Some((major.parse().ok()?, minor.parse().ok()?))
}

fn make_node(path: &str, block: bool, major: u32, minor: u32) {
    if let Some(parent) = std::path::Path::new(path).parent() {
        let _ = std::fs::create_dir_all(parent);
    }
    let Ok(c_path) = CString::new(path) else {
        return;
    };
    let kind = if block { libc::S_IFBLK } else { libc::S_IFCHR };
    unsafe { libc::mknod(c_path.as_ptr(), kind | 0o600, libc::makedev(major, minor)) };
}

/// Android normally sets the screen brightness; without it the panel can
/// stay dark. Sets every backlight that reads 0 to half of its maximum.
pub fn ensure_backlight() {
    let mut dirs: Vec<std::path::PathBuf> = vec!["/sys/class/leds/lcd-backlight".into()];
    if let Ok(entries) = std::fs::read_dir("/sys/class/backlight") {
        dirs.extend(entries.flatten().map(|e| e.path()));
    }
    for dir in dirs {
        let read = |f: &str| -> Option<u32> {
            std::fs::read_to_string(dir.join(f))
                .ok()?
                .trim()
                .parse()
                .ok()
        };
        if let (Some(0), Some(max)) = (read("brightness"), read("max_brightness")) {
            let _ = std::fs::write(dir.join("brightness"), (max / 2).max(1).to_string());
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub enum Reboot {
    /// Normal restart; on a phone booted with `fastboot boot`, this returns to Android.
    Restart,
    Bootloader,
    PowerOff,
}

pub fn reboot(kind: Reboot) -> io::Error {
    unsafe { libc::sync() };
    let rc = match kind {
        Reboot::Restart => unsafe { libc::reboot(libc::RB_AUTOBOOT) },
        Reboot::PowerOff => unsafe { libc::reboot(libc::RB_POWER_OFF) },
        Reboot::Bootloader => {
            let arg = CString::new("bootloader").unwrap();
            unsafe {
                libc::syscall(
                    libc::SYS_reboot,
                    libc::LINUX_REBOOT_MAGIC1,
                    libc::LINUX_REBOOT_MAGIC2,
                    libc::LINUX_REBOOT_CMD_RESTART2,
                    arg.as_ptr(),
                ) as libc::c_int
            }
        }
    };
    let _ = rc;
    io::Error::last_os_error()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_sysfs_dev() {
        assert_eq!(parse_dev("29:0\n"), Some((29, 0)));
        assert_eq!(parse_dev("junk"), None);
    }
}
