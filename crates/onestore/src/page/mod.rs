//! An editable page model: outlines, paragraphs, tables and images with their stored
//! identities, plus everything else on the page retained as `Unsupported`.

use crate::{
    Error, ExGuid,
    document::{Document, Element, FieldValue, Format, Kind, Layout, Revision, Tag},
};
use std::{
    collections::{BTreeMap, BTreeSet},
    sync::Arc,
};

pub mod ink;
mod linear;
pub mod link;
pub mod math;
pub mod text;

pub use ink::{Ink, InkStroke};
pub use math::Math;

/// Picture and attachment payloads travel with the model (queued intents replay them),
/// as base64 text.
mod payload {
    use base64::Engine;
    use std::sync::Arc;

    pub fn serialize<S: serde::Serializer>(
        bytes: &Option<Arc<[u8]>>,
        serializer: S,
    ) -> Result<S::Ok, S::Error> {
        match bytes {
            Some(bytes) => {
                serializer.serialize_some(&base64::engine::general_purpose::STANDARD.encode(bytes))
            }
            None => serializer.serialize_none(),
        }
    }

    pub fn deserialize<'de, D: serde::Deserializer<'de>>(
        deserializer: D,
    ) -> Result<Option<Arc<[u8]>>, D::Error> {
        let text: Option<String> = serde::Deserialize::deserialize(deserializer)?;
        text.map(|text| {
            base64::engine::general_purpose::STANDARD
                .decode(text)
                .map(Arc::from)
                .map_err(serde::de::Error::custom)
        })
        .transpose()
    }
}
pub use text::Paragraph;

/// The page node's colour (View, Page Color) as COLORREF, absent for "No color"; not in
/// MS-ONE, observed in OneNote 2010's pages (`corpus/notebook-management/native/page-color`).
pub(crate) const PAGE_COLOR: u32 = 0x14001d2a;

/// The page node's rule lines (View, Rule Lines), as OneNote 2010 orders them; not in MS-ONE,
/// observed in its pages (`corpus/rule-lines/native`). Each direction stores a kind (1 grid,
/// 2 ruled, 3 margin line), a spacing in half inches and a colour as 0xAARRGGBB.
pub(crate) const RULE_LINES: [u32; 6] = [
    HORIZONTAL_COLOR,
    HORIZONTAL_KIND,
    HORIZONTAL_SPACING,
    VERTICAL_KIND,
    VERTICAL_SPACING,
    VERTICAL_COLOR,
];
const HORIZONTAL_KIND: u32 = 0x14001cd3;
const HORIZONTAL_SPACING: u32 = 0x14001cd4;
const HORIZONTAL_COLOR: u32 = 0x14001cd5;
const VERTICAL_KIND: u32 = 0x14001cd6;
const VERTICAL_SPACING: u32 = 0x14001cd7;
const VERTICAL_COLOR: u32 = 0x14001cd8;

/// A page's rule lines (View, Rule Lines): horizontal lines every `spacing` from the margin
/// origin down, crossed by a margin line or a grid's vertical lines. Spacings are in half
/// inches, as stored, so OneNote's presets survive exactly.
#[derive(Clone, Copy, Debug, PartialEq, serde::Serialize, serde::Deserialize)]
pub struct RuleLines {
    pub spacing: f32,
    /// COLORREF.
    pub color: u32,
    pub vertical: VerticalRule,
}

#[derive(Clone, Copy, Debug, PartialEq, serde::Serialize, serde::Deserialize)]
pub enum VerticalRule {
    /// One line just left of the margin origin, COLORREF.
    Margin(u32),
    /// Lines every `spacing` either side of the margin origin.
    Grid { spacing: f32, color: u32 },
}

impl RuleLines {
    /// The page node's properties for these lines, in `RULE_LINES` order.
    pub(crate) fn properties(&self) -> [(u32, [u8; 4]); 6] {
        let argb = |color: u32| {
            let [red, green, blue, _] = color.to_le_bytes();
            [blue, green, red, 0xff]
        };
        let (kinds, spacing, color) = match self.vertical {
            VerticalRule::Margin(color) => ([2, 3], 0.0, color),
            VerticalRule::Grid { spacing, color } => ([1, 1], spacing, color),
        };
        [
            (HORIZONTAL_COLOR, argb(self.color)),
            (HORIZONTAL_KIND, u32::to_le_bytes(kinds[0])),
            (HORIZONTAL_SPACING, self.spacing.to_le_bytes()),
            (VERTICAL_KIND, u32::to_le_bytes(kinds[1])),
            (VERTICAL_SPACING, spacing.to_le_bytes()),
            (VERTICAL_COLOR, argb(color)),
        ]
    }

    /// The lines a page node's `fields` store, if any are whole enough to draw.
    fn read(fields: &[crate::document::Field<'_>]) -> Option<Self> {
        let value = |id: u32| {
            fields
                .iter()
                .find(|field| field.id == id)
                .and_then(|field| match field.value {
                    FieldValue::Bytes(&[a, b, c, d]) => Some([a, b, c, d]),
                    _ => None,
                })
        };
        let spacing = |id| value(id).map(f32::from_le_bytes);
        let color =
            |id| value(id).map(|[blue, green, red, _]| u32::from_le_bytes([red, green, blue, 0]));
        value(HORIZONTAL_KIND)?;
        let vertical = match value(VERTICAL_KIND).map(u32::from_le_bytes) {
            Some(1) => VerticalRule::Grid {
                spacing: spacing(VERTICAL_SPACING)?,
                color: color(VERTICAL_COLOR)?,
            },
            _ => VerticalRule::Margin(color(VERTICAL_COLOR)?),
        };
        Some(Self {
            spacing: spacing(HORIZONTAL_SPACING).filter(|spacing| *spacing > 0.0)?,
            color: color(HORIZONTAL_COLOR)?,
            vertical,
        })
    }
}

/// The role of a title-outline paragraph that displays the page's creation date or time.
#[derive(Clone, Copy, Debug, PartialEq, serde::Serialize, serde::Deserialize)]
pub enum DateField {
    Date = 0,
    Time = 1,
}

#[derive(Clone, Debug, PartialEq, serde::Serialize, serde::Deserialize)]
pub struct Page {
    pub title: String,
    /// The page's notebook-management identity, which internal links name as `page-id`.
    pub identity: Option<[u8; 16]>,
    /// FILETIME ticks from the page's TopologyCreationTimeStamp.
    pub created: Option<u64>,
    pub margin_origin: [f32; 2],
    /// The page's colour (View, Page Color), COLORREF; `None` is OneNote's "No color".
    #[serde(default)]
    pub color: Option<u32>,
    #[serde(default)]
    pub rule_lines: Option<RuleLines>,
    pub objects: Vec<PageObject>,
    pub definitions: BTreeMap<ExGuid, Definition>,
}

#[derive(Clone, Debug, PartialEq, serde::Serialize, serde::Deserialize)]
pub struct Definition {
    pub kind: Kind<'static>,
    pub format: Format,
}

#[derive(Clone, Debug, PartialEq, serde::Serialize, serde::Deserialize)]
pub enum PageObject {
    Outline(Outline),
    Title(Title),
    Image(Image),
    /// A file on a blank part of the page, as OneNote places one attached or dropped there.
    Attachment(Attachment),
    Ink(Ink),
    Unsupported(Unsupported),
}

impl PageObject {
    pub fn id(&self) -> ExGuid {
        match self {
            Self::Outline(value) => value.id,
            Self::Title(value) => value.id,
            Self::Image(value) => value.id,
            Self::Attachment(value) => value.id,
            Self::Ink(value) => value.id,
            Self::Unsupported(value) => value.id,
        }
    }
    pub fn layout(&self) -> &Layout {
        match self {
            Self::Outline(value) => &value.layout,
            Self::Title(value) => &value.layout,
            Self::Image(value) => &value.layout,
            Self::Attachment(value) => &value.layout,
            Self::Ink(value) => &value.layout,
            Self::Unsupported(value) => &value.layout,
        }
    }

