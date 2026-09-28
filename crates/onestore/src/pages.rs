use crate::{
    Error, ExGuid,
    create::{current_timestamps, properties, string},
    document::{FieldValue, Kind},
    op::content::{NATIVE_INDENTS, measurement_bytes},
    write::{PropertyObject, fresh_guid},
};
use serde::{Deserialize, Serialize};
use std::{
    collections::{BTreeMap, BTreeSet},
    sync::Arc,
};

fn invalid(message: &'static str) -> Error {
    Error { offset: 0, message }
}

pub(crate) fn metadata_id(space: ExGuid) -> ExGuid {
    let mut id = ExGuid {
        guid: space.guid,
        n: 1,
    };
    for (byte, salt) in id.guid.iter_mut().zip([
        0x31, 0xc0, 0xa8, 0x22, 0, 0x36, 0xee, 0x42, 0xb7, 0x14, 0xd7, 0xac, 0xda, 0x24, 0x35, 0xe8,
    ]) {
        *byte ^= salt;
    }
    id
}

pub(crate) fn metadata_guid(metadata: &crate::document::Element<'_>) -> Result<[u8; 16], Error> {
    metadata.extra[0]
        .iter()
        .find_map(|field| match field.value {
            FieldValue::Bytes(bytes) if field.id == 0x1c001c30 => bytes.try_into().ok(),
            _ => None,
        })
        .ok_or_else(|| invalid("Page metadata needs its page identifier"))
}

pub(crate) fn set_references(
    object: &mut PropertyObject,
    property: u32,
    ids: &[ExGuid],
) -> Result<(), Error> {
    let mut references = Vec::new();
    for id in ids {
        references.extend_from_slice(&object.reference(*id)?);
    }
    let source = PropertyObject {
        jcid: object.jcid,
        bytes: properties(&[(property, references)])?,
        global_ids: Arc::clone(&object.global_ids),
    };
    object.copy_property(&source, property)
}

/// A conflict page (MS-ONE 2.1.2): the version of a page a merge could not take, kept
/// read-only under the page it conflicts with.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ConflictPage {
    pub space: ExGuid,
    pub title: String,
    /// Whose version it is (`ConflictingUserName`).
    pub user: String,
    /// FILETIME of its `TopologyCreationTimeStamp`, when the merge made it.
    pub created: Option<u64>,
    /// The conflict objects on it (`IsConflictObjectForRender`), which OneNote highlights.
    pub objects: Vec<ExGuid>,
}

/// Whether an edit preserves position or moves before a page (None appends).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub enum PagePosition {
    Keep,
    Before(Option<ExGuid>),
}

/// A page's explicit position and indentation, with a stable identity for a new series.
/// Moving a page does not implicitly select its following subpages.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct PageEdit {
    guid: [u8; 16],
    space: ExGuid,
    level: u32,
    position: PagePosition,
}

impl PageEdit {
    /// Changes one page's indentation without changing its position.
    pub fn set_level(space: ExGuid, level: u32) -> Result<Self, Error> {
        Self::new(space, level, PagePosition::Keep)
    }

    /// Moves before an existing page space, or appends when `before` is None.
    pub fn move_to(space: ExGuid, before: Option<ExGuid>, level: u32) -> Result<Self, Error> {
        Self::new(space, level, PagePosition::Before(before))
    }

    pub fn space(&self) -> ExGuid {
        self.space
    }

    pub fn level(&self) -> u32 {
        self.level
    }

    pub fn position(&self) -> PagePosition {
        self.position
    }

    /// Revises placement while retaining the selected page and allocated series identity.
    pub fn reposition(&self, position: PagePosition, level: u32) -> Result<Self, Error> {
        let edit = Self {
            position,
            level,
            ..self.clone()
        };
        edit.validate()?;
        Ok(edit)
    }

    fn new(space: ExGuid, level: u32, position: PagePosition) -> Result<Self, Error> {
        let edit = Self {
            guid: fresh_guid()?,
            space,
            level,
            position,
        };
        edit.validate()?;
        Ok(edit)
    }

    fn validate(&self) -> Result<(), Error> {
        if self.guid == [0; 16] || self.space.guid == [0; 16] || !(1..=3).contains(&self.level) {
            return Err(invalid(
                "Choose an existing page space and indentation level 1 through 3",
            ));
        }
        if let PagePosition::Before(Some(before)) = self.position
            && (before.guid == [0; 16] || before == self.space)
        {
            return Err(invalid("Choose a different page as the movement anchor"));
        }
        Ok(())
    }
}

/// What page edits read of one page space: its metadata element and the stored manifest
/// and metadata objects.
pub(crate) struct PageParts<'a> {
    pub metadata: crate::document::Element<'a>,
    pub manifest: (ExGuid, PropertyObject),
    pub stored_metadata: (ExGuid, PropertyObject),
}

