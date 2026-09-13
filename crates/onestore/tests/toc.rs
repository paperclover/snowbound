use onestore::{PreparedEdit, RevisionIndex, Store, TocEdit, document::Document};

fn entries(bytes: &[u8]) -> Vec<(String, [u8; 16], u32, Option<u32>)> {
    let store = Store::parse(bytes).unwrap();
    assert!(store.checksum_mismatches.is_empty());
    let index = RevisionIndex::parse(&store).unwrap();
    let document = Document::parse(&index).unwrap();
    let revision = document.active(document.root).unwrap();
    let root = &revision.nodes[&revision.roots[&1]];
    let onestore::document::Kind::Toc { entries, .. } = &root.kind else {
        panic!()
    };
    entries
        .iter()
        .map(|id| match &revision.nodes[id].kind {
            onestore::document::Kind::Toc {
                filename,
                identity,
                order,
                color,
                ..
            } => (
                filename.clone().unwrap(),
                identity.unwrap(),
                order.unwrap(),
                *color,
            ),
            _ => panic!(),
        })
        .collect()
}

#[test]
fn sections_and_groups_are_added_renamed_coloured_ordered_and_removed() {
    let a = [1; 16];
    let b = [2; 16];
    let toc = onestore::create_table_of_contents(
        "Open Notebook.onetoc2",
        &[("Alpha.one", a), ("Beta.one", b)],
    )
    .unwrap();
    let c = [3; 16];
    let group = [4; 16];
    let written = PreparedEdit::table_of_contents(
        &toc,
        &[
            TocEdit::Add {
                filename: "Gamma.one".into(),
                identity: c,
                group: false,
            },
            TocEdit::Add {
                filename: "Archive".into(),
                identity: group,
                group: true,
            },
        ],
    )
    .unwrap();
    assert_eq!(
        entries(written.as_bytes()),
        [
            ("Alpha.one".to_owned(), a, 1, Some(0xffff_ffff)),
            ("Beta.one".to_owned(), b, 2, Some(0xffff_ffff)),
            ("Gamma.one".to_owned(), c, 3, Some(0xffff_ffff)),
            ("Archive".to_owned(), group, 4, None),
        ]
    );
    let again = PreparedEdit::table_of_contents(
        written.as_bytes(),
        &[
            TocEdit::Rename {
                identity: b,
                filename: "Renamed.one".into(),
            },
            TocEdit::Color {
                identity: c,
                color: Some(0x00d7ff),
            },
            TocEdit::Order(vec![group, c]),
            TocEdit::Remove { identity: a },
        ],
    )
    .unwrap();
    assert_eq!(
        entries(again.as_bytes()),
        [
            ("Archive".to_owned(), group, 1, None),
            ("Gamma.one".to_owned(), c, 2, Some(0x00d7ff)),
            ("Renamed.one".to_owned(), b, 3, Some(0xffff_ffff)),
        ]
    );
    assert!(
        PreparedEdit::table_of_contents(
            again.as_bytes(),
            &[TocEdit::Add {
                filename: "renamed.one".into(),
                identity: [9; 16],
                group: false
            }]
        )
        .is_err()
    );
    assert!(
        PreparedEdit::table_of_contents(again.as_bytes(), &[TocEdit::Remove { identity: a }])
            .is_err()
    );
}
