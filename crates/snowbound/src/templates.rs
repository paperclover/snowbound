//! The template strip a new page shows where its body would start until the body is typed
//! in, and the galleries of every template docked beside the page.

use crate::art;
use canvas::{
    gpu::Paper,
    template::{TEMPLATES, Template},
};
use draw::RasterImage;
use std::collections::HashMap;
use ui::{Anchor, Axis, Flags, Id, Spec, Theme, Ui, fill, px};

/// What a thumbnail shows: the top of a letter page, in points from the page origin.
const REGION: [f32; 4] = [-72.0, -40.0, 540.0, 419.0];
pub(crate) const TILE: [f32; 2] = [112.0, 84.0];
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
enum Choice {
    Template(&'static str),
    /// Opens the Page Color menu over its tile.
    Colors,
    More,
    Dismiss,
}

/// The gallery of every template's art, from the Page Color menu's Show All, to lie behind
/// the open page.
pub fn backgrounds() -> Id {
    Id::ROOT.child("backgrounds")
}

/// The gallery of every template, from the strip's More templates.
fn templates() -> Id {
    Id::ROOT.child("templates")
}

/// The page colours the strip's Solid Color opens.
fn colors() -> Id {
    Id::ROOT.child("strip colors")
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

/// The strip at `position` in the page box, `room` wide at most. Picks that do not fit give
/// way from the end; More templates stays. Returns the pick and the Solid Color tile, where
/// shown.
fn strip(
    ui: &mut Ui,
    theme: &Theme,
    thumbnails: &mut Thumbnails,
    paper: Paper,
    position: [f32; 2],
    room: f32,
) -> (Option<Choice>, Option<Id>) {
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
    let tiles = (shown.iter())
        .map(|(_, label)| GAP + tile_width(ui, label))
        .sum::<f32>()
        - GAP;
    let mut chosen = None;
    let mut solid = None;
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
        let width = tile_width(ui, label);
        let tile = tile(ui, theme, thumbnails, paper, index, choice, label, width);
        if choice == Choice::Colors {
            solid = Some(tile);
        }
        if ui.signal(tile).clicked {
            chosen = Some(choice);
        }
    }
    ui.close();
    let close = ui.leaf(
        "dismiss",
        Spec {
            flags: Flags::FLOAT | Flags::CLICKABLE,
            size: [px(22.0), px(22.0)],
            position: [tiles + 2.0 * PAD - 14.0, -8.0],
            icon: Some(art::CLOSE),
            color: Some(theme.text_dim),
            fill: Some(theme.popup),
            border: Some(theme.chip),
            hover_fill: Some(theme.hover()),
            radius: 11.0,
            center: true,
            role: Some(accesskit::Role::Button),
            ..Spec::default()
        },
    );
    crate::name(ui, ui.id("dismiss"), "Close");
    if close.clicked {
        chosen = Some(Choice::Dismiss);
    }
    ui.close();
    (chosen, solid)
}

/// Dialog `id`, titled `title`, of every template docked inside the page box's trailing edge,
/// `height` tall, while it is open. Returns the template chosen; it stays open to choose again.
#[allow(clippy::too_many_arguments)]
fn gallery(
    ui: &mut Ui,
    theme: &Theme,
    thumbnails: &mut Thumbnails,
    paper: Paper,
    id: Id,
    title: &str,
    height: f32,
) -> Option<&'static str> {
    if !ui.popup_open(id) {
        return None;
    }
    let mut chosen = None;
    let column = TILE[0] + 40.0;
    ui.open_as(
        id,
        Spec {
            axis: Axis::Y,
            size: [px(2.0 * column + GAP + 2.0 * PAD), px(height)],
            fill: Some(theme.popup),
            border: Some(theme.chip),
            shadow: Some(theme.shadow),
            radius: 8.0,
            pad: [PAD, PAD],
            gap: 8.0,
            anchor: Some(Anchor::Dock(crate::page())),
            role: Some(accesskit::Role::Dialog),
            ..Spec::default()
        },
    );
    if let Some(node) = ui.access(id) {
        node.set_label(title);
    }
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
            text: Some(title),
            bold: true,
            role: Some(accesskit::Role::Heading),
            ..Spec::default()
        },
    );
    if ui::shell::tool_button(ui, "close", art::CLOSE, theme.text_dim, None).clicked {
        ui.close_popup(id);
    }
    crate::name(ui, ui.id("close"), "Close");
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
    for (row, templates) in TEMPLATES.chunks(2).enumerate() {
        ui.open(
            row,
            Spec {
                size: [fill(), ui::children()],
                gap: GAP,
                ..Spec::default()
            },
        );
        for (index, template) in templates.iter().enumerate() {
            let choice = Choice::Template(template.name);
            let tile = tile(
                ui,
                theme,
                thumbnails,
                paper,
                index,
                choice,
                template.name,
                column,
            );
            if ui.signal(tile).clicked {
                chosen = Some(template.name);
            }
        }
        ui.close();
    }
    ui.close();
    ui.close();
    chosen
}

