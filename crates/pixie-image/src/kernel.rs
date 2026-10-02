//! Kernel image handling: unpack an `Image.lz4-dtb` / `Image.gz-dtb`, patch
//! it, and pack it again with the device trees still appended.
//!
//! The patch matters on the Pixel 2: its bootloader adds `skip_initramfs` to
//! the kernel command line, which makes a stock kernel ignore our initramfs.
//! Renaming the parameter inside the kernel (to `want_initramfs`, the same
//! trick Magisk uses) makes the kernel treat it as unknown.

use flate2::{bufread::GzDecoder, write::GzEncoder, Compression as GzLevel};
use std::io::{Read, Write};

const LZ4_LEGACY_MAGIC: u32 = 0x184C_2102;
const LZ4_FRAME_MAGIC: u32 = 0x184D_2204;
/// The kernel's legacy LZ4 format splits input into 8 MiB blocks.
const LZ4_LEGACY_BLOCK: usize = 8 << 20;
const ARM64_IMAGE_MAGIC: &[u8; 4] = b"ARM\x64";

#[derive(Debug, Clone, Copy, PartialEq)]
pub enum Compression {
    None,
    Gzip,
    Lz4Legacy,
    /// The standard `.lz4` frame format, as used by the stock Pixel 2 kernel.
    Lz4Frame,
}

#[derive(Debug, Clone, PartialEq)]
pub struct Kernel {
    pub compression: Compression,
    pub image: Vec<u8>,
    /// Device tree blobs appended after the compressed image.
    pub appended: Vec<u8>,
}

impl Kernel {
    pub fn unpack(data: &[u8]) -> Result<Self, String> {
        if data.len() >= 4 && u32::from_le_bytes(data[..4].try_into().unwrap()) == LZ4_LEGACY_MAGIC
        {
            let (image, used) = lz4_legacy_decompress(data)?;
            return Ok(Self {
                compression: Compression::Lz4Legacy,
                image,
                appended: data[used..].to_vec(),
            });
        }
        if data.len() >= 4 && u32::from_le_bytes(data[..4].try_into().unwrap()) == LZ4_FRAME_MAGIC {
            let mut rest = data;
            let mut image = Vec::new();
            lz4_flex::frame::FrameDecoder::new(&mut rest)
                .read_to_end(&mut image)
                .map_err(|e| format!("lz4 frame kernel: {e}"))?;
            return Ok(Self {
                compression: Compression::Lz4Frame,
                image,
                appended: rest.to_vec(),
            });
        }
        if data.starts_with(&[0x1f, 0x8b]) {
            let mut decoder = GzDecoder::new(data);
            let mut image = Vec::new();
            decoder
                .read_to_end(&mut image)
                .map_err(|e| format!("gzip kernel: {e}"))?;
            // The bufread decoder stops right after the first member; the rest is DTBs.
            let appended = decoder.into_inner().to_vec();
            return Ok(Self {
                compression: Compression::Gzip,
                image,
                appended,
            });
        }
        if data.len() >= 0x3c && &data[0x38..0x3c] == ARM64_IMAGE_MAGIC {
            return Ok(Self {
                compression: Compression::None,
                image: data.to_vec(),
                appended: Vec::new(),
            });
        }
        Err("unrecognised kernel format (expected lz4, gzip or a raw arm64 Image)".into())
    }

    pub fn pack(&self) -> Result<Vec<u8>, String> {
        let mut out = match self.compression {
            Compression::None => self.image.clone(),
            Compression::Gzip => {
                let mut enc = GzEncoder::new(Vec::new(), GzLevel::best());
                enc.write_all(&self.image).map_err(|e| e.to_string())?;
                enc.finish().map_err(|e| e.to_string())?
            }
            Compression::Lz4Legacy => lz4_legacy_compress(&self.image),
            Compression::Lz4Frame => {
                let mut enc = lz4_flex::frame::FrameEncoder::new(Vec::new());
                enc.write_all(&self.image).map_err(|e| e.to_string())?;
                enc.finish().map_err(|e| e.to_string())?
            }
        };
        out.extend_from_slice(&self.appended);
        Ok(out)
    }

