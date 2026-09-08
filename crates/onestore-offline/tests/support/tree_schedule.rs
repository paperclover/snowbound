use crate::disk;
use onestore::{
    CommitError, ExGuid, Insertion, PreparedEdit, RevisionIndex, Store, TreeEdit,
    document::{Document, Kind},
};
use onestore_offline::{EditStatus, Operation, Remote, Replica};
use std::{io, sync::LazyLock};

static SOURCE: LazyLock<(Vec<u8>, ExGuid, ExGuid)> = LazyLock::new(|| {
    let mut source =
        onestore::create_section("tree-schedule.one", "Original 🦀 é 0", "Author").unwrap();
    let store = Store::parse(&source).unwrap();
    let index = RevisionIndex::parse(&store).unwrap();
    let document = Document::parse(&index).unwrap();
    let (sid, page) = document.pages().unwrap()[0];
    let space = &document.spaces[&sid];
    let view = &space.revisions[&space.contexts[&ExGuid::default()]];
    let outline = *view.nodes[&page]
        .children
        .iter()
        .find(|id| matches!(view.nodes[id].kind, Kind::Outline { .. }))
        .unwrap();
    for at in 1..12 {
        let insert =
            Insertion::paragraph(outline, None, &format!("Original 🦀 é {at}"), "Author").unwrap();
        source = PreparedEdit::insert(&source, sid, &insert)
            .unwrap()
            .as_bytes()
            .to_vec();
    }
    (source, sid, outline)
});

fn rows(source: &[u8]) -> Vec<(ExGuid, ExGuid, String)> {
    let (_, sid, outline) = &*SOURCE;
    let store = Store::parse(source).unwrap();
    assert!(store.checksum_mismatches.is_empty());
    let index = RevisionIndex::parse(&store).unwrap();
    index.validate_current().unwrap();
    let document = Document::parse(&index).unwrap();
    let space = &document.spaces[sid];
    let view = &space.revisions[&space.contexts[&ExGuid::default()]];
    let (_, page) = document
        .pages()
        .unwrap()
        .into_iter()
        .find(|(space, _)| space == sid)
        .unwrap();
    if !view.nodes[&page].children.contains(outline) {
        return Vec::new();
    }
    let node = &view.nodes[outline];
    let mut rows = Vec::new();
    for paragraph in &node.children {
        let node = &view.nodes[paragraph];
        assert!(node.children.is_empty());
        let [text] = node.content.as_slice() else {
            panic!()
        };
        let Kind::RichText { text: value, .. } = &view.nodes[text].kind else {
            panic!()
        };
        rows.push((*paragraph, *text, value.clone()));
    }
    rows
}

struct Session<'a> {
    disk: &'a mut disk::Disk,
    operation: Option<&'a Operation>,
}

impl Remote for Session<'_> {
    fn read(&mut self) -> io::Result<Vec<u8>> {
        Ok(self.disk.visible.clone())
    }
    fn publish(&mut self, edit: &PreparedEdit<'_>) -> Result<(), CommitError> {
        let before = rows(&self.disk.visible);
        let mut expected = before.clone();
        match self.operation.unwrap() {
            Operation::Tree(edit) => {
                let at = expected
                    .iter()
                    .position(|(id, _, _)| *id == edit.intent.object())
                    .unwrap();
                let row = expected.remove(at);
                if let Some((parent, anchor)) = edit.intent.destination() {
                    assert_eq!(parent, SOURCE.2);
                    let at = anchor
                        .map(|anchor| {
                            expected
                                .iter()
                                .position(|(id, _, _)| *id == anchor)
                                .unwrap()
                        })
                        .unwrap_or(expected.len());
                    expected.insert(at, row);
                }
            }
            Operation::Text(edit) => {
                assert_eq!(edit.range, 0..1);
                let (_, _, text) = expected
                    .iter_mut()
                    .find(|(_, id, _)| *id == edit.object)
                    .unwrap();
                text.replace_range(0..1, &edit.replacement);
            }
            _ => panic!(),
        }
        assert_eq!(rows(edit.as_bytes()), expected);
        let result = edit.commit(self.disk);
        let durable = rows(&self.disk.durable);
        assert!(durable == before || durable == expected);
        if result.is_ok() {
            assert_eq!(durable, expected);
        }
        result
    }
    fn confirm(&mut self, snapshot: &[u8]) -> Result<(), CommitError> {
        onestore::confirm_snapshot(self.disk, snapshot)
    }
}

