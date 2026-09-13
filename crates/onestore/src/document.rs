//! Referenced MS-ONE document revisions. Coordinates are points; text offsets are UTF-16 units.
//!
//! These are inspection views of one section or table-of-contents file. Mutating
//! their public fields does not edit the notebook; use the crate's commit functions.

use crate::{
    Error, ExGuid, FileDataReference, IdStream, Object, ObjectData, Property, PropertySets,
    RevisionIndex, Store, Value, bytes::Cursor,
};
use serde::{Deserialize, Serialize};
use std::collections::{BTreeMap, BTreeSet};

type Result<T> = std::result::Result<T, Error>;

fn no_ciphertext<'a>() -> &'a [u8] {
    &[]
}

fn invalid(message: &'static str) -> Error {
    Error { offset: 0, message }
}

#[derive(Debug, Serialize)]
/// Borrows source payloads; retain the input snapshot and store while using this view.
pub struct Document<'a> {
    pub file_id: [u8; 16],
    pub root: ExGuid,
    pub spaces: BTreeMap<ExGuid, Space<'a>>,
}

#[derive(Debug, Serialize)]
pub struct Space<'a> {
    pub contexts: BTreeMap<ExGuid, ExGuid>,
    pub revisions: BTreeMap<ExGuid, Revision<'a>>,
}

impl<'a> Space<'a> {
    /// The revision of the default context, which is the current content of the space.
    pub fn active(&self) -> Option<&Revision<'a>> {
        self.revisions.get(self.contexts.get(&ExGuid::default())?)
    }

    pub fn into_active(mut self) -> Option<Revision<'a>> {
        self.revisions
            .remove(self.contexts.get(&ExGuid::default())?)
    }
}

#[derive(Debug, Serialize)]
pub struct Revision<'a> {
    pub roots: BTreeMap<u32, ExGuid>,
    pub nodes: BTreeMap<ExGuid, Element<'a>>,
}

#[derive(Debug, Serialize)]
pub struct Element<'a> {
    pub jcid: u32,
    pub children: Vec<ExGuid>,
    pub content: Vec<ExGuid>,
    pub structure: Vec<ExGuid>,
    pub spaces: Vec<ExGuid>,
    /// Relative indentation level of child outline elements.
    pub child_level: Option<u8>,
    pub layout: Layout,
    pub format: Format,
    /// Time32 seconds since 1980-01-01 UTC.
    pub created: Option<u32>,
    /// Time32 seconds since 1980-01-01 UTC.
    pub modified: Option<u32>,
    pub original_author: Option<ExGuid>,
    pub latest_author: Option<ExGuid>,
    pub media_ids: Vec<[u8; 16]>,
    pub media_time_ms: Option<u32>,
    pub tags: Vec<Tag>,
    pub kind: Kind<'a>,
    /// Uninterpreted fields retain their nested-set indices and decoded reference identities.
    pub extra: Vec<Vec<Field<'a>>>,
}

#[derive(Debug, Default, Clone, PartialEq, Serialize, Deserialize)]
pub struct Layout {
    pub x: Option<f32>,
    pub y: Option<f32>,
    pub max_width: Option<f32>,
    /// An explicit user width when true; false or absent leaves an automatic layout hint.
    pub width_set_by_user: Option<bool>,
    pub max_height: Option<f32>,
    pub reserved_width: Option<f32>,
}

macro_rules! format_fields {
    ($($field:ident: $value:ty),* $(,)?) => {
        #[derive(Debug, Default, Clone, PartialEq, Serialize, Deserialize)]
        pub struct Format { $(pub $field: Option<$value>),* }

        impl Format {
            /// Explicit false values override inherited true values.
            pub fn inherit(&self, parent: &Self) -> Self {
                Self { $($field: self.$field.as_ref().or(parent.$field.as_ref()).cloned()),* }
            }
        }
    };
}

format_fields! {
    bold: bool,
    italic: bool,
    underline: bool,
    strike: bool,
    superscript: bool,
    subscript: bool,
    hidden: bool,
    hyperlink: bool,
    hyperlink_label: bool,
    math: bool,
    embedded_object: bool,
    font: String,
    font_size: f32,
    color: u32,
    highlight: u32,
    language: u32,
    alignment: u8,
    rtl: bool,
    space_before: f32,
    space_after: f32,
    line_spacing: f32,
    list_spacing: f32,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct TextRun {
    pub start: u32,
    pub end: u32,
    pub format: Option<ExGuid>,
    /// Index in the text element's extra arena for preserved TextRunData properties.
    pub extra_set: Option<usize>,
}

#[derive(Debug, Serialize)]
pub struct ResolvedTextRun<'a> {
    pub text: &'a str,
    pub format: Format,
    pub link: Option<&'a str>,
}

