use crate::{
    page::{PageParagraph, TextObject},
    text::{EditError, Paragraph, new_id},
};
use onestore::document::Format;
use std::{
    collections::{BTreeMap, BTreeSet},
    ops::Range,
};

#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub struct TextPosition {
    pub paragraph: usize,
    pub offset: u32,
}

/// Each editable paragraph owns one rich-text object; structural edits require flat content.
#[derive(Clone, Debug, PartialEq)]
pub struct TextDocument {
    nodes: Vec<PageParagraph>,
}

#[derive(Clone, Debug, PartialEq)]
pub(crate) struct DocumentEdit {
    pub range: Range<usize>,
    pub replacement: Vec<PageParagraph>,
}

fn node(text: Paragraph, format: Format) -> Result<PageParagraph, EditError> {
    Ok(PageParagraph {
        id: new_id()?,
        parent: None,
        level: 1,
        format,
        text: vec![TextObject {
            id: new_id()?,
            text,
            tags: Vec::new(),
        }],
        unsupported: Vec::new(),
        lists: Vec::new(),
        tags: Vec::new(),
        collapsed: false,
    })
}

fn validate_nodes<'a>(nodes: impl Iterator<Item = &'a PageParagraph>) -> Result<(), EditError> {
    let mut ids = BTreeSet::new();
    let mut parents = BTreeMap::new();
    for node in nodes {
        if node.text.len() != 1 || !node.unsupported.is_empty() {
            return Err(EditError::UnsupportedContent);
        }
        if !ids.insert(node.id) || !ids.insert(node.text[0].id) {
            return Err(EditError::InvalidStructure);
        }
        if node.level == 0
            || node
                .parent
                .is_some_and(|id| parents.get(&id).is_none_or(|level| *level >= node.level))
        {
            return Err(EditError::InvalidStructure);
        }
        parents.insert(node.id, node.level);
    }
    Ok(())
}

fn validate_text(nodes: &[PageParagraph]) -> Result<(), EditError> {
    for node in nodes {
        let text = &node.text[0].text;
        text.utf16_offset(text.text().len())?;
    }
    Ok(())
}

impl TextDocument {
    pub fn new(paragraphs: Vec<Paragraph>) -> Result<Self, EditError> {
        Self::from_nodes(
            paragraphs
                .into_iter()
                .map(|text| node(text, Format::default()))
                .collect::<Result<_, _>>()?,
        )
    }

    pub fn from_nodes(nodes: Vec<PageParagraph>) -> Result<Self, EditError> {
        if nodes.is_empty() {
            return Err(EditError::InvalidRange);
        }
        validate_nodes(nodes.iter())?;
        validate_text(&nodes)?;
        Ok(Self { nodes })
    }

    pub fn nodes(&self) -> &[PageParagraph] {
        &self.nodes
    }

    pub(crate) fn validate_flat(&self) -> Result<(), EditError> {
        if self.nodes.iter().any(|node| {
            !node.lists.is_empty()
                || !node.tags.is_empty()
                || node.collapsed
                || node.parent.is_some()
                || node.level != 1
                || !node.text[0].tags.is_empty()
        }) {
            return Err(EditError::UnsupportedContent);
        }
        Ok(())
    }

    pub fn paragraphs(&self) -> impl ExactSizeIterator<Item = &Paragraph> + DoubleEndedIterator {
        self.nodes.iter().map(|node| &node.text[0].text)
    }

    pub fn slice(&self, range: Range<TextPosition>) -> Result<Vec<Paragraph>, EditError> {
        if range.start > range.end {
            return Err(EditError::InvalidRange);
        }
        let nodes = self
            .nodes
            .get(range.start.paragraph..=range.end.paragraph)
            .ok_or(EditError::InvalidRange)?;
        nodes
            .iter()
            .enumerate()
            .map(|(index, node)| {
                let paragraph = &node.text[0].text;
                let start = if index == 0 { range.start.offset } else { 0 };
                let end = if index == nodes.len() - 1 {
                    range.end.offset
                } else {
                    paragraph.utf16_offset(paragraph.text().len())?
                };
                paragraph.slice(start..end)
            })
            .collect()
    }

