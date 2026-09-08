use super::*;
use onestore::{Insertion, TreeEdit};

#[test]
fn twelve_offline_clients_reconcile_tree_text_and_interrupted_publication() {
    let mut random = 1940_u64;
    for _ in 0..60 {
        let input: Vec<_> = (0..384)
            .map(|_| {
                random ^= random << 13;
                random ^= random >> 7;
                random ^= random << 17;
                random as u8
            })
            .collect();
        tree_schedule::run(&input);
    }
}

fn fixture() -> (Vec<u8>, ExGuid, ExGuid, [ExGuid; 4], [ExGuid; 4]) {
    let (mut source, sid, outline, first, text) = super::outline::fixture();
    let mut paragraphs = [first; 4];
    let mut texts = [text; 4];
    for at in 1..4 {
        let insertion =
            Insertion::paragraph(outline, None, &format!("Sibling {at}"), "Author").unwrap();
        source = PreparedEdit::insert(&source, sid, &insertion)
            .unwrap()
            .as_bytes()
            .to_vec();
        paragraphs[at] = insertion.object();
        texts[at] = insertion.text_object();
    }
    (source, sid, outline, paragraphs, texts)
}

fn children(source: &[u8], sid: ExGuid, object: ExGuid) -> Vec<ExGuid> {
    let store = Store::parse(source).unwrap();
    let index = RevisionIndex::parse(&store).unwrap();
    index.validate_current().unwrap();
    let document = Document::parse(&index).unwrap();
    let space = &document.spaces[&sid];
    space.revisions[&space.contexts[&ExGuid::default()]].nodes[&object]
        .children
        .clone()
}

#[test]
fn move_preserves_remote_content_and_dependent_edits_through_reopen() {
    let (source, sid, outline, paragraphs, texts) = fixture();
    let directory = tempfile::tempdir().unwrap();
    let path = directory.path().join("cache.sqlite");
    let cache = Replica::create(&path, &source).unwrap();
    let intent = TreeEdit::move_to(paragraphs[0], outline, None, "Offline").unwrap();
    let id = cache.tree(&source, sid, &intent).unwrap().unwrap();
    let dependent = cache
        .edit_text(&cache.snapshot().unwrap(), sid, texts[0], 0..0, "Local ")
        .unwrap()
        .unwrap();
    let local = cache.snapshot().unwrap();
    let queue = cache.pending().unwrap();
    drop(cache);
    let cache = Replica::open(&path).unwrap();
    assert_eq!(cache.pending().unwrap(), queue);
    assert_eq!(cache.snapshot().unwrap(), local);
    let edited = PreparedEdit::format(
        &source,
        sid,
        texts[0],
        0..8,
        &[onestore::TextAttribute::Bold(true)],
    )
    .unwrap();
    let descendant =
        Insertion::paragraph(paragraphs[0], None, "New remote child", "Remote").unwrap();
    let remote = PreparedEdit::insert(edited.as_bytes(), sid, &descendant).unwrap();
    let mut server = Server::new(remote.as_bytes());
    for expected in [id, dependent] {
        assert!(
            matches!(cache.sync_once(&mut server).unwrap(), Some((actual, EditStatus::Published { .. })) if actual == expected)
        );
    }
    assert_eq!(
        children(&server.durable, sid, outline),
        [paragraphs[1], paragraphs[2], paragraphs[3], paragraphs[0]]
    );
    assert_eq!(
        children(&server.durable, sid, paragraphs[0]),
        [descendant.object()]
    );
    let node = super::outline::node(&server.durable, sid, texts[0]);
    assert_eq!(node["kind"]["text"], "Local Original 🦀 é");
    let store = Store::parse(&server.durable).unwrap();
    let index = RevisionIndex::parse(&store).unwrap();
    let document = Document::parse(&index).unwrap();
    let space = &document.spaces[&sid];
    let view = &space.revisions[&space.contexts[&ExGuid::default()]];
    assert_eq!(view.text_runs(texts[0]).unwrap()[0].format.bold, Some(true));
    assert!(cache.pending().unwrap().is_empty());
    assert_eq!(server.publications, 2);
}

