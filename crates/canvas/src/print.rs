//! Pages on sheets of paper as OneNote 2010 prints them and saves them as PDF.
//!
//! OneNote draws a page between half-inch top and bottom margins, the page's margin origin
//! an inch from the paper's left edge and at the top margin, shrinking the whole page
//! when its content reaches past the paper's right edge ("Scale content to paper width").
//! A page longer than a sheet continues on the next, which starts at the first line of text
//! or picture the sheet's foot would have cut. Rule lines and template art print across the
//! paper's width; the page colour does not. Each sheet's footer names the section and
//! numbers the sheet, counting from the first page printed, in Times New Roman.

use crate::gpu::{Paper, page::PageScene};
use crate::layout::{LayoutError, TextEngine};
use draw::{Layer, Primitive, RenderError, Sheet};
use onestore::page::{Page, text::Paragraph};
use std::fmt;

pub const LETTER: [f32; 2] = [612.0, 792.0];
pub const A4: [f32; 2] = [595.276, 841.89];

/// The paper's top and bottom margins, where no page content prints.
const MARGIN: f32 = 36.0;
/// How far from the paper's left edge the margin origin prints at full size.
const LEFT: f32 = 72.0;
/// The room OneNote leaves right of the content when it shrinks a page to the paper's width.
const RIGHT: f32 = 9.0;
/// Pixels per point pictures are decoded at, 300 a inch.
const DENSITY: f32 = 300.0 / 72.0;

/// How a page lies on its sheets.
struct Pagination {
    /// Paper points per page point.
    scale: f32,
    /// The page x at the paper's left edge.
    left: f32,
    /// Where each sheet starts down the page.
    tops: Vec<f32>,
    /// How much of the page a sheet holds, in page points.
    band: f32,
}

impl Pagination {
    /// Lays content covering `bounds`, whose `rows` (lines of text, pictures) no sheet
    /// should cut, on `paper` from the page's `margin_origin`.
    fn new(
        bounds: [f32; 4],
        rows: &[[f32; 2]],
        margin_origin: [f32; 2],
        paper: [f32; 2],
    ) -> Self {
        let left = margin_origin[0] - LEFT;
        let scale = if bounds[2].is_finite() {
            (paper[0] / (bounds[2] - left + RIGHT)).min(1.0)
        } else {
            1.0
        };
        let band = (paper[1] - 2.0 * MARGIN) / scale;
        let first = if bounds[1].is_finite() {
            margin_origin[1].min(bounds[1])
        } else {
            margin_origin[1]
        };
        let bottom = bounds[3].max(first);
        let mut tops = vec![first];
        let mut top = first;
        while top + band < bottom {
            // Moves the cut up to the top of whatever it would cross, until nothing shorter
            // than a sheet crosses it.
            let mut cut = top + band;
            while let Some(row) = rows.iter().find(|[start, end]| {
                *start < cut && *end > cut && *start > top + 1.0 && end - start <= band
            }) {
                cut = row[0];
            }
            tops.push(cut);
            top = cut;
        }
        Self {
            scale,
            left,
            tops,
            band,
        }
    }

    /// Each sheet's rows of the page, top and bottom: to where the next sheet starts, or
    /// the paper's bottom margin on the last.
    fn sheets(&self) -> impl Iterator<Item = [f32; 2]> + '_ {
        self.tops.iter().enumerate().map(|(at, top)| {
            [
                *top,
                self.tops.get(at + 1).copied().unwrap_or(top + self.band),
            ]
        })
    }
}

#[derive(Debug)]
pub enum PrintError {
    Scene(crate::gpu::page::SceneError),
    Layout(LayoutError),
    Render(RenderError),
}

impl fmt::Display for PrintError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Scene(error) => write!(formatter, "The page could not be laid out: {error}"),
            Self::Layout(error) => write!(formatter, "The footer could not be laid out: {error:?}"),
            Self::Render(error) => write!(formatter, "The PDF could not be written: {error:?}"),
        }
    }
}

