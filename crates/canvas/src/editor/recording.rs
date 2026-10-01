//! Record Audio as OneNote 2010 records (`corpus/recording`): starting splits the caret's
//! paragraph around a line saying when, in the page's `cite` style and linked to the start;
//! text written while recording links to the moment it was written; the finished file takes
//! its place after the caret's paragraph.

use super::*;
use onestore::page::{Attachment, MediaIndex};
use web_time::Instant;

/// A recording in progress.
pub(super) struct Live {
    id: [u8; 16],
    /// When the recording would have started had it never paused.
    started: Instant,
    paused: Option<Instant>,
    /// The blank paragraph before the line saying when, which the file goes before.
    place: ExGuid,
}

/// OneNote 2010's `cite` quick style: Calibri 9 in dark grey.
fn cite() -> Definition {
    Definition {
        kind: Kind::Style {
            name: Some("cite".into()),
            next: None,
        },
        format: Format {
            bold: Some(false),
            italic: Some(false),
            underline: Some(false),
            strike: Some(false),
            superscript: Some(false),
            subscript: Some(false),
            font: Some("Calibri".into()),
            font_size: Some(9.0),
            color: Some(0x595959),
            highlight: Some(0xff00_0000),
            space_before: Some(0.0),
            space_after: Some(0.0),
            line_spacing: Some(0.0),
            ..Format::default()
        },
    }
}

impl CanvasEditor {
    /// The recording that text written now links to.
    pub fn recording(&self) -> Option<[u8; 16]> {
        self.recording.as_ref().map(|live| live.id)
    }

    /// How far the recording has got, its pauses left out.
    pub fn recording_ms(&self) -> Option<u32> {
        let live = self.recording.as_ref()?;
        let now = live.paused.unwrap_or_else(Instant::now);
        Some(u32::try_from((now - live.started).as_millis()).unwrap_or(u32::MAX))
    }

    /// Pauses or resumes the recording. OneNote links nothing written while it is paused.
    pub fn pause_recording(&mut self, paused: bool) {
        let Some(live) = &mut self.recording else {
            return;
        };
        match (live.paused, paused) {
            (None, true) => live.paused = Some(Instant::now()),
            (Some(since), false) => {
                live.started += since.elapsed();
                live.paused = None;
            }
            _ => {}
        }
    }

    pub fn recording_paused(&self) -> bool {
        self.recording
            .as_ref()
            .is_some_and(|live| live.paused.is_some())
    }

    /// Starts a recording at the caret, returning its identity: the caret's paragraph splits
    /// there around a blank line, `label` (saying when, in the `cite` style, linked to the
    /// start) and the blank paragraph the caret starts. An empty first half is the blank
    /// line; a caret in the title records at the body's start.
    pub fn start_recording(
        &mut self,
        engine: &mut TextEngine,
        label: &str,
    ) -> Result<[u8; 16], EditorError> {
        if self.recording.is_some() {
            return Err(EditError::InvalidRange.into());
        }
        self.leave_title(engine)?;
        if let Some(id) = self.across(engine, |editor, engine| {
            editor.start_recording(engine, label)
        })? {
            return Ok(id);
        }
        let id = onestore::page::text::new_guid()?;
        let [anchor, focus] = self.selection().positions;
        let (start, end) = (anchor.min(focus), anchor.max(focus));
        let mut format = self.typing_format(start)?;
        link::unlinked(&mut format);
        let language = format.language;
        let head_empty = start.offset == 0;
        let parts = if head_empty { 3 } else { 4 };
        let mut edit = self.active_outline().document.replace(
            start..end,
            vec![Paragraph::new(String::new(), format); parts],
        )?;
        let style = match self.definitions.iter().find(|(_, definition)| {
            matches!(&definition.kind, Kind::Style { name: Some(name), .. } if name == "cite")
        }) {
            Some((style, _)) => *style,
            None => {
                let style = onestore::page::text::new_id()?;
                self.definitions.insert(style, cite());
                style
            }
        };
        if head_empty {
            // The caret's text, now the last part, keeps its tags and link, as Attach File's does.
            let [head, .., tail] = &mut edit.replacement[..parts] else {
                unreachable!("a split holds both halves")
            };
            tail.tags = std::mem::take(&mut head.tags);
            tail.text_mut().unwrap().tags = std::mem::take(&mut head.text_mut().unwrap().tags);
            tail.media = std::mem::take(&mut head.media);
        }
        let place = edit.replacement[parts - 3].id;
        let line = &mut edit.replacement[parts - 2];
        line.style = Some(style);
        line.lists.clear();
        line.media = MediaIndex {
            recordings: vec![id],
            time_ms: Some(0),
        };
        // The line's text reads back in its style's look.
        let look = Format {
            language,
            ..self.definitions[&style].format.clone()
        };
        line.text_mut().unwrap().text = Paragraph::new(label.to_owned(), look);
        let caret = TextPosition {
            paragraph: start.paragraph + parts - 1,
            offset: 0,
        };
        self.commit(
            engine,
            edit,
            Selection {
                positions: [caret; 2],
                affinities: [Affinity::Upstream; 2],
            },
        )?;
        self.recording = Some(Live {
            id,
            started: Instant::now(),
            paused: None,
            place,
        });
        Ok(id)
    }

