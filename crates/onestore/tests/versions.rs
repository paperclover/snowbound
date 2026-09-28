//! Page versions against OneNote 2010's own (`corpus/page-versions/native`): listing them,
//! reading them, and Restore Version and Delete Version storing what OneNote stores.

use onestore::{
    Arena, ExGuid, ObjectData, PageVersion, PropertySets, RevisionIndex, Section, Store,
    document::Document,
    op::{Edit, Op, PageOp, SectionOp},
    page::{Page, PageObject},
};
use std::collections::BTreeSet;

const AT: u64 = 134_040_000_000_000_000;
const NATIVE: &str = "corpus/page-versions/native";
/// The version history's context.
const HISTORY: &str = "{7111497F-1B6B-4209-9491-C98B04CF4C5A},1";

fn read(path: &str) -> Vec<u8> {
    std::fs::read(format!("{}/../../{path}", env!("CARGO_MANIFEST_DIR"))).unwrap()
}

fn step(n: u32) -> Vec<u8> {
    read(&format!("{NATIVE}/step-{n:02}/notebook/History.one"))
}

fn id(text: &str) -> ExGuid {
    text.parse().unwrap()
}

/// The page every step holds, and the version OneNote made when a second author edited it.
fn page() -> ExGuid {
    id("{55479BA2-FB3A-45EB-A43D-EDB93DA0593D},1")
}

fn texts(page: &Page) -> Vec<String> {
    page.objects
        .iter()
        .flat_map(|object| match object {
            PageObject::Outline(outline) => outline.paragraphs.clone(),
            _ => Vec::new(),
        })
        .filter_map(|paragraph| paragraph.text().map(|text| text.text.text().to_owned()))
        .collect()
}

fn versions(image: &[u8]) -> Vec<PageVersion> {
    let arena = Arena::default();
    let mut section = Section::open(&arena, image.to_vec()).unwrap();
    section
        .versions()
        .unwrap()
        .into_iter()
        .find(|(listed, _)| *listed == page())
        .map_or(Vec::new(), |(_, versions)| versions)
}

/// What a revision of `space` changed: its declared objects' types, sorted, with whether
/// each was new.
fn changed(index: &RevisionIndex<'_>, space: ExGuid, rid: ExGuid) -> Vec<(u32, bool)> {
    let revision = index.resolve(space, rid).unwrap();
    let base = index.spaces[&space].revisions[&rid]
        .dependency
        .map(|dependency| index.resolve(space, dependency).unwrap());
    let mut changed: Vec<(u32, bool)> = revision
        .objects
        .iter()
        .filter_map(|(id, object)| {
            let old = base.as_ref().and_then(|base| base.objects.get(id));
            let same = old.is_some_and(|old| {
                old.reference_count == object.reference_count && old.data == object.data
            });
            (!same).then_some((object.jcid, old.is_none()))
        })
        .collect();
    changed.sort();
    changed
}

/// Property identities of an object's root set.
fn properties(object: &onestore::Object<'_>) -> Vec<u32> {
    let ObjectData::Properties(bytes) = object.data else {
        panic!()
    };
    PropertySets::parse(bytes).unwrap().sets[0]
        .iter()
        .map(|property| property.id)
        .collect()
}

/// How an edit left the page's revisions, from `before` to `after`: the page revision's
/// dependency (named by what it was before), the history's, which revisions became versions,
/// the declared object types of the page and history revisions, the history's children
/// count and the proxy's properties, and whether the page metadata and the section's copy
/// say the page has versions.
#[derive(Debug, PartialEq)]
struct Shape {
    page_depends_on: &'static str,
    history_depends_on_previous: bool,
    labelled: Vec<&'static str>,
    page_changed: Vec<(u32, bool)>,
    history_changed: Vec<(u32, bool)>,
    listed: usize,
    proxy: Vec<u32>,
    flagged: [bool; 2],
}

