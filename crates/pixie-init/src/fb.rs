//! Drawing: an in-memory [`Canvas`] and the Linux framebuffer it is copied to.
//!
//! The Pixel 2 kernel exposes its display through the MDSS fbdev driver as
//! `/dev/fb0` (or `/dev/graphics/fb0`), so no GPU stack is needed.

use font8x8::UnicodeFonts;
use std::fs::{File, OpenOptions};
use std::io;
use std::os::fd::AsRawFd;

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Rgb(pub u8, pub u8, pub u8);

/// Where each colour channel sits inside a pixel, in bits.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct PixelFormat {
    pub bytes_per_pixel: usize,
    pub red: (u32, u32),
    pub green: (u32, u32),
    pub blue: (u32, u32),
    pub alpha: (u32, u32),
}

impl PixelFormat {
    /// 32-bit, red in the lowest byte (what most test tools expect).
    #[cfg_attr(not(test), allow(dead_code))]
    pub const RGBA8888: PixelFormat = PixelFormat {
        bytes_per_pixel: 4,
        red: (0, 8),
        green: (8, 8),
        blue: (16, 8),
        alpha: (24, 8),
    };

    fn pack(&self, c: Rgb) -> u32 {
        let chan = |v: u8, (offset, len): (u32, u32)| -> u32 {
            if len == 0 {
                return 0;
            }
            ((v as u32) >> (8 - len.min(8))) << offset
        };
        chan(c.0, self.red) | chan(c.1, self.green) | chan(c.2, self.blue) | chan(255, self.alpha)
    }
}

pub struct Canvas {
    pub width: usize,
    pub height: usize,
    pub stride: usize,
    pub format: PixelFormat,
    pub buf: Vec<u8>,
}

pub const GLYPH: usize = 8;

impl Canvas {
    pub fn new(width: usize, height: usize, stride: usize, format: PixelFormat) -> Self {
        Self {
            width,
            height,
            stride,
            format,
            buf: vec![0; stride * height],
        }
    }

    pub fn fill_rect(&mut self, x: usize, y: usize, w: usize, h: usize, c: Rgb) {
        let px = self.format.pack(c).to_le_bytes();
        let bpp = self.format.bytes_per_pixel;
        for row in y.min(self.height)..(y + h).min(self.height) {
            for col in x.min(self.width)..(x + w).min(self.width) {
                let at = row * self.stride + col * bpp;
                self.buf[at..at + bpp].copy_from_slice(&px[..bpp]);
            }
        }
    }

    pub fn clear(&mut self, c: Rgb) {
        self.fill_rect(0, 0, self.width, self.height, c);
    }

    /// Draws one line of text with each font pixel scaled to `scale` pixels.
    /// Returns the x just past the last glyph.
    pub fn text(&mut self, x: usize, y: usize, s: &str, scale: usize, c: Rgb) -> usize {
        let mut cx = x;
        for ch in s.chars() {
            let glyph = font8x8::BASIC_FONTS
                .get(ch)
                .or_else(|| font8x8::LATIN_FONTS.get(ch))
                .unwrap_or_else(|| font8x8::BASIC_FONTS.get('?').unwrap());
            for (gy, bits) in glyph.iter().enumerate() {
                for gx in 0..GLYPH {
                    if bits & (1 << gx) != 0 {
                        self.fill_rect(cx + gx * scale, y + gy * scale, scale, scale, c);
                    }
                }
            }
            cx += GLYPH * scale;
        }
        cx
    }

    /// Writes the canvas as a binary PPM, for screenshots in tests.
    #[cfg_attr(not(test), allow(dead_code))]
    pub fn to_ppm(&self) -> Vec<u8> {
        let mut out = format!("P6\n{} {}\n255\n", self.width, self.height).into_bytes();
        let f = self.format;
        let bpp = f.bytes_per_pixel;
        let get = |px: u32, (offset, len): (u32, u32)| -> u8 {
            if len == 0 {
                return 0;
            }
            (((px >> offset) & ((1 << len) - 1)) << (8 - len.min(8))) as u8
        };
        for row in 0..self.height {
            for col in 0..self.width {
                let at = row * self.stride + col * bpp;
                let mut bytes = [0u8; 4];
                bytes[..bpp].copy_from_slice(&self.buf[at..at + bpp]);
                let px = u32::from_le_bytes(bytes);
                out.extend_from_slice(&[get(px, f.red), get(px, f.green), get(px, f.blue)]);
            }
        }
        out
    }
}

// linux/fb.h
const FBIOGET_VSCREENINFO: libc::Ioctl = 0x4600;
const FBIOPUT_VSCREENINFO: libc::Ioctl = 0x4601;
const FBIOGET_FSCREENINFO: libc::Ioctl = 0x4602;
const FBIOPAN_DISPLAY: libc::Ioctl = 0x4606;
const FBIOBLANK: libc::Ioctl = 0x4611;
const FB_BLANK_UNBLANK: libc::c_ulong = 0;
const FB_ACTIVATE_NOW: u32 = 0;
const FB_ACTIVATE_FORCE: u32 = 128;

#[repr(C)]
#[derive(Default, Clone, Copy)]
struct FbBitfield {
    offset: u32,
    length: u32,
    msb_right: u32,
}

