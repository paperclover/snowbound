use onestore::{
    Arena, ConflictPage, ExGuid, ObjectData, PageCreation, PropertySets, RevisionIndex, Section,
    Store, Value,
    op::{Edit, Op, PageOp, SectionOp},
    page::{Page, PageObject},
};
use std::collections::BTreeMap;

const AT: u64 = 134_030_000_000_000_000;

fn read(path: &str) -> Vec<u8> {
    std::fs::read(format!("{}/../../{path}", env!("CARGO_MANIFEST_DIR"))).unwrap()
}

/// Each object's jcid and root-set property identities, by space and object.
fn objects(image: &[u8]) -> BTreeMap<ExGuid, BTreeMap<ExGuid, (u32, Vec<u32>)>> {
    let store = Store::parse(image).unwrap();
    let index = RevisionIndex::parse(&store).unwrap();
    index
        .spaces
        .keys()
        .map(|space| {
            let revision = index.resolve_active(*space).unwrap();
            let objects = revision
                .objects
                .iter()
                .map(|(id, object)| {
                    let ids = match object.data {
                        ObjectData::Properties(bytes) => PropertySets::parse(bytes).unwrap().sets
                            [0]
                        .iter()
                        .map(|property| property.id)
                        .collect(),
                        _ => Vec::new(),
                    };
                    (*id, (object.jcid, ids))
                })
                .collect();
            (*space, objects)
        })
        .collect()
}

/// The object-space references a property holds.
fn spaces(image: &[u8], space: ExGuid, object: ExGuid, property: u32) -> usize {
    let store = Store::parse(image).unwrap();
    let index = RevisionIndex::parse(&store).unwrap();
    let revision = index.resolve_active(space).unwrap();
    let ObjectData::Properties(bytes) = revision.objects[&object].data else {
        panic!()
    };
    PropertySets::parse(bytes).unwrap().sets[0]
        .iter()
        .find(|field| field.id == property)
        .map_or(0, |field| match &field.value {
            Value::References { compact_ids, .. } => compact_ids.len() / 4,
            _ => panic!(),
        })
}

fn texts(page: &Page) -> Vec<(ExGuid, String)> {
    page.objects
        .iter()
        .flat_map(|object| match object {
            PageObject::Outline(outline) => outline.paragraphs.clone(),
            PageObject::Title(title) => title
                .outlines
                .iter()
                .flat_map(|outline| outline.paragraphs.clone())
                .collect(),
            _ => Vec::new(),
        })
        .filter_map(|paragraph| {
            paragraph
                .text()
                .map(|text| (text.id, text.text.text().to_owned()))
        })
        .collect()
}

#[test]
fn native_conflict_pages_are_listed_under_their_page_last_first() {
    let arena = Arena::default();
    let mut section = Section::open(
        &arena,
        read("corpus/collaboration/round-01/offline/notebook/synthetic.one"),
    )
    .unwrap();
    let pages = section.pages().unwrap();
    let conflicts = section.conflicts().unwrap();
    assert_eq!(conflicts.len(), 1);
    let (page, listed) = &conflicts[0];
    assert_eq!(*page, pages[0].0);
    let [
        ConflictPage {
            space,
            title,
            user,
            created,
            objects,
        },
    ] = listed.as_slice()
    else {
        panic!("{listed:?}")
    };
    assert_eq!(
        (title.as_str(), user.as_str(), *created),
        (
            "Native edited while disconnected.",
            "snow",
            Some(0x01dd3d10d7afd9e0)
        )
    );
    let conflict = section.page(*space).unwrap();
    let marked: Vec<_> = texts(&conflict)
        .into_iter()
        .filter(|(id, _)| objects.contains(id))
        .map(|(_, text)| text)
        .collect();
    assert_eq!(marked, ["Native edited while disconnected."]);

    // Three clients' conflict pages: OneNote lists the one stored last first.
    let arena = Arena::default();
    let mut section = Section::open(
        &arena,
        read("corpus/conflict-page/native-three/notebook/synthetic.one"),
    )
    .unwrap();
    let users: Vec<_> = section.conflicts().unwrap()[0]
        .1
        .iter()
        .map(|page| page.user.clone())
        .collect();
    assert_eq!(
        users,
        ["ONE-M6-053BD357", "ONE-M6-9451B6C6", "ONE-M6-DDE10E0D"]
    );
}

