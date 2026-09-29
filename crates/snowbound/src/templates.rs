//! The template strip a new page shows where its body would start until the body is typed
//! in, and the gallery of every template it leads to.

use crate::art;
use canvas::{
    gpu::Paper,
    template::{PAGE_COLORS, TEMPLATES, Template},
};
use draw::RasterImage;
use std::collections::HashMap;
use ui::{Axis, Flags, Spec, Theme, Ui, fill, px};

/// What a thumbnail shows: the top of a letter page, in points from the page origin.
const REGION: [f32; 4] = [-72.0, -40.0, 540.0, 419.0];
pub(crate) const TILE: [f32; 2] = [112.0, 84.0];
const SWATCH: f32 = 24.0;
const LABEL: f32 = 22.0;
const GAP: f32 = 12.0;
const PAD: f32 = 10.0;
/// Where the strip stands below the body's first line: at its third, so Enter in the title
/// leaves a line clear above it.
const LINES_ABOVE: f32 = 2.0 * 18.0;
/// The strip's picks, in Clover's order, and their labels.
const STRIP: [(Choice, &str); 5] = [
    (Choice::Template("Ivy"), "Ivy"),
    (Choice::Template("Purple Clouds"), "Purple Clouds"),
    (Choice::Template("Notebook"), "Notebook"),
    (Choice::Colors, "Solid Color"),
    (Choice::Template("Informal Meeting Notes"), "Meeting"),
];

#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Choice {
    Template(&'static str),
    /// An index into `PAGE_COLORS`.
    Color(usize),
    /// Shows or hides the page colours.
    Colors,
    More,
    Dismiss,
}

/// What the template strip shows.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub enum View {
    #[default]
    Strip,
    /// The strip with the page colours beneath it.
    Colors,
    Gallery,
}

/// A thumbnail's art: each raster and where it lies in the tile.
type Art = Vec<(RasterImage, [f32; 4])>;

