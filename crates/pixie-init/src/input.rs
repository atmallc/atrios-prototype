//! Hardware buttons and the touchscreen, read from Linux evdev.

use std::fs::File;
use std::io::Read;
use std::sync::mpsc::Sender;

pub const KEY_VOLUMEDOWN: u16 = 114;
pub const KEY_VOLUMEUP: u16 = 115;
pub const KEY_POWER: u16 = 116;
const EV_SYN: u16 = 0;
const EV_KEY: u16 = 1;
const EV_ABS: u16 = 3;
const SYN_REPORT: u16 = 0;
const ABS_MT_POSITION_X: u16 = 0x35;
const ABS_MT_POSITION_Y: u16 = 0x36;
const ABS_MT_TRACKING_ID: u16 = 0x39;

/// size of struct input_event on 64-bit: timeval (16) + type + code + value.
const EVENT_SIZE: usize = 24;

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct KeyPress(pub u16);

/// Parses one raw `input_event`; returns the key code on a key press.
pub fn parse_event(raw: &[u8; EVENT_SIZE]) -> Option<KeyPress> {
    let kind = u16::from_ne_bytes([raw[16], raw[17]]);
    let code = u16::from_ne_bytes([raw[18], raw[19]]);
    let value = i32::from_ne_bytes([raw[20], raw[21], raw[22], raw[23]]);
    (kind == EV_KEY && value == 1).then_some(KeyPress(code))
}

/// A finger touching the screen, as a fraction of the panel: 0.0..=1.0 across and down.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Tap {
    pub x: f32,
    pub y: f32,
}

/// Turns a stream of multi-touch events into one `Tap` per touch.
pub struct TouchTracker {
    x_range: (i32, i32),
    y_range: (i32, i32),
    x: Option<i32>,
    y: Option<i32>,
    down: bool,
    emitted: bool,
}

impl TouchTracker {
    pub fn new(x_range: (i32, i32), y_range: (i32, i32)) -> Self {
        Self {
            x_range,
            y_range,
            x: None,
            y: None,
            down: false,
            emitted: false,
        }
    }

    pub fn feed(&mut self, raw: &[u8; EVENT_SIZE]) -> Option<Tap> {
        let kind = u16::from_ne_bytes([raw[16], raw[17]]);
        let code = u16::from_ne_bytes([raw[18], raw[19]]);
        let value = i32::from_ne_bytes([raw[20], raw[21], raw[22], raw[23]]);
        match (kind, code) {
            (EV_ABS, ABS_MT_TRACKING_ID) => {
                self.down = value >= 0;
                self.emitted = false;
                self.x = None;
                self.y = None;
            }
            (EV_ABS, ABS_MT_POSITION_X) => self.x = Some(value),
            (EV_ABS, ABS_MT_POSITION_Y) => self.y = Some(value),
            (EV_SYN, SYN_REPORT) if self.down && !self.emitted => {
                if let (Some(x), Some(y)) = (self.x, self.y) {
                    self.emitted = true;
                    return Some(Tap {
                        x: fraction(x, self.x_range),
                        y: fraction(y, self.y_range),
                    });
                }
            }
            _ => {}
        }
        None
    }
}

fn fraction(value: i32, (min, max): (i32, i32)) -> f32 {
    ((value - min) as f32 / (max - min) as f32).clamp(0.0, 1.0)
}

/// The (min, max) of an absolute axis, if the device has it (EVIOCGABS).
fn abs_range(file: &File, axis: u16) -> Option<(i32, i32)> {
    use std::os::fd::AsRawFd;
    // struct input_absinfo: value, minimum, maximum, fuzz, flat, resolution.
    let mut info = [0i32; 6];
    let request = (2u64 << 30) | (24u64 << 16) | (0x45u64 << 8) | (0x40 + axis as u64);
    let rc = unsafe { libc::ioctl(file.as_raw_fd(), request as _, info.as_mut_ptr()) };
    (rc >= 0 && info[2] > info[1]).then_some((info[1], info[2]))
}

