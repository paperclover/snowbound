#![no_main]
//! Replaces a native text with arbitrary text: the edit either refuses it or appends a
//! revision storing it, leaving the old header's view of the file intact.
use libfuzzer_sys::fuzz_target;
use onestore::{
    ExGuid, RevisionIndex, Store,
    document::{Document, Kind},
    op::{Op, PageOp},
};
use std::sync::LazyLock;

#[path = "../../crates/onestore/tests/support/ops.rs"]
mod ops;

const SOURCE: &[u8] =
    include_bytes!("../../corpus/native/20260905-05/snapshots/02-text/notebook/synthetic.one");

fn text(bytes: &[u8], target: Option<(ExGuid, ExGuid)>) -> Vec<(ExGuid, ExGuid, String)> {
    let store = Store::parse(bytes).unwrap();
    assert!(store.checksum_mismatches.is_empty());
    let index = RevisionIndex::parse(&store).unwrap();
    index.validate_current().unwrap();
    let document = Document::parse(&index).unwrap();
    let mut found = Vec::new();
    for (sid, space) in &document.spaces {
        let revision = &space.revisions[&space.contexts[&ExGuid::default()]];
        for (oid, node) in &revision.nodes {
            if let Kind::RichText { text, .. } = &node.kind
                && target.is_none_or(|target| target == (*sid, *oid))
            {
                found.push((*sid, *oid, text.clone()));
            }
        }
    }
    found
}

static TARGET: LazyLock<(ExGuid, ExGuid)> = LazyLock::new(|| {
    text(SOURCE, None)
        .into_iter()
        .find_map(|(sid, oid, text)| (text == "Fictitious plain text.").then_some((sid, oid)))
        .expect("Missing native seed text")
});

fuzz_target!(|value: &[u8]| {
    let (sid, oid) = *TARGET;
    let value = String::from_utf8_lossy(value);
    let op = PageOp::Text {
        text: oid,
        range: 0.."Fictitious plain text.".len() as u32,
        with: value.to_string(),
    };
    let edited = ops::apply(SOURCE, "Fuzz", vec![Op::Page { space: sid, op }]);
    if value.contains(['\0', '\n', '\u{fffc}']) {
        assert!(edited.is_err());
        return;
    }
    let bytes = edited.unwrap().image;
    let mut before_commit = bytes.clone();
    before_commit[..1024].copy_from_slice(&SOURCE[..1024]);
    assert_eq!(
        text(&before_commit, Some((sid, oid)))[0].2,
        "Fictitious plain text."
    );
    assert_eq!(text(&bytes, Some((sid, oid)))[0].2, value);
});
