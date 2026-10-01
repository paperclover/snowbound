use crate::{
    document::TextDocument,
    editor::EditorError,
    layout::TextEngine,
    outline::{OutlineLayout, TITLE_WIDTH, outline_layout},
};
use onestore::ExGuid;
use onestore::page::text::EditError;
use onestore::page::{Definition, Outline, PageParagraph};
use std::collections::BTreeMap;

pub use onestore::page::DateField;

#[derive(Clone)]
pub struct PageDate {
    timestamp: u64,
    source: Outline,
    layout: OutlineLayout,
}

impl PageDate {
    /// FILETIME ticks, in 100 ns units since 1601-01-01 UTC.
    pub fn timestamp(&self) -> u64 {
        self.timestamp
    }

    pub fn source(&self) -> &Outline {
        &self.source
    }

    pub fn layout(&self) -> &OutlineLayout {
        &self.layout
    }

    pub(crate) fn fields(&self) -> impl Iterator<Item = (DateField, &PageParagraph)> {
        let swapped = self
            .source
            .paragraphs
            .iter()
            .enumerate()
            .any(|(index, paragraph)| {
                matches!(
                    (index, paragraph.text().unwrap().date_field),
                    (0, Some(DateField::Time)) | (1, Some(DateField::Date))
                )
            });
        self.source
            .paragraphs
            .iter()
            .enumerate()
            .map(move |(index, paragraph)| {
                let field =
                    paragraph
                        .text()
                        .unwrap()
                        .date_field
                        .unwrap_or(if (index == 0) != swapped {
                            DateField::Date
                        } else {
                            DateField::Time
                        });
                (field, paragraph)
            })
    }