fn shape(before: &[u8], after: &[u8]) -> Shape {
    let (old_store, new_store) = (Store::parse(before).unwrap(), Store::parse(after).unwrap());
    let (old, new) = (
        RevisionIndex::parse(&old_store).unwrap(),
        RevisionIndex::parse(&new_store).unwrap(),
    );
    new.validate_current().unwrap();
    Document::parse(&new).unwrap();
    let space = page();
    let history = id(HISTORY);
    let (old_labels, new_labels) = (&old.spaces[&space].labels, &new.spaces[&space].labels);
    let old_page = old_labels[&(ExGuid::default(), 1)];
    let new_page = new_labels[&(ExGuid::default(), 1)];
    let name = |rid: ExGuid| -> &'static str {
        if rid == old_page {
            "the page"
        } else if old_labels.get(&(history, 1)) == Some(&rid) {
            "the history"
        } else if old_labels.iter().any(|((context, role), labelled)| {
            *role == 1 && *context != ExGuid::default() && *labelled == rid
        }) {
            "a version"
        } else {
            "another revision"
        }
    };
    let page_depends_on = if new_page == old_page {
        "unchanged"
    } else {
        new.spaces[&space].revisions[&new_page]
            .dependency
            .map_or("none", name)
    };
    let (old_history, new_history) = (old_labels[&(history, 1)], new_labels[&(history, 1)]);
    let labelled = new_labels
        .iter()
        .filter(|((context, role), _)| {
            *role == 1
                && *context != ExGuid::default()
                && *context != history
                && !old_labels.contains_key(&(*context, 1))
        })
        .map(|(_, rid)| name(*rid))
        .collect();
    let history_revision = new.resolve(space, new_history).unwrap();
    let root = &history_revision.objects[&history_revision.roots[&1]];
    let listed = root.references().unwrap().objects;
    let proxy = listed.first().map_or(Vec::new(), |proxy| {
        properties(&history_revision.objects[proxy])
    });
    let metadata = |index: &RevisionIndex<'_>| -> [bool; 2] {
        let revision = index.resolve_active(space).unwrap();
        let own = properties(&revision.objects[&revision.roots[&2]]).contains(&0x88003462);
        // The section's copy, named by the page's space (MS-ONE 2.2.81).
        let section = index.resolve_active(index.root).unwrap();
        let copy = properties(&section.objects[&id("{77EF5B93-CD3A-0705-1329-3A15E7846CD5},1")])
            .contains(&0x88003462);
        [own, copy]
    };
    Shape {
        page_depends_on,
        history_depends_on_previous: new_history != old_history
            && new.spaces[&space].revisions[&new_history].dependency == Some(old_history),
        labelled,
        page_changed: if new_page == old_page {
            Vec::new()
        } else {
            changed(&new, space, new_page)
        },
        history_changed: if new_history == old_history {
            Vec::new()
        } else {
            changed(&new, space, new_history)
        },
        listed: listed.len(),
        proxy,
        flagged: metadata(&new),
    }
}

fn apply(image: &[u8], author: &str, ops: Vec<Op>) -> Vec<u8> {
    let arena = Arena::default();
    let mut section = Section::open(&arena, image.to_vec()).unwrap();
    section.apply(author, &Edit { at: AT, ops }).unwrap();
    section.seal().unwrap().unwrap();
    section.image()
}

#[test]
fn native_versions_are_listed_newest_first_with_their_authors() {
    assert_eq!(versions(&step(1)), []);
    let virtual_version = PageVersion {
        context: id("{2F6B7C6D-DF95-0F56-085E-74357323A760},1"),
        modified: Some(0x01dd4ee2641a3f00),
        author: Some("virtual".into()),
    };
    assert_eq!(versions(&step(2)), std::slice::from_ref(&virtual_version));
    // A Restore Version made the page as it stood the newest version.
    let restored = versions(&step(4));
    assert_eq!(
        restored,
        [
            PageVersion {
                context: id("{783B5BD2-65DB-082D-3AAC-47135BA38EA9},1"),
                modified: Some(0x01dd4ee27e541d00),
                author: Some("Other Person".into()),
            },
            virtual_version.clone(),
        ]
    );
    assert_eq!(versions(&step(5)), [virtual_version]);
    assert_eq!(versions(&step(6)), []);
    // Optimizing keeps a version and the history as checkpoints.
    assert_eq!(versions(&step(10)).len(), 1);
}

#[test]
fn a_version_reads_as_the_page_it_was() {
    let arena = Arena::default();
    let section = Section::open(&arena, step(1)).unwrap();
    let before = section.page(page()).unwrap();
    let arena = Arena::default();
    let mut section = Section::open(&arena, step(2)).unwrap();
    let context = section.versions().unwrap()[0].1[0].context;
    let version = section.version(page(), context).unwrap();
    assert_eq!(version, before);
    assert_eq!(texts(&version), ["First state. Typed by virtual."]);
    assert_eq!(
        texts(&section.page(page()).unwrap()),
        ["First state. Typed by virtual.", "Second author line."]
    );
    assert!(section.version(page(), id(HISTORY)).is_err());
}

