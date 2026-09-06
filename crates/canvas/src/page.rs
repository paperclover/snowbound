use crate::text::Paragraph;
use onestore::{
    ExGuid,
    document::{Document, Format, Kind, Layout, Revision, Tag},
};
use std::{
    collections::{BTreeMap, BTreeSet},
    sync::Arc,
};

pub struct Page {
    pub title: String,
    pub margin_origin: [f32; 2],
    pub objects: Vec<PageObject>,
    pub definitions: BTreeMap<ExGuid, Definition>,
}

pub struct Definition {
    pub kind: Kind<'static>,
    pub format: Format,
}

pub enum PageObject {
    Outline(Outline),
    Title(Title),
    Image(Image),
    Unsupported(Unsupported),
}

pub struct Title {
    pub id: ExGuid,
    pub layout: Layout,
    pub outlines: Vec<Outline>,
}

pub struct Outline {
    pub id: ExGuid,
    pub layout: Layout,
    pub indents: Vec<f32>,
    pub paragraphs: Vec<PageParagraph>,
    pub unsupported: Vec<Unsupported>,
}

#[derive(Clone, Debug, PartialEq)]
pub struct PageParagraph {
    pub id: ExGuid,
    pub parent: Option<ExGuid>,
    pub level: u32,
    pub format: Format,
    pub text: Vec<TextObject>,
    pub unsupported: Vec<Unsupported>,
    pub lists: Vec<ExGuid>,
    pub tags: Vec<Tag>,
    pub collapsed: bool,
}

#[derive(Clone, Debug, PartialEq)]
pub struct TextObject {
    pub id: ExGuid,
    pub text: Paragraph,
    pub tags: Vec<Tag>,
}

pub struct Image {
    pub id: ExGuid,
    pub layout: Layout,
    pub bytes: Option<Arc<[u8]>>,
    pub alt: Option<String>,
    pub background: bool,
}

#[derive(Clone, Debug, PartialEq)]
pub struct Unsupported {
    pub id: ExGuid,
    pub jcid: u32,
    pub layout: Layout,
}

impl Page {
    pub fn from_document(document: &Document<'_>, title: &str) -> Result<Self, onestore::Error> {
        let mut selected = None;
        for (space, id) in document.pages()? {
            let space = &document.spaces[&space];
            let revision = &space.revisions[&space.contexts[&ExGuid::default()]];
            if page_title(revision, id) == Some(title) {
                if selected.is_some() {
                    return Err(onestore::Error {
                        offset: 0,
                        message: "More than one active page has the requested title",
                    });
                }
                selected = Some((revision, id));
            }
        }
        let (revision, id) = selected.ok_or(onestore::Error {
            offset: 0,
            message: "No active page has the requested title",
        })?;
        Self::from_revision(revision, id)
    }

