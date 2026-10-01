//! Android boot image (header versions 0 to 2), the format the Pixel 2
//! bootloader loads with `fastboot boot` / `fastboot flash boot`.

use std::fmt;

const MAGIC: &[u8; 8] = b"ANDROID!";
const NAME_LEN: usize = 16;
const CMDLINE_LEN: usize = 512;
const EXTRA_CMDLINE_LEN: usize = 1024;

#[derive(Debug, PartialEq)]
pub struct Error(pub String);

impl fmt::Display for Error {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.0)
    }
}

impl std::error::Error for Error {}

fn err<T>(msg: impl Into<String>) -> Result<T, Error> {
    Err(Error(msg.into()))
}

#[derive(Debug, Clone, PartialEq)]
pub struct BootImage {
    pub kernel_addr: u32,
    pub ramdisk_addr: u32,
    pub second_addr: u32,
    pub tags_addr: u32,
    pub page_size: u32,
    pub header_version: u32,
    pub os_version: u32,
    pub name: Vec<u8>,
    /// Full kernel command line (main and extra parts joined).
    pub cmdline: Vec<u8>,
    pub kernel: Vec<u8>,
    pub ramdisk: Vec<u8>,
    pub second: Vec<u8>,
    /// Header v1+.
    pub recovery_dtbo: Vec<u8>,
    /// Header v2.
    pub dtb: Vec<u8>,
    pub dtb_addr: u64,
}

impl BootImage {
    /// A header v0 image with the Pixel 2 (walleye) load addresses.
    pub fn walleye(kernel: Vec<u8>, ramdisk: Vec<u8>, cmdline: &str) -> Self {
        Self {
            kernel_addr: 0x0000_8000,
            ramdisk_addr: 0x0100_0000,
            second_addr: 0,
            tags_addr: 0x0000_0100,
            page_size: 4096,
            header_version: 0,
            os_version: 0,
            name: Vec::new(),
            cmdline: cmdline.as_bytes().to_vec(),
            kernel,
            ramdisk,
            second: Vec::new(),
            recovery_dtbo: Vec::new(),
            dtb: Vec::new(),
            dtb_addr: 0,
        }
    }

    pub fn parse(data: &[u8]) -> Result<Self, Error> {
        if data.len() < 1660 || &data[..8] != MAGIC {
            return err("not an Android boot image (no ANDROID! magic)");
        }
        let u32_at = |o: usize| u32::from_le_bytes(data[o..o + 4].try_into().unwrap());
        let u64_at = |o: usize| u64::from_le_bytes(data[o..o + 8].try_into().unwrap());
        let kernel_size = u32_at(8) as usize;
        let ramdisk_size = u32_at(16) as usize;
        let second_size = u32_at(24) as usize;
        let page_size = u32_at(36);
        let header_version = u32_at(40);
        if header_version > 2 {
            return err(format!(
                "boot image header v{header_version} is not supported"
            ));
        }
        if !page_size.is_power_of_two() || page_size < 2048 {
            return err(format!("bad page size {page_size}"));
        }
        let name = trim_nul(&data[48..48 + NAME_LEN]);
        let mut cmdline = trim_nul(&data[64..64 + CMDLINE_LEN]);
        let extra_at = 64 + CMDLINE_LEN + 32;
        cmdline.extend(trim_nul(&data[extra_at..extra_at + EXTRA_CMDLINE_LEN]));

        let (mut dtbo_size, mut dtb_size, mut dtb_addr) = (0usize, 0usize, 0u64);
        let v1_at = extra_at + EXTRA_CMDLINE_LEN; // 1632
        if header_version >= 1 {
            dtbo_size = u32_at(v1_at) as usize;
        }
        if header_version >= 2 {
            dtb_size = u32_at(v1_at + 16) as usize;
            dtb_addr = u64_at(v1_at + 20);
        }

        let page = page_size as usize;
        let mut offset = page;
        let mut take = |size: usize| -> Result<Vec<u8>, Error> {
            let end = offset.checked_add(size).filter(|&e| e <= data.len());
            let Some(end) = end else {
                return err("boot image is truncated");
            };
            let blob = data[offset..end].to_vec();
            offset += size.div_ceil(page) * page;
            Ok(blob)
        };
        let kernel = take(kernel_size)?;
        let ramdisk = take(ramdisk_size)?;
        let second = take(second_size)?;
        let recovery_dtbo = take(dtbo_size)?;
        let dtb = take(dtb_size)?;

        Ok(Self {
            kernel_addr: u32_at(12),
            ramdisk_addr: u32_at(20),
            second_addr: u32_at(28),
            tags_addr: u32_at(32),
            page_size,
            header_version,
            os_version: u32_at(44),
            name,
            cmdline,
            kernel,
            ramdisk,
            second,
            recovery_dtbo,
            dtb,
            dtb_addr,
        })
    }