/// Page moves and removals on a section kept open: the objects each changed space stores.
pub(crate) fn section_changes(
    section: &mut crate::Section<'_>,
    edits: &[PageEdit],
    removals: &[ExGuid],
) -> Result<Vec<(ExGuid, crate::active::Changes)>, Error> {
    let root = section.root();
    let (view, raw) = {
        let page = section.active(root)?;
        (page.view.clone(), page.live.revision.clone())
    };
    let section_id = *view
        .roots
        .get(&1)
        .ok_or_else(|| invalid("Section root is unavailable"))?;
    let pages: Vec<ExGuid> = view.nodes[&section_id]
        .children
        .iter()
        .flat_map(|series| view.nodes[series].spaces.clone())
        .collect();
    let section = &*section;
    let parts = |sid: ExGuid| -> Result<PageParts<'_>, Error> {
        let revision = section.revision(sid)?;
        let (manifest, stored) = (revision.roots[&1], revision.roots[&2]);
        let element = |id| {
            crate::document::Element::parse_with(
                &revision.objects[id],
                crate::FileType::Section,
                &mut |_| Err(invalid("Page metadata holds no payload")),
            )
        };
        if element(&manifest)?.content.len() != 1 {
            return Err(invalid(
                "Each page space must contain one page with a valid level",
            ));
        }
        Ok(PageParts {
            metadata: element(&stored)?,
            manifest: (
                manifest,
                PropertyObject::from_object(&revision.objects[&manifest])?,
            ),
            stored_metadata: (
                stored,
                PropertyObject::from_object(&revision.objects[&stored])?,
            ),
        })
    };
    Ok(
        page_changes(root, &view, &raw, &pages, parts, edits, removals)?
            .unwrap_or_default()
            .into_iter()
            .filter(|(_, changes)| !changes.is_empty())
            .collect(),
    )
}