/// Restore Version stores what OneNote 2010's does (`step-03` to `step-04`): the page's next
/// revision builds on the version's, the page as it stood becomes the newest version under
/// the context its revision derives, the history lists it first, and the revision metadata
/// names who restored it. `ONESTORE_VERSIONS_EXPORT` names a directory for the notebooks,
/// for a cold read.
#[test]
fn restoring_a_version_stores_what_onenote_stores() {
    let before = step(3);
    let restored_version = versions(&before)[0].context;
    let arena = Arena::default();
    let section = Section::open(&arena, before.clone()).unwrap();
    let standing = section.page(page()).unwrap();
    let version = section.version(page(), restored_version).unwrap();
    let ours = apply(
        &before,
        "Other Person",
        vec![Op::Section(SectionOp::RestoreVersion {
            page: page(),
            version: restored_version,
            guid: [7; 16],
        })],
    );
    let expected = shape(&before, &step(4));
    assert_eq!(expected.page_depends_on, "a version");
    assert_eq!(shape(&before, &ours), expected);

    let arena = Arena::default();
    let mut reopened = Section::open(&arena, ours.clone()).unwrap();
    assert_eq!(reopened.page(page()).unwrap().objects, version.objects);
    let listed = reopened.versions().unwrap().remove(0).1;
    assert_eq!(listed.len(), 2);
    assert_eq!(listed[1].context, restored_version);
    // As OneNote names it (`step-04`): the page's revision, its bits flipped by a salt.
    let store = Store::parse(&before).unwrap();
    let standing_rid = RevisionIndex::parse(&store)
        .unwrap()
        .active(page())
        .unwrap();
    let salt = |a: ExGuid, b: ExGuid| -> Vec<u8> {
        a.guid.iter().zip(b.guid).map(|(a, b)| a ^ b).collect()
    };
    assert_eq!(
        salt(listed[0].context, standing_rid),
        salt(
            id("{783B5BD2-65DB-082D-3AAC-47135BA38EA9},1"),
            id("{E569A5AE-EA67-4CB8-A9AC-55880C342743},1")
        )
    );
    assert_eq!(listed[0].author.as_deref(), Some("Other Person"));
    assert_eq!(
        reopened.version(page(), listed[0].context).unwrap(),
        standing
    );

    if let Some(output) = std::env::var_os("ONESTORE_VERSIONS_EXPORT") {
        export(&output, "restore", &ours);
    }
}

/// Delete Version stores what OneNote 2010's does: the history stops listing the version
/// (`step-04` to `step-05`), and once none is left the page's metadata and the section's copy
/// stop saying it has versions (`step-05` to `step-06`).
#[test]
fn deleting_versions_stores_what_onenote_stores() {
    let before = step(4);
    let newest = versions(&before)[0].context;
    let delete = |image: &[u8], versions: Vec<ExGuid>| {
        apply(
            image,
            "Other Person",
            vec![Op::Section(SectionOp::DeleteVersions {
                page: page(),
                versions,
            })],
        )
    };
    let ours = delete(&before, vec![newest]);
    assert_eq!(shape(&before, &ours), shape(&before, &step(5)));
    let emptied = delete(&step(5), vec![versions(&step(5))[0].context]);
    let expected = shape(&step(5), &step(6));
    assert_eq!(expected.flagged, [false, false]);
    assert_eq!(shape(&step(5), &emptied), expected);
    // Every version at once, as Delete All Versions in Section asks.
    let all = versions(&before)
        .into_iter()
        .map(|version| version.context)
        .collect();
    assert_eq!(versions(&delete(&before, all)), []);
    // A version already gone refuses the edit.
    let arena = Arena::default();
    let mut section = Section::open(&arena, step(5)).unwrap();
    let refused = section.apply(
        "Other Person",
        &Edit {
            at: AT,
            ops: vec![Op::Section(SectionOp::DeleteVersions {
                page: page(),
                versions: vec![newest],
            })],
        },
    );
    assert_eq!(
        refused,
        Err(onestore::op::OpError::TargetUnavailable(newest))
    );

    if let Some(output) = std::env::var_os("ONESTORE_VERSIONS_EXPORT") {
        export(&output, "delete", &ours);
        export(&output, "delete-all", &emptied);
    }
}