impl Revision<'_> {
    /// Resolves inherited formatting without discarding hidden fields or their source text.
    pub fn text_runs(&self, id: ExGuid) -> Result<Vec<ResolvedTextRun<'_>>> {
        let node = self
            .nodes
            .get(&id)
            .ok_or_else(|| invalid("Missing text object"))?;
        let Kind::RichText {
            text,
            runs,
            paragraph_style,
            ..
        } = &node.kind
        else {
            return Err(invalid("Expected a rich-text object"));
        };
        let style = |id: ExGuid| -> Result<&Format> {
            let node = self
                .nodes
                .get(&id)
                .ok_or_else(|| invalid("Missing text style"))?;
            if !matches!(node.kind, Kind::Style { .. }) {
                return Err(invalid("Text formatting does not reference a style"));
            }
            Ok(&node.format)
        };
        let default = Format::default();
        let parent = paragraph_style.map(style).transpose()?.unwrap_or(&default);
        let base = node.format.inherit(parent);
        let mut characters = text.char_indices();
        let (mut units, mut byte) = (0, 0);
        let mut target = None;
        let mut resolved = Vec::with_capacity(runs.len());
        for run in runs {
            if run.start != units || run.end < units {
                return Err(invalid("Text runs do not partition the text"));
            }
            let start = byte;
            while units < run.end {
                let (offset, character) = characters
                    .next()
                    .ok_or_else(|| invalid("Text run exceeds text"))?;
                units = units
                    .checked_add(character.len_utf16() as u32)
                    .ok_or_else(|| invalid("Text exceeds UTF-16 offset range"))?;
                byte = offset + character.len_utf8();
            }
            if units != run.end {
                return Err(invalid("Text-run boundary splits a surrogate pair"));
            }
            let fragment = &text[start..byte];
            let format = run
                .format
                .map(style)
                .transpose()?
                .unwrap_or(&default)
                .inherit(&base);
            let mut link = None;
            if format.hyperlink != Some(true) {
                target = None;
            } else if format.hidden == Some(true) {
                // OneNote's native field encoding is retained even when its syntax is unrecognized.
                target = fragment
                    .strip_prefix("\u{fddf}HYPERLINK \"")
                    .and_then(|s| s.strip_suffix('"'));
            } else if format.hyperlink_label == Some(true) {
                link = target;
            } else {
                link = Some(fragment);
            }
            resolved.push(ResolvedTextRun {
                text: fragment,
                format,
                link,
            });
        }
        if byte != text.len() {
            return Err(invalid("Text runs do not cover the text"));
        }
        Ok(resolved)
    }
}

impl Revision<'_> {
    /// Maps every element reachable from `roots` through children, content and structure
    /// references to its referencing parents, in visiting order.
    pub fn parents(&self, roots: &[ExGuid]) -> Result<BTreeMap<ExGuid, Vec<ExGuid>>> {
        let mut pending: Vec<_> = roots.to_vec();
        let mut seen = BTreeSet::new();
        let mut parents = BTreeMap::<_, Vec<_>>::new();
        while let Some(id) = pending.pop() {
            if !seen.insert(id) {
                continue;
            }
            let element = self
                .nodes
                .get(&id)
                .ok_or_else(|| invalid("Page content is unavailable"))?;
            for child in element
                .children
                .iter()
                .chain(&element.content)
                .chain(&element.structure)
            {
                parents.entry(*child).or_default().push(id);
                pending.push(*child);
            }
        }
        Ok(parents)
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
/// Tag dates count seconds since 1980-01-01 UTC; status retains the ActionItemStatus bits.
pub struct Tag {
    pub definition: Option<ExGuid>,
    pub action_type: Option<u16>,
    pub status: u16,
    pub created: Option<u32>,
    pub completed: Option<u32>,
    pub start: Option<u32>,
    pub due: Option<u32>,
    pub task_id: Option<[u8; 16]>,
    /// Index in the element's extra-property arena for uninterpreted tag fields.
    pub extra_set: usize,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "type")]
