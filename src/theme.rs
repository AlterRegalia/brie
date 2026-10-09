use ratatui::style::Color;
use std::sync::OnceLock;

/// A dark stage and one signal: amber, the colour of a working brief. Done is plain text, todo is muted.
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

pub fn get() -> &'static Theme {
    static THEME: OnceLock<Theme> = OnceLock::new();
    THEME.get_or_init(|| {
        let truecolor = std::env::var("COLORTERM")
            .map(|v| v.contains("truecolor") || v.contains("24bit"))
            .unwrap_or(false);
        let pick = |rgb: (u8, u8, u8), indexed: u8| {
            if truecolor {
                Color::Rgb(rgb.0, rgb.1, rgb.2)
            } else {
                Color::Indexed(indexed)
            }
        };
        // (rgb, 256-colour fallback): bg, surface, line, text, muted, accent, accent_ink, bad, info, ok, violet
        let c: [((u8, u8, u8), u8); 11] = [
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
        Theme {
            bg: pick(c[0].0, c[0].1),
            surface: pick(c[1].0, c[1].1),
            line: pick(c[2].0, c[2].1),
            text: pick(c[3].0, c[3].1),
            muted: pick(c[4].0, c[4].1),
            accent: pick(c[5].0, c[5].1),
            accent_ink: pick(c[6].0, c[6].1),
            bad: pick(c[7].0, c[7].1),
            info: pick(c[8].0, c[8].1),
            ok: pick(c[9].0, c[9].1),
            violet: pick(c[10].0, c[10].1),
        }
    })
}