/// The conflict page a merge makes of `local`, the version of `space`'s page it could not
/// take, marking the text object `conflicting`.
fn conflict(space: ExGuid, local: &Page, conflicting: ExGuid, user: &str) -> Op {
    let titled = local
        .objects
        .iter()
        .any(|object| matches!(object, PageObject::Title(_)));
    let mut objects = [conflicting];
    let page = local.copy_with(&mut objects).unwrap();
    Op::Section(SectionOp::Conflict {
        of: space,
        creation: PageCreation::new(None, titled.then_some(local.title.as_str()), user).unwrap(),
        page,
        objects: objects.to_vec(),
    })
}

/// A conflict page written through ops stores what OneNote 2010 writes for one
/// (`corpus/collaboration/round-01/offline`): its own space under the page's manifest, with
/// conflict metadata, a read-only deletable conflict page and the conflicting text marked.
/// `ONESTORE_CONFLICT_EXPORT` names a directory to write the notebook to, for a cold read.
#[test]
fn a_conflict_page_is_stored_as_onenote_stores_one() {
    let notebook = "corpus/native-ink/cold-ui-ink/notebook";
    let source = read(&format!("{notebook}/synthetic.one"));
    let arena = Arena::default();
    let mut section = Section::open(&arena, source.clone()).unwrap();
    let space = section.pages().unwrap()[0].0;
    let base = section.page(space).unwrap();
    let (text, _) = texts(&base)
        .into_iter()
        .find(|(_, text)| text.starts_with("Fictitious: café"))
        .unwrap();
    // Both machines renamed the paragraph's first word, within its Latin run.
    let replace = |with: &str| Edit {
        at: AT,
        ops: vec![Op::Page {
            space,
            op: PageOp::Text {
                text,
                range: 0..10,
                with: with.into(),
            },
        }],
    };
    section.apply("Remote", &replace("Remote")).unwrap();
    let scratch = Arena::default();
    let mut local = Section::open(&scratch, source).unwrap();
    local.apply("Clover Snow", &replace("Snowbound")).unwrap();
    let local = local.page(space).unwrap();
    let op = conflict(space, &local, text, "Clover Snow");
    let Op::Section(SectionOp::Conflict { creation, .. }) = &op else {
        unreachable!()
    };
    let created = creation.space();
    section
        .apply(
            "Clover Snow",
            &Edit {
                at: AT + 10_000_000,
                ops: vec![op.clone()],
            },
        )
        .unwrap();
    section.seal().unwrap();
    let image = section.image();

    let arena = Arena::default();
    let mut reopened = Section::open(&arena, image.clone()).unwrap();
    assert_eq!(reopened.pages().unwrap().len(), 1);
    let listed = reopened.conflicts().unwrap();
    assert_eq!(listed.len(), 1);
    assert_eq!(listed[0].0, space);
    assert_eq!(
        listed[0].1,
        [ConflictPage {
            space: created,
            title: "Snowbound: café, 東京, مرحبا".into(),
            user: "Clover Snow".into(),
            created: listed[0].1[0].created,
            objects: listed[0].1[0].objects.clone(),
        }]
    );
    let kept = reopened.page(created).unwrap();
    let marked: Vec<_> = texts(&kept)
        .into_iter()
        .filter(|(id, _)| listed[0].1[0].objects.contains(id))
        .map(|(_, text)| text)
        .collect();
    assert_eq!(marked, ["Snowbound: café, 東京, مرحبا"]);
    assert!(
        texts(&reopened.page(space).unwrap())
            .iter()
            .any(|(_, text)| text == "Remote: café, 東京, مرحبا")
    );

    // The stored form, property by property, as OneNote writes it.
    let stored = objects(&image);
    let conflict_space = &stored[&created];
    let store = Store::parse(&image).unwrap();
    let index = RevisionIndex::parse(&store).unwrap();
    let roots = index.resolve_active(created).unwrap().roots;
    let (jcid, metadata) = &conflict_space[&roots[&2]];
    assert_eq!(*jcid, 0x20038);
    assert_eq!(
        metadata,
        &[
            0x1c001cf3, 0x1c001c30, 0x14001dff, 0x14001d82, 0x1400348b, 0x18001c65, 0x1c001d9e,
            0x1c001d9f
        ]
    );
    let page_node = conflict_space
        .values()
        .find(|(jcid, _)| *jcid == 0x6000b)
        .unwrap();
    for flag in [0x88001cde, 0x88001d0c, 0x88001d7c] {
        assert!(page_node.1.contains(&flag), "{flag:#x}");
    }
    let marked: Vec<_> = conflict_space
        .iter()
        .filter(|(_, (_, ids))| ids.contains(&0x88001d96))
        .map(|(_, (jcid, ids))| (*jcid, ids.contains(&0x88001ddb)))
        .collect();
    assert_eq!(marked, [(0x6000e, true)]);
    let main = &stored[&space];
    let main_roots = index.resolve_active(space).unwrap().roots;
    assert_eq!(spaces(&image, space, main_roots[&1], 0x2c001d63), 1);
    assert!(main[&main_roots[&2]].1.contains(&0x88001d97));
    assert_eq!(
        main.values()
            .filter(|(jcid, ids)| *jcid == 0x20038 && ids.contains(&0x1c001d9e))
            .count(),
        1
    );
    assert_eq!(
        stored[&index.root]
            .values()
            .filter(|(jcid, ids)| *jcid == 0x20030 && ids.contains(&0x88001d97))
            .count(),
        1
    );

    if let Some(output) = std::env::var_os("ONESTORE_CONFLICT_EXPORT") {
        let output = std::path::Path::new(&output);
        std::fs::create_dir_all(output).unwrap();
        std::fs::write(output.join("synthetic.one"), &image).unwrap();
        std::fs::copy(
            format!(
                "{}/../../{notebook}/Open Notebook.onetoc2",
                env!("CARGO_MANIFEST_DIR")
            ),
            output.join("Open Notebook.onetoc2"),
        )
        .unwrap();
    }
}