pub enum Kind<'a> {
    Section {
        default_template: Option<ExGuid>,
    },
    PageSeries,
    Manifest {
        history: Option<ExGuid>,
    },
    VersionHistory,
    VersionProxy {
        context: ExGuid,
        modified_filetime: Option<u64>,
    },
    VersionHistoryMetadata,
    /// Native default-page template metadata, outside the published MS-ONE object list.
    TemplateMetadata {
        name: Option<String>,
    },
    Page {
        alternate_title: Option<String>,
        level: Option<u32>,
        width: Option<f32>,
        height: Option<f32>,
        margin_origin_x: Option<f32>,
        margin_origin_y: Option<f32>,
        rtl: Option<bool>,
    },
    Metadata {
        title: Option<String>,
        level: Option<u32>,
    },
    ConflictMetadata {
        title: Option<String>,
        level: Option<u32>,
        author: Option<String>,
    },
    RevisionMetadata {
        modified_filetime: Option<u64>,
    },
    SectionMetadata {
        name: Option<String>,
        color: Option<u32>,
    },
    Title,
    Outline {
        indents: Vec<f32>,
    },
    Paragraph {
        lists: Vec<ExGuid>,
        paragraph_style: Option<ExGuid>,
        /// Saved state: 0 expanded, 1 collapsed, otherwise opaque; native UI overrides can stay cache-local.
        collapse_state: Option<u8>,
    },
    OutlineGroup,
    /// An ink drawing or handwriting container: strokes hang off `data`, nested containers off
    /// the element's content list. Scaling multiplies stroke coordinates.
    Ink {
        data: Option<ExGuid>,
        scale_x: Option<f32>,
        scale_y: Option<f32>,
    },
    InkData {
        strokes: Vec<ExGuid>,
        bounds: Option<[i32; 4]>,
    },
    /// One stroke: `path` holds ISF multi-byte first differences, one block per dimension of
    /// the style's dimension table, in HIMETRIC (1/2540 inch) page coordinates.
    InkStroke {
        path: Vec<u8>,
        style: Option<ExGuid>,
        bias: Option<u8>,
        language: Option<u16>,
        identity: Option<[u8; 16]>,
        index: Option<u32>,
    },
    /// Drawing attributes shared by strokes; `dimensions` holds 32-byte entries of GUID, lower
    /// and upper limit, and resolution.
    InkStyle {
        dimensions: Vec<u8>,
        width: Option<f32>,
        height: Option<f32>,
        color: Option<u32>,
        transparency: Option<u8>,
        pen_tip: Option<u8>,
        raster_operation: Option<u8>,
        antialiased: Option<bool>,
        fit_to_curve: Option<bool>,
        ignore_pressure: Option<bool>,
    },
    RichText {
        text: String,
        runs: Vec<TextRun>,
        paragraph_style: Option<ExGuid>,
        boilerplate: bool,
    },
    Style {
        name: Option<String>,
    },
    List {
        font: Option<String>,
        format: Option<String>,
        restart: Option<u32>,
        bullet: Option<u16>,
    },
    /// Column arrays and row children use visual left-to-right order, including on RTL pages.
    Table {
        rows: Option<u32>,
        columns: Option<u32>,
        /// Stored widths in points; unlocked columns may fit their content in native layout.
        widths: Vec<f32>,
        locked: Vec<bool>,
        borders: Option<bool>,
    },
    Row,
    Cell {
        shading: Option<u32>,
        indents: Vec<f32>,
    },
    Image {
        container: Option<ExGuid>,
        filename: Option<String>,
        alt: Option<String>,
        picture_width: Option<f32>,
        picture_height: Option<f32>,
        background: Option<bool>,
        printout: Option<bool>,
        link: Option<String>,
    },
    Attachment {
        container: Option<ExGuid>,
        preview: Option<ExGuid>,
        filename: Option<String>,
        source_path: Option<String>,
        recording_id: Option<[u8; 16]>,
        recording_type: Option<u32>,
        /// Displayed icon width and height in points.
        icon_width: Option<f32>,
        icon_height: Option<f32>,
    },
    File {
        reference: FileDataReference,
        extension: String,
        #[serde(skip)]
        payload: Option<&'a [u8]>,
    },
    Author {
        name: Option<String>,
    },
    TagDefinition {
        label: Option<String>,
        action_type: Option<u16>,
        shape: Option<u16>,
        color: Option<u32>,
        highlight: Option<u32>,
    },
    Toc {
        entries: Vec<ExGuid>,
        filename: Option<String>,
        identity: Option<[u8; 16]>,
        order: Option<u32>,
        color: Option<u32>,
    },
    Encrypted {
        #[serde(serialize_with = "hex", skip_deserializing, default = "no_ciphertext")]
        ciphertext: &'a [u8],
    },
    Unknown,
}

#[derive(Debug, PartialEq, Serialize)]
pub struct Field<'a> {
    pub id: u32,
    pub value: FieldValue<'a>,
}

#[derive(Debug, PartialEq, Serialize)]
pub enum FieldValue<'a> {
    NoData,
    Bytes(#[serde(serialize_with = "hex")] &'a [u8]),
    Objects(Vec<ExGuid>),
    Spaces(Vec<ExGuid>),
    Contexts(Vec<ExGuid>),
    Sets(std::ops::Range<usize>),
}

fn hex<S: serde::Serializer>(bytes: &&[u8], serializer: S) -> std::result::Result<S::Ok, S::Error> {
    use std::fmt::Write;
    let mut text = String::with_capacity(bytes.len() * 2);
    for byte in *bytes {
        write!(text, "{byte:02x}").unwrap();
    }
    serializer.serialize_str(&text)
}

fn unicode(bytes: &[u8]) -> Result<String> {
    if !bytes.len().is_multiple_of(2) {
        return Err(invalid("Odd UTF-16 document string length"));
    }
    let units: Vec<_> = bytes
        .chunks_exact(2)
        .map(|x| u16::from_le_bytes(x.try_into().unwrap()))
        .collect();
    String::from_utf16(units.strip_suffix(&[0]).unwrap_or(&units))
        .map_err(|_| invalid("Invalid UTF-16 document string"))
}

struct Fields<'a, 'o> {
    sets: Vec<Vec<Property<'a>>>,
    ids: &'o BTreeMap<u32, [u8; 16]>,
}

impl<'a, 'o> Fields<'a, 'o> {
    fn new(object: &'o Object<'a>) -> Result<Self> {
        let ObjectData::Properties(bytes) = object.data else {
            return Err(invalid("Expected document properties"));
        };
        let sets = PropertySets::parse(bytes)?.sets;
        for set in &sets {
            let mut seen = BTreeSet::new();
            if set.iter().any(|p| !seen.insert(p.id & 0x7fffffff)) {
                return Err(invalid("Repeated document property"));
            }
        }
        Ok(Self {
            sets,
            ids: &object.global_ids,
        })
    }