/// Starts a reader thread for every /dev/input/event* device. Returns how
/// many devices it found and how many of those are touchscreens.
pub fn spawn_readers(tx: Sender<KeyPress>, taps: Sender<Tap>) -> (usize, usize) {
    let Ok(entries) = std::fs::read_dir("/dev/input") else {
        return (0, 0);
    };
    let mut count = 0;
    let mut touch_devices = 0;
    for entry in entries.flatten() {
        let name = entry.file_name();
        if !name.to_string_lossy().starts_with("event") {
            continue;
        }
        let Ok(mut file) = File::open(entry.path()) else {
            continue;
        };
        let tx = tx.clone();
        let taps = taps.clone();
        let mut tracker = match (
            abs_range(&file, ABS_MT_POSITION_X),
            abs_range(&file, ABS_MT_POSITION_Y),
        ) {
            (Some(x), Some(y)) => {
                touch_devices += 1;
                Some(TouchTracker::new(x, y))
            }
            _ => None,
        };
        count += 1;
        std::thread::spawn(move || {
            let mut raw = [0u8; EVENT_SIZE];
            while file.read_exact(&mut raw).is_ok() {
                if let Some(key) = parse_event(&raw) {
                    if tx.send(key).is_err() {
                        return;
                    }
                }
                if let Some(tap) = tracker.as_mut().and_then(|t| t.feed(&raw)) {
                    if taps.send(tap).is_err() {
                        return;
                    }
                }
            }
        });
    }
    (count, touch_devices)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn event(kind: u16, code: u16, value: i32) -> [u8; EVENT_SIZE] {
        let mut raw = [0u8; EVENT_SIZE];
        raw[16..18].copy_from_slice(&kind.to_ne_bytes());
        raw[18..20].copy_from_slice(&code.to_ne_bytes());
        raw[20..24].copy_from_slice(&value.to_ne_bytes());
        raw
    }

    #[test]
    fn only_key_presses_count() {
        assert_eq!(
            parse_event(&event(EV_KEY, KEY_VOLUMEUP, 1)),
            Some(KeyPress(KEY_VOLUMEUP))
        );
        assert_eq!(parse_event(&event(EV_KEY, KEY_VOLUMEUP, 0)), None);
        assert_eq!(parse_event(&event(3, 53, 1)), None);
    }

    #[test]
    fn one_tap_per_touch_scaled_to_the_panel() {
        let mut t = TouchTracker::new((0, 1079), (0, 1919));
        assert_eq!(t.feed(&event(EV_ABS, ABS_MT_TRACKING_ID, 7)), None);
        assert_eq!(t.feed(&event(EV_ABS, ABS_MT_POSITION_X, 540)), None);
        assert_eq!(t.feed(&event(EV_ABS, ABS_MT_POSITION_Y, 1919)), None);
        let tap = t.feed(&event(EV_SYN, SYN_REPORT, 0)).unwrap();
        assert!((tap.x - 0.5).abs() < 0.01 && (tap.y - 1.0).abs() < 0.001);
        // Moving the same finger does not tap again.
        t.feed(&event(EV_ABS, ABS_MT_POSITION_X, 600));
        assert_eq!(t.feed(&event(EV_SYN, SYN_REPORT, 0)), None);
        // Lifting and touching again does.
        t.feed(&event(EV_ABS, ABS_MT_TRACKING_ID, -1));
        t.feed(&event(EV_ABS, ABS_MT_TRACKING_ID, 8));
        t.feed(&event(EV_ABS, ABS_MT_POSITION_X, 0));
        t.feed(&event(EV_ABS, ABS_MT_POSITION_Y, 0));
        assert_eq!(t.feed(&event(EV_SYN, SYN_REPORT, 0)), Some(Tap { x: 0.0, y: 0.0 }));
    }
}
