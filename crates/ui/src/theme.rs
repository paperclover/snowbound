use draw::srgb;

/// Colours are linear RGBA; sizes are logical pixels.
#[derive(Clone, Debug)]
pub struct Theme {
    pub font_size: f32,
    /// Content surfaces, such as fields and lists.
    pub base: [f32; 4],
    /// Side panels.
    pub panel: [f32; 4],
    /// The window's title bar, toolbar and tab row.
    pub strip: [f32; 4],
    pub accent: [f32; 4],
    pub text: [f32; 4],
    pub text_dim: [f32; 4],
    /// Text on section colours.
    pub ink: [f32; 4],
    /// The page's colour, which the open page's tab shares.
    pub paper: [f32; 4],
    /// Text on the paper, and on the page wherever its colour is left automatic.
    pub paper_ink: [f32; 4],
    /// Key caps, small controls and field borders.
    pub chip: [f32; 4],
    /// Saturation and lightness each of a section's colours takes from its hue.
    pub shades: Shades,
}

/// Saturation and lightness pairs, each from 0 to 1.
#[derive(Clone, Copy, Debug)]
pub struct Shades {
    /// The frame's top and bottom; the bottom turns 10° further round the hue.
    pub frame: [[f32; 2]; 2],
    pub tab: [f32; 2],
    pub edge: [f32; 2],
    pub accent: [f32; 2],
}

/// A section's colours in a theme.
#[derive(Clone, Copy, Debug)]
pub struct Section {
    /// Top and bottom of the frame around the page, which the open tab shares.
    pub frame: [[f32; 4]; 2],
    /// Tabs not open.
    pub tab: [f32; 4],
    /// Borders around the open tab and the page.
    pub edge: [f32; 4],
    pub accent: [f32; 4],
}

impl Theme {
    /// After File Pilot's default dark scheme.
    pub fn dark() -> Self {
        Self {
            font_size: 13.0,
            base: srgb(0x19, 0x1b, 0x1c),
            panel: srgb(0x1f, 0x22, 0x23),
            strip: srgb(0x27, 0x2a, 0x2b),
            accent: srgb(0x00, 0x79, 0xa6),
            text: srgb(0xdd, 0xde, 0xe0),
            text_dim: srgb(0x6b, 0x70, 0x78),
            ink: srgb(0xe8, 0xe9, 0xeb),
            paper: srgb(0x1f, 0x20, 0x22),
            paper_ink: srgb(0xe6, 0xe6, 0xe6),
            chip: srgb(0x38, 0x3c, 0x3d),
            shades: Shades {
                frame: [[0.30, 0.36], [0.30, 0.30]],
                tab: [0.22, 0.25],
                edge: [0.30, 0.16],
                accent: [0.55, 0.55],
            },
        }
    }

    pub fn light() -> Self {
        Self {
            font_size: 13.0,
            base: srgb(0xfc, 0xfc, 0xfd),
            panel: srgb(0xf4, 0xf5, 0xf7),
            strip: srgb(0xeb, 0xed, 0xf0),
            accent: srgb(0x00, 0x79, 0xa6),
            text: srgb(0x1f, 0x23, 0x28),
            text_dim: srgb(0x7b, 0x82, 0x8c),
            ink: srgb(0x1d, 0x1e, 0x20),
            paper: [1.0; 4],
            paper_ink: [0.0, 0.0, 0.0, 1.0],
            chip: srgb(0xcf, 0xd3, 0xd9),
            shades: Shades {
                frame: [[0.55, 0.82], [0.55, 0.76]],
                tab: [0.45, 0.70],
                edge: [0.30, 0.52],
                accent: [0.60, 0.45],
            },
        }
    }

    /// Hovered controls and selected text: the accent over the base.
    pub fn hover(&self) -> [f32; 4] {
        crate::mix(self.base, self.accent, 0.35)
    }

    /// The colours a section takes from the hue of `color`, linear RGBA.
    pub fn section(&self, color: [f32; 4]) -> Section {
        let hue = hue(color);
        let shade = |hue: f32, [saturation, lightness]: [f32; 2]| hsl(hue, saturation, lightness);
        let Shades {
            frame,
            tab,
            edge,
            accent,
        } = self.shades;
        Section {
            frame: [shade(hue, frame[0]), shade(hue + 10.0, frame[1])],
            tab: shade(hue, tab),
            edge: shade(hue, edge),
            accent: shade(hue, accent),
        }
    }
}

/// The hue in degrees of a linear colour, as it looks in sRGB.
fn hue(color: [f32; 4]) -> f32 {
    let [r, g, b] = [color[0], color[1], color[2]].map(|value| {
        if value <= 0.003_130_8 {
            value * 12.92
        } else {
            1.055 * value.powf(1.0 / 2.4) - 0.055
        }
    });
    let max = r.max(g).max(b);
    let range = max - r.min(g).min(b);
    if range == 0.0 {
        return 0.0;
    }
    let sector = if max == r {
        (g - b) / range
    } else if max == g {
        (b - r) / range + 2.0
    } else {
        (r - g) / range + 4.0
    };
    (sector * 60.0).rem_euclid(360.0)
}

/// An sRGB hue, saturation and lightness as linear RGBA.
fn hsl(hue: f32, saturation: f32, lightness: f32) -> [f32; 4] {
    let chroma = (1.0 - (2.0 * lightness - 1.0).abs()) * saturation;
    let channel = |offset: f32| {
        let k = (offset + hue / 30.0).rem_euclid(12.0);
        let value = lightness - chroma / 2.0 * (k - 3.0).min(9.0 - k).clamp(-1.0, 1.0);
        (value * 255.0).round().clamp(0.0, 255.0) as u8
    };
    srgb(channel(0.0), channel(8.0), channel(4.0))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sections_keep_their_hue_at_the_theme_s_shades() {
        let orange = srgb(0xf6, 0xb0, 0x78);
        let section = Theme::dark().section(orange);
        assert!((hue(section.frame[0]) - hue(orange)).abs() < 3.0);
        assert!((hue(section.frame[1]) - hue(orange) - 10.0).abs() < 3.0);
        assert_eq!(hsl(0.0, 1.0, 0.5), srgb(0xff, 0, 0));
        assert_eq!(hsl(240.0, 0.0, 1.0), srgb(0xff, 0xff, 0xff));
    }
}