    fn take(&mut self, id: u32) -> Option<Property<'a>> {
        let index = self.sets[0].iter().position(|p| p.id & 0x7fffffff == id)?;
        Some(self.sets[0].remove(index))
    }

    fn bytes(&mut self, id: u32) -> Result<Option<&'a [u8]>> {
        self.take(id)
            .map(|p| match p.value {
                Value::Bytes(bytes) => Ok(bytes),
                _ => Err(invalid("Document property requires bytes")),
            })
            .transpose()
    }

    fn fixed<const N: usize>(&mut self, id: u32) -> Result<Option<[u8; N]>> {
        self.bytes(id)?
            .map(|b| {
                b.try_into()
                    .map_err(|_| invalid("Document scalar has an invalid length"))
            })
            .transpose()
    }

    fn u32(&mut self, id: u32) -> Result<Option<u32>> {
        Ok(self.fixed(id)?.map(u32::from_le_bytes))
    }
    fn u16(&mut self, id: u32) -> Result<Option<u16>> {
        Ok(self.fixed(id)?.map(u16::from_le_bytes))
    }
    fn u8(&mut self, id: u32) -> Result<Option<u8>> {
        Ok(self.fixed::<1>(id)?.map(|b| b[0]))
    }
    fn float(&mut self, id: u32, scale: f32) -> Result<Option<f32>> {
        self.fixed(id)?
            .map(|b| finite(f32::from_le_bytes(b) * scale))
            .transpose()
    }
    fn text(&mut self, id: u32) -> Result<Option<String>> {
        self.bytes(id)?.map(unicode).transpose()
    }
    fn boolean(&mut self, id: u32) -> Result<Option<bool>> {
        self.take(id)
            .map(|p| match p.value {
                Value::NoData => Ok(p.id & 0x80000000 != 0),
                _ => Err(invalid("Document flag requires a Boolean")),
            })
            .transpose()
    }
    fn refs(&mut self, id: u32, expected: IdStream) -> Result<Vec<ExGuid>> {
        self.take(id)
            .map(|p| match p.value {
                Value::References {
                    stream,
                    compact_ids,
                } if stream == expected => references(compact_ids, self.ids),
                _ => Err(invalid("Document reference uses the wrong stream")),
            })
            .transpose()
            .map(Option::unwrap_or_default)
    }
    fn one(&mut self, id: u32) -> Result<Option<ExGuid>> {
        let refs = self.refs(id, IdStream::Objects)?;
        if refs.len() > 1 {
            return Err(invalid("Document scalar has multiple references"));
        }
        Ok(refs.first().copied())
    }
    fn remainder(self) -> Result<Vec<Vec<Field<'a>>>> {
        self.sets
            .into_iter()
            .map(|set| {
                set.into_iter()
                    .map(|p| {
                        let value = match p.value {
                            Value::NoData => FieldValue::NoData,
                            Value::Bytes(b) => FieldValue::Bytes(b),
                            Value::Sets(r) => FieldValue::Sets(r),
                            Value::References {
                                stream,
                                compact_ids,
                            } => {
                                let refs = references(compact_ids, self.ids)?;
                                match stream {
                                    IdStream::Objects => FieldValue::Objects(refs),
                                    IdStream::ObjectSpaces => FieldValue::Spaces(refs),
                                    IdStream::Contexts => FieldValue::Contexts(refs),
                                }
                            }
                        };
                        Ok(Field { id: p.id, value })
                    })
                    .collect()
            })
            .collect()
    }
}

fn references(bytes: &[u8], ids: &BTreeMap<u32, [u8; 16]>) -> Result<Vec<ExGuid>> {
    let mut cursor = Cursor { bytes, offset: 0 };
    let mut result = Vec::new();
    while !cursor.bytes.is_empty() {
        result.push(cursor.compact(ids)?);
    }
    Ok(result)
}

fn finite(value: f32) -> Result<f32> {
    if value.is_finite() {
        Ok(value)
    } else {
        Err(invalid("Non-finite document measurement"))
    }
}

fn measurements(bytes: Option<&[u8]>, header: usize) -> Result<Vec<f32>> {
    let Some(bytes) = bytes else {
        return Ok(Vec::new());
    };
    if bytes.len() < header || bytes.len() != header + usize::from(bytes[0]) * 4 {
        return Err(invalid("Document measurement array has an invalid length"));
    }
    bytes[header..]
        .chunks_exact(4)
        .map(|b| finite(f32::from_le_bytes(b.try_into().unwrap()) * 36.0))
        .collect()
}

