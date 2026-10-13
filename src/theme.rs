use ratatui::style::Color;
use std::sync::OnceLock;

/// A dark stage and one signal: amber, the color of a working brief. Done is plain text, todo is muted.
pub struct Theme {
    pub bg: Color,
    pub surface: Color,
    pub line: Color,
    pub text: Color,
    pub muted: Color,
    pub accent: Color,
    pub accent_ink: Color,
    pub bad: Color,
    pub info: Color,
    /// Two extra hues, for telling the six message kinds apart in the memory window.
    pub ok: Color,
    pub violet: Color,
}

/// (rgb, 256-color fallback) for bg, surface, line, text, muted, accent, accent_ink, bad, info, ok, violet.
const PALETTE: [((u8, u8, u8), u8); 11] = [
    ((0x07, 0x08, 0x0c), 233),
    ((0x0e, 0x10, 0x16), 234),
    ((0x2a, 0x2c, 0x34), 237),
    ((0xf4, 0xf5, 0xf8), 255),
    ((0xa9, 0xb0, 0xbe), 249),
    ((0xff, 0xc4, 0x00), 220),
    ((0x14, 0x0e, 0x00), 232),
    ((0xff, 0x3b, 0x3b), 196),
    ((0x7c, 0xc4, 0xff), 117),
    ((0x8f, 0xe3, 0x8a), 114),
    ((0xb9, 0x9c, 0xff), 141),
];

/// The RGB form of a color from the 256-color palette. Both palettes come from the table above, so
/// every index in it has an RGB twin.
pub fn rgb_of_index(index: u8) -> Option<(u8, u8, u8)> {
    PALETTE
        .iter()
        .find(|(_, i)| *i == index)
        .map(|(rgb, _)| *rgb)
}

fn build(truecolor: bool) -> Theme {
    let pick = |k: usize| {
        let (rgb, indexed) = PALETTE[k];
        if truecolor {
            Color::Rgb(rgb.0, rgb.1, rgb.2)
        } else {
            Color::Indexed(indexed)
        }
    };
    Theme {
        bg: pick(0),
        surface: pick(1),
        line: pick(2),
        text: pick(3),
        muted: pick(4),
        accent: pick(5),
        accent_ink: pick(6),
        bad: pick(7),
        info: pick(8),
        ok: pick(9),
        violet: pick(10),
    }
}

/// The palette the terminal can draw: truecolor when it says so, 256 colors otherwise.
pub fn get() -> &'static Theme {
    static THEME: OnceLock<Theme> = OnceLock::new();
    THEME.get_or_init(|| {
        let truecolor = std::env::var("COLORTERM")
            .map(|v| v.contains("truecolor") || v.contains("24bit"))
            .unwrap_or(false);
        build(truecolor)
    })
}

/// The same palette in RGB, whatever the terminal can do: HTML has no 256-color fallback, and a
/// window keeps its own colors, whatever terminal started the daemon. Only the daemon's brief
/// window calls this, and that window is macOS only.
#[cfg_attr(not(target_os = "macos"), allow(dead_code))]
pub fn rgb() -> &'static Theme {
    static RGB: OnceLock<Theme> = OnceLock::new();
    RGB.get_or_init(|| build(true))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn both_palettes_name_the_same_colors() {
        let indexed = build(false);
        let rgb = build(true);
        for (a, b) in [
            (indexed.bg, rgb.bg),
            (indexed.accent, rgb.accent),
            (indexed.text, rgb.text),
            (indexed.violet, rgb.violet),
        ] {
            assert!(matches!(a, Color::Indexed(_)));
            assert!(matches!(b, Color::Rgb(_, _, _)));
        }
        assert_eq!(rgb.accent, Color::Rgb(0xff, 0xc4, 0x00));
        assert_eq!(rgb.bg, Color::Rgb(0x07, 0x08, 0x0c));
    }
}
