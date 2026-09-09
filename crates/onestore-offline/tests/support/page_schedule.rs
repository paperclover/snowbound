use crate::disk;
use onestore::{
    CommitError, ExGuid, Insertion, PageEdit, PagePosition, PreparedEdit, RevisionIndex, Store,
    document::{Document, Kind},
};
use onestore_offline::{EditStatus, Operation, Remote, Replica};
use std::{io, sync::LazyLock};

#[path = "../../../onestore/tests/support/current.rs"]
mod current;

static SOURCE: LazyLock<Vec<u8>> = LazyLock::new(|| {
    let mut source = onestore::create_section("page-schedule.one", "Original", "Author").unwrap();
    for _ in 0..3 {
        let page = onestore::PageCreation::new(None, Some("Same title"), "Author").unwrap();
        source = PreparedEdit::create_page(&source, &page)
            .unwrap()
            .as_bytes()
            .to_vec();
    }
    source
});

fn pages(source: &[u8]) -> Vec<(ExGuid, ExGuid, u32)> {
    let store = Store::parse(source).unwrap();
    let index = RevisionIndex::parse(&store).unwrap();
    index.validate_current().unwrap();
    let document = Document::parse(&index).unwrap();
    document
        .pages()
        .unwrap()
        .into_iter()
        .map(|(sid, page)| {
            let space = &document.spaces[&sid];
            let view = &space.revisions[&space.contexts[&ExGuid::default()]];
            let Kind::Metadata { level, .. } = view.nodes[&view.roots[&2]].kind else {
                panic!()
            };
            (sid, page, level.unwrap_or(1))
        })
        .collect()
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
        let mut expected = pages(&self.disk.visible);
        match self.operation.unwrap() {
            Operation::CreatePage(page) => expected.push((page.space(), page.object(), 1)),
            Operation::Pages(batch) => {
                for change in &batch.edits {
                    let at = expected.iter().position(|p| p.0 == change.space()).unwrap();
                    expected[at].2 = change.level();
                    if let PagePosition::Before(before) = change.position() {
                        let page = expected.remove(at);
                        let at = before.map_or(expected.len(), |before| {
                            expected.iter().position(|p| p.0 == before).unwrap()
                        });
                        expected.insert(at, page);
                    }
                }
            }
            Operation::Insert(insertion) => {
                let store = Store::parse(edit.as_bytes()).unwrap();
                let index = RevisionIndex::parse(&store).unwrap();
                let document = Document::parse(&index).unwrap();
                let matching: Vec<_> = document
                    .spaces
                    .values()
                    .filter_map(|space| {
                        let view = &space.revisions[&space.contexts[&ExGuid::default()]];
                        view.nodes.get(&insertion.text_object())
                    })
                    .collect();
                assert_eq!(matching.len(), 1);
                assert!(
                    matches!(&matching[0].kind, Kind::RichText { text, .. } if text == "Body 🦋 é")
                );
            }
            _ => panic!(),
        }
        assert_eq!(pages(edit.as_bytes()), expected);
        let old = current::current(&self.disk.durable);
        let new = current::current(edit.as_bytes());
        let result = edit.commit(self.disk);
        let observed = current::current(&self.disk.durable);
        assert!(observed == old || observed == new);
        if result.is_ok() {
            assert_eq!(observed, new);
        }
        result
    }

    fn confirm(&mut self, snapshot: &[u8]) -> Result<(), CommitError> {
        onestore::confirm_snapshot(self.disk, snapshot)
    }
}

