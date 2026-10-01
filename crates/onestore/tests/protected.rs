use onestore::{
    Arena, ExGuid, RevisionIndex, Section, Store,
    document::{Document, Kind},
    op::{Edit, Op},
    page::Page,
    protected::{Error, Key, Limits, UnlockedSection, rekey},
};
use std::{fs, path::Path};

/// The native fixtures with their passwords.
fn fixtures() -> Vec<(Vec<u8>, String)> {
    [
        ("native-encrypted", "encrypted-01/notebook/synthetic.one"),
        ("native-protected-boundaries", "notebook/synthetic.one"),
    ]
    .into_iter()
    .map(|(root, notebook)| {
        let root = Path::new("../../corpus").join(root);
        let manifest: serde_json::Value =
            serde_json::from_slice(&fs::read(root.join("manifest.json")).unwrap()).unwrap();
        (
            fs::read(root.join(notebook)).unwrap(),
            manifest["password"].as_str().unwrap().to_owned(),
        )
    })
    .collect()
}

/// Every page of a section `Section::open` or `Section::unlock` reads, in order.
fn pages(section: &mut Section<'_>) -> Vec<(ExGuid, Page)> {
    section
        .pages()
        .unwrap()
        .into_iter()
        .map(|(space, ..)| (space, section.page(space).unwrap()))
        .collect()
}

fn objects(pages: &[(ExGuid, Page)]) -> Vec<&Vec<onestore::page::PageObject>> {
    pages.iter().map(|(_, page)| &page.objects).collect()
}

#[test]
fn known_passwords_open_independent_native_fixtures() {
    for ((bytes, password), count) in fixtures().into_iter().zip([1, 11]) {
        let store = Store::parse(&bytes).unwrap();
        let index = RevisionIndex::parse(&store).unwrap();
        assert!(Document::parse(&index).unwrap().pages().unwrap().is_empty());
        let unlocked = UnlockedSection::open(&index, &password, Limits::default()).unwrap();
        let document = unlocked.document().unwrap();
        assert_eq!(document.pages().unwrap().len(), count);
        assert!(
            document
                .spaces
                .values()
                .flat_map(|s| s.revisions.values())
                .flat_map(|r| r.nodes.values())
                .all(|n| !matches!(n.kind, Kind::Encrypted { .. }))
        );
        // A section opened under its key reads as the document does.
        let arena = Arena::default();
        assert!(Section::open(&arena, bytes.clone()).is_err());
        let key = Key::open(&bytes, &password).unwrap();
        let mut section = Section::unlock(&arena, bytes.clone(), &key).unwrap();
        let read = pages(&mut section);
        assert_eq!(read.len(), count);
        for (space, page) in &read {
            assert_eq!(*page, Page::from_space(&document, *space).unwrap());
        }
        assert!(matches!(
            Key::open(&bytes, "deliberately incorrect fixture password"),
            Err(Error::PasswordMismatch)
        ));
    }
}

#[test]
fn limits_and_password_bytes_are_explicit() {
    let (bytes, password) = fixtures().remove(1);
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
            UnlockedSection::open(&index, &password, limits),
            Err(Error::Limit)
        ));
    }
    for changed in [
        password.replace("e\u{301}", "é"),
        format!("{password}\n"),
        password.to_uppercase(),
    ] {
        assert!(matches!(
            Key::open(&bytes, &changed),
            Err(Error::PasswordMismatch)
        ));
    }
    assert!(matches!(
        Key::open(&bytes, &"x".repeat(65537)),
        Err(Error::Limit)
    ));
    let ordinary = onestore::create_section("ordinary.one", "Fictitious", "Author").unwrap();
    assert!(matches!(
        Key::open(&ordinary, &password),
        Err(Error::Unsupported)
    ));
    // A key opens only the section whose encryption data it came from.
    let other = Key::new(&password).unwrap();
    assert!(matches!(
        Section::unlock(&Arena::default(), bytes, &other),
        Err(Error::PasswordMismatch)
    ));
}

/// Appends an attachment and text to the first text paragraph of `page`; none without one.
fn edit(space: ExGuid, page: &Page) -> Option<(Edit, Page)> {
    use onestore::page::{Attachment, PageObject, Paragraph, ParagraphContent};
    let mut edited = page.clone();
    let paragraphs = edited.objects.iter_mut().find_map(|object| match object {
        PageObject::Outline(outline) if outline.paragraphs.iter().any(|p| p.text().is_some()) => {
            Some(&mut outline.paragraphs)
        }
        _ => None,
    })?;
    let at = paragraphs.iter().position(|p| p.text().is_some()).unwrap();
    let mut file = paragraphs[at].clone();
    file.id = onestore::page::text::new_id().unwrap();
    file.lists.clear();
    file.tags.clear();
    file.style = None;
    file.format = Default::default();
    file.content = ParagraphContent::Attachment(Attachment {
        id: onestore::page::text::new_id().unwrap(),
        filename: "sealed.txt".into(),
        source_path: None,
        size: Some([24.0, 24.0]),
        layout: Default::default(),
        bytes: Some(std::sync::Arc::from(&b"A payload that stays sealed"[..])),
        preview: None,
        recording: None,
        tags: Vec::new(),
    });
    paragraphs.insert(at + 1, file);
    let text = paragraphs[at].text_mut().unwrap();
    let format = text.text.format_at(0).unwrap().clone();
    text.text
        .append(Paragraph::new(" still protected".to_owned(), format))
        .unwrap();
    let ops = onestore::op::lower_page(page, &edited).unwrap();
    let edit = Edit {
        at: 134_000_000_000_000_000,
        ops: ops.into_iter().map(|op| Op::Page { space, op }).collect(),
    };
    Some((edit, edited))
}

