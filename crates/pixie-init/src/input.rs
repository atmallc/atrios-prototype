//! Hardware buttons, read from Linux evdev.

use std::fs::File;
use std::io::Read;
use std::sync::mpsc::Sender;

pub const KEY_VOLUMEDOWN: u16 = 114;
pub const KEY_VOLUMEUP: u16 = 115;
pub const KEY_POWER: u16 = 116;
const EV_KEY: u16 = 1;

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

/// Starts a reader thread for every /dev/input/event* device.
pub fn spawn_readers(tx: Sender<KeyPress>) -> usize {
    let Ok(entries) = std::fs::read_dir("/dev/input") else {
        return 0;
    };
    let mut count = 0;
    for entry in entries.flatten() {
        let name = entry.file_name();
        if !name.to_string_lossy().starts_with("event") {
            continue;
        }
        let Ok(mut file) = File::open(entry.path()) else {
            continue;
        };
        let tx = tx.clone();
        count += 1;
        std::thread::spawn(move || {
            let mut raw = [0u8; EVENT_SIZE];
            while file.read_exact(&mut raw).is_ok() {
                if let Some(key) = parse_event(&raw) {
                    if tx.send(key).is_err() {
                        return;
                    }
                }
            }
        });
    }
    count
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
}
