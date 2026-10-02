//! The Pixie screen: a status bar on top, the conversation below, an on-screen
//! keyboard, and a hint line at the bottom.

use crate::fb::{Canvas, Rgb, GLYPH};

const BG: Rgb = Rgb(12, 12, 16);
const BAR: Rgb = Rgb(28, 28, 36);
const INK: Rgb = Rgb(236, 236, 240);
const QUIET: Rgb = Rgb(140, 140, 156);
const ACCENT: Rgb = Rgb(120, 170, 255);
const ALERT: Rgb = Rgb(255, 120, 110);
const KEY: Rgb = Rgb(44, 44, 58);
const KEY_SPECIAL: Rgb = Rgb(70, 70, 92);
const KEY_ENTER: Rgb = Rgb(60, 110, 200);

/// What a key on the on-screen keyboard does.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum Key {
    Char(char),
    Shift,
    Backspace,
    Enter,
    Space,
    /// Switches between letters and digits/symbols.
    Page,
}

/// A key and where it sits on the screen, in pixels.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct KeyRect {
    pub x: usize,
    pub y: usize,
    pub w: usize,
    pub h: usize,
    pub key: Key,
}

/// Rows of (key, width weight); every row's weights add up to 10.
fn rows(symbols: bool) -> Vec<Vec<(Key, f32)>> {
    let chars = |s: &str| -> Vec<(Key, f32)> { s.chars().map(|c| (Key::Char(c), 1.0)).collect() };
    let mut r2 = vec![(if symbols { Key::Char('#') } else { Key::Shift }, 1.5)];
    r2.extend(chars(if symbols { "()=+*&%" } else { "zxcvbnm" }));
    r2.push((Key::Backspace, 1.5));
    vec![
        chars(if symbols { "1234567890" } else { "qwertyuiop" }),
        // Nine letters, indented by half a key via padding weights.
        if symbols {
            chars("/.-_:,;?!@")
        } else {
            let mut r = vec![(Key::Space, 0.0)]; // spacer, see `layout`
            r.extend(chars("asdfghjkl"));
            r
        },
        r2,
        vec![
            (Key::Page, 1.5),
            (Key::Char(','), 1.0),
            (Key::Space, 4.5),
            (Key::Char('.'), 1.0),
            (Key::Enter, 2.0),
        ],
    ]
}

