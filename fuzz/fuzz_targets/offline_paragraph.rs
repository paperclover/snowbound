#![no_main]
use libfuzzer_sys::fuzz_target;
use onestore::{
    CommitError, ExGuid, ParagraphJoin, ParagraphSplit, PreparedEdit, RevisionIndex, Store,
    document::{Document, Kind},
};
use onestore_offline::{EditStatus, Remote, Replica};
use std::{io, sync::LazyLock};

#[path = "../../crates/onestore/tests/support/disk.rs"]
mod disk;

const TEXT: &str = "a🦀 e\u{301} 東京\rEnd";

static SOURCE: LazyLock<(Vec<u8>, ExGuid, ExGuid)> = LazyLock::new(|| {
    let source = onestore::create_section("offline-paragraph.one", TEXT, "Fixture").unwrap();
    let store = Store::parse(&source).unwrap();
    let index = RevisionIndex::parse(&store).unwrap();
    let doc = Document::parse(&index).unwrap();
    let (sid, page) = doc.pages().unwrap()[0];
    let space = &doc.spaces[&sid];
    let view = &space.revisions[&space.contexts[&ExGuid::default()]];
    let outline = *view.nodes[&page]
        .children
        .iter()
        .find(|id| matches!(view.nodes[id].kind, Kind::Outline { .. }))
        .unwrap();
    (source, sid, outline)
});

struct Server(disk::Disk);

impl Remote for Server {
    fn read(&mut self) -> io::Result<Vec<u8>> {
        Ok(self.0.visible.clone())
    }
    fn publish(&mut self, edit: &PreparedEdit<'_>) -> Result<(), CommitError> {
        edit.commit(&mut self.0)
    }
    fn confirm(&mut self, snapshot: &[u8]) -> Result<(), CommitError> {
        onestore::confirm_snapshot(&mut self.0, snapshot)
    }
}

fn paragraphs(source: &[u8], sid: ExGuid, outline: ExGuid) -> Vec<(ExGuid, String)> {
    let store = Store::parse(source).unwrap();
    assert!(store.checksum_mismatches.is_empty());
    let index = RevisionIndex::parse(&store).unwrap();
    index.validate_current().unwrap();
    let doc = Document::parse(&index).unwrap();
    let space = &doc.spaces[&sid];
    let view = &space.revisions[&space.contexts[&ExGuid::default()]];
    let mut output = Vec::new();
    for paragraph in &view.nodes[&outline].children {
        let node = &view.nodes[paragraph];
        assert!(node.children.is_empty());
        let [text] = node.content.as_slice() else {
            panic!()
        };
        let Kind::RichText { text: content, .. } = &view.nodes[text].kind else {
            panic!()
        };
        output.push((*text, content.clone()));
    }
    assert_eq!(
        output
            .iter()
            .map(|(_, text)| text.as_str())
            .collect::<String>(),
        TEXT
    );
    output
}

fuzz_target!(|input: &[u8]| {
    if input.len() < 8 {
        return;
    }
    let (source, sid, outline) = &*SOURCE;
    let directory = tempfile::tempdir().unwrap();
    let mut replicas: [Option<Replica>; 12] = std::array::from_fn(|_| None);
    let mut server = Server(disk::Disk {
        visible: source.clone(),
        durable: source.clone(),
        operation: 0,
        fail_at: None,
        write_limit: 4096,
        random: 1,
    });
    for step in input.chunks_exact(8).take(24) {
        let actor = usize::from(step[0]) % replicas.len();
        let path = directory.path().join(format!("{actor}.sqlite"));
        let cache = replicas[actor].get_or_insert_with(|| Replica::create(&path, source).unwrap());
        let snapshot = cache.snapshot().unwrap();
        let pending = cache.pending().unwrap();
        let rows = paragraphs(&snapshot, *sid, *outline);
        match step[1] % 5 {
            0 => {
                let (text, content) = &rows[usize::from(step[2]) % rows.len()];
                let offsets: Vec<u32> = std::iter::once(0)
                    .chain(content.chars().scan(0, |at, c| {
                        *at += c.len_utf16() as u32;
                        Some(*at)
                    }))
                    .collect();
                let offset = offsets[usize::from(step[3]) % offsets.len()];
                let intent = ParagraphSplit::new(*text, offset, "Fuzz").unwrap();
                let id = cache.split(&snapshot, *sid, &intent).unwrap().unwrap();
                assert_eq!(cache.status(id).unwrap(), Some(EditStatus::Pending));
                let next = cache.pending().unwrap();
                assert_eq!(next[..pending.len()], pending);
                assert_eq!(next.len(), pending.len() + 1);
            }
            1 if rows.len() > 1 => {
                let pair = usize::from(step[2]) % (rows.len() - 1);
                let intent = ParagraphJoin::new(rows[pair].0, rows[pair + 1].0, "Fuzz").unwrap();
                let id = cache.join(&snapshot, *sid, &intent).unwrap().unwrap();
                assert_eq!(cache.status(id).unwrap(), Some(EditStatus::Pending));
                let next = cache.pending().unwrap();
                assert_eq!(next[..pending.len()], pending);
                assert_eq!(next.len(), pending.len() + 1);
            }
            2 | 3 => {
                server.0.operation = 0;
                server.0.fail_at =
                    (step[4] != 0).then_some(usize::from(u16::from_le_bytes([step[4], step[5]])));
                server.0.write_limit = if step[6] & 1 == 0 { 17 } else { 4096 };
                server.0.random = u64::from(step[7]) + 1;
                let _ = cache.sync_once(&mut server);
                let after = cache.pending().unwrap();
                assert!(after.len() == pending.len() || after.len() + 1 == pending.len());
                if after.len() == pending.len() {
                    assert_eq!(after, pending);
                } else {
                    assert_eq!(after, pending[1..]);
                    let Some(EditStatus::Published { revision }) =
                        cache.status(pending[0].id).unwrap()
                    else {
                        panic!()
                    };
                    let store = Store::parse(&server.0.durable).unwrap();
                    let index = RevisionIndex::parse(&store).unwrap();
                    assert!(index.spaces[sid].revisions.contains_key(&revision));
                }
                if !after.is_empty() {
                    assert!(cache.snapshot().unwrap() == snapshot);
                }
                server.0.visible.clone_from(&server.0.durable);
            }
            _ => {
                drop(replicas[actor].take());
                let reopened = Replica::open(&path).unwrap();
                assert!(reopened.snapshot().unwrap() == snapshot);
                assert_eq!(reopened.pending().unwrap(), pending);
                replicas[actor] = Some(reopened);
            }
        }
        paragraphs(&server.0.durable, *sid, *outline);
        paragraphs(
            &replicas[actor].as_ref().unwrap().snapshot().unwrap(),
            *sid,
            *outline,
        );
    }
});