/// The objects page moves, indentation and removals change in each space; none when the
/// order and levels stay.
fn page_changes<'a>(
    root: ExGuid,
    view: &crate::document::Revision<'_>,
    raw: &crate::ResolvedRevision<'_>,
    pages: &[ExGuid],
    parts: impl Fn(ExGuid) -> Result<PageParts<'a>, Error>,
    edits: &[PageEdit],
    removals: &[ExGuid],
) -> Result<Option<BTreeMap<ExGuid, crate::active::Changes>>, Error> {
    if !edits.is_empty() && !removals.is_empty() {
        return Err(invalid(
            "Choose page placement or page removal for one operation",
        ));
    }
    let removed: BTreeSet<_> = removals.iter().copied().collect();
    if removed.len() != removals.len() || removed.iter().any(|sid| sid.guid == [0; 16]) {
        return Err(invalid("Choose distinct existing page spaces for removal"));
    }
    let mut selected = BTreeMap::new();
    let mut guids = BTreeSet::new();
    for edit in edits {
        edit.validate()?;
        if selected.insert(edit.space, edit).is_some() || !guids.insert(edit.guid) {
            return Err(invalid(
                "Use distinct page and edit identities within a batch",
            ));
        }
    }
    let section_id = view.roots[&1];
    let section_node = &view.nodes[&section_id];
    if !matches!(section_node.kind, Kind::Section { .. })
        || section_node.extra[0]
            .iter()
            .any(|field| field.id == 0x88001cde)
    {
        return Err(invalid("Choose an editable section for page editing"));
    }
    let mut order = Vec::new();
    let mut levels = BTreeMap::new();
    let mut page_guids = BTreeMap::new();
    let mut stored = BTreeMap::new();
    for sid in pages {
        let page = parts(*sid)?;
        let Kind::Metadata { level, .. } = page.metadata.kind else {
            return Err(invalid("Choose ordinary pages with page metadata"));
        };
        let level = level.unwrap_or(1);
        if !(1..=3).contains(&level) || levels.insert(*sid, level).is_some() {
            return Err(invalid(
                "Each page space must contain one page with a valid level",
            ));
        }
        if page_guids
            .insert(metadata_guid(&page.metadata)?, *sid)
            .is_some()
        {
            return Err(invalid("Pages must have distinct metadata identifiers"));
        }
        order.push(*sid);
        stored.insert(*sid, page);
    }
    let original_order = order.clone();
    let original_levels = levels.clone();
    let mut series_by_head = BTreeMap::new();
    let mut copies = BTreeMap::new();
    let mut copy_ids = BTreeSet::new();
    let mut stale = BTreeSet::new();
    for oid in &section_node.children {
        let series = &view.nodes[oid];
        // OneNote leaves the losing side's series empty after a merge; the rewrite drops it.
        let Some(head) = series.spaces.first() else {
            continue;
        };
        if levels.get(head) != Some(&1)
            || series.spaces[1..]
                .iter()
                .any(|sid| levels.get(sid).is_none_or(|level| *level == 1))
        {
            return Err(invalid(
                "A page series must start with its only top-level page",
            ));
        }
        if let Some(retained) = series.spaces.iter().find(|sid| !removed.contains(sid)) {
            series_by_head.insert(*retained, *oid);
        }
        if let Some(field) = series.extra[0].iter().find(|field| field.id == 0x24003442) {
            let FieldValue::Objects(ids) = &field.value else {
                return Err(invalid("Page metadata copies must be object references"));
            };
            // OneNote can leave a page's former copy beside its new one (MS-ONE 2.2.81, note
            // 9): the last copy of each page stays and the series is rewritten to match.
            let mismatched = ids.len() != series.spaces.len();
            if mismatched {
                stale.insert(*oid);
            }
            for id in ids {
                if !copy_ids.insert(*id)
                    || !matches!(
                        view.nodes.get(id).map(|n| &n.kind),
                        Some(Kind::Metadata { .. })
                    )
                {
                    return Err(invalid("Each page needs its own ordinary metadata copy"));
                }
                let sid = page_guids
                    .get(&metadata_guid(&view.nodes[id])?)
                    .filter(|sid| series.spaces.contains(sid))
                    .ok_or_else(|| invalid("Metadata copy must identify a page in its series"))?;
                if copies.insert(*sid, *id).is_some() && !mismatched {
                    return Err(invalid("Each page needs its own ordinary metadata copy"));
                }
            }
        }
    }
    for sid in &removed {
        if !levels.contains_key(sid) {
            return Err(invalid("The selected page is no longer in the section"));
        }
    }
    order.retain(|sid| !removed.contains(sid));
    if !removed.is_empty()
        && let Some(first) = order.first()
    {
        levels.insert(*first, 1);
    }
    for edit in edits {
        let Some(at) = order.iter().position(|sid| *sid == edit.space) else {
            return Err(invalid("The selected page is no longer in the section"));
        };
        levels.insert(edit.space, edit.level);
        if let PagePosition::Before(before) = edit.position {
            order.remove(at);
            let position = match before {
                None => order.len(),
                Some(before) => order
                    .iter()
                    .position(|sid| *sid == before)
                    .ok_or_else(|| invalid("The movement anchor is no longer in the section"))?,
            };
            order.insert(position, edit.space);
        }
    }
    if order.first().is_some_and(|sid| levels[sid] != 1) {
        return Err(invalid(
            "The section's first page must have indentation level 1",
        ));
    }
    if order == original_order && levels == original_levels {
        return Ok(None);
    }
    let mut groups: Vec<Vec<ExGuid>> = Vec::new();
    for sid in order {
        if levels[&sid] == 1 {
            groups.push(Vec::new());
        }
        groups.last_mut().unwrap().push(sid);
    }
    let mut changes = BTreeMap::new();
    for sid in &removed {
        let page = &stored[sid];
        let (manifest_id, mut manifest) = page.manifest.clone();
        if manifest.jcid != 0x60037 {
            return Err(invalid("Choose ordinary pages for removal"));
        }
        manifest.bytes = properties(&[])?;
        let (metadata_id, mut metadata) = page.stored_metadata.clone();
        metadata.set(&[(0x88001de9, &[])])?;
        changes.insert(
            *sid,
            BTreeMap::from([(manifest_id, manifest), (metadata_id, metadata)]),
        );
    }
    let mut replacements = BTreeMap::new();
    let mut children = Vec::new();
    for spaces in groups {
        let head = spaces[0];
        // A moved page heads a series of its own, as OneNote's drag gives one.
        let moved = selected
            .get(&head)
            .is_some_and(|edit| matches!(edit.position, PagePosition::Before(_)));
        let old_id = series_by_head.get(&head).copied().filter(|_| !moved);
        let (id, mut series) = if let Some(id) = old_id {
            (id, PropertyObject::from_object(&raw.objects[&id])?)
        } else {
            let edit = selected
                .get(&head)
                .ok_or_else(|| invalid("A new series must start at an edited page"))?;
            let id = ExGuid {
                guid: edit.guid,
                n: 1,
            };
            if raw.objects.contains_key(&id) {
                return Err(invalid("The new page-series identity already exists"));
            }
            (
                id,
                PropertyObject {
                    jcid: 0x60008,
                    bytes: properties(&[(0x1c001c30, edit.guid.to_vec())])?,
                    global_ids: Arc::new(BTreeMap::from([(0, edit.guid)])),
                },
            )
        };
        children.push(id);
        let membership_changed =
            old_id.is_none_or(|old| view.nodes[&old].spaces != spaces || stale.contains(&old));
        if !membership_changed
            && !spaces.iter().any(|sid| {
                levels[sid] != original_levels[sid]
                    || copies.get(sid).is_some_and(|id| {
                        matches!(view.nodes[id].kind, Kind::Metadata { level, .. }
                                if level.unwrap_or(1) != levels[sid])
                    })
            })
        {
            continue;
        }
        let mut metadata_ids = Vec::new();
        for sid in &spaces {
            let (page_id, original) = stored[sid].stored_metadata.clone();
            if old_id.is_none() && *sid == head {
                series.copy_property(&original, 0x18001c65)?;
            }
            if levels[sid] != original_levels[sid] {
                let mut metadata = original.clone();
                metadata.set(&[(0x14001dff, &levels[sid].to_le_bytes())])?;
                changes.insert(*sid, BTreeMap::from([(page_id, metadata)]));
            }
            let copy_id = copies
                .get(sid)
                .copied()
                .unwrap_or_else(|| metadata_id(*sid));
            let mut copy = match raw.objects.get(&copy_id) {
                Some(object) => {
                    if object.jcid != 0x20030
                        || (!copies.contains_key(sid) && copy_ids.contains(&copy_id))
                    {
                        return Err(invalid("Page metadata identities overlap"));
                    }
                    PropertyObject::from_object(object)?
                }
                None => original,
            };
            copy.set(&[(0x14001dff, &levels[sid].to_le_bytes())])?;
            copy.reference(copy_id)?;
            if replacements.insert(copy_id, copy).is_some() {
                return Err(invalid("Page metadata identities overlap"));
            }
            metadata_ids.push(copy_id);
        }
        if membership_changed || spaces.iter().any(|sid| !copies.contains_key(sid)) {
            set_references(&mut series, 0x2c001d63, &spaces)?;
            set_references(&mut series, 0x24003442, &metadata_ids)?;
            if replacements.insert(id, series).is_some() {
                return Err(invalid("Page-series and metadata identities overlap"));
            }
        }
    }
    if children != section_node.children {
        let mut parent = PropertyObject::from_object(&raw.objects[&section_id])?;
        set_references(&mut parent, 0x24001c20, &children)?;
        replacements.insert(section_id, parent);
    }
    changes.insert(root, replacements);
    Ok(Some(changes))
}

