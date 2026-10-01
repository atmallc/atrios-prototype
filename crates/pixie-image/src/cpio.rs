//! A minimal writer for the `newc` cpio format the kernel unpacks as its
//! initramfs. Device nodes are plain entries here, so no root is needed.

pub struct Cpio {
    out: Vec<u8>,
    ino: u32,
}

const S_IFDIR: u32 = 0o040000;
const S_IFREG: u32 = 0o100000;
const S_IFCHR: u32 = 0o020000;

impl Default for Cpio {
    fn default() -> Self {
        Self::new()
    }
}

impl Cpio {
    pub fn new() -> Self {
        Self {
            out: Vec::new(),
            ino: 300_000,
        }
    }

    pub fn dir(&mut self, name: &str, perm: u32) -> &mut Self {
        self.entry(name, S_IFDIR | perm, 2, (0, 0), &[])
    }

    pub fn file(&mut self, name: &str, perm: u32, data: &[u8]) -> &mut Self {
        self.entry(name, S_IFREG | perm, 1, (0, 0), data)
    }

    pub fn char_dev(&mut self, name: &str, perm: u32, major: u32, minor: u32) -> &mut Self {
        self.entry(name, S_IFCHR | perm, 1, (major, minor), &[])
    }

    fn entry(
        &mut self,
        name: &str,
        mode: u32,
        nlink: u32,
        rdev: (u32, u32),
        data: &[u8],
    ) -> &mut Self {
        self.ino += 1;
        let header = format!(
            "070701{:08x}{:08x}{:08x}{:08x}{:08x}{:08x}{:08x}{:08x}{:08x}{:08x}{:08x}{:08x}{:08x}",
            self.ino,
            mode,
            0, // uid
            0, // gid
            nlink,
            0, // mtime: fixed for reproducible builds
            data.len(),
            0, // dev major
            0, // dev minor
            rdev.0,
            rdev.1,
            name.len() + 1,
            0, // checksum
        );
        self.out.extend_from_slice(header.as_bytes());
        self.out.extend_from_slice(name.as_bytes());
        self.out.push(0);
        self.align();
        self.out.extend_from_slice(data);
        self.align();
        self
    }

    fn align(&mut self) {
        while !self.out.len().is_multiple_of(4) {
            self.out.push(0);
        }
    }

    pub fn finish(mut self) -> Vec<u8> {
        self.ino = 0;
        self.entry("TRAILER!!!", 0, 1, (0, 0), &[]);
        self.out
    }
}

/// The Pixie initramfs: `/init` plus the mount points it uses.
pub fn pixie_initramfs(init: &[u8]) -> Vec<u8> {
    let mut c = Cpio::new();
    for d in ["dev", "proc", "sys", "tmp", "data"] {
        c.dir(d, 0o755);
    }
    // The kernel opens /dev/console before /init runs.
    c.char_dev("dev/console", 0o600, 5, 1);
    c.char_dev("dev/null", 0o666, 1, 3);
    c.file("init", 0o755, init);
    c.finish()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn entries_are_aligned_and_terminated() {
        let bytes = pixie_initramfs(b"#!init");
        assert!(bytes.starts_with(b"070701"));
        assert_eq!(bytes.len() % 4, 0);
        let tail = String::from_utf8_lossy(&bytes[bytes.len() - 128..]).into_owned();
        assert!(tail.contains("TRAILER!!!"));
    }
}