    /// Replaces every `skip_initramfs` parameter name. Returns how many.
    pub fn disable_skip_initramfs(&mut self) -> usize {
        replace_all(&mut self.image, b"skip_initramfs\0", b"want_initramfs\0")
    }
}

pub fn replace_all(haystack: &mut [u8], from: &[u8], to: &[u8]) -> usize {
    assert_eq!(from.len(), to.len());
    let mut count = 0;
    let mut i = 0;
    while i + from.len() <= haystack.len() {
        if &haystack[i..i + from.len()] == from {
            haystack[i..i + from.len()].copy_from_slice(to);
            count += 1;
            i += from.len();
        } else {
            i += 1;
        }
    }
    count
}

/// Returns the decompressed data and how many input bytes the stream used.
fn lz4_legacy_decompress(data: &[u8]) -> Result<(Vec<u8>, usize), String> {
    let mut out = Vec::new();
    let mut at = 4;
    while at + 4 <= data.len() {
        let size = u32::from_le_bytes(data[at..at + 4].try_into().unwrap()) as usize;
        // Anything else (another magic, a DTB, padding) ends the stream.
        if size == 0
            || size > lz4_flex::block::get_maximum_output_size(LZ4_LEGACY_BLOCK)
            || at + 4 + size > data.len()
        {
            break;
        }
        let block = &data[at + 4..at + 4 + size];
        match lz4_flex::block::decompress(block, LZ4_LEGACY_BLOCK) {
            Ok(chunk) => out.extend(chunk),
            Err(_) if !out.is_empty() => break,
            Err(e) => return Err(format!("lz4 kernel: {e}")),
        }
        at += 4 + size;
    }
    if out.is_empty() {
        return Err("lz4 kernel: no data".into());
    }
    Ok((out, at))
}

fn lz4_legacy_compress(data: &[u8]) -> Vec<u8> {
    let mut out = LZ4_LEGACY_MAGIC.to_le_bytes().to_vec();
    for chunk in data.chunks(LZ4_LEGACY_BLOCK) {
        let block = lz4_flex::block::compress(chunk);
        out.extend_from_slice(&(block.len() as u32).to_le_bytes());
        out.extend(block);
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn fake_image() -> Vec<u8> {
        let mut img = vec![0u8; 0x40];
        img[0x38..0x3c].copy_from_slice(ARM64_IMAGE_MAGIC);
        img.extend_from_slice(b"..cmdline params: skip_initramfs\0rootwait\0skip_initramfs\0..");
        img.extend((0..100_000u32).map(|i| (i % 251) as u8));
        img
    }

    const DTB: &[u8] = &[0xd0, 0x0d, 0xfe, 0xed, 0, 0, 0, 16, 1, 2, 3, 4, 5, 6, 7, 8];

    #[test]
    fn lz4_dtb_round_trip_and_patch() {
        let mut packed = lz4_legacy_compress(&fake_image());
        packed.extend_from_slice(DTB);
        let mut k = Kernel::unpack(&packed).unwrap();
        assert_eq!(k.compression, Compression::Lz4Legacy);
        assert_eq!(k.image, fake_image());
        assert_eq!(k.appended, DTB);
        assert_eq!(k.disable_skip_initramfs(), 2);
        let again = Kernel::unpack(&k.pack().unwrap()).unwrap();
        assert_eq!(again.appended, DTB);
        assert_eq!(
            replace_all(
                &mut again.image.clone(),
                b"want_initramfs\0",
                b"want_initramfs\0"
            ),
            2
        );
    }

    #[test]
    fn lz4_frame_dtb_round_trip() {
        let k = Kernel {
            compression: Compression::Lz4Frame,
            image: fake_image(),
            appended: DTB.to_vec(),
        };
        let unpacked = Kernel::unpack(&k.pack().unwrap()).unwrap();
        assert_eq!(unpacked, k);
    }

    #[test]
    fn gzip_dtb_round_trip() {
        let k = Kernel {
            compression: Compression::Gzip,
            image: fake_image(),
            appended: DTB.to_vec(),
        };
        let unpacked = Kernel::unpack(&k.pack().unwrap()).unwrap();
        assert_eq!(unpacked, k);
    }

    #[test]
    fn raw_image_is_recognised() {
        let k = Kernel::unpack(&fake_image()).unwrap();
        assert_eq!(k.compression, Compression::None);
    }
}