    pub fn layout_mut(&mut self) -> &mut Layout {
        match self {
            Self::Outline(value) => &mut value.layout,
            Self::Title(value) => &mut value.layout,
            Self::Image(value) => &mut value.layout,
            Self::Attachment(value) => &mut value.layout,
            Self::Ink(value) => &mut value.layout,
            Self::Unsupported(value) => &mut value.layout,
        }
    }
}

#[derive(Clone, Debug, PartialEq, serde::Serialize, serde::Deserialize)]
pub struct Title {
    pub id: ExGuid,
    pub date: Option<ExGuid>,
    pub layout: Layout,
    pub outlines: Vec<Outline>,
}

#[derive(Clone, Debug, PartialEq, serde::Serialize, serde::Deserialize)]
pub struct Outline {
    pub id: ExGuid,
    pub title: bool,
    pub min_width: Option<f32>,
    pub layout: Layout,
    pub indents: Vec<f32>,
    pub paragraphs: Vec<PageParagraph>,
    pub unsupported: Vec<Unsupported>,
}

#[derive(Clone, Debug, PartialEq, serde::Serialize, serde::Deserialize)]
pub struct PageParagraph {
    pub id: ExGuid,
    pub parent: Option<ExGuid>,
    pub level: u32,
    pub style: Option<ExGuid>,
    pub format: Format,
    pub content: ParagraphContent,
    pub lists: Vec<ExGuid>,
    pub tags: Vec<Tag>,
    pub media: MediaIndex,
    pub collapsed: bool,
}

#[derive(Clone, Debug, PartialEq, serde::Serialize, serde::Deserialize)]
pub enum ParagraphContent {
    Text(TextObject),
    Table(Table),
    /// A picture inside the paragraph, as OneNote inserts pictures into outlines.
    Image(Image),
    /// An embedded file, as OneNote inserts attachments into outlines.
    Attachment(Attachment),
    /// Handwriting held by the paragraph.
    Ink(Ink),
    Unsupported(Unsupported),
}

/// Payload bytes are identified by `id`, so equality and serialization leave them out.
#[derive(Clone, Debug, serde::Serialize, serde::Deserialize)]
pub struct Attachment {
    pub id: ExGuid,
    /// The name OneNote shows and saves the file under.
    pub filename: String,
    /// Where the file was inserted from, when recorded.
    pub source_path: Option<String>,
    /// The icon's size in points.
    pub size: Option<[f32; 2]>,
    /// On the page, where the file's column sits and its extent; a paragraph's file has none.
    #[serde(default)]
    pub layout: Layout,
    #[serde(with = "payload")]
    pub bytes: Option<Arc<[u8]>>,
    #[serde(with = "payload")]
    pub preview: Option<Arc<[u8]>>,
    /// Set when the file is an audio or video recording OneNote captured; annotations on
    /// the page refer to it by identity (`PageParagraph::media`).
    pub recording: Option<Recording>,
}

/// A recording's identity and OneNote's type code for it.
#[derive(Clone, Copy, Debug, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct Recording {
    pub id: [u8; 16],
    pub kind: u32,
}

/// A paragraph's link to a moment in recordings on the page: OneNote plays from
/// `time_ms` when the paragraph is chosen. Read from native pages and preserved through
/// edits; OneNote alone creates them while recording.
#[derive(Clone, Debug, Default, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct MediaIndex {
    pub recordings: Vec<[u8; 16]>,
    pub time_ms: Option<u32>,
}

impl PartialEq for Attachment {
    fn eq(&self, other: &Self) -> bool {
        self.id == other.id
            && self.filename == other.filename
            && self.source_path == other.source_path
            && self.size == other.size
            && self.layout == other.layout
            && self.recording == other.recording
    }
}

impl Attachment {
    pub(crate) fn read(
        revision: &Revision<'_>,
        id: ExGuid,
        node: &crate::document::Element<'_>,
    ) -> Result<Self, Error> {
        let invalid = |message| Error { offset: 0, message };
        let Kind::Attachment {
            container,
            preview,
            filename,
            source_path,
            recording_id,
            recording_type,
            icon_width,
            icon_height,
        } = &node.kind
        else {
            unreachable!()
        };
        let payload = |target: &Option<ExGuid>| -> Result<Option<Arc<[u8]>>, Error> {
            let Some(target) = target else {
                return Ok(None);
            };
            let data = revision
                .nodes
                .get(target)
                .ok_or_else(|| invalid("Missing canvas attachment data"))?;
            match &data.kind {
                Kind::File { payload, .. } => Ok(payload.map(Arc::from)),
                _ => Err(invalid("Canvas attachment data has the wrong type")),
            }
        };
        Ok(Self {
            id,
            filename: filename.clone().unwrap_or_default(),
            source_path: source_path.clone(),
            size: icon_width.zip(*icon_height).map(|(w, h)| [w, h]),
            layout: node.layout.clone(),
            bytes: payload(container)?,
            preview: payload(preview)?,
            recording: recording_id.map(|id| Recording {
                id,
                kind: recording_type.unwrap_or(0),
            }),
        })
    }
}