#[test]
fn deletion_requires_review_of_remote_content_and_preserves_later_work() {
    let (source, sid, outline, paragraphs, texts) = fixture();
    let child = Insertion::paragraph(paragraphs[0], None, "Descendant", "Author").unwrap();
    let source = PreparedEdit::insert(&source, sid, &child)
        .unwrap()
        .as_bytes()
        .to_vec();
    for remote in [
        PreparedEdit::text(&source, sid, texts[0], 0..0, "Remote ")
            .unwrap()
            .as_bytes()
            .to_vec(),
        PreparedEdit::text(&source, sid, child.text_object(), 0..0, "Remote ")
            .unwrap()
            .as_bytes()
            .to_vec(),
        PreparedEdit::format(
            &source,
            sid,
            child.text_object(),
            0..10,
            &[onestore::TextAttribute::Italic(true)],
        )
        .unwrap()
        .as_bytes()
        .to_vec(),
        PreparedEdit::insert(
            &source,
            sid,
            &Insertion::paragraph(paragraphs[0], None, "New child", "Remote").unwrap(),
        )
        .unwrap()
        .as_bytes()
        .to_vec(),
    ] {
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("cache.sqlite");
        let cache = Replica::create(&path, &source).unwrap();
        let intent = TreeEdit::delete(paragraphs[0], "Offline").unwrap();
        let id = cache.tree(&source, sid, &intent).unwrap().unwrap();
        let dependent = cache
            .edit_text(&cache.snapshot().unwrap(), sid, texts[1], 0..0, "Local ")
            .unwrap()
            .unwrap();
        let queue = cache.pending().unwrap();
        let local = cache.snapshot().unwrap();
        let mut server = Server::new(&remote);
        assert_eq!(
            cache.sync_once(&mut server).unwrap(),
            Some((id, EditStatus::Conflict(ConflictKind::ContentChanged)))
        );
        assert_eq!(server.publications, 0);
        assert_eq!(cache.pending().unwrap(), queue);
        assert_eq!(cache.snapshot().unwrap(), local);
        assert!(cache.rebase_tree_conflict(id, &source, &remote).is_err());
        assert!(cache.rebase_tree_conflict(id, &local, &source).is_err());
        assert!(
            cache
                .rebase_tree_conflict(dependent, &local, &remote)
                .is_err()
        );
        assert!(cache.rebase_conflict(id, &local, &remote, 0..0).is_err());
        let archive = directory.path().join("recovery.sqlite");
        cache.export_recovery(&archive).unwrap();
        let recovery = onestore_offline::Recovery::open(&archive).unwrap();
        assert_eq!(recovery.pending().unwrap(), queue);
        assert_eq!(recovery.status(id).unwrap(), cache.status(id).unwrap());
        assert!(recovery.snapshot().unwrap() == local);
        drop(cache);
        let cache = Replica::open(&path).unwrap();
        assert_eq!(
            cache.status(id).unwrap(),
            Some(EditStatus::Conflict(ConflictKind::ContentChanged))
        );
        cache.rebase_tree_conflict(id, &local, &remote).unwrap();
        assert_eq!(cache.pending().unwrap()[1], queue[1]);
        assert_eq!(cache.snapshot().unwrap(), local);
        for expected in [id, dependent] {
            assert!(
                matches!(cache.sync_once(&mut server).unwrap(), Some((actual, EditStatus::Published { .. })) if actual == expected)
            );
        }
        assert_eq!(children(&server.durable, sid, outline), paragraphs[1..]);
        assert_eq!(
            super::outline::node(&server.durable, sid, texts[1])["kind"]["text"],
            "Local Sibling 1"
        );
    }
}

