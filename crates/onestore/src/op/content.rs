//! Paragraph and page content other than text: pictures, attachments, ink and equations,
//! written as OneNote 2010 stores them.

use super::Values;
use crate::{
    Error, ExGuid,
    active::{ActivePage, Changes},
    document::{Format, Layout},
    page::{Attachment, Image, Ink, InkStroke, Paragraph},
    write::PropertyObject,
};
use std::collections::BTreeMap;

fn invalid(message: &'static str) -> Error {
    Error { offset: 0, message }
}

/// The `<ifndf>{GUID}` form a file-data declaration uses to name a payload in the file-data store.
fn payload_reference(guid: [u8; 16]) -> String {
    let id = ExGuid { guid, n: 0 }.to_string();
    format!("<ifndf>{}", id.split(',').next().unwrap())
}

/// The compact child references a page object holds.
fn page_children(object: &PropertyObject) -> Result<Vec<u8>, Error> {
    let properties = crate::PropertySets::parse(&object.bytes)?;
    match properties.sets[0]
        .iter()
        .find(|p| p.id == 0x24001c20)
        .map(|p| &p.value)
    {
        Some(crate::Value::References { compact_ids, .. }) => Ok(compact_ids.to_vec()),
        None => Ok(Vec::new()),
        _ => Err(invalid("The page has an invalid child list")),
    }
}

/// Makes `object` the content of paragraph `holder`, or a child of the page after its others.
fn hold(
    active: &ActivePage<'_>,
    changed: &mut Changes,
    object: ExGuid,
    holder: Option<ExGuid>,
    modified: &[u8; 4],
) -> Result<(), Error> {
    let raw = &active.live.revision;
    match holder {
        Some(holder) => {
            let mut paragraph = PropertyObject::from_object(&raw.objects[&holder])?;
            let content = paragraph.reference(object)?;
            paragraph.set(&[(0x24001c1f, &content), (0x14001d7a, modified)])?;
            changed.insert(holder, paragraph);
        }
        None => {
            let [page] = active.pages[..] else {
                return Err(invalid("Choose an object space containing one active page"));
            };
            let mut node = PropertyObject::from_object(&raw.objects[&page])?;
            let mut children = page_children(&node)?;
            children.extend(node.reference(object)?);
            node.set(&[(0x24001c20, &children), (0x14001d7a, modified)])?;
            changed.insert(page, node);
        }
    }
    Ok(())
}

/// The character-style properties a span format sets; paragraph-level fields stay on the
/// paragraph style.
pub(crate) fn style_values(format: &Format) -> Values {
    let mut values = Values::new();
    for (id, flag) in [
        (0x08001c04, format.bold),
        (0x08001c05, format.italic),
        (0x08001c06, format.underline),
        (0x08001c07, format.strike),
        (0x08001c08, format.superscript),
        (0x08001c09, format.subscript),
        (0x08001e16, format.hidden),
        (0x08001e14, format.hyperlink),
        (0x08001e19, format.hyperlink_label),
        (0x08003401, format.math),
        (0x08001e22, format.embedded_object),
    ] {
        if let Some(flag) = flag {
            values.push((id | (u32::from(flag) << 31), Vec::new()));
        }
    }
    if let Some(font) = &format.font {
        values.push((0x1c001c0a, crate::create::string(font)));
    }
    if let Some(size) = format.font_size {
        values.push((
            0x10001c0b,
            ((size * 2.0).round() as u16).to_le_bytes().to_vec(),
        ));
    }
    if let Some(color) = format.color {
        values.push((0x14001c0c, color.to_le_bytes().to_vec()));
    }
    if let Some(highlight) = format.highlight {
        values.push((0x14001c0d, highlight.to_le_bytes().to_vec()));
    }
    if let Some(language) = format.language {
        values.push((0x14001c3b, language.to_le_bytes().to_vec()));
    }
    values
}

/// The indentation table, in points, OneNote gives each outline and table cell it creates.
pub(crate) const NATIVE_INDENTS: [f32; 4] = [18.0, 0.0, 27.0, 27.0];