/// Rasters of each template's art for its thumbnail, by template and paper.
#[derive(Default)]
pub struct Thumbnails(HashMap<(&'static str, bool), Art>);

impl Thumbnails {
    /// `template`'s art and where it lies in a `TILE`, drawn at `scale` for `paper`.
    pub(crate) fn art(
        &mut self,
        template: &'static Template,
        paper: Paper,
        dark: bool,
        scale: f32,
    ) -> &[(RasterImage, [f32; 4])] {
        self.0.entry((template.name, dark)).or_insert_with(|| {
            let points = TILE[0] / (REGION[2] - REGION[0]);
            template
                .art
                .iter()
                .filter_map(|art| {
                    let (image, size) =
                        canvas::gpu::page::template_art(art.art, art.size, points * scale, paper)?;
                    let [x, y] = [
                        (art.position[0] - REGION[0]) * points,
                        (art.position[1] - REGION[1]) * points,
                    ];
                    Some((image, [x, y, size[0] * points, size[1] * points]))
                })
                .collect()
        })
    }
}

/// The strip at `position` in the page box, `room` wide at most, with the page colours
/// beneath while `colors`. Picks that do not fit give way from the end; More templates
/// stays.
#[allow(clippy::too_many_arguments)]
fn strip(
    ui: &mut Ui,
    theme: &Theme,
    thumbnails: &mut Thumbnails,
    paper: Paper,
    dark: bool,
    position: [f32; 2],
    room: f32,
    colors: bool,
) -> Option<Choice> {
    let more = (Choice::More, "More templates");
    let mut used = 2.0 * PAD + tile_width(ui, more.1);
    let mut shown = Vec::new();
    for pick in STRIP {
        used += GAP + tile_width(ui, pick.1);
        if used > room {
            break;
        }
        shown.push(pick);
    }
    shown.push(more);
    let mut chosen = None;
    ui.open(
        "templates",
        Spec {
            flags: Flags::FLOAT | Flags::CLICKABLE,
            axis: Axis::Y,
            size: [ui::children(), ui::children()],
            position,
            fill: Some(theme.popup),
            shadow: Some(theme.shadow),
            radius: 8.0,
            pad: [PAD, PAD],
            gap: 8.0,
            ..Spec::default()
        },
    );
    ui.open(
        "tiles",
        Spec {
            size: [ui::children(), ui::children()],
            gap: GAP,
            ..Spec::default()
        },
    );
    for (index, (choice, label)) in shown.into_iter().enumerate() {
        if tile(ui, theme, thumbnails, paper, dark, index, choice, label) {
            chosen = Some(choice);
        }
    }
    ui.close();
    let per_row = (((room - 2.0 * PAD + 6.0) / (SWATCH + 6.0)).floor() as usize).max(1);
    for (row, swatches) in PAGE_COLORS.chunks(per_row).enumerate().filter(|_| colors) {
        ui.open(
            ("colors", row),
            Spec {
                size: [ui::children(), px(SWATCH)],
                gap: 6.0,
                ..Spec::default()
            },
        );
        for (index, (name, color)) in swatches
            .iter()
            .enumerate()
            .map(|(index, color)| (row * per_row + index, color))
        {
            let swatch = ui.leaf(
                (index, *name),
                Spec {
                    flags: Flags::CLICKABLE,
                    size: [px(SWATCH), px(SWATCH)],
                    fill: Some(page_color(*color, paper, dark)),
                    border: Some(theme.chip),
                    hover_border: Some(theme.accent),
                    radius: 4.0,
                    ..Spec::default()
                },
            );
            if swatch.clicked {
                chosen = Some(Choice::Color(index));
            }
        }
        ui.close();
    }
    let width = ui
        .rect(ui.id("tiles"))
        .map_or(0.0, |rect| rect[2] - rect[0]);
    let close = ui.leaf(
        "dismiss",
        Spec {
            flags: Flags::FLOAT | Flags::CLICKABLE,
            size: [px(22.0), px(22.0)],
            position: [width + 2.0 * PAD - 14.0, -8.0],
            icon: Some(art::CLOSE),
            color: Some(theme.text_dim),
            fill: Some(theme.popup),
            border: Some(theme.chip),
            hover_fill: Some(theme.hover()),
            radius: 11.0,
            center: true,
            ..Spec::default()
        },
    );
    if close.clicked {
        chosen = Some(Choice::Dismiss);
    }
    ui.close();
    chosen
}

/// Every template, as a scrolling grid over the page box `size` big.
fn gallery(
    ui: &mut Ui,
    theme: &Theme,
    thumbnails: &mut Thumbnails,
    paper: Paper,
    dark: bool,
    size: [f32; 2],
) -> Option<Choice> {
    let mut chosen = None;
    let margin = 24.0;
    ui.open(
        "gallery",
        Spec {
            flags: Flags::FLOAT | Flags::CLICKABLE,
            axis: Axis::Y,
            size: [px(size[0] - 2.0 * margin), px(size[1] - 2.0 * margin)],
            position: [margin, margin],
            fill: Some(theme.popup),
            shadow: Some(theme.shadow),
            radius: 8.0,
            pad: [PAD, PAD],
            gap: 8.0,
            ..Spec::default()
        },
    );
    ui.open(
        "header",
        Spec {
            size: [fill(), px(24.0)],
            ..Spec::default()
        },
    );
    ui.leaf(
        "title",
        Spec {
            size: [fill(), px(24.0)],
            text: Some("Page Templates"),
            ..Spec::default()
        },
    );
    if ui::shell::tool_button(ui, "close", art::CLOSE, theme.text_dim, false).clicked {
        chosen = Some(Choice::Dismiss);
    }
    ui.close();
    ui.open(
        "grid",
        Spec {
            flags: Flags::SCROLL | Flags::CLIP,
            axis: Axis::Y,
            size: [fill(), fill()],
            gap: GAP,
            ..Spec::default()
        },
    );
    let column = TILE[0] + 40.0;
    let columns =
        (((size[0] - 2.0 * margin - 2.0 * PAD + GAP) / (column + GAP)).floor() as usize).max(1);
    let choices: Vec<(Choice, &str)> = TEMPLATES
        .iter()
        .map(|template| (Choice::Template(template.name), template.name))
        .chain(
            PAGE_COLORS
                .iter()
                .enumerate()
                .map(|(index, (name, _))| (Choice::Color(index), *name)),
        )
        .collect();
    for (row, choices) in choices.chunks(columns).enumerate() {
        ui.open(
            row,
            Spec {
                size: [fill(), ui::children()],
                gap: GAP,
                ..Spec::default()
            },
        );
        for (index, (choice, label)) in choices.iter().enumerate() {
            if tile(ui, theme, thumbnails, paper, dark, index, *choice, label) {
                chosen = Some(*choice);
            }
        }
        ui.close();
    }
    ui.close();
    ui.close();
    chosen
}

/// A page colour as the paper shows it: on dark paper, its hue at the paper's lightness.
fn page_color(color: u32, paper: Paper, dark: bool) -> [f32; 4] {
    let color = canvas::gpu::colorref(color);
    if !dark {
        return color;
    }
    let [lightness, ..] = draw::oklab(paper.color);
    let [_, a, b] = draw::oklab(color);
    let [red, green, blue] = draw::from_oklab([lightness + 0.04, 2.0 * a, 2.0 * b]);
    [red, green, blue, 1.0]
}

/// How wide a tile labelled `label` stands: its thumbnail, or the label where it is wider.
fn tile_width(ui: &mut Ui, label: &str) -> f32 {
    TILE[0].max(ui.measure(label)[0] + 4.0)
}

/// One template's thumbnail over its label, as wide as the thumbnail or the label; true
/// when clicked.
#[allow(clippy::too_many_arguments)]
fn tile(
    ui: &mut Ui,
    theme: &Theme,
    thumbnails: &mut Thumbnails,
    paper: Paper,
    dark: bool,
    part: usize,
    choice: Choice,
    label: &str,
) -> bool {
    let scale = ui.scale();
    let width = tile_width(ui, label);
    let fill_color = match choice {
        Choice::Color(index) => page_color(PAGE_COLORS[index].1, paper, dark),
        Choice::More | Choice::Dismiss => theme.base,
        Choice::Template(_) | Choice::Colors => paper.color,
    };
    ui.open(
        (part, label),
        Spec {
            axis: Axis::Y,
            size: [px(width), px(TILE[1] + LABEL)],
            pad: [(width - TILE[0]) / 2.0, 0.0],
            ..Spec::default()
        },
    );
    let id = ui.open(
        "thumbnail",
        Spec {
            flags: Flags::CLICKABLE | Flags::CLIP,
            size: [px(TILE[0]), px(TILE[1])],
            fill: Some(fill_color),
            border: Some(theme.chip),
            hover_border: Some(theme.accent),
            radius: 4.0,
            icon: (choice == Choice::More).then_some(art::PLUS),
            color: Some(theme.text_dim),
            center: true,
            ..Spec::default()
        },
    );
    match choice {
        Choice::Template(name) => {
            if let Some(template) = canvas::template::find(name) {
                for (index, (image, [x, y, width, height])) in thumbnails
                    .art(template, paper, dark, scale)
                    .iter()
                    .enumerate()
                {
                    ui.leaf(
                        ("art", index),
                        Spec {
                            flags: Flags::FLOAT,
                            size: [px(*width), px(*height)],
                            position: [*x, *y],
                            image: Some(image),
                            ..Spec::default()
                        },
                    );
                }
            }
        }
        // A band of each of the first page colours.
        Choice::Colors => {
            let bands = 4;
            for (index, (_, color)) in PAGE_COLORS.iter().take(bands).enumerate() {
                let band = TILE[0] / bands as f32;
                ui.leaf(
                    ("band", index),
                    Spec {
                        flags: Flags::FLOAT,
                        size: [px(band), px(TILE[1])],
                        position: [band * index as f32, 0.0],
                        fill: Some(page_color(*color, paper, dark)),
                        ..Spec::default()
                    },
                );
            }
        }
        _ => {}
    }
    ui.close();
    ui.leaf(
        "label",
        Spec {
            size: [px(width), px(LABEL)],
            text: Some(label),
            color: Some(theme.text_dim),
            center: true,
            ..Spec::default()
        },
    );
    ui.close();
    ui.signal(id).clicked
}

impl crate::State {
    /// Over a page with nothing typed in its body: the strip where the body would start,
    /// or the gallery it opened.
    pub(crate) fn template_strip(&mut self, theme: &Theme) {
        let Some(session) = &self.session else {
            return;
        };
        let space = session.space;
        let editor = &self.view.editor;
        let blank = editor
            .visible_outlines()
            .chain(editor.caret_outline())
            .filter(|outline| !outline.title)
            .all(canvas::editor::TextOutline::is_empty);
        if !blank || session.read_only() || self.dismissed.contains(&space) {
            self.templates = View::Strip;
            return;
        }
        let (Some(rect), Some(start)) = (self.ui.rect(crate::page()), editor.body_start()) else {
            return;
        };
        let viewport = self.view.viewport;
        let scale = self.ui.scale();
        let position = [
            (viewport.origin[0] + start[0] * viewport.scale) / scale,
            (viewport.origin[1] + (start[1] + LINES_ABOVE) * viewport.scale) / scale,
        ];
        // Thumbnails show templates on plain paper, not the page's colour.
        let paper = Paper {
            color: self.ui.theme.paper,
            ink: self.ui.theme.paper_ink,
        };
        let dark = paper.ink[0] > paper.color[0];
        let chosen = match self.templates {
            View::Gallery => {
                let size = [rect[2] - rect[0], rect[3] - rect[1]];
                gallery(&mut self.ui, theme, &mut self.thumbnails, paper, dark, size)
            }
            view => strip(
                &mut self.ui,
                theme,
                &mut self.thumbnails,
                paper,
                dark,
                position,
                rect[2] - rect[0] - position[0] - 24.0,
                view == View::Colors,
            ),
        };
        match chosen {
            Some(Choice::More) => self.templates = View::Gallery,
            Some(Choice::Colors) => {
                self.templates = if self.templates == View::Colors {
                    View::Strip
                } else {
                    View::Colors
                }
            }
            Some(Choice::Dismiss) if self.templates == View::Gallery => {
                self.templates = View::Strip
            }
            Some(Choice::Dismiss) => {
                self.dismissed.insert(space);
            }
            Some(choice) => {
                self.templates = View::Strip;
                self.commands.push(crate::Command::Template(choice));
            }
            None => {}
        }
    }
}