/// Key rectangles for a keyboard occupying `top..top+height` of a `width`-wide screen.
pub fn layout(symbols: bool, width: usize, top: usize, height: usize) -> Vec<KeyRect> {
    let gap = (width / 120).max(2);
    let rows = rows(symbols);
    let row_h = height / rows.len();
    let mut out = Vec::new();
    for (ri, row) in rows.iter().enumerate() {
        // The nine-letter row is centred: a zero-weight Space marks it.
        let spacer = matches!(row.first(), Some((Key::Space, w)) if *w == 0.0);
        let row: Vec<_> = row.iter().filter(|(_, w)| *w > 0.0).collect();
        let total: f32 = if spacer { 10.0 } else { row.iter().map(|(_, w)| w).sum() };
        let unit = (width - gap) as f32 / total;
        let mut x = if spacer {
            gap as f32 + unit * (10.0 - row.len() as f32) / 2.0
        } else {
            gap as f32
        };
        for (key, weight) in row {
            let w = (unit * weight) as usize;
            out.push(KeyRect {
                x: x as usize,
                y: top + ri * row_h + gap / 2,
                w: w.saturating_sub(gap),
                h: row_h.saturating_sub(gap),
                key: *key,
            });
            x += unit * weight;
        }
    }
    out
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub enum Who {
    User,
    Pixie,
    Alert,
}

#[derive(Debug, Clone, Default)]
pub struct Status {
    pub time: String,
    pub battery: String,
    pub link: String,
}

pub struct Screen {
    pub status: Status,
    lines: Vec<(Who, String)>,
    /// What has been typed on the on-screen keyboard so far.
    pub input: String,
    shift: bool,
    symbols: bool,
}

impl Default for Screen {
    fn default() -> Self {
        Self::new()
    }
}

impl Screen {
    pub fn new() -> Self {
        Self {
            status: Status::default(),
            lines: Vec::new(),
            input: String::new(),
            shift: false,
            symbols: false,
        }
    }

    pub fn push(&mut self, who: Who, text: impl Into<String>) {
        self.lines.push((who, text.into()));
        if self.lines.len() > 200 {
            self.lines.drain(..100);
        }
    }

    /// Where the keyboard sits: (top of the input box, top of the keys, key area height).
    fn keyboard_area(width: usize, height: usize) -> (usize, usize, usize) {
        let scale = (width / (GLYPH * 30)).max(1);
        let small = (scale * 3 / 4).max(1);
        let hint_h = GLYPH * small + GLYPH * scale;
        let keys_h = (height / 3).min(width * 3 / 5);
        let box_h = GLYPH * scale + scale * 8;
        let keys_top = height.saturating_sub(hint_h + keys_h + GLYPH * scale);
        (keys_top.saturating_sub(box_h), keys_top, keys_h)
    }

    /// Handles a touch at pixel (x, y). Returns a line when Enter is pressed.
    pub fn tap(&mut self, x: usize, y: usize, width: usize, height: usize) -> Option<String> {
        let (_, keys_top, keys_h) = Self::keyboard_area(width, height);
        let hit = layout(self.symbols, width, keys_top, keys_h)
            .into_iter()
            .find(|k| x >= k.x && x < k.x + k.w && y >= k.y && y < k.y + k.h)?;
        match hit.key {
            Key::Char(c) => {
                let c = if self.shift { c.to_ascii_uppercase() } else { c };
                self.input.push(c);
                self.shift = false;
            }
            Key::Space => self.input.push(' '),
            Key::Backspace => {
                self.input.pop();
            }
            Key::Shift => self.shift = !self.shift,
            Key::Page => self.symbols = !self.symbols,
            Key::Enter => {
                let line = std::mem::take(&mut self.input);
                return (!line.trim().is_empty()).then_some(line);
            }
        }
        None
    }

    pub fn render(&self, c: &mut Canvas) {
        // About 30 characters across, whatever the resolution.
        let scale = (c.width / (GLYPH * 30)).max(1);
        let small = (scale * 3 / 4).max(1);
        let margin = GLYPH * scale;
        let line_h = GLYPH * scale + scale * 4;
        let bar_h = GLYPH * small + small * 8;

        c.clear(BG);
        c.fill_rect(0, 0, c.width, bar_h, BAR);
        let ty = small * 4;
        c.text(margin, ty, &self.status.time, small, INK);
        let right = format!("{}  {}", self.status.link, self.status.battery);
        let rw = right.chars().count() * GLYPH * small;
        c.text(c.width.saturating_sub(rw + margin), ty, &right, small, INK);
        let title = "pixie";
        let tw = title.len() * GLYPH * small;
        c.text((c.width - tw) / 2, ty, title, small, ACCENT);

        let hint = "vol+ photo  vol- battery  power time";
        let hint_y = c.height.saturating_sub(GLYPH * small + margin);
        let hw = hint.len() * GLYPH * small;
        c.text(c.width.saturating_sub(hw) / 2, hint_y, hint, small, QUIET);

        // Wrap every line, then show the newest that fit above the hint.
        let cols = ((c.width - 2 * margin) / (GLYPH * scale)).max(1);
        let mut wrapped: Vec<(Who, String)> = Vec::new();
        for (who, text) in &self.lines {
            let prefix = if *who == Who::User { "> " } else { "" };
            for line in wrap(&format!("{prefix}{text}"), cols) {
                wrapped.push((*who, line));
            }
            wrapped.push((*who, String::new()));
        }
        let (box_top, keys_top, keys_h) = Self::keyboard_area(c.width, c.height);
        let top = bar_h + margin;
        let rows = box_top.saturating_sub(top + margin) / line_h;
        let start = wrapped.len().saturating_sub(rows);
        for (i, (who, line)) in wrapped[start..].iter().enumerate() {
            let color = match who {
                Who::User => ACCENT,
                Who::Pixie => INK,
                Who::Alert => ALERT,
            };
            c.text(margin, top + i * line_h, line, scale, color);
        }

        // Input box, with the tail of what has been typed and a cursor.
        let box_h = keys_top - box_top;
        c.fill_rect(0, box_top, c.width, box_h, BAR);
        let typed: Vec<char> = self.input.chars().chain(['_']).collect();
        let start = typed.len().saturating_sub(cols);
        let shown: String = typed[start..].iter().collect();
        c.text(margin, box_top + scale * 4, &shown, scale, INK);

        for k in layout(self.symbols, c.width, keys_top, keys_h) {
            let (label, colour) = match k.key {
                Key::Char(ch) => (
                    if self.shift { ch.to_ascii_uppercase() } else { ch }.to_string(),
                    KEY,
                ),
                Key::Space => ("space".into(), KEY),
                Key::Backspace => ("del".into(), KEY_SPECIAL),
                Key::Shift => (if self.shift { "SHIFT" } else { "shift" }.into(), KEY_SPECIAL),
                Key::Page => (if self.symbols { "abc" } else { "123" }.into(), KEY_SPECIAL),
                Key::Enter => ("go".into(), KEY_ENTER),
            };
            c.fill_rect(k.x, k.y, k.w, k.h, colour);
            let ks = if label.len() > 2 { small } else { scale };
            let lw = label.chars().count() * GLYPH * ks;
            c.text(
                k.x + k.w.saturating_sub(lw) / 2,
                k.y + k.h.saturating_sub(GLYPH * ks) / 2,
                &label,
                ks,
                INK,
            );
        }
    }
}

/// Greedy word wrap at `cols` characters; long words are split.
pub fn wrap(text: &str, cols: usize) -> Vec<String> {
    let mut out = Vec::new();
    let mut line = String::new();
    for word in text.split_whitespace() {
        let mut word = word.to_string();
        while word.chars().count() > cols {
            if !line.is_empty() {
                out.push(std::mem::take(&mut line));
            }
            let head: String = word.chars().take(cols).collect();
            word = word.chars().skip(cols).collect();
            out.push(head);
        }
        let needed = line.chars().count() + usize::from(!line.is_empty()) + word.chars().count();
        if needed > cols && !line.is_empty() {
            out.push(std::mem::take(&mut line));
        }
        if !line.is_empty() {
            line.push(' ');
        }
        line.push_str(&word);
    }
    if !line.is_empty() || out.is_empty() {
        out.push(line);
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::fb::PixelFormat;

    #[test]
    fn wraps_on_words_and_splits_long_ones() {
        assert_eq!(
            wrap("take a photo please", 10),
            vec!["take a", "photo", "please"]
        );
        assert_eq!(wrap("abcdefghijkl", 5), vec!["abcde", "fghij", "kl"]);
        assert_eq!(wrap("", 5), vec![""]);
    }

    #[test]
    fn keyboard_types_edits_and_submits() {
        let (w, h) = (1080, 1920);
        let (_, top, kh) = Screen::keyboard_area(w, h);
        let find = |symbols: bool, key: Key| {
            let k = layout(symbols, w, top, kh).into_iter().find(|k| k.key == key).unwrap();
            (k.x + k.w / 2, k.y + k.h / 2)
        };
        let mut s = Screen::new();
        let press = |s: &mut Screen, symbols: bool, key: Key| {
            let (x, y) = find(symbols, key);
            s.tap(x, y, w, h)
        };
        press(&mut s, false, Key::Shift);
        press(&mut s, false, Key::Char('h'));
        press(&mut s, false, Key::Char('i'));
        press(&mut s, false, Key::Char('x'));
        press(&mut s, false, Key::Backspace);
        assert_eq!(s.input, "Hi");
        press(&mut s, false, Key::Page);
        press(&mut s, true, Key::Char('/'));
        assert_eq!(s.input, "Hi/");
        assert_eq!(press(&mut s, true, Key::Enter), Some("Hi/".to_string()));
        assert_eq!(s.input, "");
        // Taps that miss every key do nothing.
        assert_eq!(s.tap(5, 5, w, h), None);
    }

    #[test]
    fn every_key_fits_on_a_pixel_2() {
        let (_, top, kh) = Screen::keyboard_area(1080, 1920);
        for symbols in [false, true] {
            let keys = layout(symbols, 1080, top, kh);
            assert!(keys.len() >= 30);
            for k in keys {
                assert!(k.x + k.w <= 1080 && k.y + k.h <= 1920 && k.w > 20 && k.h > 20);
            }
        }
    }

    #[test]
    fn renders_a_pixel_2_sized_screen() {
        let mut c = Canvas::new(1080, 1920, 1080 * 4, PixelFormat::RGBA8888);
        let mut s = Screen::new();
        s.status = Status {
            time: "16:50".into(),
            battery: "87%".into(),
            link: "usb".into(),
        };
        s.push(Who::User, "take a photo");
        s.push(Who::Pixie, "Saved /data/photos/photo-1.ppm");
        s.render(&mut c);
        if let Ok(path) = std::env::var("PIXIE_SCREENSHOT") {
            std::fs::write(path, c.to_ppm()).unwrap();
        }
        // The status bar is not the background colour.
        assert_ne!(&c.buf[0..3], &[BG.0, BG.1, BG.2]);
    }
}