/// Edits not yet sealed become a version of their own when a restore follows them in the
/// same batch, and restores stack: every state the page passed through stays listed.
#[test]
fn unsaved_edits_and_stacked_restores_become_versions() {
    let before = step(2);
    let arena = Arena::default();
    let mut section = Section::open(&arena, before.clone()).unwrap();
    let first = section.versions().unwrap()[0].1[0].context;
    let text = section
        .page(page())
        .unwrap()
        .objects
        .iter()
        .find_map(|object| match object {
            PageObject::Outline(outline) => outline.paragraphs.last()?.text().map(|text| text.id),
            _ => None,
        });
    let edit = |ops| Edit { at: AT, ops };
    section
        .apply(
            "Clover Snow",
            &edit(vec![Op::Page {
                space: page(),
                op: PageOp::Text {
                    text: text.unwrap(),
                    range: 0..6,
                    with: "Unsaved".into(),
                },
            }]),
        )
        .unwrap();
    let edited = section.page(page()).unwrap();
    let restore = |version, guid| {
        edit(vec![Op::Section(SectionOp::RestoreVersion {
            page: page(),
            version,
            guid,
        })])
    };
    section
        .apply("Clover Snow", &restore(first, [1; 16]))
        .unwrap();
    let listed = section.versions().unwrap()[0].1.clone();
    assert_eq!(listed.len(), 2);
    assert_eq!(section.version(page(), listed[0].context).unwrap(), edited);
    // The unsaved state is still being saved, so it cannot be restored yet; the stored one can.
    assert!(
        section
            .apply("Clover Snow", &restore(listed[0].context, [2; 16]))
            .is_err()
    );
    section
        .apply("Clover Snow", &restore(first, [3; 16]))
        .unwrap();
    assert_eq!(section.versions().unwrap()[0].1.len(), 3);
    section.seal().unwrap().unwrap();
    let image = section.image();

    let arena = Arena::default();
    let mut reopened = Section::open(&arena, image.clone()).unwrap();
    let store = Store::parse(&image).unwrap();
    let index = RevisionIndex::parse(&store).unwrap();
    index.validate_current().unwrap();
    Document::parse(&index).unwrap();
    let listed = reopened.versions().unwrap().remove(0).1;
    assert_eq!(listed.len(), 3);
    let pages: Vec<Page> = listed
        .iter()
        .map(|version| reopened.version(page(), version.context).unwrap())
        .collect();
    let version = reopened.version(page(), first).unwrap();
    assert_eq!(pages[1], edited);
    assert_eq!(pages[0].objects, version.objects);
    assert_eq!(reopened.page(page()).unwrap().objects, version.objects);
    // The restored page edits on as any other.
    let text = texts(&reopened.page(page()).unwrap());
    assert_eq!(text, ["First state. Typed by virtual."]);
}

/// An edit that fails after a restore leaves the section as it was before the edit.
#[test]
fn a_failed_edit_undoes_its_restore() {
    let before = step(2);
    let arena = Arena::default();
    let mut section = Section::open(&arena, before.clone()).unwrap();
    let first = section.versions().unwrap()[0].1[0].context;
    let page_before = section.page(page()).unwrap();
    let refused = section.apply(
        "Clover Snow",
        &Edit {
            at: AT,
            ops: vec![
                Op::Section(SectionOp::RestoreVersion {
                    page: page(),
                    version: first,
                    guid: [1; 16],
                }),
                Op::Section(SectionOp::DeleteVersions {
                    page: page(),
                    versions: vec![id(HISTORY)],
                }),
            ],
        },
    );
    assert!(refused.is_err());
    assert_eq!(section.page(page()).unwrap(), page_before);
    assert_eq!(section.versions().unwrap()[0].1.len(), 1);
    assert!(section.seal().unwrap().is_none());
    assert!(section.image() == before);
}

#[test]
fn private_notebook_versions_read() {
    let path = format!(
        "{}/../../corpus/private/current/Art/Album.one",
        env!("CARGO_MANIFEST_DIR")
    );
    let Ok(image) = std::fs::read(path) else {
        return;
    };
    let arena = Arena::default();
    let mut section = Section::open(&arena, image).unwrap();
    let listed = section.versions().unwrap();
    assert!(
        listed
            .iter()
            .map(|(_, versions)| versions.len())
            .sum::<usize>()
            >= 10
    );
    let contexts: BTreeSet<ExGuid> = listed
        .iter()
        .flat_map(|(_, versions)| versions.iter().map(|version| version.context))
        .collect();
    assert_eq!(
        contexts.len(),
        listed.iter().map(|(_, v)| v.len()).sum::<usize>()
    );
    for (page, versions) in &listed {
        for version in versions {
            section.version(*page, version.context).unwrap();
        }
    }
}

fn export(output: &std::ffi::OsStr, name: &str, image: &[u8]) {
    let directory = std::path::Path::new(output).join(name);
    std::fs::create_dir_all(&directory).unwrap();
    std::fs::write(directory.join("History.one"), image).unwrap();
    std::fs::write(
        directory.join("Open Notebook.onetoc2"),
        read(&format!("{NATIVE}/step-01/notebook/Open Notebook.onetoc2")),
    )
    .unwrap();
}