#[derive(Clone, Debug, PartialEq, serde::Serialize, serde::Deserialize)]
pub struct Table {
    pub id: ExGuid,
    pub columns: Vec<TableColumn>,
    pub rows: Vec<TableRow>,
    pub borders: Option<bool>,
    pub layout: Layout,
    pub tags: Vec<Tag>,
}

#[derive(Clone, Debug, PartialEq, serde::Serialize, serde::Deserialize)]
pub struct TableColumn {
    pub width: f32,
    pub locked: bool,
}

#[derive(Clone, Debug, PartialEq, serde::Serialize, serde::Deserialize)]
pub struct TableRow {
    pub id: ExGuid,
    pub cells: Vec<TableCell>,
}

#[derive(Clone, Debug, PartialEq, serde::Serialize, serde::Deserialize)]
pub struct TableCell {
    pub id: ExGuid,
    pub layout: Layout,
    pub indents: Vec<f32>,
    pub shading: Option<u32>,
    pub paragraphs: Vec<PageParagraph>,
    pub unsupported: Vec<Unsupported>,
}

#[derive(Clone, Debug, PartialEq, serde::Serialize, serde::Deserialize)]
pub struct TextObject {
    pub id: ExGuid,
    pub date_field: Option<DateField>,
    pub text: Paragraph,
    pub tags: Vec<Tag>,
}

/// Payload bytes are identified by `id`, so equality and serialization leave them out.
#[derive(Clone, Debug, serde::Serialize, serde::Deserialize)]
pub struct Image {
    pub id: ExGuid,
    pub layout: Layout,
    /// Displayed picture width and height in points.
    pub size: Option<[f32; 2]>,
    #[serde(with = "payload")]
    pub bytes: Option<Arc<[u8]>>,
    pub alt: Option<String>,
    pub background: bool,
}

impl PartialEq for Image {
    fn eq(&self, other: &Self) -> bool {
        self.id == other.id
            && self.layout == other.layout
            && self.size == other.size
            && self.alt == other.alt
            && self.background == other.background
    }
}

impl Image {
    pub(crate) fn read(
        revision: &Revision<'_>,
        id: ExGuid,
        node: &crate::document::Element<'_>,
    ) -> Result<Self, Error> {
        let invalid = |message| Error { offset: 0, message };
        let Kind::Image {
            container,
            alt,
            background,
            picture_width,
            picture_height,
            ..
        } = &node.kind
        else {
            unreachable!()
        };
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
        Ok(Self {
            id,
            layout: node.layout.clone(),
            size: picture_width.zip(*picture_height).map(|(w, h)| [w, h]),
            bytes,
            alt: alt.clone(),
            background: background.unwrap_or(false),
        })
    }
}

#[derive(Clone, Debug, PartialEq, serde::Serialize, serde::Deserialize)]
pub struct Unsupported {
    pub id: ExGuid,
    pub jcid: u32,
    pub layout: Layout,
}

impl Page {
    pub fn from_document(document: &Document<'_>, title: &str) -> Result<Self, Error> {
        let mut selected = None;
        for (space, id) in document.pages()? {
            let revision = document.active(space)?;
            if page_title(revision, id)
                .map(crate::edit::without_fields)
                .as_deref()
                == Some(title)
            {
                if selected.is_some() {
                    return Err(Error {
                        offset: 0,
                        message: "More than one active page has the requested title",
                    });
                }
                selected = Some((revision, id));
            }
        }
        let (revision, id) = selected.ok_or(Error {
            offset: 0,
            message: "No active page has the requested title",
        })?;
        Self::from_revision(revision, id)
    }

    /// The page's content under fresh identities, for a copy into this or another section:
    /// OneNote gives a copied page and every object on it new identities, and stored
    /// links keep naming the original. Content outside the model has no copy, so an
    /// `Unsupported` object is an error.
    pub fn copy(&self) -> Result<Self, Error> {
        self.copy_with(&mut [])
    }

    /// `copy`, renaming `ids`, identities on this page, to theirs on the copy.
    pub fn copy_with(&self, ids: &mut [ExGuid]) -> Result<Self, Error> {
        let invalid = |message| Error { offset: 0, message };
        let mut value =
            serde_json::to_value((self, &*ids)).map_err(|_| invalid("Page is not serializable"))?;
        let mut fresh: BTreeMap<String, String> = BTreeMap::new();
        fn walk(
            value: &mut serde_json::Value,
            fresh: &mut BTreeMap<String, String>,
        ) -> Result<(), Error> {
            match value {
                serde_json::Value::String(text) if text.starts_with('{') => {
                    if text.parse::<ExGuid>().is_ok() {
                        if !fresh.contains_key(text) {
                            fresh.insert(text.clone(), text::new_id()?.to_string());
                        }
                        *text = fresh[text].clone();
                    }
                }
                serde_json::Value::Array(items) => {
                    for item in items {
                        walk(item, fresh)?;
                    }
                }
                serde_json::Value::Object(fields) => {
                    if fields.contains_key("Unsupported") {
                        return Err(Error {
                            offset: 0,
                            message: "Content outside the page model cannot be copied",
                        });
                    }
                    let keys: Vec<String> = fields.keys().cloned().collect();
                    for key in keys {
                        let mut item = fields.remove(&key).unwrap();
                        walk(&mut item, fresh)?;
                        let key = match key.parse::<ExGuid>() {
                            Ok(_) => {
                                if !fresh.contains_key(&key) {
                                    fresh.insert(key.clone(), text::new_id()?.to_string());
                                }
                                fresh[&key].clone()
                            }
                            Err(_) => key,
                        };
                        fields.insert(key, item);
                    }
                }
                _ => {}
            }
            Ok(())
        }
        walk(&mut value, &mut fresh)?;
        let (mut copy, renamed): (Self, Vec<ExGuid>) =
            serde_json::from_value(value).map_err(|_| invalid("Page copy does not deserialize"))?;
        ids.copy_from_slice(&renamed);
        copy.identity = Some(crate::write::fresh_guid()?);
        // A paragraph's indent level follows its parent chain; a level beyond that comes
        // from an outline group, which the model does not hold.
        fn levels(paragraphs: &[PageParagraph]) -> bool {
            let by_id: BTreeMap<ExGuid, u32> = paragraphs.iter().map(|p| (p.id, p.level)).collect();
            paragraphs.iter().all(|paragraph| {
                let expected = match paragraph.parent {
                    Some(parent) => by_id.get(&parent).map_or(0, |level| level + 1),
                    None => 1,
                };
                paragraph.level == expected
                    && match &paragraph.content {
                        ParagraphContent::Table(table) => table
                            .rows
                            .iter()
                            .flat_map(|row| &row.cells)
                            .all(|cell| levels(&cell.paragraphs)),
                        _ => true,
                    }
            })
        }
        let expressible = copy.objects.iter().all(|object| match object {
            PageObject::Outline(outline) => levels(&outline.paragraphs),
            _ => true,
        });
        if !expressible {
            return Err(invalid("Outline groups cannot be copied"));
        }
        Ok(copy)
    }