pub fn run(input: &[u8]) {
    let directory = tempfile::tempdir().unwrap();
    let mut replicas: [Option<Replica>; 12] = std::array::from_fn(|_| None);
    let mut owned: [Vec<(ExGuid, ExGuid)>; 12] = std::array::from_fn(|_| Vec::new());
    let mut disk = disk::Disk {
        visible: SOURCE.clone(),
        durable: SOURCE.clone(),
        operation: 0,
        fail_at: None,
        write_limit: 4096,
        random: 1,
    };
    for step in input.chunks_exact(8).take(48) {
        let actor = usize::from(step[0]) % replicas.len();
        let path = directory.path().join(format!("{actor}.sqlite"));
        let cache = replicas[actor].get_or_insert_with(|| Replica::create(&path, &SOURCE).unwrap());
        let snapshot = cache.snapshot().unwrap();
        let pending = cache.pending().unwrap();
        match step[1] % 8 {
            0 | 1 => {
                let title = (step[2] & 1 != 0).then_some("Same 🦋 é");
                let page = onestore::PageCreation::new(None, title, "Author").unwrap();
                cache.create_page(&snapshot, &page).unwrap().unwrap();
                owned[actor].push((page.space(), page.object()));
                if step[1] % 8 == 1 {
                    let insertion =
                        Insertion::outline(page.object(), 36.0, 36.0, "Body 🦋 é", "Author")
                            .unwrap();
                    cache
                        .insert(&cache.snapshot().unwrap(), page.space(), &insertion)
                        .unwrap()
                        .unwrap();
                }
            }
            2 => {
                let statuses: Vec<_> = pending
                    .iter()
                    .map(|edit| cache.status(edit.id).unwrap())
                    .collect();
                replicas[actor] = None;
                let cache = Replica::open(&path).unwrap();
                assert!(cache.snapshot().unwrap() == snapshot);
                assert_eq!(cache.pending().unwrap(), pending);
                assert_eq!(
                    pending
                        .iter()
                        .map(|edit| cache.status(edit.id).unwrap())
                        .collect::<Vec<_>>(),
                    statuses
                );
                replicas[actor] = Some(cache);
            }
            3 | 4 => {
                disk.operation = 0;
                disk.write_limit = if step[3] & 1 == 0 { 17 } else { 4096 };
                disk.fail_at =
                    (step[2] != 0).then_some(usize::from(u16::from_le_bytes([step[2], step[3]])));
                disk.random = u64::from(step[4]) + 1;
                let prior = pending
                    .first()
                    .and_then(|edit| cache.status(edit.id).unwrap());
                let mut session = Session {
                    disk: &mut disk,
                    operation: pending.first().map(|edit| &edit.operation),
                };
                let result = cache.sync_once(&mut session);
                if matches!(prior, Some(EditStatus::AwaitingConfirmation { .. })) {
                    assert!(
                        result.is_err()
                            || !matches!(result.unwrap(), Some((_, EditStatus::Conflict(_))))
                    );
                }
            }
            5 => {
                disk.visible.clone_from(&disk.durable);
                disk.fail_at = None;
            }
            6 => {
                let order = pages(&snapshot);
                let count = 1 + usize::from(step[3] & 1 != 0);
                let selected: Vec<_> = (0..count)
                    .map(|i| order[(usize::from(step[2]) + i) % order.len()].0)
                    .collect();
                let anchor = (step[4] & 1 != 0)
                    .then_some(order[usize::from(step[5]) % order.len()].0)
                    .filter(|sid| !selected.contains(sid));
                let edits: Vec<_> = selected
                    .iter()
                    .enumerate()
                    .map(|(i, sid)| {
                        let level = u32::from(step[6 + i]) % 3 + 1;
                        if step[4] & 2 == 0 {
                            PageEdit::set_level(*sid, level).unwrap()
                        } else {
                            PageEdit::move_to(*sid, anchor, level).unwrap()
                        }
                    })
                    .collect();
                let expected = PreparedEdit::pages(&snapshot, &edits);
                match cache.pages(&snapshot, &edits) {
                    Ok(id) => {
                        let expected = expected.unwrap();
                        assert_eq!(
                            current::current(&cache.snapshot().unwrap()),
                            current::current(expected.as_bytes())
                        );
                        assert_eq!(id.is_some(), expected.as_bytes() != snapshot);
                    }
                    Err(_) => {
                        assert!(expected.is_err());
                        assert_eq!(cache.snapshot().unwrap(), snapshot);
                        assert_eq!(cache.pending().unwrap(), pending);
                    }
                }
            }
            7 => {
                if let Some(edit) = pending.first()
                    && matches!(
                        cache.status(edit.id).unwrap(),
                        Some(EditStatus::Conflict(_))
                    )
                    && let Operation::Pages(batch) = &edit.operation
                {
                    let remote = cache.remote_snapshot().unwrap();
                    let order = pages(&remote);
                    let anchor = (step[2] & 1 != 0)
                        .then_some(order[usize::from(step[3]) % order.len()].0)
                        .filter(|sid| batch.edits.iter().all(|e| e.space() != *sid));
                    let edits: Vec<_> = batch
                        .edits
                        .iter()
                        .map(|edit| edit.reposition(PagePosition::Before(anchor), 1).unwrap())
                        .collect();
                    let result = cache.rebase_pages_conflict(edit.id, &snapshot, &remote, &edits);
                    assert_eq!(cache.snapshot().unwrap(), snapshot);
                    if result.is_err() {
                        assert_eq!(cache.pending().unwrap(), pending);
                    }
                }
            }
            _ => unreachable!(),
        }
        let local = pages(&replicas[actor].as_ref().unwrap().snapshot().unwrap());
        for page in &owned[actor] {
            assert!(local.iter().any(|(sid, oid, _)| (*sid, *oid) == *page));
        }
    }
}