#[test]
fn sibling_and_ancestry_changes_have_explicit_merge_or_conflict() {
    let (source, sid, outline, p, texts) = fixture();
    let insertion = Insertion::paragraph(outline, Some(p[0]), "New sibling", "Remote").unwrap();
    let cases = [
        (
            PreparedEdit::insert(&source, sid, &insertion)
                .unwrap()
                .as_bytes()
                .to_vec(),
            None,
        ),
        (
            PreparedEdit::tree(&source, sid, &TreeEdit::delete(p[1], "Remote").unwrap())
                .unwrap()
                .as_bytes()
                .to_vec(),
            None,
        ),
        (
            PreparedEdit::tree(
                &source,
                sid,
                &TreeEdit::move_to(p[1], outline, None, "Remote").unwrap(),
            )
            .unwrap()
            .as_bytes()
            .to_vec(),
            None,
        ),
        (
            PreparedEdit::tree(
                &source,
                sid,
                &TreeEdit::move_to(p[0], outline, Some(p[2]), "Remote").unwrap(),
            )
            .unwrap()
            .as_bytes()
            .to_vec(),
            Some(ConflictKind::StructureChanged),
        ),
        (
            PreparedEdit::tree(
                &source,
                sid,
                &TreeEdit::move_to(p[0], p[1], None, "Remote").unwrap(),
            )
            .unwrap()
            .as_bytes()
            .to_vec(),
            Some(ConflictKind::StructureChanged),
        ),
        (
            PreparedEdit::tree(&source, sid, &TreeEdit::delete(p[0], "Remote").unwrap())
                .unwrap()
                .as_bytes()
                .to_vec(),
            Some(ConflictKind::TargetUnavailable),
        ),
    ];
    for (remote, expected) in cases {
        let directory = tempfile::tempdir().unwrap();
        let cache = Replica::create(directory.path().join("cache.sqlite"), &source).unwrap();
        let intent = TreeEdit::move_to(p[0], outline, None, "Offline").unwrap();
        let id = cache.tree(&source, sid, &intent).unwrap().unwrap();
        let mut server = Server::new(&remote);
        let result = cache.sync_once(&mut server).unwrap().unwrap();
        assert_eq!(result.0, id);
        if let Some(kind) = expected {
            assert_eq!(result.1, EditStatus::Conflict(kind));
            assert_eq!(server.publications, 0);
        } else {
            assert!(matches!(result.1, EditStatus::Published { .. }));
            assert_eq!(children(&server.durable, sid, outline).last(), Some(&p[0]));
            assert_eq!(
                super::outline::node(&server.durable, sid, texts[0])["kind"]["text"],
                "Original 🦀 é"
            );
        }
    }
}

#[test]
fn moved_destination_and_missing_anchor_retain_conflicts_but_unrelated_text_does_not() {
    let (source, sid, outline, p, texts) = fixture();
    let child = Insertion::paragraph(p[1], None, "Anchor", "Author").unwrap();
    let source = PreparedEdit::insert(&source, sid, &child)
        .unwrap()
        .as_bytes()
        .to_vec();
    let intent = TreeEdit::move_to(p[0], p[1], Some(child.object()), "Offline").unwrap();
    let cases = [
        (
            PreparedEdit::tree(
                &source,
                sid,
                &TreeEdit::move_to(p[1], p[2], None, "Remote").unwrap(),
            )
            .unwrap()
            .as_bytes()
            .to_vec(),
            ConflictKind::StructureChanged,
        ),
        (
            PreparedEdit::tree(
                &source,
                sid,
                &TreeEdit::move_to(child.object(), outline, None, "Remote").unwrap(),
            )
            .unwrap()
            .as_bytes()
            .to_vec(),
            ConflictKind::UnsupportedEdit,
        ),
    ];
    for (remote, kind) in cases {
        let directory = tempfile::tempdir().unwrap();
        let cache = Replica::create(directory.path().join("cache.sqlite"), &source).unwrap();
        let id = cache.tree(&source, sid, &intent).unwrap().unwrap();
        let mut server = Server::new(&remote);
        assert_eq!(
            cache.sync_once(&mut server).unwrap(),
            Some((id, EditStatus::Conflict(kind)))
        );
        assert_eq!(server.publications, 0);
    }
    let directory = tempfile::tempdir().unwrap();
    let cache = Replica::create(directory.path().join("cache.sqlite"), &source).unwrap();
    let id = cache
        .tree(&source, sid, &TreeEdit::delete(p[0], "Offline").unwrap())
        .unwrap()
        .unwrap();
    let remote = PreparedEdit::text(&source, sid, texts[1], 0..0, "Remote ").unwrap();
    let mut server = Server::new(remote.as_bytes());
    assert!(
        matches!(cache.sync_once(&mut server).unwrap(), Some((n, EditStatus::Published { .. })) if n == id)
    );
    assert_eq!(
        super::outline::node(&server.durable, sid, texts[1])["kind"]["text"],
        "Remote Sibling 1"
    );
}