    /// The date and time the title's date fields show, when it has both.
    pub fn date_text(&self) -> Option<[String; 2]> {
        let title = self.objects.iter().find_map(|object| match object {
            PageObject::Title(title) => Some(title),
            _ => None,
        })?;
        let outline = title
            .outlines
            .iter()
            .find(|outline| Some(outline.id) == title.date)?;
        let [first, second] = &outline.paragraphs[..] else {
            return None;
        };
        let [first, second] = [first.text()?, second.text()?];
        let text = |object: &TextObject| object.text.text().to_owned();
        // OneNote writes the date first; a field marked as the other role says otherwise.
        Some(
            if first.date_field == Some(DateField::Time)
                || second.date_field == Some(DateField::Date)
            {
                [text(second), text(first)]
            } else {
                [text(first), text(second)]
            },
        )
    }

    /// The notebook-management identity of the page in an active page revision, which
    /// internal links name.
    pub fn identity_of(revision: &Revision<'_>) -> Option<[u8; 16]> {
        revision
            .roots
            .get(&2)
            .and_then(|id| revision.nodes.get(id))
            .filter(|node| matches!(node.kind, Kind::Metadata { .. }))
            .and_then(|node| node.extra.first())
            .and_then(|fields| fields.iter().find(|field| field.id == 0x1c001c30))
            .and_then(|field| match field.value {
                FieldValue::Bytes(bytes) => bytes.try_into().ok(),
                _ => None,
            })
    }

    /// The title and outline level (1 at the top) a section's page list shows, read from
    /// the page's metadata without building its content.
    pub fn heading(revision: &Revision<'_>, id: ExGuid) -> (String, u32) {
        let level = revision
            .roots
            .get(&2)
            .and_then(|id| revision.nodes.get(id))
            .and_then(|node| match node.kind {
                Kind::Metadata { level, .. } => level,
                _ => None,
            });
        (
            crate::edit::without_fields(page_title(revision, id).unwrap_or_default()),
            level.unwrap_or(1),
        )
    }

    /// The single active page declared in one page object space.
    pub fn from_space(document: &Document<'_>, space: ExGuid) -> Result<Self, Error> {
        let pages = document.pages_in(space)?;
        let [id] = pages.as_slice() else {
            return Err(Error {
                offset: 0,
                message: "Choose an object space containing one active page",
            });
        };
        Self::from_revision(document.active(space)?, *id)
    }