/// How wide a tile labelled `label` stands: its thumbnail, or the label where it is wider.
fn tile_width(ui: &mut Ui, label: &str) -> f32 {
    TILE[0].max(ui.measure(label)[0] + 4.0)
}

/// One template's thumbnail over its label, `width` wide; returns the thumbnail, which takes
/// the click.
#[allow(clippy::too_many_arguments)]
fn tile(
    ui: &mut Ui,
    theme: &Theme,
    thumbnails: &mut Thumbnails,
    paper: Paper,
    part: usize,
    choice: Choice,
    label: &str,
    width: f32,
) -> Id {
    let scale = ui.scale();
    let dark = paper.ink[0] > paper.color[0];
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
            fill: Some(if choice == Choice::More {
                theme.base
            } else {
                paper.color
            }),
            border: Some(theme.chip),
            hover_border: Some(theme.accent),
            radius: 4.0,
            icon: (choice == Choice::More).then_some(art::PLUS),
            color: Some(theme.text_dim),
            center: true,
            role: Some(accesskit::Role::Button),
            ..Spec::default()
        },
    );
    crate::name(ui, id, label);
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
        // A band of each of the first page colours, as the page shows them.
        Choice::Colors => {
            let bands = 4;
            for (index, (_, color)) in canvas::template::PAGE_COLORS.iter().take(bands).enumerate()
            {
                let band = TILE[0] / bands as f32;
                ui.leaf(
                    ("band", index),
                    Spec {
                        flags: Flags::FLOAT,
                        size: [px(band), px(TILE[1])],
                        position: [band * index as f32, 0.0],
                        fill: Some(paper.colored(Some(*color)).color),
                        ..Spec::default()
                    },
                );
            }
        }
        Choice::More | Choice::Dismiss => {}
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
    id
}

impl crate::State {
    /// The template galleries while open, and over a page with nothing typed in its body the
    /// strip where the body would start.
    pub(crate) fn template_strip(&mut self, theme: &Theme) {
        let Some(session) = &self.session else {
            return;
        };
        let space = session.space;
        let read_only = session.read_only();
        let Some(rect) = self.ui.laid_out(crate::page()) else {
            return;
        };
        // Thumbnails show templates on plain paper, not the page's colour.
        let paper = Paper {
            color: self.ui.theme.paper,
            ink: self.ui.theme.paper_ink,
        };
        let height = rect[3] - rect[1] - 8.0;
        if let Some(name) = gallery(
            &mut self.ui,
            theme,
            &mut self.thumbnails,
            paper,
            backgrounds(),
            "Backgrounds",
            height,
        ) {
            self.choose(crate::commands::Choice::Art(Some(name)));
        }
        let editor = &self.view.editor;
        let blank = editor
            .visible_outlines()
            .chain(editor.caret_outline())
            .filter(|outline| !outline.title)
            .all(canvas::editor::TextOutline::is_empty);
        if !blank || read_only || self.dismissed.contains(&space) {
            return;
        }
        let Some(start) = editor.body_start() else {
            return;
        };
        let (color, rule_lines) = (editor.page_color(), editor.rule_lines());
        let viewport = self.view.viewport;
        let scale = self.ui.scale();
        let position = [
            (viewport.origin[0] + start[0] * viewport.scale) / scale,
            (viewport.origin[1] + (start[1] + LINES_ABOVE) * viewport.scale) / scale,
        ];
        let (chosen, solid) = strip(
            &mut self.ui,
            theme,
            &mut self.thumbnails,
            paper,
            position,
            rect[2] - rect[0] - position[0] - 24.0,
        );
        match chosen {
            Some(Choice::Template(name)) => self.commands.push(crate::Command::Template(name)),
            Some(Choice::Colors) => self.ui.open_popup(colors()),
            Some(Choice::More) => self.ui.open_popup(templates()),
            Some(Choice::Dismiss) => {
                self.dismissed.insert(space);
            }
            None => {}
        }
        if let Some(name) = gallery(
            &mut self.ui,
            theme,
            &mut self.thumbnails,
            paper,
            templates(),
            "Page Templates",
            height,
        ) {
            self.commands.push(crate::Command::Template(name));
        }
        if let Some(choice) = solid.and_then(|tile| {
            crate::background::menu(
                &mut self.ui,
                colors(),
                Anchor::Below(tile),
                &mut self.thumbnails,
                paper,
                color,
                rule_lines,
                true,
                &mut self.page_color_preview,
            )
        }) {
            self.choose(choice);
        }
    }
}