#[test]
fn unknown_tree_attempts_require_revision_evidence_even_when_effect_is_visible() {
    let (source, sid, outline, p, _) = fixture();
    for deletion in [false, true] {
        for fault in [
            Fault::UnknownBefore,
            Fault::UnknownAfter,
            Fault::Before,
            Fault::Committed,
        ] {
            let directory = tempfile::tempdir().unwrap();
            let path = directory.path().join("cache.sqlite");
            let cache = Replica::create(&path, &source).unwrap();
            let intent = if deletion {
                TreeEdit::delete(p[0], "Offline")
            } else {
                TreeEdit::move_to(p[0], outline, None, "Offline")
            }
            .unwrap();
            let id = cache.tree(&source, sid, &intent).unwrap().unwrap();
            let mut server = Server::new(&source);
            server.fault = fault;
            assert!(cache.sync_once(&mut server).is_err());
            let local = cache.snapshot().unwrap();
            let before = cache.status(id).unwrap();
            drop(cache);
            let cache = Replica::open(&path).unwrap();
            assert_eq!(cache.status(id).unwrap(), before);
            if matches!(fault, Fault::UnknownBefore) {
                let independent = if deletion {
                    TreeEdit::delete(p[0], "Other")
                } else {
                    TreeEdit::move_to(p[0], outline, None, "Other")
                }
                .unwrap();
                server = Server::new(
                    PreparedEdit::tree(&source, sid, &independent)
                        .unwrap()
                        .as_bytes(),
                );
                assert!(
                    matches!(cache.sync_once(&mut server).unwrap(), Some((n, EditStatus::AwaitingConfirmation { .. })) if n == id)
                );
                assert!(
                    cache
                        .rebase_tree_conflict(id, &local, &server.visible)
                        .is_err()
                );
                assert_eq!(server.publications, 0);
                assert_eq!(cache.snapshot().unwrap(), local);
            } else if matches!(fault, Fault::Committed) {
                assert!(matches!(
                    cache.status(id).unwrap(),
                    Some(EditStatus::Published { .. })
                ));
            } else {
                assert!(
                    matches!(cache.sync_once(&mut server).unwrap(), Some((n, EditStatus::Published { .. })) if n == id)
                );
                assert_eq!(
                    server.publications,
                    if matches!(fault, Fault::Before) { 2 } else { 1 }
                );
                let saved = cache.snapshot().unwrap();
                assert!(saved[..212] == server.durable[..212]);
                assert!(saved[252..] == server.durable[252..]);
                assert_eq!(
                    children(&saved, sid, outline),
                    children(&server.durable, sid, outline)
                );
            }
        }
    }
}