    pub fn to_bytes(&self) -> Result<Vec<u8>, Error> {
        if self.cmdline.len() > CMDLINE_LEN - 1 + EXTRA_CMDLINE_LEN - 1 {
            return err("kernel command line is too long");
        }
        if self.name.len() > NAME_LEN - 1 {
            return err("board name is too long");
        }
        let page = self.page_size as usize;
        let mut h = Vec::with_capacity(page);
        let put32 = |h: &mut Vec<u8>, v: u32| h.extend_from_slice(&v.to_le_bytes());
        h.extend_from_slice(MAGIC);
        put32(&mut h, self.kernel.len() as u32);
        put32(&mut h, self.kernel_addr);
        put32(&mut h, self.ramdisk.len() as u32);
        put32(&mut h, self.ramdisk_addr);
        put32(&mut h, self.second.len() as u32);
        put32(&mut h, self.second_addr);
        put32(&mut h, self.tags_addr);
        put32(&mut h, self.page_size);
        put32(&mut h, self.header_version);
        put32(&mut h, self.os_version);
        h.extend(padded(&self.name, NAME_LEN));
        let split = self.cmdline.len().min(CMDLINE_LEN - 1);
        h.extend(padded(&self.cmdline[..split], CMDLINE_LEN));
        h.extend(self.id());
        h.extend(padded(&self.cmdline[split..], EXTRA_CMDLINE_LEN));
        if self.header_version >= 1 {
            let dtbo_offset = if self.recovery_dtbo.is_empty() {
                0
            } else {
                (page
                    + pages(self.kernel.len(), page)
                    + pages(self.ramdisk.len(), page)
                    + pages(self.second.len(), page)) as u64
            };
            put32(&mut h, self.recovery_dtbo.len() as u32);
            h.extend_from_slice(&dtbo_offset.to_le_bytes());
            let header_size: u32 = if self.header_version == 1 { 1648 } else { 1660 };
            put32(&mut h, header_size);
        }
        if self.header_version >= 2 {
            put32(&mut h, self.dtb.len() as u32);
            h.extend_from_slice(&self.dtb_addr.to_le_bytes());
        }
        let mut out = padded(&h, page);
        for blob in [
            &self.kernel,
            &self.ramdisk,
            &self.second,
            &self.recovery_dtbo,
            &self.dtb,
        ] {
            out.extend(padded(blob, pages(blob.len(), page)));
        }
        Ok(out)
    }

    /// SHA-1 over each section and its size, as mkbootimg computes it.
    fn id(&self) -> Vec<u8> {
        let mut sha = sha1_smol::Sha1::new();
        let mut blobs = vec![&self.kernel, &self.ramdisk, &self.second];
        if self.header_version >= 1 {
            blobs.push(&self.recovery_dtbo);
        }
        if self.header_version >= 2 {
            blobs.push(&self.dtb);
        }
        for blob in blobs {
            sha.update(blob);
            sha.update(&(blob.len() as u32).to_le_bytes());
        }
        let mut id = sha.digest().bytes().to_vec();
        id.resize(32, 0);
        id
    }
}

fn pages(len: usize, page: usize) -> usize {
    len.div_ceil(page) * page
}

fn padded(bytes: &[u8], len: usize) -> Vec<u8> {
    let mut v = bytes.to_vec();
    v.resize(len.max(bytes.len()), 0);
    v
}

fn trim_nul(bytes: &[u8]) -> Vec<u8> {
    let end = bytes.iter().position(|&b| b == 0).unwrap_or(bytes.len());
    bytes[..end].to_vec()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn round_trips_v0() {
        let img = BootImage::walleye(vec![1; 5000], vec![2; 10], "console=ttyMSM0 quiet");
        let bytes = img.to_bytes().unwrap();
        assert_eq!(bytes.len(), 4096 * 4);
        assert_eq!(BootImage::parse(&bytes).unwrap(), img);
    }

    #[test]
    fn round_trips_v2_with_long_cmdline() {
        let mut img = BootImage::walleye(vec![1; 100], vec![2; 100], &"x".repeat(900));
        img.header_version = 2;
        img.recovery_dtbo = vec![3; 7];
        img.dtb = vec![4; 9];
        img.dtb_addr = 0x0100_0000;
        let parsed = BootImage::parse(&img.to_bytes().unwrap()).unwrap();
        assert_eq!(parsed, img);
    }

    #[test]
    fn matches_mkbootimg_layout() {
        let img = BootImage::walleye(vec![0xAA; 3], vec![0xBB; 2], "a=b");
        let b = img.to_bytes().unwrap();
        assert_eq!(&b[0..8], b"ANDROID!");
        assert_eq!(u32::from_le_bytes(b[8..12].try_into().unwrap()), 3);
        assert_eq!(u32::from_le_bytes(b[12..16].try_into().unwrap()), 0x8000);
        assert_eq!(u32::from_le_bytes(b[36..40].try_into().unwrap()), 4096);
        assert_eq!(&b[64..67], b"a=b");
        assert_eq!(b[4096], 0xAA);
        assert_eq!(b[8192], 0xBB);
    }

    #[test]
    fn rejects_garbage() {
        assert!(BootImage::parse(&[0u8; 4096]).is_err());
    }
}