impl std::error::Error for PrintError {}

/// `pages` in order as a PDF on `paper` (points), each on as many sheets as it takes, the
/// footers naming `section`.
pub fn pdf(
    pages: Vec<Page>,
    engine: &mut TextEngine,
    paper: [f32; 2],
    section: &str,
) -> Result<Vec<u8>, PrintError> {
    let title = pages
        .first()
        .map(|page| page.title.clone())
        .unwrap_or_default();
    let mut printed = Vec::with_capacity(pages.len());
    for page in pages {
        let (margin_origin, rules) = (page.margin_origin, page.rule_lines);
        let mut scene = PageScene::new(page, engine).map_err(PrintError::Scene)?;
        scene.settle(None, DENSITY, Paper::WHITE);
        let (bounds, rows) = scene.printed_extent().map_err(PrintError::Scene)?;
        let pagination = Pagination::new(bounds, &rows, margin_origin, paper);
        printed.push((scene, pagination, margin_origin, rules));
    }
    let mut contents = Vec::with_capacity(printed.len());
    for (scene, pagination, margin_origin, rules) in &printed {
        let mut primitives = Vec::new();
        scene
            .append_primitives(&mut primitives, [0.0; 2], Paper::WHITE)
            .map_err(PrintError::Scene)?;
        let rules: Vec<_> = pagination
            .sheets()
            .map(|[top, _]| {
                let right = pagination.left + paper[0] / pagination.scale;
                crate::interaction::rule_primitives(
                    *rules,
                    *margin_origin,
                    [pagination.left, top, right, top + pagination.band],
                    0.0,
                    Paper::WHITE,
                )
            })
            .collect();
        contents.push((primitives, rules));
    }
    let format = onestore::document::Format {
        font: Some("Times New Roman".into()),
        font_size: Some(10.0),
        ..Default::default()
    };
    let mut footers = Vec::new();
    for (sheet, _) in printed
        .iter()
        .flat_map(|(_, pagination, ..)| pagination.sheets())
        .enumerate()
    {
        let text = match section {
            "" => format!("Page {}", sheet + 1),
            section => format!("{section} Page {}", sheet + 1),
        };
        let layout = engine
            .layout(&Paragraph::new(text, format.clone()), f32::MAX)
            .map_err(PrintError::Layout)?;
        footers.push(layout);
    }
    let footer_primitives: Vec<_> = footers
        .iter()
        .map(|layout| {
            let (line, bounds) = layout.lines().next().expect("A laid-out footer has a line");
            [Primitive::Text {
                text: layout,
                origin: [-line.metrics().advance / 2.0, -bounds.baseline],
                clip: None,
                ink: Paper::WHITE.ink,
            }]
        })
        .collect();
    let mut sheets = Vec::new();
    let mut footer = footer_primitives.iter();
    for ((_, pagination, ..), (primitives, rules)) in printed.iter().zip(&contents) {
        let scale = pagination.scale;
        for ([top, bottom], rules) in pagination.sheets().zip(rules) {
            let origin = [-pagination.left * scale, MARGIN - top * scale];
            // Rule lines run to the bottom margin, the page's content to where the next
            // sheet takes it up.
            let page = |primitives, bottom: f32| Layer {
                scale,
                origin,
                clip: Some([0.0, MARGIN, paper[0], MARGIN + (bottom - top) * scale]),
                backdrop: None,
                round: None,
                motion: None,
                primitives,
            };
            // OneNote shrinks the footer with the page, towards the paper's bottom left.
            let footer = Layer {
                scale,
                origin: [
                    paper[0] / 2.0 * scale,
                    paper[1] - 2.0 - (1.0 - scale) * 16.0,
                ],
                clip: None,
                backdrop: None,
                round: None,
                motion: None,
                primitives: footer.next().expect("Each sheet has a footer"),
            };
            sheets.push(Sheet {
                size: paper,
                layers: vec![
                    page(rules, top + pagination.band),
                    page(primitives, bottom),
                    footer,
                ],
            });
        }
    }
    draw::pdf(&title, &sheets).map_err(PrintError::Render)
}