#[test]
fn independently_satisfied_move_confirms_without_republication() {
    let (source, sid, outline, p, _) = fixture();
    let directory = tempfile::tempdir().unwrap();
    let cache = Replica::create(directory.path().join("cache.sqlite"), &source).unwrap();
    let id = cache
        .tree(
            &source,
            sid,
            &TreeEdit::move_to(p[0], outline, None, "Offline").unwrap(),
        )
        .unwrap()
        .unwrap();
    let remote = PreparedEdit::tree(
        &source,
        sid,
        &TreeEdit::move_to(p[0], outline, None, "Remote").unwrap(),
    )
    .unwrap();
    let mut server = Server::new(remote.as_bytes());
    assert!(
        matches!(cache.sync_once(&mut server).unwrap(), Some((n, EditStatus::Published { .. })) if n == id)
    );
    assert_eq!(server.publications, 0);
    assert_eq!(server.confirmations, 1);
}

#[test]
fn emptied_cell_replacement_is_durable_and_cannot_be_silently_omitted_on_replay() {
    let source =
        include_bytes!("../../../../corpus/outline-edit/tree/before/notebook/synthetic.one");
    let store = Store::parse(source).unwrap();
    let index = RevisionIndex::parse(&store).unwrap();
    let document = Document::parse(&index).unwrap();
    let (sid, page) = document.pages().unwrap().into_iter().find(|(sid, _)| {
        let space = &document.spaces[sid];
        let view = &space.revisions[&space.contexts[&ExGuid::default()]];
        matches!(&view.nodes[&view.roots[&2]].kind, Kind::Metadata { title: Some(title), .. } if title == "Delete sole cell paragraph")
    }).unwrap();
    let space = &document.spaces[&sid];
    let view = &space.revisions[&space.contexts[&ExGuid::default()]];
    let mut pending = vec![page];
    let mut selected = None;
    let mut other = None;
    while let Some(id) = pending.pop() {
        let node = &view.nodes[&id];
        pending.extend(&node.children);
        pending.extend(&node.content);
        if matches!(node.kind, Kind::Cell { .. }) {
            let paragraph = node.children[0];
            let text = view.nodes[&paragraph].content[0];
            if matches!(&view.nodes[&text].kind, Kind::RichText { text, .. } if text.starts_with("Target "))
            {
                selected = Some((id, paragraph));
            } else {
                other = Some((id, text));
            }
        }
    }
    let (cell, target) = selected.unwrap();
    let (other_cell, other_text) = other.unwrap();
    for move_out in [false, true] {
        for competing_child in [false, true] {
            let directory = tempfile::tempdir().unwrap();
            let path = directory.path().join("cell.sqlite");
            let cache = Replica::create(&path, source).unwrap();
            let intent = if move_out {
                TreeEdit::move_to(target, other_cell, None, "Offline")
            } else {
                TreeEdit::delete(target, "Offline")
            }
            .unwrap();
            let id = cache.tree(source, sid, &intent).unwrap().unwrap();
            let local = cache.snapshot().unwrap();
            let replacement = children(&local, sid, cell);
            assert_eq!(replacement.len(), 1);
            assert_ne!(replacement[0], target);
            let replacement_text: ExGuid =
                super::outline::node(&local, sid, replacement[0])["content"][0]
                    .as_str()
                    .unwrap()
                    .parse()
                    .unwrap();
            let dependent = cache
                .edit_text(&local, sid, replacement_text, 0..0, "Local replacement")
                .unwrap()
                .unwrap();
            let local = cache.snapshot().unwrap();
            let queue = cache.pending().unwrap();
            drop(cache);
            let cache = Replica::open(&path).unwrap();
            assert_eq!(cache.pending().unwrap(), queue);
            assert_eq!(cache.snapshot().unwrap(), local);
            let remote = if competing_child {
                PreparedEdit::insert(
                    source,
                    sid,
                    &Insertion::paragraph(cell, None, "Remote sibling", "Remote").unwrap(),
                )
                .unwrap()
                .as_bytes()
                .to_vec()
            } else {
                PreparedEdit::text(source, sid, other_text, 0..0, "Remote ")
                    .unwrap()
                    .as_bytes()
                    .to_vec()
            };
            let mut server = Server::new(&remote);
            if competing_child {
                assert_eq!(
                    cache.sync_once(&mut server).unwrap(),
                    Some((id, EditStatus::Conflict(ConflictKind::StructureChanged)))
                );
                assert_eq!(server.publications, 0);
                assert!(cache.rebase_tree_conflict(id, &local, &remote).is_err());
                assert_eq!(cache.pending().unwrap(), queue);
                assert_eq!(cache.snapshot().unwrap(), local);
            } else {
                for expected in [id, dependent] {
                    assert!(
                        matches!(cache.sync_once(&mut server).unwrap(), Some((actual, EditStatus::Published { .. })) if actual == expected)
                    );
                }
                assert_eq!(children(&server.durable, sid, cell), replacement);
                assert_eq!(
                    super::outline::node(&server.durable, sid, replacement_text)["kind"]["text"],
                    "Local replacement"
                );
                assert_eq!(
                    super::outline::node(&server.durable, sid, other_text)["kind"]["text"],
                    "Remote Other cell"
                );
                if let Some(output) = std::env::var_os("ONESTORE_OFFLINE_TREE_OUTPUT") {
                    let output = std::path::PathBuf::from(output)
                        .join(if move_out { "cell-move" } else { "cell-delete" })
                        .join("candidate");
                    std::fs::create_dir_all(&output).unwrap();
                    std::fs::write(output.join("synthetic.one"), &server.durable).unwrap();
                }
            }
        }
    }
}