pub fn run(input: &[u8]) {
    let (source, sid, outline) = &*SOURCE;
    let directory = tempfile::tempdir().unwrap();
    let mut replicas: [Option<Replica>; 12] = std::array::from_fn(|_| None);
    let mut disk = disk::Disk {
        visible: source.clone(),
        durable: source.clone(),
        operation: 0,
        fail_at: None,
        write_limit: 4096,
        random: 1,
    };
    for step in input.chunks_exact(8).take(48) {
        let actor = usize::from(step[0]) % replicas.len();
        let path = directory.path().join(format!("{actor}.sqlite"));
        let cache = replicas[actor].get_or_insert_with(|| Replica::create(&path, source).unwrap());
        let snapshot = cache.snapshot().unwrap();
        let pending = cache.pending().unwrap();
        let local = rows(&snapshot);
        match step[1] % 8 {
            0..=2 if !local.is_empty() => {
                let target = usize::from(step[2]) % local.len();
                let id = match step[1] % 8 {
                    0 => {
                        let before =
                            (step[3] & 1 == 0).then(|| local[usize::from(step[4]) % local.len()].0);
                        cache
                            .tree(
                                &snapshot,
                                *sid,
                                &TreeEdit::move_to(local[target].0, *outline, before, "Offline")
                                    .unwrap(),
                            )
                            .unwrap()
                    }
                    1 => cache
                        .tree(
                            &snapshot,
                            *sid,
                            &TreeEdit::delete(local[target].0, "Offline").unwrap(),
                        )
                        .unwrap(),
                    _ => cache
                        .edit_text(
                            &snapshot,
                            *sid,
                            local[target].1,
                            0..1,
                            &char::from(b'A' + step[3] % 26).to_string(),
                        )
                        .unwrap(),
                };
                let next = cache.pending().unwrap();
                assert_eq!(next[..pending.len()], pending);
                assert_eq!(next.len(), pending.len() + usize::from(id.is_some()));
            }
            3 | 4 => {
                disk.operation = 0;
                disk.fail_at = (step[4] != 0)
                    .then_some(usize::from(u16::from_le_bytes([step[4], step[5]])) % 2048 + 1);
                disk.write_limit = if step[6] & 1 == 0 { 17 } else { 4096 };
                disk.random = u64::from(step[7]) + 1;
                let _ = cache.sync_once(&mut Session {
                    disk: &mut disk,
                    operation: pending.first().map(|p| &p.operation),
                });
                let next = cache.pending().unwrap();
                if next.len() == pending.len() {
                    assert_eq!(next, pending);
                } else {
                    assert_eq!(next, pending[1..]);
                    let Some(EditStatus::Published { revision }) =
                        cache.status(pending[0].id).unwrap()
                    else {
                        panic!()
                    };
                    let store = Store::parse(&disk.durable).unwrap();
                    let index = RevisionIndex::parse(&store).unwrap();
                    assert!(index.spaces[sid].revisions.contains_key(&revision));
                }
                if !next.is_empty() {
                    assert!(cache.snapshot().unwrap() == snapshot);
                }
                disk.visible.clone_from(&disk.durable);
            }
            5 => {
                let mut expected = rows(&disk.visible);
                if !expected.is_empty() {
                    let target = usize::from(step[2]) % expected.len();
                    let (_, text, content) = &mut expected[target];
                    let at = u32::try_from(content.encode_utf16().count()).unwrap();
                    content.push('R');
                    let snapshot = disk.visible.clone();
                    let edit = PreparedEdit::text(&snapshot, *sid, *text, at..at, "R").unwrap();
                    disk.fail_at = None;
                    edit.commit(&mut disk).unwrap();
                    assert_eq!(rows(&disk.durable), expected);
                }
            }
            _ => {
                if step[1] % 8 == 6
                    && let Some(first) = pending.first()
                    && matches!(first.operation, Operation::Tree(_))
                    && matches!(
                        cache.status(first.id).unwrap(),
                        Some(EditStatus::Conflict(_))
                    )
                {
                    let remote = cache.remote_snapshot().unwrap();
                    let _ = cache.rebase_tree_conflict(first.id, &snapshot, &remote);
                }
                let pending = cache.pending().unwrap();
                drop(replicas[actor].take());
                let cache = Replica::open(&path).unwrap();
                assert!(cache.snapshot().unwrap() == snapshot);
                assert_eq!(cache.pending().unwrap(), pending);
                replicas[actor] = Some(cache);
            }
        }
        rows(&disk.durable);
        rows(&replicas[actor].as_ref().unwrap().snapshot().unwrap());
    }
}
