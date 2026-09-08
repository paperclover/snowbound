#![cfg(feature = "protected")]

use onestore::{
    RevisionIndex, Store,
    document::{Document, Kind},
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
        assert!(
            onestore::PreparedEdit::text(&bytes, space, *object, 0..0, "Must remain protected")
                .is_err()
        );
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