/// Every revision of every space of `bytes` names its key, as MS-ONESTORE 2.5.19 asks.
fn keyed(bytes: &[u8]) -> bool {
    let store = Store::parse(bytes).unwrap();
    let index = RevisionIndex::parse(&store).unwrap();
    index
        .spaces
        .values()
        .flat_map(|space| space.revisions.values())
        .all(|revision| revision.encrypted && revision.nodes[0].id == 0x7c)
}

/// The first text paragraph of every page gains text and an attachment; each seal appends
/// one revision to that page alone, stores none of it in the clear, and reads back as the
/// edited model under the same password.
#[test]
fn a_protected_page_edit_is_stored_under_the_section_key() {
    for (mut bytes, password) in fixtures() {
        let key = Key::open(&bytes, &password).unwrap();
        let mut expected =
            pages(&mut Section::unlock(&Arena::default(), bytes.clone(), &key).unwrap());
        let mut edited = 0;
        for (space, page) in &mut expected {
            let Some((edit, after)) = edit(*space, page) else {
                continue;
            };
            *page = after;
            edited += 1;
            let arena = Arena::default();
            let mut section = Section::unlock(&arena, bytes.clone(), &key).unwrap();
            section.apply("Rust", &edit).unwrap();
            let transaction = section.seal().unwrap().unwrap();
            let mut written = bytes.clone();
            transaction.apply(&mut written).unwrap();
            assert_eq!(written, section.image());
            for clear in [&b" still protected"[..], b"stays sealed"] {
                let utf16: Vec<u8> = String::from_utf8_lossy(clear)
                    .encode_utf16()
                    .flat_map(u16::to_le_bytes)
                    .collect();
                for clear in [clear, &utf16[..]] {
                    assert!(!written.windows(clear.len()).any(|w| w == clear));
                }
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
            // The section kept open reads its own seal back.
            assert_eq!(section.page(*space).unwrap().objects, page.objects);
            bytes = written;
        }
        assert!(edited > 0);
        let stored = pages(&mut Section::unlock(&Arena::default(), bytes.clone(), &key).unwrap());
        assert_eq!(objects(&stored), objects(&expected));
        // The document reader agrees, under the password alone.
        let store = Store::parse(&bytes).unwrap();
        let index = RevisionIndex::parse(&store).unwrap();
        let unlocked = UnlockedSection::open(&index, &password, Limits::default()).unwrap();
        let document = unlocked.document().unwrap();
        for (space, page) in &expected {
            assert_eq!(
                Page::from_space(&document, *space).unwrap().objects,
                page.objects
            );
        }
    }
}

/// OneNote's revisions that depend on an earlier one carry no key node of their own.
#[test]
fn a_native_revision_inherits_the_key_of_its_dependency() {
    let bytes = fs::read("../../corpus/protected-edit/native-after/synthetic.one").unwrap();
    let (_, password) = fixtures().remove(0);
    let store = Store::parse(&bytes).unwrap();
    let index = RevisionIndex::parse(&store).unwrap();
    assert!(index.spaces.values().any(|space| {
        space.revisions.values().any(|revision| {
            revision.encrypted && revision.nodes.first().is_none_or(|node| node.id != 0x7c)
        })
    }));
    let key = Key::open(&bytes, &password).unwrap();
    let read = pages(&mut Section::unlock(&Arena::default(), bytes, &key).unwrap());
    assert!(format!("{:?}", read[0].1.objects).contains("Native edit after Rust."));
}

/// Setting, changing and removing a password rewrites the section under new identities,
/// keeping every page, version and payload; each password opens only its own image.
#[test]
fn a_password_is_set_changed_and_removed() {
    let plain = fs::read("../../corpus/page-versions/native/step-09/notebook/History.one").unwrap();
    let arena = Arena::default();
    let mut section = Section::open(&arena, plain.clone()).unwrap();
    let before = pages(&mut section);
    let versions = section.versions().unwrap();
    assert!(!versions.is_empty());
    let identity = |bytes: &[u8]| {
        let store = Store::parse(bytes).unwrap();
        (
            store.header.file_id,
            RevisionIndex::parse(&store).unwrap().root,
        )
    };

    let first = Key::new("first password").unwrap();
    let protected = rekey(&plain, None, Some(&first)).unwrap();
    assert!(keyed(&protected));
    assert_eq!(protected[128..148], plain[128..148]);
    assert_ne!(identity(&protected).0, identity(&plain).0);
    assert_ne!(identity(&protected).1, identity(&plain).1);
    assert!(Section::open(&Arena::default(), protected.clone()).is_err());
    // The encryption data OneNote writes, which the password alone opens again.
    let reopened = Key::open(&protected, "first password").unwrap();
    assert_eq!(reopened.secret(), first.secret());
    assert!(matches!(
        Key::open(&protected, "First password"),
        Err(Error::PasswordMismatch)
    ));
    let arena = Arena::default();
    let mut section = Section::unlock(&arena, protected.clone(), &reopened).unwrap();
    assert_eq!(objects(&pages(&mut section)), objects(&before));
    assert_eq!(
        section
            .versions()
            .unwrap()
            .iter()
            .map(|(_, versions)| versions.len())
            .collect::<Vec<_>>(),
        versions
            .iter()
            .map(|(_, versions)| versions.len())
            .collect::<Vec<_>>()
    );
    let store = Store::parse(&protected).unwrap();
    let index = RevisionIndex::parse(&store).unwrap();
    let unlocked = UnlockedSection::open(&index, "first password", Limits::default()).unwrap();
    assert_eq!(
        unlocked.document().unwrap().pages().unwrap().len(),
        before.len()
    );

    let second = Key::new("second password").unwrap();
    assert!(matches!(
        rekey(&protected, None, Some(&second)),
        Err(Error::PasswordMismatch)
    ));
    let changed = rekey(&protected, Some(&first), Some(&second)).unwrap();
    assert_ne!(first.secret(), second.secret());
    assert!(matches!(
        Key::open(&changed, "first password"),
        Err(Error::PasswordMismatch)
    ));
    assert!(matches!(
        Section::unlock(&Arena::default(), changed.clone(), &first),
        Err(Error::PasswordMismatch)
    ));
    let arena = Arena::default();
    let mut section = Section::unlock(&arena, changed.clone(), &second).unwrap();
    assert_eq!(objects(&pages(&mut section)), objects(&before));

    let removed = rekey(&changed, Some(&second), None).unwrap();
    assert!(
        RevisionIndex::parse(&Store::parse(&removed).unwrap())
            .unwrap()
            .spaces
            .values()
            .flat_map(|space| space.revisions.values())
            .all(|revision| !revision.encrypted)
    );
    let arena = Arena::default();
    let mut section = Section::open(&arena, removed).unwrap();
    assert_eq!(objects(&pages(&mut section)), objects(&before));
    assert_eq!(section.versions().unwrap().len(), versions.len());
}

/// Payloads survive a password of their own: every native attachment boundary length.
#[test]
fn payloads_survive_a_new_password() {
    let (bytes, password) = fixtures().remove(1);
    let key = Key::open(&bytes, &password).unwrap();
    let before = pages(&mut Section::unlock(&Arena::default(), bytes.clone(), &key).unwrap());
    let new = Key::new("another").unwrap();
    let changed = rekey(&bytes, Some(&key), Some(&new)).unwrap();
    let after = pages(&mut Section::unlock(&Arena::default(), changed, &new).unwrap());
    assert_eq!(objects(&after), objects(&before));
}

/// What OneNote 2010 wrote setting, changing and removing a password in the lab, and its edit
/// of a section Snowbound protected, edited and gave another password, each read here.
#[test]
fn onenote_protected_changed_and_removed_sections_read_back() {
    let root = Path::new("../../corpus/protected-sections");
    let manifest: serde_json::Value =
        serde_json::from_slice(&fs::read(root.join("manifest.json")).unwrap()).unwrap();
    let plain = fs::read(root.join("source/synthetic.one")).unwrap();
    let titles = |section: &mut Section<'_>| -> Vec<String> {
        section
            .pages()
            .unwrap()
            .into_iter()
            .map(|(_, title, _)| title)
            .collect()
    };
    let expected = titles(&mut Section::open(&Arena::default(), plain).unwrap());
    assert_eq!(expected.len(), 2);
    for (file, password) in manifest["native"].as_object().unwrap() {
        let bytes = fs::read(root.join("native").join(file)).unwrap();
        let arena = Arena::default();
        let mut section = match password.as_str() {
            Some(password) => {
                let key = Key::open(&bytes, password).unwrap();
                assert!(Section::open(&Arena::default(), bytes.clone()).is_err());
                Section::unlock(&arena, bytes, &key).unwrap()
            }
            None => Section::open(&arena, bytes).unwrap(),
        };
        let read = titles(&mut section);
        assert_eq!(read.len(), 2, "{file}");
        if file.starts_with("snowbound") {
            let (space, ..) = section.pages().unwrap()[0];
            let page = format!("{:?}", section.page(space).unwrap().objects);
            assert!(page.contains("typed by OneNote"), "{file}");
        } else {
            assert_eq!(read, expected, "{file}");
        }
    }
}