    /// Stops recording: `file`, the recording made, goes where it started, linked to its
    /// start, or at the caret when that place is gone.
    pub fn finish_recording(
        &mut self,
        engine: &mut TextEngine,
        mut file: Attachment,
    ) -> Result<(), EditorError> {
        let Some(live) = self.recording.take() else {
            return Err(EditError::InvalidRange.into());
        };
        if file
            .recording
            .is_none_or(|recording| recording.id != live.id)
        {
            return Err(EditError::InvalidStructure.into());
        }
        let place = self.outlines.iter().find_map(|outline| {
            descendants(outline.document.nodes(), None).find_map(|(container, index, node)| {
                (node.id == live.place).then(|| (outline.id, container, index, node.clone()))
            })
        });
        let Some((outline, container, index, place)) = place else {
            return self.insert_attachment(engine, file);
        };
        file.layout = Default::default();
        let node = PageParagraph {
            id: onestore::page::text::new_id()?,
            content: ParagraphContent::Attachment(file),
            style: None,
            lists: Vec::new(),
            tags: Vec::new(),
            collapsed: false,
            media: MediaIndex {
                recordings: vec![live.id],
                time_ms: Some(0),
            },
            ..place
        };
        self.focus_outline(outline)?;
        let selection = self.selection();
        self.commit(
            engine,
            DocumentEdit {
                columns: BTreeMap::new(),
                container,
                range: index..index,
                replacement: vec![node],
            },
            selection,
        )
    }

    /// Links text `edit` writes while recording to the moment it is written, as OneNote
    /// links notes taken then; blank paragraphs and those already linked stay as they are.
    pub(super) fn link_notes(&self, edit: &mut DocumentEdit) -> Result<(), EditError> {
        let Some(time) = self.recording_ms().filter(|_| !self.recording_paused()) else {
            return Ok(());
        };
        let id = self.recording().expect("a recording is live");
        let nodes = &self.active_outline().document.container(edit.container)?[edit.range.clone()];
        let before: BTreeMap<ExGuid, &str> = descendants(nodes, None)
            .filter_map(|(_, _, node)| Some((node.id, node.text()?.text.text())))
            .collect();
        let mut pending: Vec<&mut PageParagraph> = edit.replacement.iter_mut().collect();
        while let Some(node) = pending.pop() {
            let written = node.text().is_some_and(|text| {
                !text.text.text().is_empty() && before.get(&node.id) != Some(&text.text.text())
            });
            if written && node.media == MediaIndex::default() {
                node.media = MediaIndex {
                    recordings: vec![id],
                    time_ms: Some(time),
                };
            }
            if let ParagraphContent::Table(table) = &mut node.content {
                pending.extend(
                    table
                        .rows
                        .iter_mut()
                        .flat_map(|row| &mut row.cells)
                        .flat_map(|cell| &mut cell.paragraphs),
                );
            }
        }
        Ok(())
    }
}