    pub fn new(
        timestamp: u64,
        source: Outline,
        engine: &mut TextEngine,
        definitions: &BTreeMap<ExGuid, Definition>,
    ) -> Result<Self, EditorError> {
        if source.title || source.paragraphs.is_empty() || source.paragraphs.len() > 2 {
            return Err(EditError::UnsupportedContent.into());
        }
        TextDocument::from_nodes(source.paragraphs.clone())?;
        // A date field is one plain run in a plain paragraph.
        if source.paragraphs.iter().any(|paragraph| {
            !paragraph.lists.is_empty()
                || !paragraph.tags.is_empty()
                || paragraph.collapsed
                || paragraph.parent.is_some()
                || paragraph
                    .text()
                    .is_none_or(|text| !text.tags.is_empty() || text.text.spans().len() != 1)
        }) {
            return Err(EditError::UnsupportedContent.into());
        }
        let layout = outline_layout(&source, engine, definitions, TITLE_WIDTH)?;
        let date = Self {
            timestamp,
            source,
            layout,
        };
        let mut seen = [false; 2];
        for (field, _) in date.fields() {
            if std::mem::replace(&mut seen[field as usize], true) {
                return Err(EditError::UnsupportedContent.into());
            }
        }
        Ok(date)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::editor::{CanvasEditor, TextOutline};
    use onestore::page::text::Paragraph;

    #[test]
    fn date_and_text_edits_share_history_without_changing_identities_or_selection() {
        let mut engine = TextEngine::default();
        let definitions = BTreeMap::new();
        let body = TextOutline::new(
            &mut engine,
            TextDocument::new(vec![Paragraph::new("Body".into(), Default::default())]).unwrap(),
            240.0,
            [36.0, 90.0],
        )
        .unwrap();
        let duplicate = PageDate::new(1, body.snapshot(), &mut engine, &definitions).unwrap();
        assert!(
            CanvasEditor::from_text_outlines(vec![body.clone()], BTreeMap::new(), Some(duplicate))
                .is_err()
        );
        let source = TextOutline::new(
            &mut engine,
            TextDocument::new(
                ["Monday, September 07, 2026", "6:14 AM"]
                    .map(|text| Paragraph::new(text.into(), Default::default()))
                    .into(),
            )
            .unwrap(),
            468.0,
            [0.0; 2],
        )
        .unwrap()
        .snapshot();
        let original = source.paragraphs.clone();
        let date = PageDate::new(100_123_456, source, &mut engine, &definitions).unwrap();
        let original_layout = date.layout.paragraphs[0].text.id();
        let mut editor =
            CanvasEditor::from_text_outlines(vec![body], definitions, Some(date)).unwrap();
        let body = editor.active_outline().document().clone();
        editor.compose(&mut engine, "new ".into(), 4..4).unwrap();
        let selection = editor.selection();
        let text = ["Wednesday, September 09, 2026".into(), "6:14 AM".into()];
        assert!(
            editor
                .change_date(&mut engine, 200_123_456, text.clone())
                .unwrap()
        );
        assert!(editor.marked_range().is_none());
        assert_eq!(editor.selection(), selection);
        let changed = editor.date().unwrap().source.paragraphs.clone();
        for (old, new) in original.iter().zip(&changed) {
            assert_eq!(old.id, new.id);
            assert_eq!(old.text().unwrap().id, new.text().unwrap().id);
            let mut restored = new.clone();
            restored.text_mut().unwrap().text = old.text().unwrap().text.clone();
            assert_eq!(&restored, old);
        }
        assert!(editor.undo(&mut engine).unwrap());
        assert_eq!(editor.date().unwrap().timestamp, 100_123_456);
        assert_eq!(editor.date().unwrap().source.paragraphs, original);
        assert_eq!(
            editor.date().unwrap().layout.paragraphs[0].text.id(),
            original_layout
        );
        assert_eq!(editor.selection(), selection);
        assert!(editor.undo(&mut engine).unwrap());
        assert_eq!(editor.active_outline().document(), &body);
        assert!(editor.redo(&mut engine).unwrap());
        assert!(editor.redo(&mut engine).unwrap());
        assert_eq!(editor.date().unwrap().source.paragraphs, changed);
        assert!(!editor.change_date(&mut engine, 200_123_456, text).unwrap());
        editor.undo(&mut engine).unwrap();
        editor
            .change_date(
                &mut engine,
                300_123_456,
                ["Friday".into(), "7:00 AM".into()],
            )
            .unwrap();
        assert!(!editor.redo(&mut engine).unwrap());
    }

    #[test]
    fn explicit_roles_control_reordered_and_time_only_fields() {
        let mut engine = TextEngine::default();
        let source = TextOutline::new(
            &mut engine,
            TextDocument::new(
                ["6:14 AM", "Monday"]
                    .map(|text| Paragraph::new(text.into(), Default::default()))
                    .into(),
            )
            .unwrap(),
            468.0,
            [0.0; 2],
        )
        .unwrap()
        .snapshot();
        for count in [1, 2] {
            let mut source = source.clone();
            source.paragraphs.truncate(count);
            source.paragraphs[0].text_mut().unwrap().date_field = Some(DateField::Time);
            let date = PageDate::new(1, source, &mut engine, &BTreeMap::new()).unwrap();
            assert_eq!(date.fields().next().unwrap().0, DateField::Time);
            let mut editor = CanvasEditor::new(
                &mut engine,
                TextDocument::new(vec![Paragraph::new("Body".into(), Default::default())]).unwrap(),
                200.0,
            )
            .unwrap();
            let outlines = editor.outlines().to_vec();
            editor =
                CanvasEditor::from_text_outlines(outlines, BTreeMap::new(), Some(date)).unwrap();
            editor
                .change_date(&mut engine, 2, ["New date".into(), "New time".into()])
                .unwrap();
            assert_eq!(
                editor.date().unwrap().source.paragraphs[0]
                    .text()
                    .unwrap()
                    .text
                    .text(),
                "New time"
            );
            if count == 2 {
                assert_eq!(
                    editor.date().unwrap().source.paragraphs[1]
                        .text()
                        .unwrap()
                        .text
                        .text(),
                    "New date"
                );
                let mut duplicate = editor.date().unwrap().source.clone();
                duplicate.paragraphs[1].text_mut().unwrap().date_field = Some(DateField::Time);
                assert!(PageDate::new(3, duplicate, &mut engine, &BTreeMap::new()).is_err());
            }
        }
    }
}
