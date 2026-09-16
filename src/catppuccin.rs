//! The four Catppuccin flavors (<https://catppuccin.com/palette>), as compile-time `COLORREF`s.
//! Only the swatches FastPad paints are listed; `window::palette` and the lexer style tables map
//! them to UI roles following the Catppuccin style guide.

use crate::languages::rgb;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct Flavor {
    pub red: u32,
    pub maroon: u32,
    pub peach: u32,
    pub green: u32,
    pub blue: u32,
    pub text: u32,
    pub subtext0: u32,
    pub overlay2: u32,
    pub overlay1: u32,
    pub surface1: u32,
    pub surface0: u32,
    pub base: u32,
    pub mantle: u32,
    pub crust: u32,
}

/// `0xRRGGBB` as written in the official palette, packed as a Windows `COLORREF`.
const fn hex(value: u32) -> u32 {
    rgb(value >> 16, (value >> 8) & 0xFF, value & 0xFF)
}

/// Composites `foreground` at `alpha`/255 opacity over an opaque `background`, so the style guide's
/// translucent overlays become solid colors at compile time.
pub const fn blend(foreground: u32, background: u32, alpha: u32) -> u32 {
    const fn channel(foreground: u32, background: u32, alpha: u32, shift: u32) -> u32 {
        let fg = (foreground >> shift) & 0xFF;
        let bg = (background >> shift) & 0xFF;
        ((fg * alpha + bg * (255 - alpha) + 127) / 255) << shift
    }
    channel(foreground, background, alpha, 0)
        | channel(foreground, background, alpha, 8)
        | channel(foreground, background, alpha, 16)
}

pub const LATTE: Flavor = Flavor {
    red: hex(0xD20F39),
    maroon: hex(0xE64553),
    peach: hex(0xFE640B),
    green: hex(0x40A02B),
    blue: hex(0x1E66F5),
    text: hex(0x4C4F69),
    subtext0: hex(0x6C6F85),
    overlay2: hex(0x7C7F93),
    overlay1: hex(0x8C8FA1),
    surface1: hex(0xBCC0CC),
    surface0: hex(0xCCD0DA),
    base: hex(0xEFF1F5),
    mantle: hex(0xE6E9EF),
    crust: hex(0xDCE0E8),
};

pub const FRAPPE: Flavor = Flavor {
    red: hex(0xE78284),
    maroon: hex(0xEA999C),
    peach: hex(0xEF9F76),
    green: hex(0xA6D189),
    blue: hex(0x8CAAEE),
    text: hex(0xC6D0F5),
    subtext0: hex(0xA5ADCE),
    overlay2: hex(0x949CBB),
    overlay1: hex(0x838BA7),
    surface1: hex(0x51576D),
    surface0: hex(0x414559),
    base: hex(0x303446),
    mantle: hex(0x292C3C),
    crust: hex(0x232634),
};

pub const MACCHIATO: Flavor = Flavor {
    red: hex(0xED8796),
    maroon: hex(0xEE99A0),
    peach: hex(0xF5A97F),
    green: hex(0xA6DA95),
    blue: hex(0x8AADF4),
    text: hex(0xCAD3F5),
    subtext0: hex(0xA5ADCB),
    overlay2: hex(0x939AB7),
    overlay1: hex(0x8087A2),
    surface1: hex(0x494D64),
    surface0: hex(0x363A4F),
    base: hex(0x24273A),
    mantle: hex(0x1E2030),
    crust: hex(0x181926),
};

pub const MOCHA: Flavor = Flavor {
    red: hex(0xF38BA8),
    maroon: hex(0xEBA0AC),
    peach: hex(0xFAB387),
    green: hex(0xA6E3A1),
    blue: hex(0x89B4FA),
    text: hex(0xCDD6F4),
    subtext0: hex(0xA6ADC8),
    overlay2: hex(0x9399B2),
    overlay1: hex(0x7F849C),
    surface1: hex(0x45475A),
    surface0: hex(0x313244),
    base: hex(0x1E1E2E),
    mantle: hex(0x181825),
    crust: hex(0x11111B),
};

#[cfg(test)]
mod tests {
    use super::{MOCHA, blend, hex};
    use crate::languages::rgb;

    #[test]
    fn hex_reads_official_palette_values_in_rgb_order() {
        // Break caught: reading 0xRRGGBB in COLORREF order would swap red and blue on every swatch.
        assert_eq!(MOCHA.base, rgb(0x1E, 0x1E, 0x2E));
        assert_eq!(hex(0x123456), rgb(0x12, 0x34, 0x56));
    }

    #[test]
    fn blend_interpolates_each_channel_independently() {
        let white = rgb(255, 255, 255);
        let black = rgb(0, 0, 0);
        assert_eq!(blend(white, black, 255), white);
        assert_eq!(blend(white, black, 0), black);
        assert_eq!(
            blend(rgb(200, 0, 100), rgb(0, 200, 100), 51),
            rgb(40, 160, 100)
        );
    }
}