#[cfg(test)]
mod tests {
    use super::*;
    use onestore::{RevisionIndex, Store, document::Document};
    use std::collections::HashMap;

    fn section(path: &str) -> Vec<Page> {
        let bytes = std::fs::read(format!("{}/../../{path}", env!("CARGO_MANIFEST_DIR"))).unwrap();
        let store = Store::parse(&bytes).unwrap();
        let index = RevisionIndex::parse(&store).unwrap();
        let document = Document::parse(&index).unwrap();
        let mut spaces: Vec<_> = document
            .pages()
            .unwrap()
            .into_iter()
            .map(|(space, _)| space)
            .collect();
        spaces.dedup();
        spaces
            .into_iter()
            .map(|space| Page::from_space(&document, space).unwrap())
            .collect()
    }

    /// Each sheet's size and text, as a reader extracts it through the fonts' ToUnicode maps.
    fn sheets(pdf: &[u8]) -> Vec<([f32; 2], String)> {
        let find = |haystack: &[u8], needle: &[u8]| {
            haystack
                .windows(needle.len())
                .position(|window| window == needle)
        };
        let mut objects: HashMap<u32, (&[u8], Vec<u8>)> = HashMap::new();
        let mut rest = pdf;
        while let Some(start) = find(rest, b" 0 obj\n") {
            let number = rest[..start]
                .rsplit(|byte| !byte.is_ascii_digit())
                .next()
                .unwrap();
            let number: u32 = std::str::from_utf8(number).unwrap().parse().unwrap();
            rest = &rest[start + 7..];
            let end = find(rest, b"endobj").unwrap();
            let object = &rest[..end];
            let (dictionary, stream) = match find(object, b"stream\n") {
                Some(at) => {
                    let data = &object[at + 7..find(object, b"\nendstream").unwrap()];
                    (
                        &object[..at],
                        miniz_oxide::inflate::decompress_to_vec_zlib(data).unwrap_or_default(),
                    )
                }
                None => (object, Vec::new()),
            };
            objects.insert(number, (dictionary, stream));
            rest = &rest[end..];
        }
        let text = |bytes: &[u8]| String::from_utf8_lossy(bytes).into_owned();
        let reference = |dictionary: &[u8], key: &str| -> Option<u32> {
            let dictionary = text(dictionary);
            let at = dictionary.find(&format!("{key} "))? + key.len() + 1;
            dictionary[at..].split(' ').next()?.parse().ok()
        };
        let tree = objects
            .values()
            .find(|(dictionary, _)| text(dictionary).contains("/Type /Pages"))
            .unwrap();
        let kids = text(tree.0);
        let kids = &kids[kids.find("/Kids [").unwrap() + 7..];
        let kids: Vec<u32> = kids[..kids.find(']').unwrap()]
            .split(" 0 R")
            .filter_map(|kid| kid.trim().parse().ok())
            .collect();
        kids.into_iter()
            .map(|page| {
                let dictionary = text(objects[&page].0);
                let media: Vec<f32> = dictionary[dictionary.find("/MediaBox [").unwrap() + 11..]
                    .split(']')
                    .next()
                    .unwrap()
                    .split_whitespace()
                    .map(|value| value.parse().unwrap())
                    .collect();
                let mut maps: HashMap<String, HashMap<u16, String>> = HashMap::new();
                for (at, _) in dictionary.match_indices("/F") {
                    let mut words = dictionary[at + 1..].split_whitespace();
                    let (Some(name), Some(font)) = (
                        words.next(),
                        words.next().and_then(|id| id.parse::<u32>().ok()),
                    ) else {
                        continue;
                    };
                    let Some(unicode) = reference(objects[&font].0, "/ToUnicode") else {
                        continue;
                    };
                    let cmap = text(&objects[&unicode].1);
                    let mut map = HashMap::new();
                    for line in cmap.lines().filter(|line| line.starts_with('<')) {
                        let hex: Vec<&str> = line
                            .split(['<', '>', ' '])
                            .filter(|part| !part.is_empty())
                            .collect();
                        let units: Vec<u16> = hex[1]
                            .as_bytes()
                            .chunks(4)
                            .map(|unit| {
                                u16::from_str_radix(std::str::from_utf8(unit).unwrap(), 16).unwrap()
                            })
                            .collect();
                        map.insert(
                            u16::from_str_radix(hex[0], 16).unwrap(),
                            String::from_utf16_lossy(&units),
                        );
                    }
                    maps.insert(name.to_owned(), map);
                }
                let content = &objects[&reference(objects[&page].0, "/Contents").unwrap()].1;
                let (mut shown, mut font, mut name, mut at) =
                    (String::new(), String::new(), String::new(), 0);
                while at < content.len() {
                    let byte = content[at];
                    at += 1;
                    let mut string = Vec::new();
                    match byte {
                        b'(' => {
                            while content[at] != b')' {
                                if content[at] == b'\\' {
                                    at += 1;
                                    if content[at].is_ascii_digit() {
                                        let octal =
                                            std::str::from_utf8(&content[at..at + 3]).unwrap();
                                        string.push(u8::from_str_radix(octal, 8).unwrap());
                                        at += 3;
                                        continue;
                                    }
                                    string.push(match content[at] {
                                        b'n' => b'\n',
                                        b'r' => b'\r',
                                        b't' => b'\t',
                                        b'b' => 8,
                                        b'f' => 12,
                                        other => other,
                                    });
                                } else {
                                    string.push(content[at]);
                                }
                                at += 1;
                            }
                            at += 1;
                        }
                        b'<' if content[at] != b'<' => {
                            let end =
                                at + content[at..].iter().position(|byte| *byte == b'>').unwrap();
                            string = content[at..end]
                                .chunks(2)
                                .map(|pair| {
                                    u8::from_str_radix(std::str::from_utf8(pair).unwrap(), 16)
                                        .unwrap()
                                })
                                .collect();
                            at = end + 1;
                        }
                        b'/' => {
                            let end = at
                                + content[at..]
                                    .iter()
                                    .position(|byte| byte.is_ascii_whitespace())
                                    .unwrap();
                            name = text(&content[at..end]);
                            at = end;
                        }
                        b'T' if content.get(at) == Some(&b'f') => font = name.clone(),
                        b'T' if content.get(at) == Some(&b'm') => shown.push('\n'),
                        _ => {}
                    }
                    if let Some(map) = maps.get(&font) {
                        for cid in string.chunks(2).filter(|pair| pair.len() == 2) {
                            if let Some(text) = map.get(&u16::from_be_bytes([cid[0], cid[1]])) {
                                shown.push_str(text);
                            }
                        }
                    }
                }
                ([media[2], media[3]], shown)
            })
            .collect()
    }