    pub fn from_revision(revision: &Revision<'_>, id: ExGuid) -> Result<Self, onestore::Error> {
        let invalid = |message| onestore::Error { offset: 0, message };
        let default_format = Format::default();
        let style = |id: Option<ExGuid>| -> Result<&Format, onestore::Error> {
            let Some(id) = id else {
                return Ok(&default_format);
            };
            let node = revision
                .nodes
                .get(&id)
                .ok_or_else(|| invalid("Missing canvas paragraph style"))?;
            if !matches!(node.kind, Kind::Style { .. }) {
                return Err(invalid("Canvas paragraph style has the wrong type"));
            }
            Ok(&node.format)
        };
        let root = revision
            .nodes
            .get(&id)
            .ok_or_else(|| invalid("Missing canvas page"))?;
        let Kind::Page {
            margin_origin_x,
            margin_origin_y,
            ..
        } = &root.kind
        else {
            return Err(invalid("Canvas root is not a page"));
        };
        let mut page = Self {
            title: page_title(revision, id).unwrap_or_default().to_owned(),
            margin_origin: [
                margin_origin_x.unwrap_or(0.0),
                margin_origin_y.unwrap_or(0.0),
            ],
            objects: Vec::new(),
            definitions: BTreeMap::new(),
        };
        let mut roots: Vec<_> = root
            .children
            .iter()
            .chain(&root.structure)
            .rev()
            .map(|id| (*id, None))
            .collect();
        let mut seen = BTreeSet::new();
        while let Some((id, title_index)) = roots.pop() {
            if !seen.insert(id) {
                return Err(invalid("Repeated canvas page object"));
            }
            let node = revision
                .nodes
                .get(&id)
                .ok_or_else(|| invalid("Missing canvas page object"))?;
            if title_index.is_some() && !matches!(node.kind, Kind::Outline { .. }) {
                return Err(invalid("Canvas title child is not an outline"));
            }
            match &node.kind {
                Kind::Title => {
                    let index = page.objects.len();
                    page.objects.push(PageObject::Title(Title {
                        id,
                        layout: node.layout.clone(),
                        outlines: Vec::new(),
                    }));
                    roots.extend(node.children.iter().rev().map(|id| (*id, Some(index))));
                }
                Kind::Outline { indents } => {
                    let mut outline = Outline {
                        id,
                        layout: node.layout.clone(),
                        indents: indents.clone(),
                        paragraphs: Vec::new(),
                        unsupported: Vec::new(),
                    };
                    let mut pending: Vec<_> = node
                        .children
                        .iter()
                        .rev()
                        .map(|id| {
                            (
                                *id,
                                None,
                                u32::from(node.child_level.unwrap_or(0)),
                                node.format.clone(),
                            )
                        })
                        .collect();
                    while let Some((id, parent, level, inherited)) = pending.pop() {
                        if !seen.insert(id) {
                            return Err(invalid("Repeated canvas outline object"));
                        }
                        let node = revision
                            .nodes
                            .get(&id)
                            .ok_or_else(|| invalid("Missing canvas outline object"))?;
                        let next_level = level
                            .checked_add(u32::from(node.child_level.unwrap_or(0)))
                            .ok_or_else(|| invalid("Canvas outline level overflow"))?;
                        match &node.kind {
                            Kind::Paragraph {
                                lists,
                                paragraph_style,
                                collapse_state,
                            } => {
                                let format = node
                                    .format
                                    .inherit(style(*paragraph_style)?)
                                    .inherit(&inherited);
                                let mut text = Vec::new();
                                let mut unsupported = Vec::new();
                                for id in &node.content {
                                    let content = revision.nodes.get(id).ok_or_else(|| {
                                        invalid("Missing canvas paragraph content")
                                    })?;
                                    if let Kind::RichText {
                                        paragraph_style, ..
                                    } = &content.kind
                                    {
                                        let runs = revision.text_runs(*id)?;
                                        let text_content = if runs.is_empty() {
                                            Paragraph::new(
                                                String::new(),
                                                content
                                                    .format
                                                    .inherit(style(*paragraph_style)?)
                                                    .inherit(&format),
                                            )
                                        } else {
                                            Paragraph::from_runs(runs.into_iter().map(|run| {
                                                (run.text.to_owned(), run.format.inherit(&format))
                                            }))
                                        };
                                        text.push(TextObject {
                                            id: *id,
                                            text: text_content,
                                            tags: content.tags.clone(),
                                        });
                                    } else {
                                        unsupported.push(Unsupported {
                                            id: *id,
                                            jcid: content.jcid,
                                            layout: content.layout.clone(),
                                        });
                                    }
                                }
                                for (id, is_list) in lists.iter().map(|id| (id, true)).chain(
                                    node.tags
                                        .iter()
                                        .chain(text.iter().flat_map(|t| &t.tags))
                                        .filter_map(|tag| tag.definition.as_ref())
                                        .map(|id| (id, false)),
                                ) {
                                    let definition = revision.nodes.get(id).ok_or_else(|| {
                                        invalid("Missing canvas list or tag definition")
                                    })?;
                                    let kind = match &definition.kind {
                                        Kind::List {
                                            font,
                                            format,
                                            restart,
                                            bullet,
                                        } if is_list => Kind::List {
                                            font: font.clone(),
                                            format: format.clone(),
                                            restart: *restart,
                                            bullet: *bullet,
                                        },
                                        Kind::TagDefinition {
                                            label,
                                            action_type,
                                            shape,
                                            color,
                                            highlight,
                                        } if !is_list => Kind::TagDefinition {
                                            label: label.clone(),
                                            action_type: *action_type,
                                            shape: *shape,
                                            color: *color,
                                            highlight: *highlight,
                                        },
                                        _ => {
                                            return Err(invalid(
                                                "Canvas list or tag definition has the wrong type",
                                            ));
                                        }
                                    };
                                    page.definitions.insert(
                                        *id,
                                        Definition {
                                            kind,
                                            format: definition.format.clone(),
                                        },
                                    );
                                }
                                outline.paragraphs.push(PageParagraph {
                                    id,
                                    parent,
                                    level,
                                    format: format.clone(),
                                    text,
                                    unsupported,
                                    lists: lists.clone(),
                                    tags: node.tags.clone(),
                                    collapsed: *collapse_state == Some(1),
                                });
                                pending.extend(
                                    node.children.iter().rev().map(|child| {
                                        (*child, Some(id), next_level, format.clone())
                                    }),
                                );
                            }
                            Kind::OutlineGroup => {
                                pending.extend(node.children.iter().rev().map(|child| {
                                    (*child, parent, next_level, node.format.inherit(&inherited))
                                }))
                            }
                            _ => outline.unsupported.push(Unsupported {
                                id,
                                jcid: node.jcid,
                                layout: node.layout.clone(),
                            }),
                        }
                    }
                    if let Some(index) = title_index {
                        let PageObject::Title(title) = &mut page.objects[index] else {
                            unreachable!()
                        };
                        title.outlines.push(outline);
                    } else {
                        page.objects.push(PageObject::Outline(outline));
                    }
                }
                Kind::Image {
                    container,
                    alt,
                    background,
                    ..
                } => {
                    let bytes = if let Some(container) = container {
                        let data = revision
                            .nodes
                            .get(container)
                            .ok_or_else(|| invalid("Missing canvas image data"))?;
                        match &data.kind {
                            Kind::File { payload, .. } => payload.map(Arc::from),
                            _ => return Err(invalid("Canvas image data has the wrong type")),
                        }
                    } else {
                        None
                    };
                    page.objects.push(PageObject::Image(Image {
                        id,
                        layout: node.layout.clone(),
                        bytes,
                        alt: alt.clone(),
                        background: background.unwrap_or(false),
                    }));
                }
                _ => page.objects.push(PageObject::Unsupported(Unsupported {
                    id,
                    jcid: node.jcid,
                    layout: node.layout.clone(),
                })),
            }
        }
        Ok(page)
    }
}

