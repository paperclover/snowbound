use draw::srgb;

/// Colours are linear RGBA; sizes are logical pixels.
#[derive(Clone, Debug)]
pub struct Theme {
    pub font_size: f32,
    /// Content surfaces, such as the active tab and lists.
    pub base: [f32; 4],
    /// Side panels.
    pub panel: [f32; 4],
    /// The window's top strip and inactive tabs.
    pub strip: [f32; 4],
    /// Hairlines between rows and panels.
    pub separator: [f32; 4],
    pub accent: [f32; 4],
    /// A hovered row under the pointer, outlined in the accent.
    pub hover: [f32; 4],
    pub text: [f32; 4],
    pub text_dim: [f32; 4],
    /// Key caps and small controls.
    pub chip: [f32; 4],
}

impl Theme {
    /// After File Pilot's default dark scheme.
    pub fn dark() -> Self {
        Self {
            font_size: 13.0,
            base: srgb(0x19, 0x1b, 0x1c),
            panel: srgb(0x1f, 0x22, 0x23),
            strip: srgb(0x27, 0x2a, 0x2b),
            separator: srgb(0x20, 0x23, 0x24),
            accent: srgb(0x00, 0x79, 0xa6),
            hover: srgb(0x10, 0x3c, 0x4c),
            text: srgb(0xdd, 0xde, 0xe0),
            text_dim: srgb(0x6b, 0x70, 0x78),
            chip: srgb(0x38, 0x3c, 0x3d),
        }
    }
}