/// A native measurement array: a count byte, `header - 1` reserved bytes, then each value
/// in half-inch units.
pub(crate) fn measurement_bytes(values: &[f32], header: usize) -> Result<Vec<u8>, Error> {
    let count = u8::try_from(values.len())
        .map_err(|_| invalid("A measurement array exceeds the document range"))?;
    let mut bytes = vec![0u8; header];
    bytes[0] = count;
    for value in values {
        if !value.is_finite() {
            return Err(invalid("A measurement must be finite"));
        }
        bytes.extend_from_slice(&(value / 36.0).to_le_bytes());
    }
    Ok(bytes)
}

/// A stored picture keeps what `Picture` cannot change.
pub(crate) fn picture_fixed_fields(stored: &Image, image: &Image) -> Result<(), Error> {
    if stored.id != image.id
        || stored.bytes != image.bytes
        || stored.size != image.size
        || stored.background != image.background
    {
        return Err(invalid(
            "A stored picture keeps its payload, intrinsic size and background state",
        ));
    }
    Ok(())
}

/// Position and displayed-size properties that take `layout` from `stored`, and the ones
/// to remove.
fn layout_values(stored: &Layout, layout: &Layout) -> Result<(Values, Vec<u32>), Error> {
    let mut values = Values::new();
    let mut removed = Vec::new();
    if (layout.x, layout.y) != (stored.x, stored.y) {
        let (Some(x), Some(y)) = (layout.x, layout.y) else {
            return Err(invalid("A picture position needs both coordinates"));
        };
        if !(x.is_finite() && y.is_finite()) {
            return Err(invalid("A picture position must be finite"));
        }
        values.push((0x14001c14, (x / 36.0).to_le_bytes().to_vec()));
        values.push((0x14001c15, (y / 36.0).to_le_bytes().to_vec()));
    }
    if (
        layout.max_width,
        layout.max_height,
        layout.width_set_by_user,
    ) != (
        stored.max_width,
        stored.max_height,
        stored.width_set_by_user,
    ) {
        match (layout.max_width, layout.max_height) {
            (Some(width), Some(height)) => {
                if !(width.is_finite() && height.is_finite() && width > 0.0 && height > 0.0) {
                    return Err(invalid("Picture size must be positive"));
                }
                values.push((0x14001c1b, (width / 36.0).to_le_bytes().to_vec()));
                values.push((0x14001c1c, (height / 36.0).to_le_bytes().to_vec()));
                let user_set = u32::from(layout.width_set_by_user == Some(true));
                values.push((0x08001cbd | (user_set << 31), Vec::new()));
            }
            (None, None) => removed.extend([0x14001c1b, 0x14001c1c, 0x08001cbd]),
            _ => return Err(invalid("A picture size needs both dimensions")),
        }
    }
    if layout.reserved_width != stored.reserved_width {
        return Err(invalid("A picture has no reserved width"));
    }
    Ok((values, removed))
}

fn icon_size(values: &mut Values, size: Option<[f32; 2]>, message: &'static str) -> Result<(), Error> {
    if let Some([width, height]) = size {
        if !(width.is_finite() && height.is_finite() && width > 0.0 && height > 0.0) {
            return Err(invalid(message));
        }
        values.push((0x140034cd, (width / 36.0).to_le_bytes().to_vec()));
        values.push((0x140034ce, (height / 36.0).to_le_bytes().to_vec()));
    }
    Ok(())
}