/// A page creation's changes to the root space, and the new space's roots and objects.
pub(crate) type Creation = (
    crate::active::Changes,
    BTreeMap<u32, ExGuid>,
    crate::active::Changes,
);

/// An empty top-level page with stable identities and creation time.
/// Retain the intent across retries; an existing page identity rejects duplicate creation.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct PageCreation {
    guid: [u8; 16],
    series_guid: [u8; 16],
    before: Option<ExGuid>,
    title: Option<String>,
    author: String,
    created: u32,
    /// The title's date and time fields' text, for a titled page.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    date: Option<[String; 2]>,
    /// The page identity and creation time (FILETIME) a page keeps from elsewhere.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    kept: Option<([u8; 16], u64)>,
}

impl PageCreation {
    /// Appends a page, or inserts before the first page space of an existing series.
    /// `Some("")` creates an empty title field; `None` creates a page without a title node.
    /// The page has no body outlines or generated date/time text and does not apply a template.
    pub fn new(before: Option<ExGuid>, title: Option<&str>, author: &str) -> Result<Self, Error> {
        let page = Self {
            guid: fresh_guid()?,
            series_guid: fresh_guid()?,
            before,
            title: title.map(str::to_owned),
            author: author.to_owned(),
            created: current_timestamps()?.0,
            date: None,
            kept: None,
        };
        page.validate()?;
        Ok(page)
    }

    /// Gives a titled page's title OneNote 2010's date and time fields, showing `date` and
    /// `time`: the creation time (`created`) as the user's long date and short time.
    pub fn dated(mut self, date: &str, time: &str) -> Result<Self, Error> {
        if self.title.is_none() {
            return Err(invalid("Only a titled page shows its date"));
        }
        self.date = Some([date.to_owned(), time.to_owned()]);
        self.validate()?;
        Ok(self)
    }

    /// Keeps `identity`, the page identity internal links name, and `created`, as OneNote
    /// 2010 keeps both for a page it moves to the notebook's recycle bin.
    pub fn keeping(mut self, identity: [u8; 16], created: u64) -> Result<Self, Error> {
        if identity == [0; 16] {
            return Err(invalid("Keep an existing page identity"));
        }
        let seconds = (created / 10_000_000)
            .checked_sub(11_644_473_600 + 315_532_800)
            .and_then(|seconds| u32::try_from(seconds).ok())
            .ok_or_else(|| invalid("The kept creation time is outside OneNote's range"))?;
        self.created = seconds;
        self.kept = Some((identity, created));
        Ok(self)
    }

    /// Who creates the page.
    pub fn author(&self) -> &str {
        &self.author
    }

    /// When the page was created, FILETIME, which its date and time fields show.
    pub fn created(&self) -> u64 {
        match self.kept {
            Some((_, created)) => created,
            None => (u64::from(self.created) + 315_532_800 + 11_644_473_600) * 10_000_000,
        }
    }

    /// The page identity internal links name as `page-id`.
    fn identity(&self) -> [u8; 16] {
        self.kept.map_or(self.guid, |(identity, _)| identity)
    }

    pub fn space(&self) -> ExGuid {
        ExGuid {
            guid: self.guid,
            n: 1,
        }
    }

    pub fn object(&self) -> ExGuid {
        ExGuid {
            guid: self.guid,
            n: 12,
        }
    }

    pub fn title_object(&self) -> Option<ExGuid> {
        self.title.as_ref().map(|_| ExGuid {
            guid: self.guid,
            n: 16,
        })
    }

    /// The page the new page goes before; `None` appends it.
    pub fn before(&self) -> Option<ExGuid> {
        self.before
    }

    /// Changes the insertion anchor while retaining all identities and creation metadata.
    pub fn reposition(&self, before: Option<ExGuid>) -> Result<Self, Error> {
        let mut page = self.clone();
        page.before = before;
        page.validate()?;
        Ok(page)
    }

    fn validate(&self) -> Result<(), Error> {
        if self.guid == [0; 16]
            || self.series_guid == [0; 16]
            || self.guid == self.series_guid
            || self.author.contains('\0')
            || self.before.is_some_and(|id| id.guid == [0; 16])
            || self
                .title
                .iter()
                .chain(self.date.iter().flatten())
                .any(|text| text.contains(['\0', '\r', '\n', '\u{fffc}', '\u{fddf}']))
        {
            return Err(invalid(
                "Use a new page identity, an existing page anchor, and ordinary single-line title text",
            ));
        }
        Ok(())
    }

    /// The root space's changes creating this page, and the new space's roots and objects.
    pub(crate) fn changes(&self, section: &mut crate::Section<'_>) -> Result<Creation, Error> {
        self.validate()?;
        if section.revision(self.space()).is_ok() {
            return Err(invalid(
                "This page identity already exists; reconcile the original creation",
            ));
        }
        let root = section.root();
        let page = section.active(root)?;
        self.creation(&page.view, &page.live.revision)
    }