    pub fn from_revision(revision: &Revision<'_>, id: ExGuid) -> Result<Self, Error> {
        let invalid = |message| Error { offset: 0, message };
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
        let metadata = revision
            .roots
            .get(&2)
            .and_then(|id| revision.nodes.get(id))
            .filter(|node| matches!(node.kind, Kind::Metadata { .. }))
            .and_then(|node| node.extra.first());
        let mut page = Self {
            title: crate::edit::without_fields(page_title(revision, id).unwrap_or_default()),
            identity: Self::identity_of(revision),
            created: metadata
                .and_then(|fields| fields.iter().find(|field| field.id == 0x18001c65))
                .map(|field| {
                    let FieldValue::Bytes(bytes) = field.value else {
                        return Err(invalid("The page creation date is damaged"));
                    };
                    Ok(u64::from_le_bytes(bytes.try_into().map_err(|_| {
                        invalid("The page creation date is damaged")
                    })?))
                })
                .transpose()?,
            // OneNote 2010 snaps to, and on the first move writes, (36, 14.4) when unset: 1.0
            // and 0.4 half inches, read as stored ones are.
            margin_origin: [
                margin_origin_x.unwrap_or(1.0_f32 * 36.0),
                margin_origin_y.unwrap_or(0.4_f32 * 36.0),
            ],
            color: root
                .extra
                .first()
                .and_then(|fields| fields.iter().find(|field| field.id == PAGE_COLOR))
                .map(|field| match field.value {
                    FieldValue::Bytes(&[red, green, blue, alpha]) => {
                        Ok(u32::from_le_bytes([red, green, blue, alpha]))
                    }
                    _ => Err(invalid("The page color is damaged")),
                })
                .transpose()?,
            rule_lines: root
                .extra
                .first()
                .and_then(|fields| RuleLines::read(fields)),
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
                        date: None,
                        layout: node.layout.clone(),
                        outlines: Vec::new(),
                    }));
                    roots.extend(node.children.iter().rev().map(|id| (*id, Some(index))));
                }
                Kind::Outline { indents } => {
                    let fields = node.extra.first().map(Vec::as_slice).unwrap_or_default();
                    if let Some(index) = title_index
                        && fields.iter().any(|field| {
                            field.id == 0x88001cb5 && matches!(field.value, FieldValue::NoData)
                        })
                    {
                        let PageObject::Title(title) = &mut page.objects[index] else {
                            unreachable!()
                        };
                        if title.date.replace(id).is_some() {
                            return Err(invalid("The page has more than one date field"));
                        }
                    }
                    let min_width = fields
                        .iter()
                        .find(|field| field.id == 0x14001cec)
                        .map(|field| {
                            let FieldValue::Bytes(bytes) = field.value else {
                                return Err(invalid("Invalid canvas minimum outline width"));
                            };
                            let width =
                                f32::from_le_bytes(bytes.try_into().map_err(|_| {
                                    invalid("Invalid canvas minimum outline width")
                                })?) * 36.0;
                            if !width.is_finite() || width <= 0.0 {
                                return Err(invalid("Invalid canvas minimum outline width"));
                            }
                            Ok(width)
                        })
                        .transpose()?;
                    let title_text = |node: &Element<'_>| {
                        node.extra.first().is_some_and(|fields| {
                            fields.iter().any(|field| {
                                field.id == 0x88001cb4 && matches!(field.value, FieldValue::NoData)
                            })
                        })
                    };
                    let mut outline = Outline {
                        id,
                        // OneNote flags the outline and its paragraph, and still shows an outline
                        // whose paragraph alone is flagged as the title.
                        title: title_text(node)
                            || node
                                .children
                                .iter()
                                .any(|id| revision.nodes.get(id).is_some_and(title_text)),
                        min_width,
                        layout: node.layout.clone(),
                        indents: indents.clone(),
                        paragraphs: Vec::new(),
                        unsupported: Vec::new(),
                    };
                    (outline.paragraphs, outline.unsupported) = read_paragraphs(
                        revision,
                        node,
                        &node.format,
                        &mut page.definitions,
                        &mut seen,
                        0,
                    )?;
                    if let Some(index) = title_index {
                        let PageObject::Title(title) = &mut page.objects[index] else {
                            unreachable!()
                        };
                        title.outlines.push(outline);
                    } else {
                        page.objects.push(PageObject::Outline(outline));
                    }
                }
                Kind::Image { .. } => {
                    page.objects
                        .push(PageObject::Image(Image::read(revision, id, node)?));
                }
                Kind::Attachment { .. } => {
                    page.objects.push(PageObject::Attachment(Attachment::read(
                        revision, id, node,
                    )?));
                }
                Kind::Ink { .. } => {
                    page.objects
                        .push(PageObject::Ink(Ink::read(revision, id, node)?));
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

/// The format `container` passes to its children, as reading its page inherits it.
fn inherited_by(
    revision: &Revision<'_>,
    parents: &BTreeMap<ExGuid, Vec<ExGuid>>,
    container: ExGuid,
) -> Result<Format, Error> {
    let invalid = |message| Error { offset: 0, message };
    let node = revision
        .nodes
        .get(&container)
        .ok_or_else(|| invalid("Missing canvas outline object"))?;
    let parent = || -> Result<ExGuid, Error> {
        match parents.get(&container).map(Vec::as_slice) {
            Some([parent]) => Ok(*parent),
            _ => Err(invalid("Page content must have one parent")),
        }
    };
    Ok(match &node.kind {
        Kind::Outline { .. } => node.format.clone(),
        Kind::OutlineGroup => node
            .format
            .inherit(&inherited_by(revision, parents, parent()?)?),
        Kind::Paragraph {
            paragraph_style, ..
        } => {
            let style = match paragraph_style {
                Some(id) => revision
                    .nodes
                    .get(id)
                    .map(|n| n.format.clone())
                    .unwrap_or_default(),
                None => Format::default(),
            };
            node.format
                .inherit(&style)
                .inherit(&inherited_by(revision, parents, parent()?)?)
        }
        Kind::Cell { .. } => {
            let row = parent()?;
            let [table] = parents.get(&row).map(Vec::as_slice).unwrap_or_default() else {
                return Err(invalid("A table row must have one table"));
            };
            let [holder] = parents.get(table).map(Vec::as_slice).unwrap_or_default() else {
                return Err(invalid("A table must have one paragraph"));
            };
            node.format.inherit(
                &revision.nodes[table]
                    .format
                    .inherit(&inherited_by(revision, parents, *holder)?),
            )
        }
        _ => Format::default(),
    })
}

/// The text of rich-text object `text` as its page model shows it, read from its ancestors
/// alone.
pub(crate) fn text_of(
    revision: &Revision<'_>,
    parents: &BTreeMap<ExGuid, Vec<ExGuid>>,
    text: ExGuid,
) -> Result<Paragraph, Error> {
    let invalid = |message| Error { offset: 0, message };
    let [paragraph] = parents.get(&text).map(Vec::as_slice).unwrap_or_default() else {
        return Err(invalid("Select text belonging to one paragraph"));
    };
    let format = inherited_by(revision, parents, *paragraph)?;
    let node = revision
        .nodes
        .get(&text)
        .ok_or_else(|| invalid("Missing text object"))?;
    let Kind::RichText {
        paragraph_style, ..
    } = &node.kind
    else {
        return Err(invalid("Select a rich-text object"));
    };
    let runs = revision.text_runs(text)?;
    Ok(if runs.is_empty() {
        let style = match paragraph_style {
            Some(id) => revision
                .nodes
                .get(id)
                .map(|n| n.format.clone())
                .unwrap_or_default(),
            None => Format::default(),
        };
        Paragraph::new(String::new(), node.format.inherit(&style).inherit(&format))
    } else {
        Paragraph::from_runs(
            runs.into_iter()
                .map(|run| (run.text.to_owned(), run.format.inherit(&format))),
        )
    })
}

fn read_paragraphs(
    revision: &Revision<'_>,
    container: &Element<'_>,
    inherited: &Format,
    definitions: &mut BTreeMap<ExGuid, Definition>,
    seen: &mut BTreeSet<ExGuid>,
    depth: usize,
) -> Result<(Vec<PageParagraph>, Vec<Unsupported>), Error> {
    let invalid = |message| Error { offset: 0, message };
    let default_format = Format::default();
    let style = |id: Option<ExGuid>| -> Result<&Format, Error> {
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

    let mut paragraphs = Vec::new();
    let mut unsupported = Vec::new();
    let mut pending: Vec<_> = container
        .children
        .iter()
        .rev()
        .map(|id| {
            (
                *id,
                None,
                u32::from(container.child_level.unwrap_or(0)),
                inherited.clone(),
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
                let mut base_style = *paragraph_style;
                let [content_id] = node.content.as_slice() else {
                    return Err(invalid("A paragraph must contain one content object"));
                };
                if !seen.insert(*content_id) {
                    return Err(invalid("Repeated canvas paragraph content"));
                }
                let content = revision
                    .nodes
                    .get(content_id)
                    .ok_or_else(|| invalid("Missing canvas paragraph content"))?;
                let media = MediaIndex {
                    recordings: content.media_ids.clone(),
                    time_ms: content.media_time_ms,
                };
                let content = if let Kind::RichText {
                    paragraph_style, ..
                } = &content.kind
                {
                    base_style = paragraph_style.or(base_style);
                    let runs = revision.text_runs(*content_id)?;
                    let text_content = if runs.is_empty() {
                        Paragraph::new(
                            String::new(),
                            content
                                .format
                                .inherit(style(*paragraph_style)?)
                                .inherit(&format),
                        )
                    } else {
                        Paragraph::from_runs(
                            runs.into_iter()
                                .map(|run| (run.text.to_owned(), run.format.inherit(&format))),
                        )
                    };
                    ParagraphContent::Text(TextObject {
                        id: *content_id,
                        date_field: content
                            .extra
                            .first()
                            .map(Vec::as_slice)
                            .unwrap_or_default()
                            .iter()
                            .filter(|field| matches!(field.value, FieldValue::NoData))
                            .filter_map(|field| match field.id {
                                0x88001cb5 => Some(DateField::Date),
                                0x88001c87 => Some(DateField::Time),
                                _ => None,
                            })
                            .try_fold(None, |previous, field| {
                                if previous.is_some() {
                                    Err(invalid("The page date field has conflicting roles"))
                                } else {
                                    Ok(Some(field))
                                }
                            })?,
                        text: text_content,
                        tags: content.tags.clone(),
                    })
                } else if matches!(content.kind, Kind::Image { .. }) {
                    ParagraphContent::Image(Image::read(revision, *content_id, content)?)
                } else if matches!(content.kind, Kind::Attachment { .. }) {
                    ParagraphContent::Attachment(Attachment::read(revision, *content_id, content)?)
                } else if matches!(content.kind, Kind::Ink { .. }) {
                    ParagraphContent::Ink(Ink::read(revision, *content_id, content)?)
                } else if matches!(content.kind, Kind::Table { .. }) {
                    ParagraphContent::Table(read_table(
                        revision,
                        *content_id,
                        &content.format.inherit(&format),
                        definitions,
                        seen,
                        depth + 1,
                    )?)
                } else {
                    ParagraphContent::Unsupported(Unsupported {
                        id: *content_id,
                        jcid: content.jcid,
                        layout: content.layout.clone(),
                    })
                };
                for (id, is_list) in lists.iter().map(|id| (id, true)).chain(
                    node.tags
                        .iter()
                        .chain(match &content {
                            ParagraphContent::Text(text) => text.tags.as_slice(),
                            ParagraphContent::Table(table) => table.tags.as_slice(),
                            ParagraphContent::Image(_)
                            | ParagraphContent::Attachment(_)
                            | ParagraphContent::Ink(_)
                            | ParagraphContent::Unsupported(_) => &[],
                        })
                        .filter_map(|tag| tag.definition.as_ref())
                        .map(|id| (id, false)),
                ) {
                    let definition = revision
                        .nodes
                        .get(id)
                        .ok_or_else(|| invalid("Missing canvas list or tag definition"))?;
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
                    definitions.insert(
                        *id,
                        Definition {
                            kind,
                            format: definition.format.clone(),
                        },
                    );
                }
                if let Some(id) = base_style {
                    let definition = revision
                        .nodes
                        .get(&id)
                        .ok_or_else(|| invalid("Missing canvas paragraph style"))?;
                    let Kind::Style { name } = &definition.kind else {
                        return Err(invalid("Canvas paragraph style has the wrong type"));
                    };
                    definitions.entry(id).or_insert_with(|| Definition {
                        kind: Kind::Style { name: name.clone() },
                        format: definition.format.clone(),
                    });
                }
                paragraphs.push(PageParagraph {
                    id,
                    parent,
                    level,
                    style: base_style,
                    format: format.clone(),
                    content,
                    lists: lists.clone(),
                    tags: node.tags.clone(),
                    media,
                    collapsed: *collapse_state == Some(1),
                });
                pending.extend(
                    node.children
                        .iter()
                        .rev()
                        .map(|child| (*child, Some(id), next_level, format.clone())),
                );
            }
            Kind::OutlineGroup => pending.extend(
                node.children
                    .iter()
                    .rev()
                    .map(|child| (*child, parent, next_level, node.format.inherit(&inherited))),
            ),
            _ => unsupported.push(Unsupported {
                id,
                jcid: node.jcid,
                layout: node.layout.clone(),
            }),
        }
    }
    Ok((paragraphs, unsupported))
}

fn read_table(
    revision: &Revision<'_>,
    id: ExGuid,
    inherited: &Format,
    definitions: &mut BTreeMap<ExGuid, Definition>,
    seen: &mut BTreeSet<ExGuid>,
    depth: usize,
) -> Result<Table, Error> {
    let invalid = |message| Error { offset: 0, message };
    if depth > 64 {
        return Err(invalid("Tables are nested too deeply"));
    }
    let node = &revision.nodes[&id];
    let Kind::Table {
        rows,
        columns,
        widths,
        locked,
        borders,
    } = &node.kind
    else {
        unreachable!()
    };
    if rows.map(|n| n as usize) != Some(node.children.len())
        || node.children.is_empty()
        || columns.map(|n| n as usize) != Some(widths.len())
        || widths.is_empty()
        || (!locked.is_empty() && locked.len() != widths.len())
        || widths.iter().any(|w| !w.is_finite() || *w < 36.0)
    {
        return Err(invalid("Table dimensions are inconsistent"));
    }
    let mut table_rows = Vec::new();
    for row_id in &node.children {
        if !seen.insert(*row_id) {
            return Err(invalid("Repeated canvas table row"));
        }
        let row = revision
            .nodes
            .get(row_id)
            .ok_or_else(|| invalid("Missing canvas table row"))?;
        if !matches!(row.kind, Kind::Row) || row.children.len() != widths.len() {
            return Err(invalid("Table row dimensions are inconsistent"));
        }
        let mut cells = Vec::new();
        for cell_id in &row.children {
            if !seen.insert(*cell_id) {
                return Err(invalid("Repeated canvas table cell"));
            }
            let cell = revision
                .nodes
                .get(cell_id)
                .ok_or_else(|| invalid("Missing canvas table cell"))?;
            if cell.children.is_empty() {
                return Err(invalid("A table cell has no paragraphs"));
            }
            let Kind::Cell { shading, indents } = &cell.kind else {
                return Err(invalid("A table row contains an invalid cell"));
            };
            let (paragraphs, unsupported) = read_paragraphs(
                revision,
                cell,
                &cell.format.inherit(inherited),
                definitions,
                seen,
                depth,
            )?;
            cells.push(TableCell {
                id: *cell_id,
                layout: cell.layout.clone(),
                indents: indents.clone(),
                shading: *shading,
                paragraphs,
                unsupported,
            });
        }
        table_rows.push(TableRow { id: *row_id, cells });
    }
    Ok(Table {
        id,
        columns: widths
            .iter()
            .enumerate()
            .map(|(index, &width)| TableColumn {
                width,
                locked: locked.get(index).copied().unwrap_or(false),
            })
            .collect(),
        rows: table_rows,
        borders: *borders,
        layout: node.layout.clone(),
        tags: node.tags.clone(),
    })
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
    pub fn text(&self) -> Option<&TextObject> {
        match &self.content {
            ParagraphContent::Text(text) => Some(text),
            _ => None,
        }
    }
    pub fn text_mut(&mut self) -> Option<&mut TextObject> {
        match &mut self.content {
            ParagraphContent::Text(text) => Some(text),
            _ => None,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::document::{Element, Field, FieldValue, TextRun};

    fn id(n: u32) -> ExGuid {
        ExGuid {
            n,
            ..ExGuid::default()
        }
    }

    const IDENTITY: [u8; 16] = [7; 16];

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
        let mut metadata = element(Kind::Metadata {
            title: Some("Page title".into()),
            level: None,
        });
        metadata.extra = vec![vec![Field {
            id: 0x1c001c30,
            value: FieldValue::Bytes(&IDENTITY),
        }]];
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

    fn table_revision() -> Revision<'static> {
        let mut source = revision();
        source.nodes.get_mut(&id(3)).unwrap().content = vec![id(7)];
        let mut table = element(Kind::Table {
            rows: Some(1),
            columns: Some(2),
            widths: vec![37.11, 99.0],
            locked: vec![false, true],
            borders: Some(false),
        });
        table.children = vec![id(8)];
        let mut row = element(Kind::Row);
        row.children = vec![id(9), id(10)];
        source.nodes.insert(id(7), table);
        source.nodes.insert(id(8), row);
        for (cell, paragraph, text) in [(9, 11, 4), (10, 12, 13)] {
            let mut node = element(Kind::Cell {
                shading: Some(0x00ffff),
                indents: vec![18.0, 0.0, 27.0, 27.0],
            });
            node.child_level = Some(1);
            node.layout.max_width = Some(268.8);
            node.children = vec![id(paragraph)];
            if cell == 10 {
                node.format.font_size = Some(13.0);
            }
            source.nodes.insert(id(cell), node);
            let mut node = element(Kind::Paragraph {
                lists: Vec::new(),
                paragraph_style: None,
                collapse_state: None,
            });
            node.content = vec![id(text)];
            source.nodes.insert(id(paragraph), node);
        }
        source.nodes.insert(
            id(13),
            element(Kind::RichText {
                text: "cd".into(),
                runs: vec![TextRun {
                    start: 0,
                    end: 2,
                    format: None,
                    extra_set: None,
                }],
                paragraph_style: None,
                boilerplate: false,
            }),
        );
        source
    }

    #[test]
    fn table_import_owns_cells_and_inherits_format_across_containers() {
        let page = {
            let source = table_revision();
            let before = serde_json::to_vec(&source).unwrap();
            let page = Page::from_revision(&source, id(1)).unwrap();
            assert_eq!(serde_json::to_vec(&source).unwrap(), before);
            page
        };
        let PageObject::Outline(outline) = &page.objects[0] else {
            panic!()
        };
        let ParagraphContent::Table(table) = &outline.paragraphs[0].content else {
            panic!()
        };
        assert_eq!(table.id, id(7));
        assert_eq!(table.borders, Some(false));
        assert_eq!(
            table.columns,
            [
                TableColumn {
                    width: 37.11,
                    locked: false
                },
                TableColumn {
                    width: 99.0,
                    locked: true
                }
            ]
        );
        assert_eq!(table.rows[0].id, id(8));
        for (index, cell) in table.rows[0].cells.iter().enumerate() {
            assert_eq!(cell.id, id(9 + index as u32));
            assert_eq!(cell.layout.max_width, Some(268.8));
            assert_eq!(cell.indents, [18.0, 0.0, 27.0, 27.0]);
            assert_eq!(cell.shading, Some(0x00ffff));
            let paragraph = &cell.paragraphs[0];
            assert_eq!(paragraph.parent, None);
            assert_eq!(paragraph.level, 1);
            assert_eq!(paragraph.id, id(11 + index as u32));
            let text = paragraph.text().unwrap();
            assert_eq!(text.id, if index == 0 { id(4) } else { id(13) });
            assert_eq!(text.text.text(), if index == 0 { "ab" } else { "cd" });
            assert_eq!(
                text.text.spans()[0].format.font_size,
                Some(if index == 0 { 12.0 } else { 13.0 })
            );
            assert_eq!(text.text.spans()[0].format.bold, Some(false));
        }
    }

    #[test]
    fn absent_column_locks_use_the_specified_unlocked_default() {
        let mut source = table_revision();
        let Kind::Table { locked, .. } = &mut source.nodes.get_mut(&id(7)).unwrap().kind else {
            panic!()
        };
        locked.clear();
        let page = Page::from_revision(&source, id(1)).unwrap();
        let PageObject::Outline(outline) = &page.objects[0] else {
            panic!()
        };
        let ParagraphContent::Table(table) = &outline.paragraphs[0].content else {
            panic!()
        };
        assert!(table.columns.iter().all(|column| !column.locked));
    }

    #[test]
    fn table_import_rejects_aliases_cycles_and_inconsistent_dimensions() {
        for case in 0..7 {
            let mut source = table_revision();
            match case {
                0 => source.nodes.get_mut(&id(8)).unwrap().children[1] = id(9),
                1 => source.nodes.get_mut(&id(11)).unwrap().content = vec![id(7)],
                2 => source.nodes.get_mut(&id(12)).unwrap().content = vec![id(4)],
                3 => source.nodes.get_mut(&id(3)).unwrap().content.push(id(4)),
                4 => source.nodes.get_mut(&id(9)).unwrap().children.clear(),
                5 => {
                    let Kind::Table { columns, .. } =
                        &mut source.nodes.get_mut(&id(7)).unwrap().kind
                    else {
                        panic!()
                    };
                    *columns = Some(3);
                }
                _ => {
                    let Kind::Table { widths, .. } =
                        &mut source.nodes.get_mut(&id(7)).unwrap().kind
                    else {
                        panic!()
                    };
                    widths[0] = f32::INFINITY;
                }
            }
            assert!(Page::from_revision(&source, id(1)).is_err(), "case {case}");
        }
    }

    #[test]
    fn nested_tables_import_with_a_bounded_depth() {
        for levels in [1, 62, 65] {
            let mut source = table_revision();
            let mut paragraph = id(11);
            for depth in 0..levels {
                let base = 20 + depth * 4;
                source.nodes.get_mut(&paragraph).unwrap().content = vec![id(base)];
                let mut table = element(Kind::Table {
                    rows: Some(1),
                    columns: Some(1),
                    widths: vec![72.0],
                    locked: vec![true],
                    borders: Some(true),
                });
                table.children = vec![id(base + 1)];
                let mut row = element(Kind::Row);
                row.children = vec![id(base + 2)];
                let mut cell = element(Kind::Cell {
                    shading: None,
                    indents: vec![18.0, 0.0, 27.0, 27.0],
                });
                cell.child_level = Some(1);
                cell.children = vec![id(base + 3)];
                let mut child = element(Kind::Paragraph {
                    lists: Vec::new(),
                    paragraph_style: None,
                    collapse_state: None,
                });
                child.content = vec![id(4)];
                source.nodes.extend([
                    (id(base), table),
                    (id(base + 1), row),
                    (id(base + 2), cell),
                    (id(base + 3), child),
                ]);
                paragraph = id(base + 3);
            }
            assert_eq!(Page::from_revision(&source, id(1)).is_ok(), levels < 64);
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
        assert_eq!(paragraph.style, Some(id(5)));
        assert!(matches!(&page.definitions[&id(5)].kind,
            Kind::Style { name: Some(name) } if name == "Body"));
        assert_eq!(page.definitions[&id(5)].format.font_size, Some(12.0));
        let text = &paragraph.text().unwrap().text;
        assert_eq!(text.text(), "ab");
        assert_eq!(text.spans()[0].format.font_size, Some(12.0));
        assert_eq!(text.spans()[0].format.bold, Some(false));
        assert_eq!(paragraph.text().unwrap().tags[0].created, Some(123));
        assert!(matches!(&page.definitions[&id(7)].kind,
            Kind::TagDefinition { label: Some(label), shape: Some(3), .. } if label == "To Do"));
    }

    #[test]
    fn creation_time_comes_from_page_metadata_and_date_role_from_the_title_child() {
        let mut source = revision();
        source.nodes.get_mut(&id(1)).unwrap().created = Some(7);
        let bytes = 134_333_468_649_123_456_u64.to_le_bytes();
        source.nodes.get_mut(&id(6)).unwrap().extra = vec![vec![
            Field {
                id: 0x18001c65,
                value: FieldValue::Bytes(&bytes),
            },
            Field {
                id: 0x1c001c30,
                value: FieldValue::Bytes(&IDENTITY),
            },
        ]];
        source.nodes.get_mut(&id(1)).unwrap().children.clear();
        source.nodes.get_mut(&id(1)).unwrap().structure.push(id(7));
        let mut title = element(Kind::Title);
        title.children.push(id(2));
        source.nodes.insert(id(7), title);
        source.nodes.get_mut(&id(2)).unwrap().extra = vec![vec![Field {
            id: 0x88001cb5,
            value: FieldValue::NoData,
        }]];
        source.nodes.get_mut(&id(4)).unwrap().extra = vec![vec![Field {
            id: 0x88001c87,
            value: FieldValue::NoData,
        }]];
        let page = Page::from_revision(&source, id(1)).unwrap();
        assert_eq!(page.created, Some(134_333_468_649_123_456));
        let PageObject::Title(title) = &page.objects[0] else {
            panic!()
        };
        assert_eq!(title.date, Some(id(2)));
        assert_eq!(
            title.outlines[0].paragraphs[0].text().unwrap().date_field,
            Some(DateField::Time)
        );
        source.nodes.get_mut(&id(6)).unwrap().extra[0][0].value = FieldValue::Bytes(&bytes[..7]);
        assert!(Page::from_revision(&source, id(1)).is_err());
        source.nodes.get_mut(&id(6)).unwrap().extra.clear();
        let bare = Page::from_revision(&source, id(1)).unwrap();
        assert_eq!((bare.created, bare.identity), (None, None));
    }

    #[test]
    fn title_role_and_minimum_width_come_from_root_properties() {
        let mut source = revision();
        source.nodes.get_mut(&id(1)).unwrap().children.clear();
        source.nodes.get_mut(&id(1)).unwrap().structure.push(id(7));
        let mut title = element(Kind::Title);
        title.children.push(id(2));
        source.nodes.insert(id(7), title);
        source.nodes.get_mut(&id(2)).unwrap().extra = vec![vec![
            Field {
                id: 0x88001cb4,
                value: FieldValue::NoData,
            },
            Field {
                id: 0x14001cec,
                value: FieldValue::Bytes(&[0, 0, 0x90, 0x40]),
            },
        ]];
        let page = Page::from_revision(&source, id(1)).unwrap();
        let PageObject::Title(title) = &page.objects[0] else {
            panic!()
        };
        assert!(title.outlines[0].title);
        assert_eq!(title.outlines[0].min_width, Some(162.0));
        source.nodes.get_mut(&id(2)).unwrap().extra[0][1].value =
            FieldValue::Bytes(&[0, 0, 0x80, 0x7f]);
        assert!(Page::from_revision(&source, id(1)).is_err());
        source.nodes.get_mut(&id(2)).unwrap().extra = vec![
            vec![Field {
                id: 0x08001cb4,
                value: FieldValue::NoData,
            }],
            vec![Field {
                id: 0x88001cb4,
                value: FieldValue::NoData,
            }],
        ];
        let page = Page::from_revision(&source, id(1)).unwrap();
        let PageObject::Title(title) = &page.objects[0] else {
            panic!()
        };
        assert!(!title.outlines[0].title);
        assert_eq!(title.outlines[0].min_width, None);
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
            reference: crate::FileDataReference::Internal([0; 16]),
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
        let mut child = element(Kind::Paragraph {
            lists: Vec::new(),
            paragraph_style: None,
            collapse_state: None,
        });
        child.content.push(id(13));
        source.nodes.insert(
            id(13),
            element(Kind::RichText {
                text: "".into(),
                runs: Vec::new(),
                paragraph_style: None,
                boilerplate: false,
            }),
        );
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
            outline.paragraphs[0].text().unwrap().text.spans()[0]
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