/// What OneNote stores for an inserted picture: a file-data object declaring payload
/// `payload` by identity and extension, and picture object `id` that paragraph `holder`
/// holds as content or the page lists as a child. The caller embeds the payload.
pub(crate) fn picture_changes(
    active: &ActivePage<'_>,
    image: &Image,
    id: ExGuid,
    file: ExGuid,
    payload: [u8; 16],
    holder: Option<ExGuid>,
) -> Result<Changes, Error> {
    let Some(bytes) = &image.bytes else {
        return Err(invalid("A new picture needs its payload"));
    };
    let extension = match bytes.as_ref() {
        [0x89, b'P', b'N', b'G', ..] => ".png",
        [0xff, 0xd8, 0xff, ..] => ".jpg",
        [b'G', b'I', b'F', b'8', ..] => ".gif",
        [b'B', b'M', ..] => ".bmp",
        _ => return Err(invalid("Choose a PNG, JPEG, GIF or BMP picture")),
    };
    let modified = crate::create::current_timestamps()?.0.to_le_bytes();
    let mut values: Values = vec![(0x14001d7a, modified.to_vec())];
    if let Some([width, height]) = image.size {
        if !(width.is_finite() && height.is_finite() && width > 0.0 && height > 0.0) {
            return Err(invalid("Picture size must be positive"));
        }
        values.push((0x140034cd, (width / 36.0).to_le_bytes().to_vec()));
        values.push((0x140034ce, (height / 36.0).to_le_bytes().to_vec()));
    }
    values.extend(layout_values(&Default::default(), &image.layout)?.0);
    if let Some(alt) = &image.alt {
        values.push((0x1c001e58, crate::create::string(alt)));
    }
    if image.background {
        values.push((0x08001d13 | (1 << 31), Vec::new()));
    }
    values.push((0x14001c3b, 0x409_u32.to_le_bytes().to_vec()));
    values.push((0x08001d85, Vec::new()));
    let mut changed = BTreeMap::new();
    changed.insert(
        file,
        PropertyObject::file(file, &payload_reference(payload), extension)?,
    );
    let mut picture = PropertyObject {
        jcid: 0x60011,
        bytes: crate::create::properties(&values)?,
        global_ids: std::sync::Arc::new(BTreeMap::from([(0, id.guid)])),
    };
    picture.reference(id)?;
    let container = picture.reference(file)?;
    picture.set(&[(0x20001c3f, &container)])?;
    changed.insert(id, picture);
    hold(active, &mut changed, id, holder, &modified)?;
    Ok(changed)
}

/// A moved, resized or described picture as OneNote stores one: the position, the layout
/// width and height with the user flag and the description on the picture object, leaving
/// the intrinsic size alone.
pub(crate) fn picture_edit_changes(
    active: &ActivePage<'_>,
    object: ExGuid,
    stored: (&Layout, &Option<String>),
    layout: &Layout,
    alt: &Option<String>,
) -> Result<Changes, Error> {
    let mut values: Values = vec![(
        0x14001d7a,
        crate::create::current_timestamps()?
            .0
            .to_le_bytes()
            .to_vec(),
    )];
    let (layout, mut removed) = layout_values(stored.0, layout)?;
    values.extend(layout);
    if alt != stored.1 {
        match alt {
            Some(alt) => values.push((0x1c001e58, crate::create::string(alt))),
            None => removed.push(0x1c001e58),
        }
    }
    let raw = &active.live.revision;
    let mut picture = PropertyObject::from_object(&raw.objects[&object])?;
    picture.remove(&removed)?;
    let values: Vec<(u32, &[u8])> = values
        .iter()
        .map(|(id, bytes)| (*id, bytes.as_slice()))
        .collect();
    picture.set(&values)?;
    Ok(BTreeMap::from([(object, picture)]))
}

/// Identities of a new attachment's objects and payloads.
pub(crate) struct AttachmentIds {
    pub object: ExGuid,
    pub file: ExGuid,
    pub payload: [u8; 16],
    /// The preview icon's payload and file-data object, when it has one.
    pub preview: Option<([u8; 16], ExGuid)>,
}

