#![cfg(feature = "protected")]

use onestore::{
    RevisionIndex, Store,
    document::{Document, Kind},
    op::{Edit, Op, PageOp},
    protected::{Error, Limits, UnlockedSection},
};
use std::{fs, path::Path};

#[test]
fn known_passwords_open_independent_native_fixtures() {
    for (root, notebook, pages) in [
        ("native-encrypted", "encrypted-01/notebook/synthetic.one", 1),
        ("native-protected-boundaries", "notebook/synthetic.one", 11),
    ] {
        let root = Path::new("../../corpus").join(root);
        let manifest: serde_json::Value =
            serde_json::from_slice(&fs::read(root.join("manifest.json")).unwrap()).unwrap();
        let password = manifest["password"].as_str().unwrap();
        let bytes = fs::read(root.join(notebook)).unwrap();
        let before = bytes.clone();
        let store = Store::parse(&bytes).unwrap();
        let index = RevisionIndex::parse(&store).unwrap();
        assert!(Document::parse(&index).unwrap().pages().unwrap().is_empty());
        let unlocked = UnlockedSection::open(&index, password, Limits::default()).unwrap();
        let document = unlocked.document().unwrap();
        assert_eq!(document.pages().unwrap().len(), pages);
        let (space, _) = document.pages().unwrap()[0];
        let current = &document.spaces[&space];
        let revision = &current.revisions[&current.contexts[&onestore::ExGuid::default()]];
        let (object, _) = revision
            .nodes
            .iter()
            .find(|(_, node)| matches!(node.kind, Kind::RichText { .. }))
            .unwrap();
        // The ordinary section writer refuses a protected section.
        let arena = onestore::Arena::default();
        assert!(onestore::Section::open(&arena, bytes.clone()).is_err());
        let op = PageOp::Text {
            text: *object,
            range: 0..0,
            with: "Must remain protected".into(),
        };
        let edit = Edit {
            at: 134_000_000_000_000_000,
            ops: vec![Op::Page { space, op }],
        };
        assert!(unlocked.apply("Rust", &edit).is_ok());
        assert!(
            document
                .spaces
                .values()
                .flat_map(|s| s.revisions.values())
                .flat_map(|r| r.nodes.values())
                .all(|n| !matches!(n.kind, Kind::Encrypted { .. }))
        );
        assert!(matches!(
            UnlockedSection::open(
                &index,
                "deliberately incorrect fixture password",
                Limits::default()
            ),
            Err(Error::PasswordMismatch)
        ));
        assert!(Document::parse(&index).unwrap().pages().unwrap().is_empty());
        assert_eq!(bytes, before);
    }
}

#[test]
fn limits_and_password_bytes_are_explicit() {
    let root = Path::new("../../corpus/native-protected-boundaries");
    let manifest: serde_json::Value =
        serde_json::from_slice(&fs::read(root.join("manifest.json")).unwrap()).unwrap();
    let password = manifest["password"].as_str().unwrap();
    let bytes = fs::read(root.join("notebook/synthetic.one")).unwrap();
    let store = Store::parse(&bytes).unwrap();
    let index = RevisionIndex::parse(&store).unwrap();
    for limits in [
        Limits {
            kdf_rounds: 0,
            ..Limits::default()
        },
        Limits {
            decoded_bytes: 0,
            ..Limits::default()
        },
        Limits {
            object_visits: 0,
            ..Limits::default()
        },
    ] {
        assert!(matches!(
            UnlockedSection::open(&index, password, limits),
            Err(Error::Limit)
        ));
    }
    for changed in [
        password.replace("e\u{301}", "é"),
        format!("{password}\n"),
        password.to_uppercase(),
    ] {
        assert!(matches!(
            UnlockedSection::open(&index, &changed, Limits::default()),
            Err(Error::PasswordMismatch)
        ));
    }
    assert!(matches!(
        UnlockedSection::open(&index, &"x".repeat(65537), Limits::default()),
        Err(Error::Limit)
    ));
    let ordinary = onestore::create_section("ordinary.one", "Fictitious", "Author").unwrap();
    let store = Store::parse(&ordinary).unwrap();
    let index = RevisionIndex::parse(&store).unwrap();
    assert!(matches!(
        UnlockedSection::open(&index, password, Limits::default()),
        Err(Error::Unsupported)
    ));
}