impl<'a> Document<'a> {
    /// Active (object space, page) identities in section order, excluding history and conflicts.
    /// Encrypted sections return no visible pages; table-of-contents files return an error.
    /// The active revision of one object space.
    pub fn active(&self, space: ExGuid) -> Result<&Revision<'a>> {
        self.spaces
            .get(&space)
            .and_then(Space::active)
            .ok_or_else(|| invalid("Object space has no active revision"))
    }

    /// Active page objects declared in one page space, in section order.
    pub fn pages_in(&self, space: ExGuid) -> Result<Vec<ExGuid>> {
        Ok(self
            .pages()?
            .into_iter()
            .filter_map(|(sid, page)| (sid == space).then_some(page))
            .collect())
    }

    pub fn pages(&self) -> Result<Vec<(ExGuid, ExGuid)>> {
        let active = |sid| self.active(sid);
        let root = active(self.root)?;
        let section = root
            .roots
            .get(&1)
            .and_then(|id| root.nodes.get(id))
            .ok_or_else(|| invalid("Section root is unavailable"))?;
        if matches!(section.kind, Kind::Encrypted { .. }) {
            return Ok(Vec::new());
        }
        if !matches!(section.kind, Kind::Section { .. }) {
            return Err(invalid("Expected a section document"));
        }
        let mut pages = Vec::new();
        let mut seen = BTreeSet::new();
        for id in &section.children {
            let series = root
                .nodes
                .get(id)
                .ok_or_else(|| invalid("Page series is unavailable"))?;
            if !matches!(series.kind, Kind::PageSeries) {
                return Err(invalid("Section child is not a page series"));
            }
            for sid in &series.spaces {
                let revision = active(*sid)?;
                let manifest = revision
                    .roots
                    .get(&1)
                    .and_then(|id| revision.nodes.get(id))
                    .ok_or_else(|| invalid("Page manifest is unavailable"))?;
                if !matches!(manifest.kind, Kind::Manifest { .. }) {
                    return Err(invalid("Page space has no manifest"));
                }
                for page in &manifest.content {
                    if !matches!(
                        revision.nodes.get(page).map(|n| &n.kind),
                        Some(Kind::Page { .. })
                    ) || !seen.insert((*sid, *page))
                    {
                        return Err(invalid(
                            "Section page is missing, duplicated or has the wrong type",
                        ));
                    }
                    pages.push((*sid, *page));
                }
            }
        }
        Ok(pages)
    }

    /// Rejects checksum damage and follows referenced contexts, retaining encrypted payloads opaquely.
    pub fn parse(index: &RevisionIndex<'a>) -> Result<Self> {
        Self::parse_with(
            index,
            |space, revision| index.resolve(space, revision),
            |id| index.store.file_data(id),
        )
    }

    pub(crate) fn parse_with(
        index: &RevisionIndex<'a>,
        mut resolve: impl FnMut(ExGuid, ExGuid) -> Result<crate::ResolvedRevision<'a>>,
        mut file_data: impl FnMut([u8; 16]) -> Result<&'a [u8]>,
    ) -> Result<Self> {
        if !index.store.checksum_mismatches.is_empty() {
            return Err(invalid("Document transaction checksum mismatch"));
        }
        let mut spaces: BTreeMap<ExGuid, Space<'a>> = BTreeMap::new();
        let mut pending = vec![(index.root, ExGuid::default())];
        while let Some((id, context)) = pending.pop() {
            let space = spaces.entry(id).or_insert_with(|| Space {
                contexts: BTreeMap::new(),
                revisions: BTreeMap::new(),
            });
            if space.contexts.contains_key(&context) {
                continue;
            }
            let rid = *index
                .spaces
                .get(&id)
                .and_then(|s| s.labels.get(&(context, 1)))
                .ok_or_else(|| invalid("Document context has no revision"))?;
            space.contexts.insert(context, rid);
            if space.revisions.contains_key(&rid) {
                continue;
            }
            let revision = resolve(id, rid)?;
            let mut reachable = BTreeSet::new();
            let mut objects: Vec<_> = revision.roots.values().copied().collect();
            let mut encrypted = false;
            while let Some(oid) = objects.pop() {
                if !reachable.insert(oid) {
                    continue;
                }
                let object = revision
                    .objects
                    .get(&oid)
                    .ok_or_else(|| invalid("Document object has no declaration"))?;
                if matches!(object.data, ObjectData::Encrypted(_)) {
                    encrypted = true;
                } else {
                    objects.extend(object.references()?.objects);
                }
            }
            if !encrypted {
                revision.reachable()?;
            }
            let mut nodes = BTreeMap::new();
            for oid in reachable {
                let object = &revision.objects[&oid];
                if !matches!(object.data, ObjectData::Encrypted(_)) {
                    let refs = object.references()?;
                    pending.extend(
                        refs.object_spaces
                            .into_iter()
                            .map(|sid| (sid, ExGuid::default())),
                    );
                    pending.extend(refs.contexts.into_iter().map(|context| (id, context)));
                }
                nodes.insert(
                    oid,
                    Element::parse_with(object, index.store, &mut file_data)?,
                );
            }
            for node in nodes.values() {
                if let Kind::RichText {
                    runs,
                    paragraph_style,
                    ..
                } = &node.kind
                {
                    for target in paragraph_style
                        .iter()
                        .chain(runs.iter().filter_map(|r| r.format.as_ref()))
                    {
                        if !matches!(nodes.get(target).map(|n| &n.kind), Some(Kind::Style { .. })) {
                            return Err(invalid("Text formatting does not reference a style"));
                        }
                    }
                }
                let mut tag_types = BTreeSet::new();
                for tag in &node.tags {
                    let action_type = if tag.status & 4 != 0 {
                        tag.action_type
                    } else {
                        match tag
                            .definition
                            .and_then(|id| nodes.get(&id))
                            .map(|n| &n.kind)
                        {
                            Some(Kind::TagDefinition { action_type, .. }) => *action_type,
                            _ => {
                                return Err(invalid(
                                    "Note tag does not reference a tag definition",
                                ));
                            }
                        }
                    }
                    .ok_or_else(|| invalid("Note tag has no action type"))?;
                    if !tag_types.insert(action_type) {
                        return Err(invalid("Repeated note tag action type"));
                    }
                }
            }
            space.revisions.insert(
                rid,
                Revision {
                    roots: revision.roots,
                    nodes,
                },
            );
        }
        for space in spaces.values() {
            for node in space.revisions.values().flat_map(|r| r.nodes.values()) {
                let (context, history) = match &node.kind {
                    Kind::Manifest {
                        history: Some(context),
                    } => (*context, true),
                    Kind::VersionProxy { context, .. } => (*context, false),
                    _ => continue,
                };
                let rid = space.contexts[&context];
                let revision = &space.revisions[&rid];
                let root = revision.roots.get(&1).and_then(|id| revision.nodes.get(id));
                if history {
                    if !matches!(root.map(|n| &n.kind), Some(Kind::VersionHistory)) {
                        return Err(invalid(
                            "History context does not resolve to version history",
                        ));
                    }
                } else if !matches!(root.map(|n| &n.kind), Some(Kind::Manifest { .. }))
                    || space.contexts.get(&ExGuid::default()) == Some(&rid)
                {
                    return Err(invalid(
                        "Version context does not resolve to a historical page",
                    ));
                }
            }
        }
        Ok(Self {
            file_id: index.store.header.file_id,
            root: index.root,
            spaces,
        })
    }
}