#[test]
fn native_tree_and_layout_changes_reconcile_without_discarding_unreviewed_content() {
    let source = include_bytes!("../../../../corpus/outline-edit/before/notebook/synthetic.one");
    let native = include_bytes!("../../../../corpus/outline-edit/after/notebook/synthetic.one");
    let store = Store::parse(source).unwrap();
    let index = RevisionIndex::parse(&store).unwrap();
    let document = Document::parse(&index).unwrap();
    let mut server = Server::new(native);
    let mut counts = [0; 3];
    let mut records = Vec::new();
    for (sid, page) in document.pages().unwrap() {
        let space = &document.spaces[&sid];
        let view = &space.revisions[&space.contexts[&ExGuid::default()]];
        let Kind::Metadata {
            title: Some(name), ..
        } = &view.nodes[&view.roots[&2]].kind
        else {
            continue;
        };
        if name == "Unicode rich text" {
            continue;
        }
        let outlines: Vec<_> = view.nodes[&page]
            .children
            .iter()
            .copied()
            .filter(|id| matches!(view.nodes[id].kind, Kind::Outline { .. }))
            .collect();
        if outlines.len() != 2 {
            continue;
        }
        let outline = outlines[0];
        let other = view.nodes[&outlines[1]].children[0];
        let dependent_text = view.nodes[&other].content[0];
        let mut pending = vec![outline];
        let mut paragraphs = std::collections::BTreeMap::new();
        while let Some(id) = pending.pop() {
            let node = &view.nodes[&id];
            pending.extend(&node.children);
            if let Some(text) = node.content.first()
                && let Kind::RichText { text, .. } = &view.nodes[text].kind
            {
                paragraphs.insert(text.as_str(), id);
            }
        }
        let target = *paragraphs
            .iter()
            .find(|(text, _)| text.starts_with("Target "))
            .unwrap()
            .1;
        let (intent, conflict) = match name.as_str() {
            "Move leaf down" | "Move subtree down" => {
                (TreeEdit::move_to(target, outline, None, "Offline"), None)
            }
            "Move subtree up" => (
                TreeEdit::move_to(target, outline, Some(paragraphs["Anchor"]), "Offline"),
                None,
            ),
            "Indent subtree" | "Outdent subtree" => (
                TreeEdit::delete(target, "Offline"),
                Some(ConflictKind::StructureChanged),
            ),
            "Collapse subtree" | "Expand subtree" => (
                TreeEdit::delete(target, "Offline"),
                Some(ConflictKind::ContentChanged),
            ),
            "Delete leaf" | "Delete subtree" | "Delete only paragraph" => (
                TreeEdit::delete(target, "Offline"),
                Some(ConflictKind::TargetUnavailable),
            ),
            "Delete outline" => (
                TreeEdit::delete(outline, "Offline"),
                Some(ConflictKind::TargetUnavailable),
            ),
            "Move outline" | "Resize outline" | "Automatic outline size" => (
                TreeEdit::delete(outline, "Offline"),
                Some(ConflictKind::ContentChanged),
            ),
            _ => panic!("{name}"),
        };
        let intent = intent.unwrap();
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("native.sqlite");
        let cache = Replica::create(&path, source).unwrap();
        let id = cache.tree(source, sid, &intent).unwrap().unwrap();
        let dependent = cache
            .edit_text(
                &cache.snapshot().unwrap(),
                sid,
                dependent_text,
                0..0,
                "Offline ",
            )
            .unwrap()
            .unwrap();
        let local = cache.snapshot().unwrap();
        let queue = cache.pending().unwrap();
        let old_publications = server.publications;
        let (actual, status) = cache.sync_once(&mut server).unwrap().unwrap();
        assert_eq!(actual, id);
        if let Some(kind) = conflict {
            assert_eq!(status, EditStatus::Conflict(kind), "{name}");
            assert_eq!(server.publications, old_publications);
            assert_eq!(cache.pending().unwrap(), queue);
            assert!(cache.snapshot().unwrap() == local);
            drop(cache);
            let cache = Replica::open(&path).unwrap();
            assert_eq!(cache.status(id).unwrap(), Some(status));
            if kind == ConflictKind::TargetUnavailable {
                counts[2] += 1;
                assert!(
                    cache
                        .rebase_tree_conflict(id, &local, &server.visible)
                        .is_err()
                );
                records.push(serde_json::json!({"page": name, "space": sid, "result": "retained"}));
                continue;
            }
            counts[1] += 1;
            cache
                .rebase_tree_conflict(id, &local, &server.visible)
                .unwrap();
            assert_eq!(cache.pending().unwrap()[1], queue[1]);
            for expected in [id, dependent] {
                assert!(
                    matches!(cache.sync_once(&mut server).unwrap(), Some((actual, EditStatus::Published { .. })) if actual == expected)
                );
            }
        } else {
            counts[0] += 1;
            assert!(matches!(status, EditStatus::Published { .. }), "{name}");
            assert_eq!(server.publications, old_publications);
            assert!(
                matches!(cache.sync_once(&mut server).unwrap(), Some((actual, EditStatus::Published { .. })) if actual == dependent)
            );
        }
        assert_eq!(
            super::outline::node(&server.durable, sid, dependent_text)["kind"]["text"],
            format!(
                "Offline {}",
                match &view.nodes[&dependent_text].kind {
                    Kind::RichText { text, .. } => text,
                    _ => panic!(),
                }
            )
        );
        records.push(serde_json::json!({"page": name, "space": sid, "result": if conflict.is_some() { "reviewed" } else { "converged" }}));
    }
    assert_eq!(counts, [3, 7, 4]);
    if let Some(output) = std::env::var_os("ONESTORE_OFFLINE_TREE_OUTPUT") {
        let output = std::path::PathBuf::from(output);
        std::fs::create_dir_all(output.join("candidate")).unwrap();
        std::fs::write(output.join("candidate/synthetic.one"), &server.durable).unwrap();
        std::fs::write(
            output.join("manifest.json"),
            serde_json::to_vec_pretty(&records).unwrap(),
        )
        .unwrap();
    }
}
