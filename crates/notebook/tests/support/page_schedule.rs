use crate::{
    disk, model_ops,
    server::{remote_snapshot, snapshot},
};
use notebook::{Remote, Replica};
use onestore::{
    CommitError, ExGuid, PageEdit, RevisionIndex, Stamp, Store, Transaction,
    document::{Document, Format, Kind, Layout},
    op::SectionOp,
    page::{
        Outline, Page, PageObject, PageParagraph, Paragraph, ParagraphContent, TextObject,
        text::new_id,
    },
};
use std::{io, sync::LazyLock};

#[path = "../../../onestore/tests/support/ops.rs"]
mod ops;

const BODY: &str = "Body 🦋 é";

#[path = "../../../onestore/tests/support/current.rs"]
mod current;

static SOURCE: LazyLock<Vec<u8>> = LazyLock::new(|| {
    let mut source = onestore::create_section("page-schedule.one", "Original", "Author").unwrap();
    for _ in 0..3 {
        let page = onestore::PageCreation::new(None, Some("Same title"), "Author").unwrap();
        source = ops::section_op(&source, SectionOp::Create(page.clone()))
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

/// Adds a body outline holding one plain paragraph, returning its text identity. A page the
/// section just created has no body text for `model_ops::insert_outline` to copy formatting from.
pub fn body_outline(page: &mut Page, text: &str) -> ExGuid {
    // The writer creates outline text with the store's default style; the model must state it.
    let format = Format {
        font: Some("Calibri".to_owned()),
        font_size: Some(11.0),
        language: Some(0x409),
        ..Default::default()
    };
    let content = TextObject {
        id: new_id().unwrap(),
        date_field: None,
        text: Paragraph::new(text.into(), format),
        tags: Vec::new(),
    };
    let id = content.id;
    let outline = Outline {
        id: new_id().unwrap(),
        title: false,
        min_width: None,
        layout: Layout {
            x: Some(36.0),
            y: Some(36.0),
            ..Default::default()
        },
        indents: Vec::new(),
        paragraphs: vec![PageParagraph {
            id: new_id().unwrap(),
            parent: None,
            level: 1,
            style: None,
            format: Default::default(),
            content: ParagraphContent::Text(content),
            lists: Vec::new(),
            tags: Vec::new(),
            media: Default::default(),
            collapsed: false,
        }],
        unsupported: Vec::new(),
    };
    let at = page
        .objects
        .iter()
        .position(|object| matches!(object, PageObject::Title(_)))
        .unwrap_or(page.objects.len());
    page.objects.insert(at, PageObject::Outline(outline));
    id
}

/// Publishes through the crash-injecting disk, asserting each commit is atomic.
struct Session<'a> {
    disk: &'a mut disk::Disk,
}

impl Remote for Session<'_> {
    fn read(&mut self) -> io::Result<Vec<u8>> {
        Ok(self.disk.visible.clone())
    }

    fn stamp(&mut self) -> io::Result<Stamp> {
        Stamp::of(&self.disk.visible).map_err(io::Error::other)
    }

    fn publish(&mut self, transaction: &Transaction) -> Result<(), CommitError> {
        let mut image = self.disk.visible.clone();
        transaction.apply(&mut image).unwrap();
        let old = current::current(&self.disk.durable);
        let new = current::current(&image);
        let result = transaction.commit(self.disk);
        let observed = current::current(&self.disk.durable);
        assert!(observed == old || observed == new);
        if result.is_ok() {
            assert_eq!(observed, new);
        }
        result
    }

    fn confirm(&mut self, base: &Stamp) -> Result<(), CommitError> {
        onestore::confirm(self.disk, base)
    }
}

fn section_op(cache: &Replica, op: onestore::op::SectionOp) -> Result<u64, notebook::Error> {
    cache.apply(
        "Author",
        onestore::op::Edit {
            at: 133_000_000_000_000_000,
            ops: vec![onestore::op::Op::Section(op)],
        },
    )
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
        let image = snapshot(cache);
        let listed = pages(&image);
        let pending = cache.pending().unwrap();
        match step[1] % 8 {
            0 | 1 => {
                let title = (step[2] & 1 != 0).then_some("Same 🦋 é");
                let page = onestore::PageCreation::new(None, title, "Author").unwrap();
                section_op(cache, onestore::op::SectionOp::Create(page.clone())).unwrap();
                owned[actor].push((page.space(), page.object()));
                if step[1] % 8 == 1 {
                    let mut model = cache.page(page.space()).unwrap();
                    body_outline(&mut model, BODY);
                    model_ops::save_as(cache, page.space(), &model, "Author")
                        .unwrap()
                        .unwrap();
                    let local = cache.page(page.space()).unwrap();
                    assert!(
                        local
                            .objects
                            .iter()
                            .any(|object| matches!(object, PageObject::Outline(o) if o.paragraphs[0].text().is_some_and(|t| t.text.text() == BODY)))
                    );
                }
            }
            2 => {
                let statuses: Vec<_> = pending
                    .iter()
                    .map(|edit| cache.status(edit.id).unwrap())
                    .collect();
                replicas[actor] = None;
                let cache = Replica::open(&path).unwrap();
                assert_eq!(pages(&snapshot(&cache)), listed);
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
                let result = cache.sync_once(&mut Session { disk: &mut disk });
                if result.is_ok() && cache.pending().unwrap().is_empty() {
                    assert_eq!(pages(&snapshot(cache)), pages(&remote_snapshot(cache)));
                }
            }
            5 => {
                disk.visible.clone_from(&disk.durable);
                disk.fail_at = None;
            }
            6 => {
                let count = 1 + usize::from(step[3] & 1 != 0);
                let selected: Vec<_> = (0..count)
                    .map(|i| listed[(usize::from(step[2]) + i) % listed.len()].0)
                    .collect();
                let anchor = (step[4] & 1 != 0)
                    .then_some(listed[usize::from(step[5]) % listed.len()].0)
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
                let expected = ops::section_op(&image, SectionOp::Pages(edits.to_vec()));
                match section_op(cache, onestore::op::SectionOp::Pages(edits)) {
                    Ok(_) => assert_eq!(
                        pages(&snapshot(cache)),
                        pages(expected.unwrap().as_bytes())
                    ),
                    Err(_) => {
                        assert!(expected.is_err());
                        assert_eq!(pages(&snapshot(cache)), listed);
                        assert_eq!(cache.pending().unwrap(), pending);
                    }
                }
            }
            7 => {
                // The actor merged a conflict page's version by hand and deletes it.
                if let Some(conflict) = cache
                    .conflicts()
                    .unwrap()
                    .first()
                    .map(|(_, pages)| pages[usize::from(step[2]) % pages.len()].space)
                {
                    section_op(cache, onestore::op::SectionOp::Delete(vec![conflict])).unwrap();
                    assert_eq!(pages(&snapshot(cache)), listed);
                }
            }
            _ => unreachable!(),
        }
        let local = pages(&snapshot(replicas[actor].as_ref().unwrap()));
        for page in &owned[actor] {
            assert!(local.iter().any(|(sid, oid, _)| (*sid, *oid) == *page));
        }
    }
}