    /// A ligature's glyph reads back as the letters it joins.
    #[test]
    fn glyphs_read_back_as_their_text() {
        let mut engine = TextEngine::default();
        let format = onestore::document::Format {
            font: Some("Calibri".into()),
            font_size: Some(11.0),
            ..Default::default()
        };
        let layout = engine
            .layout(&Paragraph::new("office fish ffi".into(), format), f32::MAX)
            .unwrap();
        let primitives = [Primitive::Text {
            text: &layout,
            origin: [72.0, 72.0],
            clip: None,
            ink: Paper::WHITE.ink,
        }];
        let sheet = Sheet {
            size: LETTER,
            layers: vec![Layer {
                scale: 1.0,
                origin: [0.0; 2],
                clip: None,
                backdrop: None,
                round: None,
                motion: None,
                primitives: &primitives,
            }],
        };
        let pdf = draw::pdf("", &[sheet]).unwrap();
        assert_eq!(sheets(&pdf)[0].1.trim(), "office fish ffi");
    }

    #[test]
    fn sheets_break_between_lines() {
        // Lines 20 points tall from 90 down; a Letter sheet holds 720 points of page.
        let rows: Vec<[f32; 2]> = (0..60)
            .map(|line| [90.0 + line as f32 * 20.0, 110.0 + line as f32 * 20.0])
            .collect();
        let pagination = Pagination::new([36.0, 14.4, 400.0, 1290.0], &rows, [36.0, 14.4], LETTER);
        assert_eq!(pagination.scale, 1.0);
        // 14.4 + 720 cuts the line from 730 to 750, which starts the next sheet.
        assert_eq!(pagination.tops, [14.4, 730.0]);
        let sheets: Vec<_> = pagination.sheets().collect();
        assert_eq!(sheets, [[14.4, 730.0], [730.0, 1450.0]]);
    }