    pub(crate) fn replace(
        &self,
        range: Range<TextPosition>,
        replacement: Vec<Paragraph>,
    ) -> Result<DocumentEdit, EditError> {
        if range.start > range.end {
            return Err(EditError::InvalidRange);
        }
        if range.start.paragraph != range.end.paragraph || replacement.len() != 1 {
            self.validate_flat()?;
        }
        let first = self
            .nodes
            .get(range.start.paragraph)
            .ok_or(EditError::InvalidRange)?;
        let last = self
            .nodes
            .get(range.end.paragraph)
            .ok_or(EditError::InvalidRange)?;
        let mut prefix = first.text[0].text.slice(0..range.start.offset)?;
        let last_text = &last.text[0].text;
        let suffix =
            last_text.slice(range.end.offset..last_text.utf16_offset(last_text.text().len())?)?;
        let mut replacement = replacement.into_iter();
        prefix.append(replacement.next().ok_or(EditError::InvalidRange)?)?;
        let mut head = first.clone();
        head.text[0].text = prefix;
        let mut nodes = vec![head];
        let following = replacement.len();
        for (index, text) in replacement.enumerate() {
            if index + 1 == following && range.start.paragraph != range.end.paragraph {
                let mut end = last.clone();
                end.text[0].text = text;
                nodes.push(end);
            } else {
                nodes.push(node(text, first.format.clone())?);
            }
        }
        if !suffix.text().is_empty() {
            let end = nodes.last_mut().unwrap();
            end.text[0].text.append(suffix)?;
        }
        Ok(DocumentEdit {
            range: range.start.paragraph..range.end.paragraph + 1,
            replacement: nodes,
        })
    }

    pub(crate) fn validate_edit(&self, edit: &DocumentEdit) -> Result<(), EditError> {
        if edit.range.start > edit.range.end
            || edit.range.end > self.nodes.len()
            || (edit.range.len() == self.nodes.len() && edit.replacement.is_empty())
        {
            return Err(EditError::InvalidRange);
        }
        validate_nodes(
            self.nodes[..edit.range.start]
                .iter()
                .chain(&edit.replacement)
                .chain(&self.nodes[edit.range.end..]),
        )?;
        validate_text(&edit.replacement)
    }