/// What OneNote stores for an inserted file: an embedded-file container declaring the
/// payload and an attachment object naming the file, which paragraph `holder` holds as
/// content. The caller embeds the payloads.
pub(crate) fn attachment_changes(
    active: &ActivePage<'_>,
    attachment: &Attachment,
    ids: &AttachmentIds,
    holder: ExGuid,
) -> Result<Changes, Error> {
    let name = attachment.filename.as_str();
    if name.is_empty() || name.contains(['\0', '/', '\\']) {
        return Err(invalid(
            "An attachment needs a file name without path separators",
        ));
    }
    if attachment.recording.is_some() {
        return Err(invalid("Recordings are captured by OneNote, not inserted"));
    }
    if attachment.bytes.is_none() {
        return Err(invalid("A new attachment needs its payload"));
    }
    if attachment
        .preview
        .as_ref()
        .is_some_and(|icon| !icon.starts_with(&[0x89, b'P', b'N', b'G']))
    {
        return Err(invalid("An attachment preview is a PNG icon"));
    }
    let extension = name
        .rfind('.')
        .filter(|dot| *dot > 0)
        .map(|dot| &name[dot..])
        .unwrap_or("");
    let modified = crate::create::current_timestamps()?.0.to_le_bytes();
    let mut values: Values = vec![(0x14001d7a, modified.to_vec())];
    icon_size(&mut values, attachment.size, "Attachment icon size must be positive")?;
    values.push((0x14001c3b, 0x409_u32.to_le_bytes().to_vec()));
    values.push((0x10001cfe, 0x409_u16.to_le_bytes().to_vec()));
    values.push((0x1c001dcf, vec![0; 32]));
    values.push((
        0x1c001d61,
        [16u32, 1, 0, 0, 0]
            .iter()
            .flat_map(|v| v.to_le_bytes())
            .collect(),
    ));
    values.push((0x14001c3e, 1u32.to_le_bytes().to_vec()));
    values.push((0x14001c84, 1u32.to_le_bytes().to_vec()));
    values.push((0x1c001c22, crate::create::string(name)));
    values.push((0x1c001d9c, crate::create::string(name)));
    if let Some(path) = &attachment.source_path {
        values.push((0x1c001d9d, crate::create::string(path)));
    }
    let mut changed = BTreeMap::new();
    let mut file = PropertyObject::file(ids.file, &payload_reference(ids.payload), extension)?;
    file.jcid = crate::write::EMBEDDED_FILE_JCID;
    changed.insert(ids.file, file);
    let mut node = PropertyObject {
        jcid: 0x60035,
        bytes: crate::create::properties(&values)?,
        global_ids: std::sync::Arc::new(BTreeMap::from([(0, ids.object.guid)])),
    };
    node.reference(ids.object)?;
    let container = node.reference(ids.file)?;
    node.set(&[(0x20001d9b, &container)])?;
    if let Some((payload, icon)) = ids.preview {
        changed.insert(
            icon,
            PropertyObject::file(icon, &payload_reference(payload), ".png")?,
        );
        let reference = node.reference(icon)?;
        node.set(&[(0x20001c3f, &reference)])?;
    }
    changed.insert(ids.object, node);
    hold(active, &mut changed, ids.object, Some(holder), &modified)?;
    Ok(changed)
}

/// A stored attachment keeps its payload and preview; its shown name, recorded source
/// path and icon size change in place, as OneNote's rename does.
pub(crate) fn attachment_edit_changes(
    active: &ActivePage<'_>,
    stored: &Attachment,
    attachment: &Attachment,
) -> Result<Changes, Error> {
    let mut values: Values = vec![(
        0x14001d7a,
        crate::create::current_timestamps()?
            .0
            .to_le_bytes()
            .to_vec(),
    )];
    if attachment.recording != stored.recording {
        return Err(invalid("A recording stays the recording OneNote captured"));
    }
    let name = attachment.filename.as_str();
    if name.is_empty() || name.contains(['\0', '/', '\\']) {
        return Err(invalid(
            "An attachment needs a file name without path separators",
        ));
    }
    let mut removed = Vec::new();
    if attachment.filename != stored.filename {
        let name = crate::create::string(&attachment.filename);
        values.push((0x1c001c22, name.clone()));
        values.push((0x1c001d9c, name));
    }
    if attachment.source_path != stored.source_path {
        match &attachment.source_path {
            Some(path) => values.push((0x1c001d9d, crate::create::string(path))),
            None => removed.push(0x1c001d9d),
        }
    }
    if attachment.size != stored.size {
        match attachment.size {
            Some(_) => icon_size(&mut values, attachment.size, "Attachment icon size must be positive")?,
            None => removed.extend([0x140034cd, 0x140034ce]),
        }
    }
    let object = stored.id;
    let raw = &active.live.revision;
    let mut node = PropertyObject::from_object(&raw.objects[&object])?;
    node.remove(&removed)?;
    let values: Vec<(u32, &[u8])> = values
        .iter()
        .map(|(id, bytes)| (*id, bytes.as_slice()))
        .collect();
    node.set(&values)?;
    Ok(BTreeMap::from([(object, node)]))
}