    #[test]
    fn a_line_taller_than_a_sheet_is_cut() {
        let pagination = Pagination::new(
            [36.0, 14.4, 400.0, 1000.0],
            &[[20.0, 1000.0]],
            [36.0, 14.4],
            LETTER,
        );
        assert_eq!(pagination.tops, [14.4, 734.4]);
    }

    /// OneNote's scales for pages whose content ends 866.7, 581.8 and 1221.4 points across,
    /// on its 612.36-point Letter sheet (`corpus/print`).
    #[test]
    fn wide_pages_shrink_to_the_paper_as_onenote_shrinks_them() {
        for (right, onenote) in [
            (866.7, 0.67167),
            (581.8, 0.978),
            (1221.4, 0.48361),
            (400.0, 1.0),
        ] {
            let scale = Pagination::new(
                [36.0, 14.4, right, 100.0],
                &[],
                [36.0, 14.4],
                [612.36, 790.92],
            )
            .scale;
            assert!(
                (scale - onenote).abs() < 0.002,
                "{right}: {scale} for {onenote}"
            );
        }
    }

    /// `corpus/print`: the section OneNote 2010 saved as `native/section.pdf`, on the
    /// same sheets with the same text.
    #[test]
    fn a_section_prints_on_onenotes_sheets() {
        let pdf = pdf(
            section("corpus/print/native/Print.one"),
            &mut TextEngine::default(),
            LETTER,
            "Print",
        )
        .unwrap();
        let sheets = sheets(&pdf);
        assert_eq!(sheets.len(), 5);
        for (at, (size, text)) in sheets.iter().enumerate() {
            assert_eq!(*size, LETTER);
            assert!(text.contains(&format!("Print Page {}", at + 1)), "{text}");
        }
        let [first, second] = [&sheets[0].1, &sheets[1].1];
        assert!(first.contains("Printing test"));
        assert!(
            first.contains(
                "Line 37 of the long outline, with enough words to show wrapping where the"
            )
        );
        assert!(first.contains("outline is narrow. Searchable-37"));
        assert!(first.contains("Far right outline past the paper width"));
        assert!(!first.contains("Searchable-38"));
        assert!(second.contains("Line 38 of the long outline"));
        assert!(second.contains("Searchable-70"));
        assert!(!second.contains("Searchable-37\n"));
        assert!(sheets[2].1.contains("Short second page"));
    }

    /// `corpus/page-background`'s pages, ruled and with template art, take the twelve
    /// sheets OneNote 2010 prints them on (`corpus/print/native/rules-section.pdf`), the
    /// art reaching past the Very Large Grid page's first.
    #[test]
    fn ruled_pages_and_art_print_on_onenotes_sheets() {
        let pdf = pdf(
            section("corpus/page-background/candidate/Rules.one"),
            &mut TextEngine::default(),
            LETTER,
            "Rules",
        )
        .unwrap();
        let sheets = sheets(&pdf);
        assert_eq!(sheets.len(), 12);
        assert!(sheets[8].1.contains("VeryLargeGrid"));
        assert!(sheets[9].1.contains("Rules Page 10"));
        assert!(!sheets[9].1.contains("VeryLargeGrid"));
    }
}