    /// The object space of a conflict page this creation makes, as OneNote 2010 stores one:
    /// the page's space with `jcidConflictPageMetaData` (MS-ONE 2.2.35) as its metadata,
    /// naming this creation's author as the conflicting user and `title` as the page's.
    /// Returns the roots, the objects and the metadata, which the conflicting page's
    /// manifest also keeps a copy of.
    pub(crate) fn conflict(
        &self,
        section: &mut crate::Section<'_>,
        title: &str,
        level: u32,
    ) -> Result<
        (
            BTreeMap<u32, ExGuid>,
            crate::active::Changes,
            PropertyObject,
        ),
        Error,
    > {
        if self.before.is_some() {
            return Err(invalid("A conflict page is not placed in the page list"));
        }
        let (_, roots, mut objects) = self.changes(section)?;
        let timestamp = self.created();
        let initials = crate::create::initials(&self.author);
        let metadata = PropertyObject {
            jcid: 0x20038,
            bytes: properties(&[
                (0x1c001cf3, string(title)),
                (0x1c001c30, self.identity().to_vec()),
                (0x14001dff, level.to_le_bytes().to_vec()),
                (0x14001d82, 40_u32.to_le_bytes().to_vec()),
                (0x1400348b, 40_u32.to_le_bytes().to_vec()),
                (0x18001c65, timestamp.to_le_bytes().to_vec()),
                (0x1c001d9e, string(&self.author)),
                (0x1c001d9f, string(&initials)),
            ])?,
            global_ids: Arc::new(BTreeMap::from([(0, self.guid)])),
        };
        objects.insert(roots[&2], metadata.clone());
        Ok((roots, objects, metadata))
    }

    /// The title's second outline as OneNote 2010 writes it for a new page (object numbers,
    /// jcids and properties): a read-only outline of two paragraphs, the date then the
    /// time, in the `PageDateTime` style.
    fn date_fields(&self, date: &str, time: &str) -> Vec<(u32, u32, crate::op::Values)> {
        let reference = |n: u32| n.to_le_bytes().to_vec();
        let modified = || (0x14001d7a, self.created.to_le_bytes().to_vec());
        let yes = |id: u32| (id | 1 << 31, Vec::new());
        let paragraph = |text: u32| {
            vec![
                modified(),
                (0x20001d79, reference(17)),
                (0x20001d78, reference(17)),
                (0x14001d09, self.created.to_le_bytes().to_vec()),
                (0x0c001c03, vec![1]),
                (0x24001c1f, reference(text)),
                yes(0x08001cb2),
                yes(0x08001d0c),
                (0x08001c34, Vec::new()),
            ]
        };
        let text = |value: &str, role: u32| {
            // OneNote keeps Latin-1 text as TextExtendedAscii, other text as Unicode.
            let content = match value
                .chars()
                .map(|c| u8::try_from(c).ok())
                .collect::<Option<Vec<u8>>>()
            {
                Some(latin) => (0x1c003498, latin),
                None => (0x1c001c22, string(value)),
            };
            vec![
                modified(),
                (0x2000342c, reference(23)),
                yes(0x08001cde),
                yes(0x08001c00),
                yes(0x08001c88),
                yes(0x08001d0c),
                yes(role),
                (0x10001cfe, 0x409_u16.to_le_bytes().to_vec()),
                (0x1c001cc8, vec![0, 0, 0, 0, 3]),
                content,
                (0x24001e13, reference(19)),
                yes(0x080034dd),
            ]
        };
        let no = |id: u32| (id, Vec::new());
        vec![
            (
                20,
                0x6000c,
                vec![
                    modified(),
                    (0x24001c20, [reference(21), reference(24)].concat()),
                    (
                        0x1c001c12,
                        measurement_bytes(&NATIVE_INDENTS, 4).expect("Native indents encode"),
                    ),
                    (0x0c001c13, vec![0]),
                    (0x0c001c03, vec![1]),
                    (0x14001c1c, 0.6_f32.to_le_bytes().to_vec()),
                    yes(0x08001cb5),
                    yes(0x08001c00),
                    yes(0x08001cb2),
                    yes(0x08001cde),
                    yes(0x08001d0c),
                    (0x14001c3e, reference(0)),
                    (0x14001c84, reference(0xc)),
                ],
            ),
            (21, 0x6000d, paragraph(22)),
            (22, 0x6000e, text(date, 0x08001cb5)),
            (
                23,
                0x12004d,
                vec![
                    (0x1c00345a, string("PageDateTime")),
                    no(0x08001c04),
                    no(0x08001c05),
                    no(0x08001c06),
                    (0x14001c0c, 0x0080_8080_u32.to_le_bytes().to_vec()),
                    (0x1c001c0a, string("Calibri")),
                    (0x10001c0b, 20_u16.to_le_bytes().to_vec()),
                    (0x1400342e, 0_f32.to_le_bytes().to_vec()),
                    (0x1400342f, 0_f32.to_le_bytes().to_vec()),
                    (0x14003430, 0_f32.to_le_bytes().to_vec()),
                    no(0x08001c07),
                    (0x14001c0d, 0xff00_0000_u32.to_le_bytes().to_vec()),
                    no(0x08001c09),
                    no(0x08001c08),
                ],
            ),
            (24, 0x6000d, paragraph(25)),
            (25, 0x6000e, text(time, 0x08001c87)),
        ]
    }