impl<'a> Element<'a> {
    pub(crate) fn parse(object: &Object<'a>, store: &Store<'a>) -> Result<Self> {
        Self::parse_with(object, store, &mut |id| store.file_data(id))
    }

    fn parse_with(
        object: &Object<'a>,
        store: &Store<'a>,
        file_data: &mut impl FnMut([u8; 16]) -> Result<&'a [u8]>,
    ) -> Result<Self> {
        let empty = |kind| Self {
            jcid: object.jcid,
            children: vec![],
            content: vec![],
            structure: vec![],
            spaces: vec![],
            child_level: None,
            layout: Layout::default(),
            format: Format::default(),
            created: None,
            modified: None,
            original_author: None,
            latest_author: None,
            media_ids: vec![],
            media_time_ms: None,
            tags: vec![],
            kind,
            extra: vec![],
        };
        if let ObjectData::Encrypted(ciphertext) = object.data {
            return Ok(empty(Kind::Encrypted { ciphertext }));
        }
        if let ObjectData::File { extension, .. } = object.data {
            let reference = object
                .file_reference()?
                .ok_or_else(|| invalid("File object has no reference"))?;
            let payload = if let FileDataReference::Internal(guid) = &reference {
                Some(file_data(*guid)?)
            } else {
                None
            };
            return Ok(empty(Kind::File {
                reference,
                extension: unicode(extension)?,
                payload,
            }));
        }
        let mut f = Fields::new(object)?;
        let children = f.refs(0x24001c20, IdStream::Objects)?;
        let content = f.refs(0x24001c1f, IdStream::Objects)?;
        let structure = f.refs(0x24001d5f, IdStream::Objects)?;
        let spaces = f.refs(0x2c001d63, IdStream::ObjectSpaces)?;
        let child_level = f.u8(0x0c001c03)?;
        if child_level.is_some_and(|level| !(1..=31).contains(&level)) {
            return Err(invalid("Child indentation level is out of range"));
        }
        let layout = Layout {
            x: f.float(0x14001c14, 36.0)?,
            y: f.float(0x14001c15, 36.0)?,
            max_width: f.float(0x14001c1b, 36.0)?,
            width_set_by_user: f.boolean(0x08001cbd)?,
            max_height: f.float(0x14001c1c, 36.0)?,
            reserved_width: f.float(0x14001cdb, 36.0)?,
        };
        let format = Format {
            bold: f.boolean(0x08001c04)?,
            italic: f.boolean(0x08001c05)?,
            underline: f.boolean(0x08001c06)?,
            strike: f.boolean(0x08001c07)?,
            superscript: f.boolean(0x08001c08)?,
            subscript: f.boolean(0x08001c09)?,
            hidden: f.boolean(0x08001e16)?,
            hyperlink: f.boolean(0x08001e14)?,
            hyperlink_label: f.boolean(0x08001e19)?,
            math: f.boolean(0x08003401)?,
            embedded_object: f.boolean(0x08001e22)?,
            font: f.text(0x1c001c0a)?,
            font_size: f.u16(0x10001c0b)?.map(|v| f32::from(v) / 2.0),
            color: f.u32(0x14001c0c)?,
            highlight: f.u32(0x14001c0d)?,
            language: f.u32(0x14001c3b)?,
            alignment: f.u8(0x0c003477)?,
            rtl: f.boolean(0x08003476)?,
            space_before: f.float(0x1400342e, 36.0)?,
            space_after: f.float(0x1400342f, 36.0)?,
            line_spacing: f.float(0x14003430, 36.0)?,
            list_spacing: f.float(0x14001ccb, 36.0)?,
        };
        let mut tags = Vec::new();
        if let Some(property) = f.take(0x40003489) {
            let Value::Sets(range) = property.value else {
                return Err(invalid("Note tags require property sets"));
            };
            if range.len() > 9 {
                return Err(invalid("An element has more than nine note tags"));
            }
            for extra_set in range {
                let mut tag = Fields {
                    sets: vec![std::mem::take(&mut f.sets[extra_set])],
                    ids: f.ids,
                };
                tags.push(Tag {
                    definition: tag.one(0x20003488)?,
                    action_type: tag.u16(0x10003463)?,
                    status: tag
                        .u16(0x10003470)?
                        .ok_or_else(|| invalid("Note tag has no status"))?,
                    created: tag.u32(0x1400346e)?,
                    completed: tag.u32(0x1400346f)?,
                    start: tag.u32(0x1400346a)?,
                    due: tag.u32(0x1400346b)?,
                    task_id: tag.fixed(0x1c003469)?,
                    extra_set,
                });
                f.sets[extra_set] = tag.sets.remove(0);
            }
        }
        let kind = match object.jcid {
            0x60007 => {
                let templates = f.refs(0x2c001d62, IdStream::ObjectSpaces)?;
                if templates.len() > 1 {
                    return Err(invalid("Section has multiple default page templates"));
                }
                Kind::Section {
                    default_template: templates.first().copied(),
                }
            }
            0x60008 => Kind::PageSeries,
            0x60037 => {
                let contexts = f.refs(0x3400347b, IdStream::Contexts)?;
                if contexts.len() > 1 {
                    return Err(invalid("Page manifest has multiple history contexts"));
                }
                Kind::Manifest {
                    history: contexts.first().copied(),
                }
            }
            0x6003c => Kind::VersionHistory,
            0x6003d => {
                let contexts = f.refs(0x3400347b, IdStream::Contexts)?;
                let [context] = contexts.as_slice() else {
                    return Err(invalid("Version proxy requires one context"));
                };
                Kind::VersionProxy {
                    context: *context,
                    modified_filetime: f.fixed(0x18001d77)?.map(u64::from_le_bytes),
                }
            }
            0x20046 => Kind::VersionHistoryMetadata,
            0x2003e => Kind::TemplateMetadata {
                name: f.text(0x1c001ca5)?,
            },
            0x6000b => Kind::Page {
                alternate_title: f.text(0x1c001d3c)?,
                level: f.u32(0x14001dff)?,
                width: f.float(0x14001c01, 36.0)?,
                height: f.float(0x14001c02, 36.0)?,
                margin_origin_x: f.float(0x14001d0f, 36.0)?,
                margin_origin_y: f.float(0x14001d10, 36.0)?,
                rtl: f.boolean(0x08001c92)?,
            },
            0x20030 => Kind::Metadata {
                title: f.text(0x1c001cf3)?,
                level: f.u32(0x14001dff)?,
            },
            0x20038 => Kind::ConflictMetadata {
                title: f.text(0x1c001cf3)?,
                level: f.u32(0x14001dff)?,
                author: f.text(0x1c001d9e)?,
            },
            0x60014 => Kind::Ink {
                data: f.one(0x20003415)?,
                scale_x: f.float(0x14001c46, 1.0)?,
                scale_y: f.float(0x14001c47, 1.0)?,
            },
            0x2003b => Kind::InkData {
                strokes: f.refs(0x24003416, IdStream::Objects)?,
                bounds: f.fixed::<16>(0x1c003418)?.map(|b| {
                    std::array::from_fn(|i| {
                        i32::from_le_bytes(b[i * 4..i * 4 + 4].try_into().unwrap())
                    })
                }),
            },
            0x20047 => Kind::InkStroke {
                path: f.bytes(0x1c00340b)?.unwrap_or_default().to_vec(),
                style: f.one(0x20003409)?,
                bias: f.u8(0x0c00341c)?,
                language: f.u16(0x1000341b)?,
                identity: f.fixed(0x1c00341a)?,
                index: f.u32(0x14003419)?,
            },
            0x120048 => Kind::InkStyle {
                dimensions: f.bytes(0x1c00340a)?.unwrap_or_default().to_vec(),
                width: f.float(0x1400340d, 1.0)?,
                height: f.float(0x1400340c, 1.0)?,
                color: f.u32(0x1400340f)?,
                transparency: f.u8(0x0c003414)?,
                pen_tip: f.u8(0x0c003412)?,
                raster_operation: f.u8(0x0c003413)?,
                antialiased: f.boolean(0x0800340e)?,
                fit_to_curve: f.boolean(0x08003410)?,
                ignore_pressure: f.boolean(0x08003411)?,
            },
            0x20031 => Kind::SectionMetadata {
                name: f.text(0x1c00349b)?,
                color: f.u32(0x14001cbe)?,
            },
            0x20044 => Kind::RevisionMetadata {
                modified_filetime: f.fixed(0x18001d77)?.map(u64::from_le_bytes),
            },
            0x6002c => Kind::Title,
            0x6000c => Kind::Outline {
                indents: measurements(f.bytes(0x1c001c12)?, 4)?,
            },
            0x6000d => Kind::Paragraph {
                lists: f.refs(0x24001c26, IdStream::Objects)?,
                paragraph_style: f.one(0x2000342c)?,
                collapse_state: f.u8(0x0c001c11)?,
            },
            0x60019 => Kind::OutlineGroup,
            0x6000e => {
                let utf16 = f.bytes(0x1c001c22)?;
                let ascii = f.bytes(0x1c003498)?;
                let text = match utf16 {
                    Some(b) => unicode(b)?,
                    None => ascii
                        .unwrap_or_default()
                        .iter()
                        .map(|b| char::from(*b))
                        .collect(),
                };
                let length = u32::try_from(text.encode_utf16().count())
                    .map_err(|_| invalid("Text exceeds UTF-16 offset range"))?;
                let mut bounds = vec![0];
                if let Some(bytes) = f.bytes(0x1c001e12)? {
                    if !bytes.len().is_multiple_of(4) {
                        return Err(invalid("Text-run index has an invalid length"));
                    }
                    bounds.extend(
                        bytes
                            .chunks_exact(4)
                            .map(|b| u32::from_le_bytes(b.try_into().unwrap())),
                    );
                }
                bounds.push(length);
                if bounds.windows(2).any(|b| b[0] > b[1]) {
                    return Err(invalid("Text-run indices exceed text or are out of order"));
                }
                let mut boundaries = bounds.iter().copied().peekable();
                let mut position = 0;
                for character in text.chars() {
                    while boundaries.peek() == Some(&position) {
                        boundaries.next();
                    }
                    position += character.len_utf16() as u32;
                    if boundaries.peek().is_some_and(|next| *next < position) {
                        return Err(invalid("Text-run boundary splits a surrogate pair"));
                    }
                }
                let formats = f.refs(0x24001e13, IdStream::Objects)?;
                if !formats.is_empty() && formats.len() != bounds.len() - 1 {
                    return Err(invalid("Text-run indices and formats disagree"));
                }
                let data = f
                    .take(0x40003499)
                    .map(|property| {
                        let Value::Sets(range) = property.value else {
                            return Err(invalid("Text-run data requires property sets"));
                        };
                        if range.len() != bounds.len() - 1 {
                            return Err(invalid("Text-run indices and data disagree"));
                        }
                        Ok(range)
                    })
                    .transpose()?;
                let runs = bounds
                    .windows(2)
                    .enumerate()
                    .map(|(i, b)| TextRun {
                        start: b[0],
                        end: b[1],
                        format: formats.get(i).copied(),
                        extra_set: data.as_ref().map(|range| range.start + i),
                    })
                    .collect();
                let boilerplate = f.boolean(0x08001c87)?.unwrap_or(false)
                    | f.boolean(0x08001c88)?.unwrap_or(false)
                    | f.boolean(0x08001cb5)?.unwrap_or(false);
                Kind::RichText {
                    text,
                    runs,
                    paragraph_style: f.one(0x2000342c)?,
                    boilerplate,
                }
            }
            0x12004d => Kind::Style {
                name: f.text(0x1c00345a)?,
            },
            0x60012 => {
                let format = f
                    .bytes(0x1c001c1a)?
                    .map(|bytes| {
                        if bytes.len() < 2
                            || bytes.len()
                                != 2 + usize::from(u16::from_le_bytes(
                                    bytes[..2].try_into().unwrap(),
                                )) * 2
                        {
                            return Err(invalid("List format has an invalid length"));
                        }
                        let units = bytes[2..]
                            .chunks_exact(2)
                            .map(|b| u16::from_le_bytes(b.try_into().unwrap()))
                            .collect::<Vec<_>>();
                        String::from_utf16(&units)
                            .map_err(|_| invalid("Invalid UTF-16 list format"))
                    })
                    .transpose()?;
                Kind::List {
                    font: f.text(0x1c001c52)?,
                    format,
                    restart: f.u32(0x14001cb7)?,
                    bullet: f.u16(0x10001d0e)?,
                }
            }
            0x60022 => {
                let columns = f.u32(0x14001d58)?;
                let widths = measurements(f.bytes(0x1c001d66)?, 1)?;
                let mut locked = Vec::new();
                if let Some(bytes) = f.bytes(0x1c001d7d)? {
                    let count = usize::from(
                        *bytes
                            .first()
                            .ok_or_else(|| invalid("Empty table column locks"))?,
                    );
                    if bytes.len() != 1 + count.div_ceil(8) || columns != Some(count as u32) {
                        return Err(invalid("Table column locks disagree with column count"));
                    }
                    locked.extend((0..count).map(|i| bytes[1 + i / 8] & (1 << (i % 8)) != 0));
                }
                if !widths.is_empty() && columns != Some(widths.len() as u32) {
                    return Err(invalid("Table widths disagree with column count"));
                }
                Kind::Table {
                    rows: f.u32(0x14001d57)?,
                    columns,
                    widths,
                    locked,
                    borders: f.boolean(0x08001d5e)?,
                }
            }
            0x60023 => Kind::Row,
            0x60024 => Kind::Cell {
                shading: f.u32(0x14001e26)?,
                indents: measurements(f.bytes(0x1c001c12)?, 4)?,
            },
            0x60011 => Kind::Image {
                container: f.one(0x20001c3f)?,
                filename: f.text(0x1c001dd7)?,
                alt: f.text(0x1c001e58)?,
                picture_width: f.float(0x140034cd, 36.0)?,
                picture_height: f.float(0x140034ce, 36.0)?,
                background: f.boolean(0x08001d13)?,
                printout: f.boolean(0x08001d85)?,
                link: f.text(0x1c001e20)?,
            },
            0x60035 => Kind::Attachment {
                container: f.one(0x20001d9b)?,
                preview: f.one(0x20001c3f)?,
                filename: f.text(0x1c001d9c)?,
                source_path: f.text(0x1c001d9d)?,
                recording_id: f.fixed(0x1c001c97)?,
                recording_type: f.u32(0x14001d24)?,
                icon_width: f.float(0x140034cd, 36.0)?,
                icon_height: f.float(0x140034ce, 36.0)?,
            },
            0x120001 => Kind::Author {
                name: f.text(0x1c001d75)?,
            },
            0x120043 => Kind::TagDefinition {
                label: f.text(0x1c003468)?,
                action_type: f.u16(0x10003463)?,
                shape: f.u16(0x10003464)?,
                color: f.u32(0x14003466)?,
                highlight: f.u32(0x14003465)?,
            },
            0x20001 if store.header.file_type == crate::FileType::TableOfContents => Kind::Toc {
                entries: f.refs(0x24001cf6, IdStream::Objects)?,
                filename: f.text(0x1c001d6b)?,
                identity: f.fixed(0x1c001d94)?,
                order: f.u32(0x14001cb9)?,
                color: f.u32(0x14001cbe)?,
            },
            _ => Kind::Unknown,
        };
        let media_ids = f.bytes(0x1c001c98)?.unwrap_or_default();
        if !media_ids.len().is_multiple_of(16) {
            return Err(invalid("Media identifiers have an invalid length"));
        }
        let media_ids = media_ids
            .chunks_exact(16)
            .map(|id| id.try_into().unwrap())
            .collect();
        Ok(Self {
            jcid: object.jcid,
            children,
            content,
            structure,
            spaces,
            child_level,
            layout,
            format,
            created: f.u32(0x14001d09)?,
            modified: f.u32(0x14001d7a)?,
            original_author: f.one(0x20001d78)?,
            latest_author: f.one(0x20001d79)?,
            media_ids,
            media_time_ms: f.u32(0x14001c99)?,
            tags,
            kind,
            extra: f.remainder()?,
        })
    }
}
