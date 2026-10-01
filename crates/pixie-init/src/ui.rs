//! The Pixie screen: a status bar on top, the conversation below, and a hint
//! line at the bottom.

use crate::fb::{Canvas, Rgb, GLYPH};

const BG: Rgb = Rgb(12, 12, 16);
const BAR: Rgb = Rgb(28, 28, 36);
const INK: Rgb = Rgb(236, 236, 240);
const QUIET: Rgb = Rgb(140, 140, 156);
const ACCENT: Rgb = Rgb(120, 170, 255);
const ALERT: Rgb = Rgb(255, 120, 110);

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
        }
    }

    pub fn push(&mut self, who: Who, text: impl Into<String>) {
        self.lines.push((who, text.into()));
        if self.lines.len() > 200 {
            self.lines.drain(..100);
        }
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
        let top = bar_h + margin;
        let rows = hint_y.saturating_sub(top + margin) / line_h;
        let start = wrapped.len().saturating_sub(rows);
        for (i, (who, line)) in wrapped[start..].iter().enumerate() {
            let color = match who {
                Who::User => ACCENT,
                Who::Pixie => INK,
                Who::Alert => ALERT,
            };
            c.text(margin, top + i * line_h, line, scale, color);
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