/// Creates stroke objects (numbered from `first`) and one drawing-attribute object per
/// distinct pen; returns the compact references for the data node's stroke list.
fn write_strokes(
    changed: &mut Changes,
    data_object: &mut PropertyObject,
    strokes: &[(ExGuid, &InkStroke)],
    first: usize,
    filetime: u64,
) -> Result<Vec<u8>, Error> {
    let mut styles: Vec<(InkPen, ExGuid)> = Vec::new();
    let mut references = Vec::new();
    for (offset, (stroke_id, stroke)) in strokes.iter().enumerate() {
        let pen = InkPen::of(stroke);
        let style = match styles.iter().find(|(known, _)| *known == pen) {
            Some((_, id)) => *id,
            None => {
                let id = ExGuid {
                    guid: crate::write::fresh_guid()?,
                    n: 1,
                };
                let mut style = PropertyObject {
                    jcid: 0x120048,
                    bytes: crate::create::properties(&pen.values())?,
                    global_ids: std::sync::Arc::new(BTreeMap::from([(0, id.guid)])),
                };
                style.reference(id)?;
                changed.insert(id, style);
                styles.push((pen, id));
                id
            }
        };
        let mut object = PropertyObject {
            jcid: 0x20047,
            bytes: crate::create::properties(&stroke_values(
                stroke,
                (first + offset) as u32 + 1,
                filetime,
            )?)?,
            global_ids: std::sync::Arc::new(BTreeMap::from([(0, stroke_id.guid)])),
        };
        object.reference(*stroke_id)?;
        let style_reference = object.reference(style)?;
        object.set(&[(0x20003409, &style_reference)])?;
        changed.insert(*stroke_id, object);
        references.extend(data_object.reference(*stroke_id)?);
    }
    Ok(references)
}

/// The drawing attributes OneNote shares between strokes drawn with the same pen.
#[derive(PartialEq)]
struct InkPen {
    width: u32,
    height: u32,
    color: Option<u32>,
    transparency: Option<u8>,
    pen_tip: Option<u8>,
}

impl InkPen {
    fn of(stroke: &InkStroke) -> Self {
        Self {
            width: (stroke.width * 2540.0 / 72.0).to_bits(),
            height: (stroke.height * 2540.0 / 72.0).to_bits(),
            color: stroke.color,
            transparency: stroke.transparency,
            pen_tip: stroke.pen_tip,
        }
    }

    fn values(&self) -> Values {
        let mut values: Values = vec![
            (0x1c00340a, crate::page::ink::DIMENSIONS.to_vec()),
            (0x1400340c, self.height.to_le_bytes().to_vec()),
            (0x1400340d, self.width.to_le_bytes().to_vec()),
        ];
        if let Some(color) = self.color {
            values.push((0x1400340f, color.to_le_bytes().to_vec()));
        }
        if let Some(transparency) = self.transparency {
            values.push((0x0c003414, vec![transparency]));
        }
        if let Some(tip) = self.pen_tip {
            values.push((0x0c003412, vec![tip]));
        }
        values
    }
}

