use draw::{hsl, hue, srgb};

/// Colours are linear RGBA; sizes are logical pixels.
#[derive(Clone, Debug)]
pub struct Theme {
    pub font_size: f32,
    /// Content surfaces, such as fields and lists.
    pub base: [f32; 4],
    /// Side panels.
    pub panel: [f32; 4],
    /// The window's title bar, toolbar, tab row and sidebar; transparent where the system's
    /// backdrop shows through them.
    pub strip: [f32; 4],
    pub accent: [f32; 4],
    /// The text caret, and selected text's fill with and without keyboard focus; the
    /// platform's own where it has them.
    pub caret: [f32; 4],
    pub selection: [f32; 4],
    pub inactive_selection: [f32; 4],
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
    /// Menus and other popups, and the shadow they cast.
    pub popup: [f32; 4],
    pub shadow: [f32; 4],
    /// Saturation and lightness each of a section's colours takes from its hue.
    pub shades: Shades,
    /// Menus as the desktop draws its own, where Snowbound follows it; see `Theme::menu`.
    pub desktop_menu: Option<Menu>,
}

/// How menus and the other popups look and move. Colours are linear RGBA; sizes are
/// logical pixels.
#[derive(Clone, Debug, PartialEq)]
pub struct Menu {
    pub fill: [f32; 4],
    pub border: [f32; 4],
    /// The layers of the shadow a popup casts, painted in order.
    pub shadows: Vec<Shadow>,
    pub text: [f32; 4],
    /// Shortcuts and headings.
    pub dim: [f32; 4],
    pub disabled: [f32; 4],
    /// The row the pointer or keyboard is on, and its outline.
    pub highlight: [f32; 4],
    pub highlight_border: Option<[f32; 4]>,
    /// The line between groups of rows.
    pub rule: [f32; 4],
    pub font_size: f32,
    pub radius: f32,
    /// Inset of the popup's contents.
    pub pad: f32,
    pub row: f32,
    pub row_radius: f32,
    /// Inset of a row's icon and text inside its highlight.
    pub row_pad: f32,
    /// Height of the band between groups, and how far the rule across it stays from the
    /// rows' ends.
    pub rule_band: f32,
    pub rule_inset: f32,
    pub motion: PopupMotion,
}

/// A soft shadow blurred `blur` wide, `drop` below its box and grown `spread` past it, as
/// CSS's `box-shadow` takes them.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Shadow {
    pub color: [f32; 4],
    pub blur: f32,
    pub drop: f32,
    pub spread: f32,
}

/// How popups other than dialogs open and close.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum PopupMotion {
    /// Growing out of what they open from as they fade, Snowbound's own.
    Grow,
    /// At once, as GTK 4 shows its popovers.
    Cut,
    /// Opacity alone: in linearly over the first seconds, out over the second easing as
    /// OutQuart, as KWin's Fading Popups.
    Fade([f32; 2]),
}