fn page_title<'a>(revision: &'a Revision<'_>, id: ExGuid) -> Option<&'a str> {
    revision
        .roots
        .get(&2)
        .and_then(|id| revision.nodes.get(id))
        .and_then(|node| {
            if let Kind::Metadata { title, .. } = &node.kind {
                title.as_deref()
            } else {
                None
            }
        })
        .or_else(|| {
            if let Kind::Page {
                alternate_title, ..
            } = &revision.nodes.get(&id)?.kind
            {
                alternate_title.as_deref()
            } else {
                None
            }
        })
}

impl PageParagraph {
    pub fn combined_text(&self) -> Paragraph {
        if self.text.is_empty() {
            return Paragraph::new(String::new(), self.format.clone());
        }
        Paragraph::from_runs(self.text.iter().flat_map(|object| {
            let mut start = 0;
            object.text.spans().iter().map(move |span| {
                let text = object.text.text()[start..span.end].to_owned();
                start = span.end;
                (text, span.format.clone())
            })
        }))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use onestore::document::{Element, TextRun};

    fn id(n: u32) -> ExGuid {
        ExGuid {
            n,
            ..ExGuid::default()
        }
    }

    fn element(kind: Kind<'_>) -> Element<'_> {
        Element {
            jcid: 0,
            children: Vec::new(),
            content: Vec::new(),
            structure: Vec::new(),
            spaces: Vec::new(),
            child_level: None,
            layout: Layout::default(),
            format: Format::default(),
            created: None,
            modified: None,
            original_author: None,
            latest_author: None,
            media_ids: Vec::new(),
            media_time_ms: None,
            tags: Vec::new(),
            kind,
            extra: Vec::new(),
        }
    }

    fn revision() -> Revision<'static> {
        let mut page = element(Kind::Page {
            alternate_title: Some("Fallback".into()),
            level: None,
            width: None,
            height: None,
            margin_origin_x: Some(36.0),
            margin_origin_y: Some(12.0),
            rtl: None,
        });
        page.children.push(id(2));
        let mut outline = element(Kind::Outline {
            indents: vec![18.0, 0.0, 36.0],
        });
        outline.children.push(id(3));
        outline.child_level = Some(1);
        outline.format.font_size = Some(9.0);
        outline.format.bold = Some(true);
        let mut paragraph = element(Kind::Paragraph {
            lists: Vec::new(),
            paragraph_style: Some(id(5)),
            collapse_state: Some(1),
        });
        paragraph.content.push(id(4));
        paragraph.format.bold = Some(false);
        let text = element(Kind::RichText {
            text: "ab".into(),
            runs: vec![TextRun {
                start: 0,
                end: 2,
                format: None,
                extra_set: None,
            }],
            paragraph_style: None,
            boilerplate: false,
        });
        let mut style = element(Kind::Style {
            name: Some("Body".into()),
        });
        style.format.font_size = Some(12.0);
        let metadata = element(Kind::Metadata {
            title: Some("Page title".into()),
            level: None,
        });
        Revision {
            roots: BTreeMap::from([(2, id(6))]),
            nodes: BTreeMap::from([
                (id(1), page),
                (id(2), outline),
                (id(3), paragraph),
                (id(4), text),
                (id(5), style),
                (id(6), metadata),
            ]),
        }
    }

    #[test]
    fn owns_page_content_and_resolves_style_before_ancestor_defaults() {
        let page = {
            let mut source = revision();
            source.nodes.get_mut(&id(4)).unwrap().tags.push(Tag {
                definition: Some(id(7)),
                action_type: None,
                status: 0,
                created: Some(123),
                completed: None,
                start: None,
                due: None,
                task_id: None,
                extra_set: 0,
            });
            source.nodes.insert(
                id(7),
                element(Kind::TagDefinition {
                    label: Some("To Do".into()),
                    action_type: Some(0),
                    shape: Some(3),
                    color: None,
                    highlight: None,
                }),
            );
            let before = serde_json::to_vec(&source).unwrap();
            let page = Page::from_revision(&source, id(1)).unwrap();
            assert_eq!(before, serde_json::to_vec(&source).unwrap());
            page
        };
        assert_eq!(page.title, "Page title");
        assert_eq!(page.margin_origin, [36.0, 12.0]);
        let PageObject::Outline(outline) = &page.objects[0] else {
            panic!()
        };
        assert_eq!(outline.paragraphs.len(), 1);
        let paragraph = &outline.paragraphs[0];
        assert!(paragraph.collapsed);
        assert_eq!(paragraph.level, 1);
        assert_eq!(paragraph.parent, None);
        let text = paragraph.combined_text();
        assert_eq!(text.text(), "ab");
        assert_eq!(text.spans()[0].format.font_size, Some(12.0));
        assert_eq!(text.spans()[0].format.bold, Some(false));
        assert_eq!(paragraph.text[0].tags[0].created, Some(123));
        assert!(matches!(&page.definitions[&id(7)].kind,
            Kind::TagDefinition { label: Some(label), shape: Some(3), .. } if label == "To Do"));
    }

    #[test]
    fn preserves_paint_order_nested_parents_and_owned_image_payloads() {
        let bytes = vec![1, 2, 3, 4];
        let mut source = revision();
        let image = element(Kind::Image {
            container: Some(id(8)),
            filename: None,
            alt: Some("Image".into()),
            picture_width: None,
            picture_height: None,
            background: Some(true),
            printout: None,
            link: None,
        });
        let file = element(Kind::File {
            reference: onestore::FileDataReference::Internal([0; 16]),
            extension: "png".into(),
            payload: Some(bytes.as_slice()),
        });
        let mut title = element(Kind::Title);
        title.layout.x = Some(12.0);
        title.layout.y = Some(24.0);
        title.children.push(id(10));
        let title_outline = element(Kind::Outline {
            indents: Vec::new(),
        });
        let mut group = element(Kind::OutlineGroup);
        group.children.push(id(12));
        group.child_level = Some(1);
        let child = element(Kind::Paragraph {
            lists: Vec::new(),
            paragraph_style: None,
            collapse_state: None,
        });
        source.nodes.extend([
            (id(7), image),
            (id(8), file),
            (id(9), title),
            (id(10), title_outline),
            (id(11), group),
            (id(12), child),
        ]);
        source
            .nodes
            .get_mut(&id(1))
            .unwrap()
            .children
            .insert(0, id(7));
        source.nodes.get_mut(&id(1)).unwrap().structure.push(id(9));
        source.nodes.get_mut(&id(3)).unwrap().children.push(id(11));
        source.nodes.get_mut(&id(3)).unwrap().child_level = Some(1);
        let page = Page::from_revision(&source, id(1)).unwrap();
        drop(source);
        drop(bytes);
        assert_eq!(page.objects.len(), 3);
        let PageObject::Image(image) = &page.objects[0] else {
            panic!()
        };
        assert_eq!(image.bytes.as_deref(), Some([1, 2, 3, 4].as_slice()));
        let PageObject::Outline(outline) = &page.objects[1] else {
            panic!()
        };
        assert_eq!(outline.paragraphs[1].id, id(12));
        assert_eq!(outline.paragraphs[1].parent, Some(id(3)));
        assert_eq!(outline.paragraphs[1].level, 3);
        let PageObject::Title(title) = &page.objects[2] else {
            panic!()
        };
        assert_eq!(title.id, id(9));
        assert_eq!([title.layout.x, title.layout.y], [Some(12.0), Some(24.0)]);
        assert_eq!(title.outlines[0].id, id(10));
        assert_eq!(title.outlines[0].layout.x, None);
    }

    #[test]
    fn rejects_title_children_that_are_not_outlines() {
        let mut source = revision();
        let mut title = element(Kind::Title);
        title.children.push(id(8));
        source.nodes.insert(id(7), title);
        source.nodes.insert(id(8), element(Kind::Title));
        source.nodes.get_mut(&id(1)).unwrap().structure.push(id(7));
        assert_eq!(
            Page::from_revision(&source, id(1)).err().unwrap().message,
            "Canvas title child is not an outline"
        );
    }

    #[test]
    fn validates_empty_text_styles_missing_references_and_cycles() {
        let mut source = revision();
        source.nodes.get_mut(&id(4)).unwrap().kind = Kind::RichText {
            text: String::new(),
            runs: Vec::new(),
            paragraph_style: Some(id(5)),
            boilerplate: false,
        };
        source.nodes.get_mut(&id(3)).unwrap().format.font_size = Some(10.0);
        let page = Page::from_revision(&source, id(1)).unwrap();
        let PageObject::Outline(outline) = &page.objects[0] else {
            panic!()
        };
        assert_eq!(
            outline.paragraphs[0].combined_text().spans()[0]
                .format
                .font_size,
            Some(12.0)
        );
        source.nodes.get_mut(&id(3)).unwrap().children.push(id(2));
        assert!(Page::from_revision(&source, id(1)).is_err());
        source.nodes.get_mut(&id(3)).unwrap().children.clear();
        source.nodes.remove(&id(5));
        assert!(Page::from_revision(&source, id(1)).is_err());
    }
}