/// Deleting a conflict page stores what OneNote 2010's Delete Conflict Page does
/// (`corpus/conflict-page/native-delete`): the page's manifest stops listing the conflict
/// page, keeping its metadata copy, and the page and the section's copy of its metadata no
/// longer say it has conflict pages; the conflict page's space is left alone.
/// `ONESTORE_CONFLICT_DELETE_EXPORT` names a directory to write the notebook to.
#[test]
fn deleting_a_conflict_page_stores_what_onenote_stores() {
    let notebook = "corpus/conflict-page/native/conflict/notebook";
    let arena = Arena::default();
    let mut section = Section::open(&arena, read(&format!("{notebook}/synthetic.one"))).unwrap();
    let (page, listed) = section.conflicts().unwrap().remove(0);
    let conflict = listed[0].space;
    section
        .apply(
            "Clover Snow",
            &Edit {
                at: AT,
                ops: vec![Op::Section(SectionOp::Delete(vec![conflict]))],
            },
        )
        .unwrap();
    section.seal().unwrap();
    let image = section.image();
    let arena = Arena::default();
    let mut reopened = Section::open(&arena, image.clone()).unwrap();
    assert!(reopened.conflicts().unwrap().is_empty());
    assert_eq!(reopened.pages().unwrap().len(), 1);

    // Each space's objects and their property identities, as OneNote left them.
    let native = read("corpus/conflict-page/native-delete/notebook/synthetic.one");
    let shapes = |image: &[u8]| {
        let store = Store::parse(image).unwrap();
        let index = RevisionIndex::parse(&store).unwrap();
        let root = index.root;
        let roots = index.resolve_active(page).unwrap().roots;
        let objects = objects(image);
        let manifest = objects[&page][&roots[&1]].clone();
        let metadata = objects[&page][&roots[&2]].clone();
        let flagged = objects[&root]
            .values()
            .filter(|(_, ids)| ids.contains(&0x88001d97))
            .count();
        (manifest, metadata, flagged, objects[&conflict].clone())
    };
    let (manifest, metadata, flagged, space) = shapes(&image);
    let (native_manifest, native_metadata, native_flagged, native_space) = shapes(&native);
    assert_eq!(
        manifest
            .1
            .iter()
            .filter(|id| **id != 0x3400347b)
            .collect::<Vec<_>>(),
        [&0x24001c1f, &0x24003442]
    );
    assert_eq!(
        native_manifest
            .1
            .iter()
            .filter(|id| **id != 0x3400347b)
            .collect::<Vec<_>>(),
        [&0x24001c1f, &0x24003442]
    );
    assert!(!metadata.1.contains(&0x88001d97) && !native_metadata.1.contains(&0x88001d97));
    assert_eq!((flagged, native_flagged), (0, 0));
    assert_eq!(space, native_space);

    if let Some(output) = std::env::var_os("ONESTORE_CONFLICT_DELETE_EXPORT") {
        let output = std::path::Path::new(&output);
        std::fs::create_dir_all(output).unwrap();
        std::fs::write(output.join("synthetic.one"), &image).unwrap();
        std::fs::copy(
            format!(
                "{}/../../{notebook}/Open Notebook.onetoc2",
                env!("CARGO_MANIFEST_DIR")
            ),
            output.join("Open Notebook.onetoc2"),
        )
        .unwrap();
    }
}