fn stroke_values(stroke: &InkStroke, index: u32, filetime: u64) -> Result<Values, Error> {
    if stroke.points.is_empty() {
        return Err(invalid("A stroke needs at least one point"));
    }
    if stroke
        .points
        .iter()
        .any(|p| !p[0].is_finite() || !p[1].is_finite())
        || !(stroke.width.is_finite() && stroke.height.is_finite())
        || stroke.width <= 0.0
        || stroke.height <= 0.0
    {
        return Err(invalid(
            "Stroke points and pen size must be finite and positive",
        ));
    }
    let left = stroke
        .points
        .iter()
        .map(|p| p[0])
        .fold(f32::INFINITY, f32::min);
    let top = stroke
        .points
        .iter()
        .map(|p| p[1])
        .fold(f32::INFINITY, f32::min);
    let mut origin = ((left - stroke.width / 2.0) / 36.0).to_le_bytes().to_vec();
    origin.extend_from_slice(&((top - stroke.height / 2.0) / 36.0).to_le_bytes());
    Ok(vec![
        (0x1c00340b, stroke.packet()),
        (0x14003419, index.to_le_bytes().to_vec()),
        (0x1000341b, 0x409_u16.to_le_bytes().to_vec()),
        (0x0c00341c, vec![0]),
        (0x1c00341a, crate::write::fresh_guid()?.to_vec()),
        (0x1c00341d, filetime.to_le_bytes().to_vec()),
        (0x1c00345b, origin),
    ])
}

/// New ink as OneNote 2010 stores a drawing: container `id` that the page lists as a child
/// (or paragraph `holder` holds as content), its data node `data` listing the strokes, each
/// stroke's packet and half-inch origin, and one drawing-attribute object per distinct pen.
pub(crate) fn ink_changes(
    active: &ActivePage<'_>,
    ink: &Ink,
    id: ExGuid,
    data: ExGuid,
    strokes: &[(ExGuid, &InkStroke)],
    holder: Option<ExGuid>,
) -> Result<Changes, Error> {
    if !ink.groups.is_empty() {
        return Err(invalid("New ink holds strokes, not nested groups"));
    }
    if ink.strokes.is_empty() {
        return Err(invalid("New ink needs at least one stroke"));
    }
    if ink.layout != Default::default() {
        return Err(invalid("Ink positions come from its strokes"));
    }
    let (modified, filetime) = crate::create::current_timestamps()?;
    let modified = modified.to_le_bytes();
    let mut changed = BTreeMap::new();
    let mut data_object = PropertyObject {
        jcid: 0x2003b,
        bytes: crate::create::properties(&[])?,
        global_ids: std::sync::Arc::new(BTreeMap::from([(0, data.guid)])),
    };
    data_object.reference(data)?;
    let references = write_strokes(&mut changed, &mut data_object, strokes, 0, filetime)?;
    data_object.set(&[(0x24003416, &references)])?;
    changed.insert(data, data_object);
    let mut object = PropertyObject {
        jcid: 0x60014,
        bytes: crate::create::properties(&[
            (0x14001d7a, modified.to_vec()),
            (0x14001d4e, 1u32.to_le_bytes().to_vec()),
        ])?,
        global_ids: std::sync::Arc::new(BTreeMap::from([(0, id.guid)])),
    };
    object.reference(id)?;
    let data_reference = object.reference(data)?;
    object.set(&[(0x20003415, &data_reference)])?;
    changed.insert(id, object);
    hold(active, &mut changed, id, holder, &modified)?;
    Ok(changed)
}

/// Rewrites a drawing's stroke list: `kept` strokes stay as they are (OneNote erases whole
/// strokes rather than editing them), the rest leave the list, `added` ones are created.
pub(crate) fn strokes_changes(
    active: &ActivePage<'_>,
    container: ExGuid,
    kept: &[ExGuid],
    added: &[(ExGuid, &InkStroke)],
) -> Result<Changes, Error> {
    let data = match active
        .view
        .nodes
        .get(&container)
        .map(|node| &node.kind)
    {
        Some(crate::document::Kind::Ink {
            data: Some(data), ..
        }) => *data,
        _ => return Err(invalid("Stored ink has no stroke data to rewrite")),
    };
    let (modified, filetime) = crate::create::current_timestamps()?;
    let modified = modified.to_le_bytes();
    let raw = &active.live.revision;
    let mut changed = BTreeMap::new();
    let mut data_object = PropertyObject::from_object(&raw.objects[&data])?;
    let mut references = Vec::new();
    for id in kept {
        references.extend(data_object.reference(*id)?);
    }
    references.extend(write_strokes(
        &mut changed,
        &mut data_object,
        added,
        kept.len(),
        filetime,
    )?);
    data_object.set(&[(0x24003416, &references)])?;
    changed.insert(data, data_object);
    let mut object = PropertyObject::from_object(&raw.objects[&container])?;
    object.set(&[(0x14001d7a, &modified)])?;
    changed.insert(container, object);
    Ok(changed)
}