#[repr(C)]
#[derive(Default, Clone, Copy)]
struct FbVarScreeninfo {
    xres: u32,
    yres: u32,
    xres_virtual: u32,
    yres_virtual: u32,
    xoffset: u32,
    yoffset: u32,
    bits_per_pixel: u32,
    grayscale: u32,
    red: FbBitfield,
    green: FbBitfield,
    blue: FbBitfield,
    transp: FbBitfield,
    nonstd: u32,
    activate: u32,
    height: u32,
    width: u32,
    accel_flags: u32,
    pixclock: u32,
    left_margin: u32,
    right_margin: u32,
    upper_margin: u32,
    lower_margin: u32,
    hsync_len: u32,
    vsync_len: u32,
    sync: u32,
    vmode: u32,
    rotate: u32,
    colorspace: u32,
    reserved: [u32; 4],
}

#[repr(C)]
#[derive(Clone, Copy)]
struct FbFixScreeninfo {
    id: [u8; 16],
    smem_start: libc::c_ulong,
    smem_len: u32,
    type_: u32,
    type_aux: u32,
    visual: u32,
    xpanstep: u16,
    ypanstep: u16,
    ywrapstep: u16,
    line_length: u32,
    mmio_start: libc::c_ulong,
    mmio_len: u32,
    accel: u32,
    capabilities: u16,
    reserved: [u16; 2],
}

pub struct Framebuffer {
    file: File,
    var: FbVarScreeninfo,
    map: *mut u8,
    map_len: usize,
    stride: usize,
}

// The mapping is only touched through &mut self.
unsafe impl Send for Framebuffer {}

impl Framebuffer {
    pub fn open_first() -> io::Result<Self> {
        let mut last = io::Error::new(io::ErrorKind::NotFound, "no framebuffer");
        for path in ["/dev/fb0", "/dev/graphics/fb0"] {
            match Self::open(path) {
                Ok(fb) => return Ok(fb),
                Err(e) => last = e,
            }
        }
        Err(last)
    }

    pub fn open(path: &str) -> io::Result<Self> {
        let file = OpenOptions::new().read(true).write(true).open(path)?;
        let fd = file.as_raw_fd();
        // Wake the panel; harmless if it is already on.
        unsafe { libc::ioctl(fd, FBIOBLANK, FB_BLANK_UNBLANK) };
        let mut var = FbVarScreeninfo::default();
        if unsafe { libc::ioctl(fd, FBIOGET_VSCREENINFO, &mut var) } < 0 {
            return Err(io::Error::last_os_error());
        }
        let mut fix: FbFixScreeninfo = unsafe { std::mem::zeroed() };
        if unsafe { libc::ioctl(fd, FBIOGET_FSCREENINFO, &mut fix) } < 0 {
            return Err(io::Error::last_os_error());
        }
        let map_len = fix.smem_len as usize;
        let map = unsafe {
            libc::mmap(
                std::ptr::null_mut(),
                map_len,
                libc::PROT_READ | libc::PROT_WRITE,
                libc::MAP_SHARED,
                fd,
                0,
            )
        };
        if map == libc::MAP_FAILED {
            return Err(io::Error::last_os_error());
        }
        Ok(Self {
            file,
            var,
            map: map as *mut u8,
            map_len,
            stride: fix.line_length as usize,
        })
    }

    pub fn canvas(&self) -> Canvas {
        let v = &self.var;
        let format = PixelFormat {
            bytes_per_pixel: (v.bits_per_pixel as usize).div_ceil(8),
            red: (v.red.offset, v.red.length),
            green: (v.green.offset, v.green.length),
            blue: (v.blue.offset, v.blue.length),
            alpha: (v.transp.offset, v.transp.length),
        };
        Canvas::new(v.xres as usize, v.yres as usize, self.stride, format)
    }

    /// Copies the canvas to the visible buffer and asks the driver to show it.
    pub fn present(&mut self, canvas: &Canvas) {
        let offset = self.var.yoffset as usize * self.stride;
        let len = canvas.buf.len().min(self.map_len.saturating_sub(offset));
        unsafe { std::ptr::copy_nonoverlapping(canvas.buf.as_ptr(), self.map.add(offset), len) };
        let fd = self.file.as_raw_fd();
        let mut var = self.var;
        var.activate = FB_ACTIVATE_NOW | FB_ACTIVATE_FORCE;
        // MDSS commits on pan; fall back to a full mode set.
        if unsafe { libc::ioctl(fd, FBIOPAN_DISPLAY, &mut var) } < 0 {
            unsafe { libc::ioctl(fd, FBIOPUT_VSCREENINFO, &mut var) };
        }
    }
}

impl Drop for Framebuffer {
    fn drop(&mut self) {
        unsafe { libc::munmap(self.map as *mut libc::c_void, self.map_len) };
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn struct_sizes_match_the_kernel() {
        assert_eq!(std::mem::size_of::<FbVarScreeninfo>(), 160);
        #[cfg(target_pointer_width = "64")]
        assert_eq!(std::mem::size_of::<FbFixScreeninfo>(), 80);
    }

    #[test]
    fn pack_and_read_back() {
        let mut c = Canvas::new(2, 1, 8, PixelFormat::RGBA8888);
        c.fill_rect(1, 0, 1, 1, Rgb(10, 20, 30));
        let ppm = c.to_ppm();
        assert_eq!(&ppm[ppm.len() - 6..], &[0, 0, 0, 10, 20, 30]);
    }

    #[test]
    fn bgr565_round_trips_high_bits() {
        let f = PixelFormat {
            bytes_per_pixel: 2,
            red: (11, 5),
            green: (5, 6),
            blue: (0, 5),
            alpha: (0, 0),
        };
        let mut c = Canvas::new(1, 1, 2, f);
        c.fill_rect(0, 0, 1, 1, Rgb(255, 0, 255));
        assert_eq!(&c.to_ppm()[11..], &[248, 0, 248]);
    }
}