/// How far before a linked note's moment it plays from: OneNote 2010's default (Options,
/// Audio & Video, `corpus/recording/native/read/onenote-audio-video-options.png`).
const REWIND_MS: u32 = 5_000;

/// A play button's side, and how far its right edge stands before the text it plays from,
/// in points, as OneNote 2010 draws it at 100%.
const BUTTON: [f32; 2] = [10.5, 24.0];

impl CanvasEditor {
    /// The recording `id` names, in an outline or on the page.
    pub(crate) fn recording_file(&self, id: [u8; 16]) -> Option<&Attachment> {
        let named = |file: &&Attachment| file.recording.is_some_and(|r| r.id == id);
        self.objects
            .iter()
            .filter_map(|object| object.file().map(|(file, _)| file))
            .find(named)
            .or_else(|| {
                self.visible_outlines()
                    .flat_map(|outline| descendants(outline.document.nodes(), None))
                    .filter_map(|(_, _, node)| match &node.content {
                        ParagraphContent::Attachment(file) => Some(file),
                        _ => None,
                    })
                    .find(named)
            })
    }

    /// The note See Playback highlights `at_ms` into recording `id`: the text last linked at
    /// or before then. The line saying when recording started, linked at its start, is none.
    pub fn played_note(&self, id: [u8; 16], at_ms: u32) -> Option<ExGuid> {
        self.visible_outlines()
            .flat_map(|outline| descendants(outline.document.nodes(), None))
            .filter(|(_, _, node)| {
                node.text().is_some() && node.media.recordings.first() == Some(&id)
            })
            .filter_map(|(_, _, node)| Some((node.media.time_ms?, node.id)))
            .filter(|(time, _)| (1..=at_ms).contains(time))
            .max_by_key(|(time, _)| *time)
            .map(|(_, id)| id)
    }

    /// The play button OneNote shows for document point `point`: beside the linked note or
    /// the recording level with it, over its outline or in the margin before it. Gives the
    /// button's square, the recording and the moment it plays from.
    pub(crate) fn play_button(&self, point: [f32; 2]) -> Option<([f32; 4], [u8; 16], u32)> {
        let [x, y] = point;
        let outline = self.visible_outlines().find(|outline| {
            let bounds = outline.bounds();
            (bounds.x0 as f32 - BUTTON[1] - BUTTON[0]..=bounds.x1 as f32).contains(&x)
                && (bounds.y0 as f32..=bounds.y1 as f32).contains(&y)
        })?;
        let [left, top] = outline.origin();
        let square = |text_left: f32, line_top: f32, height: f32| {
            let x1 = left + text_left - BUTTON[1];
            let y0 = top + line_top + (height - BUTTON[0]) / 2.0;
            [x1 - BUTTON[0], y0, x1, y0 + BUTTON[0]]
        };
        let shaped = outline.shaped();
        let (rect, recording, time) = if let Some(paragraph) = shaped
            .paragraphs
            .iter()
            .find(|p| (top + p.origin[1]..=top + p.origin[1] + p.text.height()).contains(&y))
        {
            let (_, _, node) = descendants(outline.document.nodes(), None)
                .find(|(_, _, node)| node.id == paragraph.id)?;
            let line = paragraph
                .text
                .lines()
                .next()
                .map_or(paragraph.text.height(), |(_, line)| line.height);
            (
                square(paragraph.origin[0], paragraph.origin[1], line),
                *node.media.recordings.first()?,
                node.media.time_ms?.saturating_sub(REWIND_MS),
            )
        } else {
            let object = shaped.objects.iter().find(|object| {
                matches!(object.kind, crate::outline::ObjectKind::File(_))
                    && (top + object.rect[1]..=top + object.bottom).contains(&y)
            })?;
            let [x0, y0, _, y1] = object.rect;
            let recording = self.attachment(object.id)?.recording?.id;
            (square(x0, y0, y1 - y0), recording, 0)
        };
        self.recording_file(recording)?;
        Some((rect, recording, time))
    }
}