/// The platform's caret and selection colours in a light or `dark` appearance.
fn text_colors(dark: bool) -> [[f32; 4]; 3] {
    draw::edit::Platform::CURRENT
        .text_colors(dark)
        .map(|([red, green, blue], alpha)| {
            let [red, green, blue, _] = srgb(red, green, blue);
            [red, green, blue, alpha]
        })
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

impl Section {
    /// A tab under the pointer: most of the way to the accent, which lies past the frame's
    /// lightness from the tabs in both themes, so it stands out from both.
    pub fn hover(&self) -> [f32; 4] {
        crate::mix(self.tab, self.accent, 0.7)
    }
}

impl Theme {
    /// After File Pilot's default dark scheme.
    pub fn dark() -> Self {
        let [caret, selection, inactive_selection] = text_colors(true);
        Self {
            font_size: 13.0,
            base: srgb(0x19, 0x1b, 0x1c),
            panel: srgb(0x1f, 0x22, 0x23),
            strip: srgb(0x27, 0x2a, 0x2b),
            accent: srgb(0x00, 0x79, 0xa6),
            caret,
            selection,
            inactive_selection,
            text: srgb(0xdd, 0xde, 0xe0),
            text_dim: srgb(0x6b, 0x70, 0x78),
            ink: srgb(0xe8, 0xe9, 0xeb),
            paper: srgb(0x1f, 0x20, 0x22),
            paper_ink: srgb(0xe6, 0xe6, 0xe6),
            chip: srgb(0x38, 0x3c, 0x3d),
            popup: srgb(0x2b, 0x2e, 0x30),
            shadow: [0.0, 0.0, 0.0, 0.6],
            shades: Shades {
                frame: [[0.30, 0.36], [0.30, 0.30]],
                tab: [0.22, 0.25],
                edge: [0.30, 0.16],
                accent: [0.55, 0.55],
            },
            desktop_menu: None,
        }
    }

    pub fn light() -> Self {
        let [caret, selection, inactive_selection] = text_colors(false);
        Self {
            font_size: 13.0,
            base: srgb(0xfc, 0xfc, 0xfd),
            panel: srgb(0xf4, 0xf5, 0xf7),
            strip: srgb(0xeb, 0xed, 0xf0),
            accent: srgb(0x00, 0x79, 0xa6),
            caret,
            selection,
            inactive_selection,
            text: srgb(0x1f, 0x23, 0x28),
            text_dim: srgb(0x7b, 0x82, 0x8c),
            ink: srgb(0x1d, 0x1e, 0x20),
            paper: [1.0; 4],
            paper_ink: [0.0, 0.0, 0.0, 1.0],
            chip: srgb(0xcf, 0xd3, 0xd9),
            popup: [1.0; 4],
            shadow: [0.0, 0.0, 0.0, 0.3],
            shades: Shades {
                frame: [[0.55, 0.82], [0.55, 0.76]],
                tab: [0.45, 0.70],
                edge: [0.30, 0.52],
                accent: draw::LIGHT_ACCENT,
            },
            desktop_menu: None,
        }
    }

    /// The theme over a system material that shows through the strip: dim text, controls
    /// and fields take the text colour at part opacity, as vibrancy draws them.
    pub fn over_backdrop(self) -> Self {
        let text = |alpha| [self.text[0], self.text[1], self.text[2], alpha];
        Self {
            strip: [0.0; 4],
            text_dim: text(0.5),
            chip: text(0.1),
            base: text(0.05),
            ..self
        }
    }

    /// Menus as the desktop draws them, or Snowbound's own.
    pub fn menu(&self) -> Menu {
        use crate::popup::{MENU_ROW, PAD};
        self.desktop_menu.clone().unwrap_or_else(|| Menu {
            fill: self.popup,
            border: self.chip,
            shadows: vec![Shadow {
                color: self.shadow,
                blur: crate::POPUP_SHADOW[0],
                drop: crate::POPUP_SHADOW[1],
                spread: 0.0,
            }],
            text: self.text,
            dim: self.text_dim,
            disabled: self.text_dim,
            highlight: self.hover(),
            highlight_border: None,
            rule: self.chip,
            font_size: self.font_size,
            radius: 6.0,
            pad: PAD,
            row: MENU_ROW,
            row_radius: 4.0,
            row_pad: 8.0,
            rule_band: 9.0,
            rule_inset: 0.0,
            motion: PopupMotion::Grow,
        })
    }

    /// Hovered controls: the accent over the base.
    pub fn hover(&self) -> [f32; 4] {
        crate::mix(self.base, self.accent, 0.35)
    }

    /// Colours for icons' slots under a section of `accent`, with `highlight` the colour
    /// the highlighter applies. The accent keeps its hue but stands as clear of the toolbar's
    /// lightness as text must; the badge takes the first of green, blue and orange whose hue
    /// stands clear of the accent's.
    pub fn icon_palette(&self, accent: [f32; 4], highlight: [f32; 4]) -> draw::Palette {
        // Over a system material the strip is clear, and the panel has its lightness.
        let behind = if self.strip[3] == 1.0 {
            self.strip
        } else {
            self.panel
        };
        let [under, ..] = draw::oklab(behind);
        let [lightness, a, b] = draw::oklab(accent);
        let lightness = if under < 0.5 {
            lightness.max(under + ICON_CONTRAST)
        } else {
            lightness.min(under - ICON_CONTRAST)
        };
        let [r, g, b] = draw::from_oklab([lightness, a, b]);
        let accent = [r, g, b, 1.0];
        let apart = |a: f32, b: f32| {
            let turn = (a - b).rem_euclid(360.0);
            turn.min(360.0 - turn)
        };
        let accent_hue = hue(accent);
        let badge = [135.0, 215.0, 30.0]
            .into_iter()
            .find(|badge| apart(*badge, accent_hue) >= 60.0)
            .expect("Three hues this far apart leave one clear of any other");
        let rgb = |[r, g, b, _]: [f32; 4]| [r, g, b];
        draw::Palette {
            accent: Some(rgb(accent)),
            highlight: Some(rgb(highlight)),
            badge: Some(rgb(hsl(badge, 0.65, 0.45))),
        }
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

/// Least OKLab lightness difference between icons' accent marks and the toolbar.
const ICON_CONTRAST: f32 = 0.4;

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

    #[test]
    fn icon_badges_stand_clear_of_the_accent() {
        let badge = |accent_hue: f32| {
            let palette = Theme::light().icon_palette(hsl(accent_hue, 0.6, 0.45), [1.0; 4]);
            let [r, g, b] = palette.badge.unwrap();
            hue([r, g, b, 1.0]).round()
        };
        assert_eq!(badge(210.0), 135.0);
        assert_eq!(badge(120.0), 215.0);
        assert_eq!(badge(170.0), 30.0);
    }

    #[test]
    fn icon_accents_stand_clear_of_the_toolbar() {
        for theme in [Theme::light(), Theme::dark()] {
            let [under, ..] = draw::oklab(theme.strip);
            for hue in (0..360).step_by(15) {
                let accent = theme.section(hsl(hue as f32, 0.6, 0.6)).accent;
                let [r, g, b] = theme.icon_palette(accent, [1.0; 4]).accent.unwrap();
                let [lightness, ..] = draw::oklab([r, g, b, 1.0]);
                assert!(
                    (lightness - under).abs() >= ICON_CONTRAST - 0.02,
                    "{hue}: {lightness} on {under}"
                );
            }
        }
    }

    #[test]
    fn a_hovered_tab_stands_out_from_the_frame_and_its_neighbours() {
        let lightness = |[r, g, b, _]: [f32; 4]| {
            let y = 0.2126 * r + 0.7152 * g + 0.0722 * b;
            116.0 * y.cbrt() - 16.0
        };
        for theme in [Theme::light(), Theme::dark()] {
            for hue in (0..360).step_by(5) {
                let section = theme.section(hsl(hue as f32, 0.6, 0.6));
                let hover = lightness(section.hover());
                for other in [section.frame[0], section.frame[1], section.tab] {
                    assert!((hover - lightness(other)).abs() > 5.0, "{hue}");
                }
            }
        }
    }

    #[test]
    fn colours_mix_as_premultiplied_so_transparency_fades_a_colour_in() {
        let [white, black] = [[1.0; 4], [0.0, 0.0, 0.0, 1.0]];
        assert_eq!(crate::mix(white, black, 0.25), [0.75, 0.75, 0.75, 1.0]);
        let accent = Theme::light().accent;
        let faded = crate::mix([0.0; 4], accent, 0.6);
        assert!((0..3).all(|channel| (faded[channel] - accent[channel]).abs() < 1e-6));
        assert!((faded[3] - 0.6).abs() < 1e-6);
        assert_eq!(crate::mix([0.0; 4], [0.0; 4], 0.5), [0.0; 4]);
    }
}