    pub(crate) fn apply(&mut self, edit: DocumentEdit) -> Result<DocumentEdit, EditError> {
        self.validate_edit(&edit)?;
        let end = edit
            .range
            .start
            .checked_add(edit.replacement.len())
            .ok_or(EditError::TextTooLong)?;
        let range = edit.range.start..end;
        let replacement = self.nodes.splice(edit.range, edit.replacement).collect();
        Ok(DocumentEdit { range, replacement })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use onestore::document::Format;

    fn position(paragraph: usize, offset: u32) -> TextPosition {
        TextPosition { paragraph, offset }
    }

    #[test]
    #[ignore = "requires CANVAS_TEST_SECTION and CANVAS_TEST_PAGE private fixture inputs"]
    fn imported_nodes_preserve_identity_through_edit_and_undo() {
        use crate::page::{Page, PageObject};
        use onestore::{RevisionIndex, Store, document::Document};
        let bytes = std::fs::read(std::env::var_os("CANVAS_TEST_SECTION").unwrap()).unwrap();
        let page = {
            let store = Store::parse(&bytes).unwrap();
            let index = RevisionIndex::parse(&store).unwrap();
            Page::from_document(
                &Document::parse(&index).unwrap(),
                &std::env::var("CANVAS_TEST_PAGE").unwrap(),
            )
            .unwrap()
        };
        drop(bytes);
        let mut outlines = 0;
        let mut paragraphs = 0;
        for outline in page.objects.iter().flat_map(|object| match object {
            PageObject::Outline(outline) => std::slice::from_ref(outline),
            PageObject::Title(title) => &title.outlines,
            _ => &[],
        }) {
            let original = TextDocument::from_nodes(outline.paragraphs.clone()).unwrap();
            for paragraph in 0..original.nodes().len() {
                let mut document = original.clone();
                let position = TextPosition {
                    paragraph,
                    offset: 0,
                };
                let format = document
                    .paragraphs()
                    .nth(paragraph)
                    .unwrap()
                    .format_at(0)
                    .unwrap()
                    .clone();
                let edit = document
                    .replace(
                        position..position,
                        vec![Paragraph::new("probe".into(), format)],
                    )
                    .unwrap();
                let undo = document.apply(edit).unwrap();
                let edited = document.clone();
                assert_eq!(edited.nodes()[paragraph].id, original.nodes()[paragraph].id);
                assert_eq!(
                    edited.nodes()[paragraph].text[0].id,
                    original.nodes()[paragraph].text[0].id
                );
                let mut restored_text = edited.nodes().to_vec();
                restored_text[paragraph].text[0].text =
                    original.nodes()[paragraph].text[0].text.clone();
                assert_eq!(restored_text, original.nodes());
                let redo = document.apply(undo).unwrap();
                assert_eq!(document, original);
                document.apply(redo).unwrap();
                assert_eq!(document, edited);
                paragraphs += 1;
            }
            assert_eq!(outline.paragraphs, original.nodes());
            outlines += 1;
        }
        assert!(outlines > 0);
        eprintln!(
            "Edited and restored {outlines} imported outlines containing {paragraphs} paragraphs"
        );
    }

    #[test]
    fn structural_edits_preserve_surviving_node_identity_and_undo_all_metadata() {
        let mut source = ["A🌲B", "", "tail"]
            .into_iter()
            .enumerate()
            .map(|(index, text)| {
                node(
                    Paragraph::new(text.into(), Format::default()),
                    Format {
                        language: Some(1033 + index as u32),
                        ..Format::default()
                    },
                )
                .unwrap()
            })
            .collect::<Vec<_>>();
        source[2].text[0].text = Paragraph::new(
            "tail".into(),
            Format {
                bold: Some(true),
                ..Format::default()
            },
        );
        let original = TextDocument::from_nodes(source).unwrap();
        let mut document = original.clone();
        let edit = document
            .replace(
                position(0, 1)..position(2, 2),
                ["x", "y", "z"]
                    .into_iter()
                    .map(|s| Paragraph::new(s.into(), Format::default()))
                    .collect(),
            )
            .unwrap();
        let undo = document.apply(edit).unwrap();
        assert_eq!(
            document
                .paragraphs()
                .map(Paragraph::text)
                .collect::<Vec<_>>(),
            ["Ax", "y", "zil"]
        );
        for index in [0, 2] {
            assert_eq!(document.nodes()[index].id, original.nodes()[index].id);
            assert_eq!(
                document.nodes()[index].text[0].id,
                original.nodes()[index].text[0].id
            );
            assert_eq!(
                document.nodes()[index].format,
                original.nodes()[index].format
            );
        }
        let old_ids: BTreeSet<_> = original
            .nodes()
            .iter()
            .flat_map(|n| [n.id, n.text[0].id])
            .collect();
        assert!(!old_ids.contains(&document.nodes()[1].id));
        assert!(!old_ids.contains(&document.nodes()[1].text[0].id));
        let edited = document.clone();
        let redo = document.apply(undo).unwrap();
        assert_eq!(document, original);
        document.apply(redo).unwrap();
        assert_eq!(document, edited);
        let mut split = original.clone();
        let edit = split
            .replace(
                position(0, 1)..position(0, 1),
                vec![Paragraph::new(String::new(), Format::default()); 2],
            )
            .unwrap();
        split.apply(edit).unwrap();
        assert_eq!(split.nodes()[0].id, original.nodes()[0].id);
        assert!(!old_ids.contains(&split.nodes()[1].id));
        assert_eq!(&split.nodes()[2..], &original.nodes()[1..]);
    }

    #[test]
    fn replacement_cannot_alias_existing_objects_or_publish_unsupported_nodes() {
        let mut document =
            TextDocument::new(vec![Paragraph::new("text".into(), Format::default())]).unwrap();
        let original = document.clone();
        let duplicate = DocumentEdit {
            range: 1..1,
            replacement: original.nodes().to_vec(),
        };
        assert_eq!(document.apply(duplicate), Err(EditError::InvalidStructure));
        let mut unsupported = original.nodes().to_vec();
        unsupported[0]
            .text
            .push(original.nodes()[0].text[0].clone());
        assert_eq!(
            TextDocument::from_nodes(unsupported.clone()),
            Err(EditError::UnsupportedContent)
        );
        assert_eq!(
            document.apply(DocumentEdit {
                range: 0..1,
                replacement: unsupported
            }),
            Err(EditError::UnsupportedContent)
        );
        assert_eq!(document, original);
    }

    #[test]
    fn paragraph_links_require_an_earlier_parent_at_a_shallower_level() {
        let mut nodes = TextDocument::new(
            ["parent", "child", "grandchild"]
                .into_iter()
                .map(|text| Paragraph::new(text.into(), Format::default()))
                .collect(),
        )
        .unwrap()
        .nodes;
        nodes[1].parent = Some(nodes[0].id);
        nodes[1].level = 3;
        nodes[2].parent = Some(nodes[1].id);
        nodes[2].level = 5;
        let original = TextDocument::from_nodes(nodes.clone()).unwrap();
        for parent in [nodes[2].id, nodes[0].text[0].id, new_id().unwrap()] {
            let mut invalid = nodes.clone();
            invalid[1].parent = Some(parent);
            assert_eq!(
                TextDocument::from_nodes(invalid),
                Err(EditError::InvalidStructure)
            );
        }
        for level in [0, 1] {
            let mut invalid = nodes.clone();
            invalid[1].level = level;
            assert_eq!(
                TextDocument::from_nodes(invalid),
                Err(EditError::InvalidStructure)
            );
        }
        let mut document = original.clone();
        assert_eq!(
            document.apply(DocumentEdit {
                range: 0..1,
                replacement: Vec::new()
            }),
            Err(EditError::InvalidStructure)
        );
        assert_eq!(document, original);
    }

    #[test]
    fn editing_nested_tagged_text_preserves_metadata_and_refuses_structural_changes() {
        let mut nodes = TextDocument::new(
            ["parent", "a🌳e\u{301}z", "child"]
                .into_iter()
                .map(|text| Paragraph::new(text.into(), Format::default()))
                .collect(),
        )
        .unwrap()
        .nodes;
        nodes[1].parent = Some(nodes[0].id);
        nodes[1].level = 2;
        nodes[1].lists.push(new_id().unwrap());
        nodes[1].tags.push(onestore::document::Tag {
            definition: Some(new_id().unwrap()),
            action_type: Some(0),
            status: 1,
            created: Some(123),
            completed: None,
            start: Some(124),
            due: Some(456),
            task_id: Some([7; 16]),
            extra_set: 0,
        });
        nodes[1].text[0].tags = nodes[1].tags.clone();
        nodes[1].collapsed = true;
        nodes[2].parent = Some(nodes[1].id);
        nodes[2].level = 3;
        let original = TextDocument::from_nodes(nodes).unwrap();
        let text = original.paragraphs().nth(1).unwrap();
        let offsets: Vec<_> = text
            .text()
            .char_indices()
            .map(|(byte, _)| text.utf16_offset(byte).unwrap())
            .chain(std::iter::once(
                text.utf16_offset(text.text().len()).unwrap(),
            ))
            .collect();
        for &start in &offsets {
            for &end in offsets.iter().filter(|&&end| end >= start) {
                let mut document = original.clone();
                let edit = document
                    .replace(
                        position(1, start)..position(1, end),
                        vec![Paragraph::new(
                            "👩🏽‍💻".into(),
                            Format {
                                bold: Some(true),
                                ..Format::default()
                            },
                        )],
                    )
                    .unwrap();
                let undo = document.apply(edit).unwrap();
                let edited = document.clone();
                let mut restored = edited.nodes().to_vec();
                restored[1].text[0].text = text.clone();
                assert_eq!(restored, original.nodes());
                let redo = document.apply(undo).unwrap();
                assert_eq!(document, original);
                document.apply(redo).unwrap();
                assert_eq!(document, edited);
            }
        }
        for (range, count) in [
            (position(1, 0)..position(1, 0), 2),
            (position(0, 6)..position(1, 0), 1),
            (position(1, 0)..position(2, 0), 1),
        ] {
            assert_eq!(
                original.replace(
                    range,
                    vec![Paragraph::new(String::new(), Format::default()); count]
                ),
                Err(EditError::UnsupportedContent)
            );
        }
    }

    #[test]
    fn split_join_and_undo_preserve_empty_paragraph_formats() {
        let regular = Format::default();
        let bold = Format {
            bold: Some(true),
            ..Format::default()
        };
        let original = TextDocument::new(vec![
            Paragraph::new("left".into(), regular.clone()),
            Paragraph::new(String::new(), bold.clone()),
            Paragraph::new("right".into(), bold.clone()),
        ])
        .unwrap();
        let mut document = original.clone();
        let split = document
            .replace(
                position(0, 2)..position(0, 2),
                vec![Paragraph::new(String::new(), regular.clone()); 2],
            )
            .unwrap();
        let undo = document.apply(split).unwrap();
        assert_eq!(
            document
                .paragraphs()
                .map(Paragraph::text)
                .collect::<Vec<_>>(),
            ["le", "ft", "", "right"]
        );
        let redo = document.apply(undo).unwrap();
        assert_eq!(document, original);
        document.apply(redo).unwrap();
        let join = document
            .replace(
                position(1, 2)..position(3, 0),
                vec![Paragraph::new(String::new(), regular)],
            )
            .unwrap();
        let before_join = document.clone();
        let undo = document.apply(join).unwrap();
        assert_eq!(document.paragraphs().nth(1).unwrap().text(), "ftright");
        assert_eq!(
            document.paragraphs().nth(1).unwrap().spans()[1].format,
            bold
        );
        document.apply(undo).unwrap();
        assert_eq!(document, before_join);
    }

    #[test]
    fn every_scalar_range_replaces_across_paragraphs_and_round_trips() {
        let regular = Format::default();
        let bold = Format {
            bold: Some(true),
            ..Format::default()
        };
        let original = TextDocument::new(vec![
            Paragraph::from_runs([
                ("a🌳".into(), regular.clone()),
                ("e\u{301}".into(), bold.clone()),
            ]),
            Paragraph::new(String::new(), bold.clone()),
            Paragraph::new("日本z".into(), regular.clone()),
        ])
        .unwrap();
        let positions: Vec<_> = original
            .paragraphs()
            .enumerate()
            .flat_map(|(index, paragraph)| {
                (0..=paragraph.text().len()).filter_map(move |byte| {
                    paragraph
                        .utf16_offset(byte)
                        .ok()
                        .map(|offset| position(index, offset))
                })
            })
            .collect();
        let replacements = [
            vec![Paragraph::new(String::new(), regular.clone())],
            vec![Paragraph::new("x👩🏽‍💻".into(), bold.clone())],
            vec![
                Paragraph::new(String::new(), bold.clone()),
                Paragraph::new(String::new(), regular.clone()),
            ],
            vec![
                Paragraph::new("one".into(), bold.clone()),
                Paragraph::new(String::new(), regular.clone()),
                Paragraph::new("two".into(), bold),
            ],
        ];
        let plain = original
            .paragraphs()
            .map(Paragraph::text)
            .collect::<Vec<_>>()
            .join("\n");
        for &start in &positions {
            for &end in positions.iter().filter(|&&end| end >= start) {
                for replacement in &replacements {
                    let mut document = original.clone();
                    let planned = document.replace(start..end, replacement.clone()).unwrap();
                    assert_eq!(document, original);
                    let undo = document.apply(planned).unwrap();
                    assert!(document.paragraphs().len() > 0);
                    let byte = |position: TextPosition| {
                        original
                            .paragraphs()
                            .take(position.paragraph)
                            .map(|p| p.text().len() + 1)
                            .sum::<usize>()
                            + original
                                .paragraphs()
                                .nth(position.paragraph)
                                .unwrap()
                                .byte_offset(position.offset)
                                .unwrap()
                    };
                    let mut expected = plain.clone();
                    expected.replace_range(
                        byte(start)..byte(end),
                        &replacement
                            .iter()
                            .map(Paragraph::text)
                            .collect::<Vec<_>>()
                            .join("\n"),
                    );
                    assert_eq!(
                        document
                            .paragraphs()
                            .map(Paragraph::text)
                            .collect::<Vec<_>>()
                            .join("\n"),
                        expected
                    );
                    let after = document.clone();
                    let redo = document.apply(undo).unwrap();
                    assert_eq!(document, original, "{start:?}..{end:?}");
                    document.apply(redo).unwrap();
                    assert_eq!(document, after);
                }
            }
        }
    }

    #[test]
    fn invalid_ranges_and_last_paragraph_removal_are_atomic() {
        let original =
            TextDocument::new(vec![Paragraph::new("🌳".into(), Format::default())]).unwrap();
        for range in [
            position(0, 1)..position(0, 2),
            position(0, 0)..position(1, 0),
            position(0, 2)..position(0, 0),
        ] {
            assert!(
                original
                    .replace(
                        range,
                        vec![Paragraph::new(String::new(), Format::default())]
                    )
                    .is_err()
            );
        }
        for range in [0..1, 0..2, Range { start: 2, end: 1 }] {
            let mut document = original.clone();
            assert!(
                document
                    .apply(DocumentEdit {
                        range,
                        replacement: Vec::new()
                    })
                    .is_err()
            );
            assert_eq!(document, original);
        }
        assert!(TextDocument::new(Vec::new()).is_err());
    }
}