/// An equation as OneNote stores one: the linear text, one run per span with a style
/// carrying the span's format, the run-data array naming each run's inline object, and
/// the math language marker on the text object.
pub(crate) fn equation_changes(
    active: &ActivePage<'_>,
    object: ExGuid,
    text: &Paragraph,
) -> Result<Changes, Error> {
    if text.text().contains('\u{fffc}') {
        return Err(invalid("Equations cannot hold embedded objects"));
    }
    let encoded: Vec<u8> = text
        .text()
        .encode_utf16()
        .chain([0])
        .flat_map(u16::to_le_bytes)
        .collect();
    let mut ends = Vec::new();
    let mut styles: Vec<Values> = Vec::new();
    let mut sets = Vec::new();
    let all_math = text.spans().iter().all(|s| s.format.math == Some(true));
    for span in text.spans() {
        ends.extend(text.utf16_offset(span.end)?.to_le_bytes());
        styles.push(style_values(&span.format));
        sets.push(match &span.format.math_object {
            Some(object) => {
                let mut set = vec![(0x1400344f, object.kind.to_le_bytes().to_vec())];
                if let Some(count) = object.arguments {
                    set.push((0x14003450, count.to_le_bytes().to_vec()));
                }
                if let Some(columns) = object.columns {
                    set.push((0x0c003451, vec![columns]));
                }
                for (id, symbol) in [0x10003453, 0x10003454, 0x10003455]
                    .into_iter()
                    .zip(&object.symbols)
                {
                    let unit = u16::try_from(u32::from(*symbol))
                        .map_err(|_| invalid("Math symbols are single UTF-16 units"))?;
                    set.push((id, unit.to_le_bytes().to_vec()));
                }
                set
            }
            None => Vec::new(),
        });
    }
    ends.truncate(ends.len() - 4);
    let has_objects = sets.iter().any(|set| !set.is_empty());
    let modified = crate::create::current_timestamps()?.0.to_le_bytes();
    let raw = &active.live.revision;
    let mut changed = BTreeMap::new();
    let mut target = PropertyObject::from_object(&raw.objects[&object])?;
    let mut references = Vec::new();
    let mut created: Vec<(Values, ExGuid)> = Vec::new();
    for values in &styles {
        let id = match created.iter().find(|(known, _)| known == values) {
            Some((_, id)) => *id,
            None => {
                let id = ExGuid {
                    guid: crate::write::fresh_guid()?,
                    n: 1,
                };
                let mut style = PropertyObject {
                    jcid: 0x12004d,
                    bytes: crate::create::properties(values)?,
                    global_ids: std::sync::Arc::new(BTreeMap::from([(0, id.guid)])),
                };
                style.reference(id)?;
                changed.insert(id, style);
                created.push((values.clone(), id));
                id
            }
        };
        references.extend(target.reference(id)?);
    }
    target.remove(&[0x1c003498, 0x40003499])?;
    target.set(&[
        (0x1c001c22, &encoded),
        (0x1c001e12, &ends),
        (0x24001e13, &references),
        (0x14001d7a, &modified),
    ])?;
    if has_objects {
        target.set_sets(0x40003499, 0x44000811, &sets)?;
    }
    if all_math {
        // The flags and language marker OneNote's equation editor leaves on every equation
        // text object.
        target.set(&[
            (0x10001cfe, &0x7f_u16.to_le_bytes()),
            (0x14001c3e, &1u32.to_le_bytes()),
            (0x14001c84, &1u32.to_le_bytes()),
        ])?;
    }
    changed.insert(object, target);
    Ok(changed)
}