/// The first text paragraph of every page gains text; the written file stays protected,
/// opens with the same password and reads back as the edited model.
#[test]
fn a_protected_page_edit_is_stored_under_the_section_key() {
    use onestore::page::{Page, PageObject, Paragraph};
    for (root, notebook) in [
        ("native-encrypted", "encrypted-01/notebook/synthetic.one"),
        ("native-protected-boundaries", "notebook/synthetic.one"),
    ] {
        let root = Path::new("../../corpus").join(root);
        let manifest: serde_json::Value =
            serde_json::from_slice(&fs::read(root.join("manifest.json")).unwrap()).unwrap();
        let password = manifest["password"].as_str().unwrap();
        let mut bytes = fs::read(root.join(notebook)).unwrap();
        let pages = |bytes: &[u8]| -> Vec<(onestore::ExGuid, Page)> {
            let store = Store::parse(bytes).unwrap();
            let index = RevisionIndex::parse(&store).unwrap();
            assert!(Document::parse(&index).unwrap().pages().unwrap().is_empty());
            let unlocked = UnlockedSection::open(&index, password, Limits::default()).unwrap();
            let document = unlocked.document().unwrap();
            document
                .pages()
                .unwrap()
                .into_iter()
                .map(|(space, _)| (space, Page::from_space(&document, space).unwrap()))
                .collect()
        };
        let mut expected = pages(&bytes);
        let mut edited = 0;
        for (space, page) in &mut expected {
            let before = page.clone();
            let paragraphs = page.objects.iter_mut().find_map(|object| match object {
                PageObject::Outline(outline)
                    if outline.paragraphs.iter().any(|p| p.text().is_some()) =>
                {
                    Some(&mut outline.paragraphs)
                }
                _ => None,
            });
            let Some(paragraphs) = paragraphs else {
                continue;
            };
            edited += 1;
            let at = paragraphs.iter().position(|p| p.text().is_some()).unwrap();
            let mut file = paragraphs[at].clone();
            file.id = onestore::page::text::new_id().unwrap();
            file.lists.clear();
            file.tags.clear();
            file.style = None;
            file.format = Default::default();
            file.content =
                onestore::page::ParagraphContent::Attachment(onestore::page::Attachment {
                    id: onestore::page::text::new_id().unwrap(),
                    filename: "sealed.txt".into(),
                    source_path: None,
                    size: Some([24.0, 24.0]),
                    bytes: Some(std::sync::Arc::from(&b"A payload that stays sealed"[..])),
                    preview: None,
                    recording: None,
                });
            paragraphs.insert(at + 1, file);
            let text = paragraphs[at].text_mut().unwrap();
            let format = text.text.format_at(0).unwrap().clone();
            text.text
                .append(Paragraph::new(" still protected".to_owned(), format))
                .unwrap();
            let ops = onestore::op::lower_page(&before, page).unwrap();
            let edit = Edit {
                at: 134_000_000_000_000_000,
                ops: ops
                    .into_iter()
                    .map(|op| Op::Page { space: *space, op })
                    .collect(),
            };
            let store = Store::parse(&bytes).unwrap();
            let index = RevisionIndex::parse(&store).unwrap();
            assert!(matches!(
                UnlockedSection::open(&index, "wrong", Limits::default()),
                Err(Error::PasswordMismatch)
            ));
            let unlocked = UnlockedSection::open(&index, password, Limits::default()).unwrap();
            let transaction = unlocked.apply("Rust", &edit).unwrap();
            let mut written = bytes.clone();
            transaction.apply(&mut written).unwrap();
            for clear in [&b" still protected"[..], b"stays sealed"] {
                assert!(!written.windows(clear.len()).any(|w| w == clear));
            }
            let revisions = |bytes: &[u8]| {
                let store = Store::parse(bytes).unwrap();
                let index = RevisionIndex::parse(&store).unwrap();
                index
                    .spaces
                    .iter()
                    .map(|(id, space)| (*id, space.revisions.len()))
                    .collect::<Vec<_>>()
            };
            let grown: Vec<_> = revisions(&bytes)
                .into_iter()
                .zip(revisions(&written))
                .filter(|(before, after)| before != after)
                .map(|(before, _)| before.0)
                .collect();
            assert_eq!(grown, [*space]);
            bytes = written;
        }
        assert!(edited > 0);
        let stored = pages(&bytes);
        assert_eq!(stored.len(), expected.len());
        for ((_, stored), (_, expected)) in stored.iter().zip(&expected) {
            assert_eq!(stored.objects, expected.objects);
        }
    }
}

/// OneNote's revisions that depend on an earlier one carry no key node of their own.
#[test]
fn a_native_revision_inherits_the_key_of_its_dependency() {
    let bytes = fs::read("../../corpus/protected-edit/native-after/synthetic.one").unwrap();
    let manifest: serde_json::Value =
        serde_json::from_slice(&fs::read("../../corpus/native-encrypted/manifest.json").unwrap())
            .unwrap();
    let store = Store::parse(&bytes).unwrap();
    let index = RevisionIndex::parse(&store).unwrap();
    assert!(index.spaces.values().any(|space| {
        space.revisions.values().any(|revision| {
            revision.encrypted && revision.nodes.first().is_none_or(|node| node.id != 0x7c)
        })
    }));
    let unlocked = UnlockedSection::open(
        &index,
        manifest["password"].as_str().unwrap(),
        Limits::default(),
    )
    .unwrap();
    let document = unlocked.document().unwrap();
    let (space, _) = document.pages().unwrap()[0];
    let page = onestore::page::Page::from_space(&document, space).unwrap();
    assert!(format!("{:?}", page.objects).contains("Native edit after Rust."));
}
