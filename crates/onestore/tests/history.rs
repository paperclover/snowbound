//! Earlier states of a section: a page as one of its stored revisions holds it, the section
//! as some revisions of its spaces leave it, and revisions sealed under chosen names.

use onestore::{
    Arena, ExGuid, PageCreation, RevisionIndex, Section, Store,
    op::{Edit, Op, PageOp, SectionOp},
};
use std::collections::BTreeMap;

fn read(path: &str) -> Vec<u8> {
    std::fs::read(format!(
        "{}/../../corpus/{path}",
        env!("CARGO_MANIFEST_DIR")
    ))
    .unwrap()
}

/// The active revision of each object space.
fn active(image: &[u8]) -> BTreeMap<ExGuid, ExGuid> {
    let store = Store::parse(image).unwrap();
    let index = RevisionIndex::parse(&store).unwrap();
    index
        .spaces
        .keys()
        .filter_map(|space| Some((*space, index.active(*space).ok()?)))
        .collect()
}

/// Two OneNote 2010 clients' captures of one notebook, earliest first: each later file holds
/// every revision the earlier ones made current.
const STAGES: [&[&str]; 3] = [
    &[
        "conflict-page/native/initial/notebook/synthetic.one",
        "conflict-page/native/b-published/notebook/synthetic.one",
        "conflict-page/native/conflict/notebook/synthetic.one",
    ],
    &[
        "conflict-page/native-pages/initial/notebook/synthetic.one",
        "conflict-page/native-pages/b-published/notebook/synthetic.one",
        "conflict-page/native-pages/merged/notebook/synthetic.one",
    ],
    &[
        "conflict-page/native-restore/initial/notebook/synthetic.one",
        "conflict-page/native-restore/b-published/notebook/synthetic.one",
        "conflict-page/native-restore/merged/notebook/synthetic.one",
    ],
];

#[test]
fn each_stored_revision_of_a_page_reads_as_onenote_showed_it_then() {
    let mut compared = 0;
    for stages in STAGES {
        let last = read(stages[stages.len() - 1]);
        let arena = Arena::default();
        let history = Section::open(&arena, last.clone()).unwrap();
        for earlier in &stages[..stages.len() - 1] {
            let earlier = read(earlier);
            let arena = Arena::default();
            let mut then = Section::open(&arena, earlier.clone()).unwrap();
            let revisions = active(&earlier);
            for (space, ..) in then.pages().unwrap() {
                assert_eq!(
                    history.page_at(space, revisions[&space]).unwrap(),
                    then.page(space).unwrap(),
                    "{space} at {}",
                    revisions[&space]
                );
                compared += 1;
            }
            // The section as those revisions leave it is the earlier file's.
            let arena = Arena::default();
            let mut state = Section::open_at(&arena, vec![last.clone()], &revisions).unwrap();
            assert_eq!(state.pages().unwrap(), then.pages().unwrap());
            assert_eq!(state.series().unwrap(), then.series().unwrap());
            for (space, ..) in then.pages().unwrap() {
                assert_eq!(state.page(space).unwrap(), then.page(space).unwrap());
            }
        }
    }
    assert!(compared >= 10, "{compared}");
}

#[test]
fn an_earlier_state_needs_its_page_list() {
    let image = read("conflict-page/native/conflict/notebook/synthetic.one");
    let mut revisions = active(&image);
    let arena = Arena::default();
    let root = Section::open(&arena, image.clone()).unwrap().root();
    revisions.remove(&root);
    assert!(Section::open_at(&Arena::default(), vec![image], &revisions).is_err());
}

#[test]
fn a_sealed_revision_takes_the_name_given_it() {
    let image = read("conflict-page/native/initial/notebook/synthetic.one");
    let arena = Arena::default();
    let mut section = Section::open(&arena, image).unwrap();
    let (space, ..) = section.pages().unwrap()[0].clone();
    let body = section
        .page(space)
        .unwrap()
        .objects
        .iter()
        .find_map(|object| match object {
            onestore::page::PageObject::Outline(outline) => outline
                .paragraphs
                .iter()
                .find_map(|paragraph| paragraph.text().map(|text| text.id)),
            _ => None,
        })
        .unwrap();
    let current = section.revisions().find(|(id, _)| *id == space).unwrap().1;
    let type_body = |section: &mut Section<'_>| {
        section
            .apply(
                "Author",
                &Edit {
                    at: 134_030_000_000_000_000,
                    ops: vec![Op::Page {
                        space,
                        op: PageOp::Text {
                            text: body,
                            range: 0..0,
                            with: "Named ".into(),
                        },
                    }],
                },
            )
            .unwrap()
    };
    type_body(&mut section);
    assert!(
        section
            .seal_as(&BTreeMap::from([(space, current)]))
            .is_err()
    );
    let arena = Arena::default();
    let image = read("conflict-page/native/initial/notebook/synthetic.one");
    let mut section = Section::open(&arena, image).unwrap();
    type_body(&mut section);
    let creation = PageCreation::new(None, Some("Also named"), "Author")
        .unwrap()
        .in_space([7; 16])
        .unwrap();
    let created = creation.space();
    section
        .apply(
            "Author",
            &Edit {
                at: 134_030_000_000_000_000,
                ops: vec![Op::Section(SectionOp::Create(creation))],
            },
        )
        .unwrap();
    let name = |n| ExGuid {
        guid: [n; 16],
        n: 1,
    };
    section
        .seal_as(&BTreeMap::from([(space, name(1)), (created, name(2))]))
        .unwrap()
        .unwrap();
    let sealed = section.image();
    let revisions = active(&sealed);
    assert_eq!(revisions[&space], name(1));
    assert_eq!(revisions[&created], name(2));
    assert_eq!(
        created,
        ExGuid {
            guid: [7; 16],
            n: 1
        }
    );
    // The earlier revision still reads as it was.
    let arena = Arena::default();
    let reopened = Section::open(&arena, sealed).unwrap();
    let typed =
        |page: onestore::page::Page| serde_json::to_string(&page).unwrap().contains("Named ");
    assert!(!typed(reopened.page_at(space, current).unwrap()));
    assert!(typed(reopened.page(space).unwrap()));
}