    fn creation(
        &self,
        view: &crate::document::Revision<'_>,
        raw: &crate::ResolvedRevision<'_>,
    ) -> Result<Creation, Error> {
        {
            let section_id = view.roots[&1];
            let section_node = &view.nodes[&section_id];
            if !matches!(section_node.kind, Kind::Section { .. })
                || section_node.extra[0]
                    .iter()
                    .any(|field| field.id == 0x88001cde)
            {
                return Err(invalid("Choose an editable section for the new page"));
            }
            let position = match self.before {
                None => section_node.children.len(),
                Some(before) => section_node
                    .children
                    .iter()
                    .position(|id| view.nodes[id].spaces.first() == Some(&before))
                    .ok_or_else(|| {
                        invalid("Insert before the first page of an existing series, or append")
                    })?,
            };
            let id = |n| ExGuid { guid: self.guid, n };
            let reference = |n: u32| n.to_le_bytes().to_vec();
            let timestamp = self.created().to_le_bytes().to_vec();
            let modified = || (0x14001d7a, self.created.to_le_bytes().to_vec());
            let mut table = BTreeMap::from([(0, self.guid)]);
            let metadata_id = metadata_id(self.space());
            table.insert(1, metadata_id.guid);
            let table = Arc::new(table);
            let object = |jcid, values: Vec<_>| -> Result<_, Error> {
                Ok(PropertyObject {
                    jcid,
                    bytes: properties(&values)?,
                    global_ids: Arc::clone(&table),
                })
            };
            let title = self.title.as_deref().unwrap_or_default().trim_start();
            let metadata = vec![
                (0x1c001c30, self.identity().to_vec()),
                (0x1c001cf3, string(title)),
                (0x14001d82, reference(40)),
                (0x1400348b, reference(40)),
                (0x14001dff, reference(1)),
                (0x18001c65, timestamp.clone()),
            ];
            let mut page = [
                vec![
                    modified(),
                    (0x1c001d75, string(&self.author)),
                    (0x1c001df8, string(&crate::create::initials(&self.author))),
                    (0x1c001d3c, string("")),
                ],
                crate::create::page_margins(),
            ]
            .concat();
            if self.title.is_some() {
                page.push((0x24001d5f, reference(13)));
            }
            let mut objects = BTreeMap::from([
                (id(10), object(0x60037, vec![(0x24001c1f, reference(12))])?),
                (id(11), object(0x20030, metadata.clone())?),
                (id(12), object(0x6000b, page)?),
            ]);
            if let Some(title) = &self.title {
                for (n, jcid, values) in [
                    (
                        13,
                        0x6002c,
                        vec![
                            modified(),
                            (
                                0x24001c20,
                                [
                                    reference(14),
                                    self.date.iter().flat_map(|_| reference(20)).collect(),
                                ]
                                .concat(),
                            ),
                            (0x14001c14, 0_f32.to_le_bytes().to_vec()),
                            (0x14001c15, 0_f32.to_le_bytes().to_vec()),
                            (0x14001c3e, reference(0x0009_000c)),
                            (0x14001c84, reference(0)),
                            (0x14001cf1, reference(0)),
                        ],
                    ),
                    (
                        14,
                        0x6000c,
                        vec![
                            modified(),
                            (0x24001c20, reference(15)),
                            (0x1c001c12, measurement_bytes(&NATIVE_INDENTS, 4)?),
                            (0x0c001c13, vec![0]),
                            (0x0c001c03, vec![1]),
                            (0x14001c1c, 0.6_f32.to_le_bytes().to_vec()),
                            (0x88001cf9, vec![]),
                            (0x88001cb2, vec![]),
                            (0x88001cb4, vec![]),
                            (0x88001c91, vec![]),
                            (0x14001cec, 4.5_f32.to_le_bytes().to_vec()),
                            (0x88001cff, vec![]),
                            (0x14001c3e, reference(0)),
                            (0x14001c84, reference(0xc)),
                        ],
                    ),
                    (
                        15,
                        0x6000d,
                        vec![
                            modified(),
                            (0x24001c1f, reference(16)),
                            (0x0c001c03, vec![1]),
                            (0x20001d78, reference(17)),
                            (0x20001d79, reference(17)),
                            (0x14001d09, self.created.to_le_bytes().to_vec()),
                            (0x88001cb2, vec![]),
                            (0x88001cb4, vec![]),
                        ],
                    ),
                    (
                        16,
                        0x6000e,
                        vec![
                            modified(),
                            (0x1c001c22, string(title)),
                            (0x24001e13, reference(19)),
                            (0x2000342c, reference(18)),
                            (0x10001cfe, 0x409_u16.to_le_bytes().to_vec()),
                            (0x88001cb4, vec![]),
                        ],
                    ),
                    (17, 0x120001, crate::create::author_properties(&self.author)),
                    (
                        18,
                        0x12004d,
                        vec![
                            (0x1c00345a, string("PageTitle")),
                            (0x1c001c0a, string("Calibri")),
                            (0x10001c0b, 34_u16.to_le_bytes().to_vec()),
                        ],
                    ),
                    (19, 0x12004d, vec![(0x14001c3b, reference(0x409))]),
                ] {
                    objects.insert(id(n), object(jcid, values)?);
                }
                if let Some([date, time]) = &self.date {
                    for (n, jcid, values) in self.date_fields(date, time) {
                        objects.insert(id(n), object(jcid, values)?);
                    }
                }
            }
            let series = object(
                0x60008,
                vec![
                    (0x1c001c30, self.series_guid.to_vec()),
                    (0x18001c65, timestamp),
                    (0x2c001d63, reference(1)),
                    (0x24003442, reference(257)),
                ],
            )?;
            if raw.objects.contains_key(&id(2)) || raw.objects.contains_key(&metadata_id) {
                return Err(invalid("The new page's section identities already exist"));
            }
            let mut parent = PropertyObject::from_object(&raw.objects[&section_id])?;
            let mut children = section_node.children.clone();
            children.insert(position, id(2));
            let mut references = Vec::new();
            for child in children {
                references.extend_from_slice(&parent.reference(child)?);
            }
            parent.set(&[(0x24001c20, &references)])?;
            Ok((
                BTreeMap::from([
                    (section_id, parent),
                    (id(2), series),
                    (metadata_id, object(0x20030, metadata)?),
                ]),
                BTreeMap::from([(1, id(10)), (2, id(11))]),
                objects,
            ))
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{
        RevisionIndex, Store,
        document::Document,
        op::{Edit, Op, OpError, SectionOp},
        write::{RevisionEdit, write_revisions},
    };

    /// Page moves and removals through a section, sealed: the image they leave.
    fn edit_pages(
        source: &[u8],
        edits: &[PageEdit],
        removals: &[ExGuid],
    ) -> Result<Vec<u8>, OpError> {
        let arena = crate::Arena::default();
        let mut section = crate::Section::open(&arena, source.to_vec()).map_err(OpError::Failed)?;
        let mut ops = vec![Op::Section(SectionOp::Pages(edits.to_vec()))];
        if !removals.is_empty() {
            ops.push(Op::Section(SectionOp::Delete(removals.to_vec())));
        }
        section.apply(
            "Author",
            &Edit {
                at: 133_700_000_000_000_000,
                ops,
            },
        )?;
        section.seal().map_err(OpError::Failed)?;
        Ok(section.image())
    }

    #[test]
    fn page_movement_restores_an_optional_metadata_cache_from_history() {
        let source =
            include_bytes!("../../../corpus/page-lifecycle/04-nested/notebook/Lifecycle.one");
        let store = Store::parse(source).unwrap();
        let index = RevisionIndex::parse(&store).unwrap();
        let document = Document::parse(&index).unwrap();
        let sid = document.pages().unwrap()[3].0;
        let view = document.active(document.root).unwrap();
        let series_id = view.nodes[&view.roots[&1]].children[3];
        let copy_id = metadata_id(sid);
        for unknown in [false, true] {
            let source = if unknown {
                write_revisions(source, |index| {
                    let raw = index.resolve_active(index.root)?;
                    let mut copy = PropertyObject::from_object(&raw.objects[&copy_id])?;
                    copy.set(&[(0x1400abcd, &77_u32.to_le_bytes())])?;
                    Ok(BTreeMap::from([(
                        index.root,
                        RevisionEdit::Update(BTreeMap::from([(copy_id, copy)])),
                    )]))
                })
                .unwrap()
            } else {
                source.to_vec()
            };
            let source = write_revisions(&source, |index| {
                let raw = index.resolve_active(index.root)?;
                let mut series = PropertyObject::from_object(&raw.objects[&series_id])?;
                series.remove(&[0x24003442])?;
                Ok(BTreeMap::from([(
                    index.root,
                    RevisionEdit::Update(BTreeMap::from([(series_id, series)])),
                )]))
            })
            .unwrap();
            for changed in [false, true] {
                for unrelated_edit in [false, true] {
                    let result = write_revisions(&source, |index| {
                        let raw = index.resolve_active(index.root)?;
                        assert!(!raw.reachable()?.contains(&copy_id));
                        let mut copy = PropertyObject::from_object(&raw.objects[&copy_id])?;
                        if changed {
                            copy.set(&[(0x14001dff, &2_u32.to_le_bytes())])?;
                        }
                        let mut objects = BTreeMap::from([(copy_id, copy)]);
                        if unrelated_edit {
                            let mut series = PropertyObject::from_object(&raw.objects[&series_id])?;
                            series.set(&[(0x1400abcd, &88_u32.to_le_bytes())])?;
                            objects.insert(series_id, series);
                        }
                        Ok(BTreeMap::from([(
                            index.root,
                            RevisionEdit::Update(objects),
                        )]))
                    });
                    assert_eq!(
                        result.unwrap_err().message,
                        "Edited object is not reachable in the resulting revision"
                    );
                }
            }
            let written =
                edit_pages(&source, &[PageEdit::set_level(sid, 2).unwrap()], &[]).unwrap();
            let store = Store::parse(&written).unwrap();
            let index = RevisionIndex::parse(&store).unwrap();
            index.validate_current().unwrap();
            assert_eq!(Document::parse(&index).unwrap().pages().unwrap().len(), 9);
            let current = index
                .resolve(
                    index.root,
                    index.spaces[&index.root].labels[&(ExGuid::default(), 1)],
                )
                .unwrap();
            assert!(current.reachable().unwrap().contains(&copy_id));
            let copy = PropertyObject::from_object(&current.objects[&copy_id]).unwrap();
            let properties = crate::PropertySets::parse(&copy.bytes).unwrap();
            assert_eq!(unknown, properties.sets[0].iter().any(|field| field.id == 0x1400abcd
            && matches!(field.value, crate::Value::Bytes(bytes) if bytes == 77_u32.to_le_bytes())));
            let old_store = Store::parse(&source).unwrap();
            let old_index = RevisionIndex::parse(&old_store).unwrap();
            for (space, revisions) in &old_index.spaces {
                for revision in revisions.revisions.keys() {
                    assert_eq!(
                        format!("{:?}", old_index.resolve(*space, *revision).unwrap()),
                        format!("{:?}", index.resolve(*space, *revision).unwrap())
                    );
                }
            }
            if !unknown && let Some(output) = std::env::var_os("ONESTORE_PAGE_MOVEMENT_OUTPUT") {
                let output = std::path::Path::new(&output);
                assert!(output.is_absolute());
                for (name, bytes) in [
                    ("optional-cache-source", source),
                    ("optional-cache", written),
                ] {
                    let path = output.join(name);
                    std::fs::create_dir_all(&path).unwrap();
                    std::fs::write(path.join("Lifecycle.one"), bytes).unwrap();
                }
            }
        }
    }

    #[test]
    fn page_movement_rejects_empty_manifests_and_series() {
        let source =
            include_bytes!("../../../corpus/page-lifecycle/04-nested/notebook/Lifecycle.one");
        let store = Store::parse(source).unwrap();
        let index = RevisionIndex::parse(&store).unwrap();
        let document = Document::parse(&index).unwrap();
        let sid = document.pages().unwrap()[3].0;
        for empty_series in [false, true] {
            let invalid_source = write_revisions(source, |index| {
                let target = if empty_series { index.root } else { sid };
                let raw = index.resolve_active(target)?;
                let id = if empty_series {
                    let view = document.active(document.root).unwrap();
                    view.nodes[&view.roots[&1]].children[3]
                } else {
                    raw.roots[&1]
                };
                let mut object = PropertyObject::from_object(&raw.objects[&id])?;
                set_references(
                    &mut object,
                    if empty_series { 0x2c001d63 } else { 0x24001c1f },
                    &[],
                )?;
                Ok(BTreeMap::from([(
                    target,
                    RevisionEdit::Update(BTreeMap::from([(id, object)])),
                )]))
            })
            .unwrap();
            assert!(
                edit_pages(
                    &invalid_source,
                    &[PageEdit::set_level(sid, 2).unwrap()],
                    &[]
                )
                .is_err()
            );
        }
    }

    #[test]
    fn page_moves_retain_unknown_series_and_metadata_properties() {
        let source =
            include_bytes!("../../../corpus/page-lifecycle/04-nested/notebook/Lifecycle.one");
        let store = Store::parse(source).unwrap();
        let index = RevisionIndex::parse(&store).unwrap();
        let document = Document::parse(&index).unwrap();
        let pages = document.pages().unwrap();
        let view = document.active(document.root).unwrap();
        let series_id = view.nodes[&view.roots[&1]].children[3];
        let field = view.nodes[&series_id].extra[0]
            .iter()
            .find(|f| f.id == 0x24003442)
            .unwrap();
        let FieldValue::Objects(copies) = &field.value else {
            panic!()
        };
        let copy_id = copies[0];
        let source = write_revisions(source, |index| {
            let raw = index.resolve_active(index.root)?;
            let mut objects = BTreeMap::new();
            for id in [series_id, copy_id] {
                let mut object = PropertyObject::from_object(&raw.objects[&id])?;
                object.set(&[(0x1400abcd, &77_u32.to_le_bytes())])?;
                objects.insert(id, object);
            }
            Ok(BTreeMap::from([(
                index.root,
                RevisionEdit::Update(objects),
            )]))
        })
        .unwrap();
        for edits in [
            vec![PageEdit::set_level(pages[3].0, 2).unwrap()],
            pages[3..6]
                .iter()
                .enumerate()
                .map(|(i, (sid, _))| PageEdit::move_to(*sid, None, i as u32 + 1).unwrap())
                .collect(),
        ] {
            let written = edit_pages(&source, &edits, &[]).unwrap();
            let old_store = Store::parse(&source).unwrap();
            let new_store = Store::parse(&written).unwrap();
            let old = RevisionIndex::parse(&old_store).unwrap();
            let new = RevisionIndex::parse(&new_store).unwrap();
            let old_id = old.spaces[&old.root].labels[&(ExGuid::default(), 1)];
            let new_id = new.spaces[&new.root].labels[&(ExGuid::default(), 1)];
            assert_eq!(
                format!("{:?}", old.resolve(old.root, old_id).unwrap()),
                format!("{:?}", new.resolve(new.root, old_id).unwrap())
            );
            let current = new.resolve(new.root, new_id).unwrap();
            for id in [series_id, copy_id] {
                let object = PropertyObject::from_object(&current.objects[&id]).unwrap();
                let properties = crate::PropertySets::parse(&object.bytes).unwrap();
                assert!(properties.sets[0].iter().any(|field| field.id == 0x1400abcd
                    && matches!(field.value, crate::Value::Bytes(bytes) if bytes == 77_u32.to_le_bytes())));
            }
        }
    }
}
