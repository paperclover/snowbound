#[path = "support/ops.rs"]
mod ops;
#[path = "support/current.rs"]
mod current;
#[path = "support/disk.rs"]
mod disk;
#[path = "support/sweep.rs"]
mod sweep;
use onestore::{
    ExGuid, RevisionIndex, Store, TextAttribute as A,
    document::{Document, Kind},
    op::PageOp,
};
use serde_json::{Value, json};
fn target(source: &[u8]) -> (ExGuid, ExGuid) {
    let store = Store::parse(source).unwrap();
    let index = RevisionIndex::parse(&store).unwrap();
    let doc = Document::parse(&index).unwrap();
    let (sid, page) = doc.pages().unwrap()[0];
    let space = &doc.spaces[&sid];
    let view = &space.revisions[&space.contexts[&ExGuid::default()]];
    let mut pending = view.nodes[&page].children.clone();
    while let Some(id) = pending.pop() {
        let n = &view.nodes[&id];
        if matches!(n.kind, Kind::RichText { .. }) {
            return (sid, id);
        }
        pending.extend(&n.children);
        pending.extend(&n.content);
    }
    panic!("Missing text")
}
fn characters(source: &[u8], sid: ExGuid, id: ExGuid) -> Vec<(char, Value)> {
    let store = Store::parse(source).unwrap();
    let index = RevisionIndex::parse(&store).unwrap();
    index.validate_current().unwrap();
    let doc = Document::parse(&index).unwrap();
    let s = &doc.spaces[&sid];
    let view = &s.revisions[&s.contexts[&ExGuid::default()]];
    view.text_runs(id)
        .unwrap()
        .into_iter()
        .flat_map(|r| {
            let format = serde_json::to_value(r.format).unwrap();
            r.text.chars().map(move |c| (c, format.clone()))
        })
        .collect()
}
#[test]
fn overlapping_unicode_format_edits_match_an_independent_character_model() {
    let text = "abcdefgh 東京 🦀 café\rSecond\tline";
    let original = onestore::create_section("format.one", text, "Author").unwrap();
    let (sid, id) = target(&original);
    for seed in sweep::seeds(1..17, 16) {
        let mut rng = seed;
        let mut source = original.clone();
        let mut expected = characters(&source, sid, id);
        let offsets: Vec<u32> = std::iter::once(0)
            .chain(text.chars().scan(0, |n, c| {
                *n += c.len_utf16() as u32;
                Some(*n)
            }))
            .collect();
        for step in 0..24 {
            let mut next = || {
                rng ^= rng << 13;
                rng ^= rng >> 7;
                rng ^= rng << 17;
                rng
            };
            let start = next() as usize % expected.len();
            let end = start + 1 + next() as usize % (expected.len() - start);
            let enabled = next() & 1 != 0;
            let (attribute, key, value) = match step % 9 {
                0 => (A::Bold(enabled), "bold", json!(enabled)),
                1 => (A::Italic(enabled), "italic", json!(enabled)),
                2 => (A::Underline(enabled), "underline", json!(enabled)),
                3 => (A::Strike(enabled), "strike", json!(enabled)),
                4 => (A::Font("Arial".into()), "font", json!("Arial")),
                5 => (A::FontSize(13.5), "font_size", json!(13.5)),
                6 => (
                    A::Color(Some([0x24, 0x68, 0xac])),
                    "color",
                    json!(0xac6824_u32),
                ),
                7 => (A::Highlight(None), "highlight", json!(0xff000000_u32)),
                _ => (
                    A::Highlight(Some([0, 255, 0])),
                    "highlight",
                    json!(0x00ff00_u32),
                ),
            };
            let edit = ops::page_op(&source, sid, PageOp::Format { text: id, range: offsets[start]..offsets[end], set: std::slice::from_ref(&attribute).to_vec(), clear: Vec::new() })
            .unwrap();
            for (_, style) in &mut expected[start..end] {
                style[key] = value.clone();
            }
            assert_eq!(
                characters(edit.as_bytes(), sid, id),
                expected,
                "seed {seed}, step {step}"
            );
            assert_eq!(
                ops::page_op(edit.as_bytes(), sid, PageOp::Format { text: id, range: offsets[start]..offsets[end], set: vec![attribute], clear: Vec::new() })
                .unwrap()
                .as_bytes(),
                edit.as_bytes()
            );
            let old_store = Store::parse(&source).unwrap();
            let old = RevisionIndex::parse(&old_store).unwrap();
            let new_store = Store::parse(edit.as_bytes()).unwrap();
            let new = RevisionIndex::parse(&new_store).unwrap();
            let rid = old.spaces[&sid].labels[&(ExGuid::default(), 1)];
            assert_eq!(
                format!("{:?}", old.resolve(sid, rid).unwrap()),
                format!("{:?}", new.resolve(sid, rid).unwrap())
            );
            source = edit.as_bytes().to_vec();
        }
    }
}
#[test]
fn script_positions_are_exclusive_and_explicit_false_overrides_true() {
    let source = onestore::create_section("script.one", "abc", "Author").unwrap();
    let (sid, id) = target(&source);
    let superscript = ops::page_op(&source, sid, PageOp::Format { text: id, range: 0..3, set: vec![A::Superscript(true), A::Bold(true)], clear: Vec::new() })
    .unwrap();
    let subscript = ops::page_op(superscript.as_bytes(), sid, PageOp::Format { text: id, range: 1..2, set: vec![A::Subscript(true), A::Bold(false)], clear: Vec::new() })
    .unwrap();
    let chars = characters(subscript.as_bytes(), sid, id);
    assert_eq!(chars[0].1["superscript"], true);
    assert_eq!(chars[0].1["subscript"], false);
    assert_eq!(chars[0].1["bold"], true);
    assert_eq!(chars[1].1["superscript"], false);
    assert_eq!(chars[1].1["subscript"], true);
    assert_eq!(chars[1].1["bold"], false);
    assert_eq!(chars[0].1, chars[2].1);
}
#[test]
fn empty_paragraph_style_is_used_by_later_text_edits() {
    let source = onestore::create_section("empty.one", "", "Author").unwrap();
    let (sid, id) = target(&source);
    let formatted = ops::page_op(&source, sid, PageOp::Format { text: id, range: 0..0, set: vec![A::Italic(true), A::FontSize(18.0)], clear: Vec::new() })
    .unwrap();
    let filled = ops::page_op(formatted.as_bytes(), sid, PageOp::Text { text: id, range: 0..0, with: "Added 🦀".into() }).unwrap();
    for (_, format) in characters(filled.as_bytes(), sid, id) {
        assert_eq!(format["italic"], true);
        assert_eq!(format["font_size"], 18.0);
    }
}
#[test]
fn invalid_ranges_attributes_and_fields_are_rejected() {
    let source = onestore::create_section("invalid.one", "a🦀b", "Author").unwrap();
    let (sid, id) = target(&source);
    for (start, end) in [(2, 3), (1, 2), (0, 8), (2, 2), (3, 1), (1, 1)] {
        let range = start..end;
        assert!(ops::page_op(&source, sid, PageOp::Format { text: id, range, set: vec![A::Bold(true)], clear: Vec::new() }).is_err());
    }
    for attributes in [
        vec![],
        vec![A::Bold(true), A::Bold(false)],
        vec![A::Superscript(true), A::Subscript(true)],
        vec![A::Font("".into())],
        vec![A::Font("a\0b".into())],
    ] {
        assert!(ops::page_op(&source, sid, PageOp::Format { text: id, range: 0..4, set: attributes.to_vec(), clear: Vec::new() }).is_err());
    }
    for size in [f32::NAN, f32::INFINITY, 0.0, 5.5, 130.5, 144.0, 144.5, 12.1] {
        assert!(ops::page_op(&source, sid, PageOp::Format { text: id, range: 0..4, set: vec![A::FontSize(size)], clear: Vec::new() }).is_err());
    }
    assert!(ops::page_op(&source, sid, PageOp::Format { text: ExGuid::default(), range: 0..4, set: vec![A::Bold(true)], clear: Vec::new() }).is_err());
}
#[test]
fn formatting_publication_faults_preserve_complete_old_or_new_styles() {
    let source = onestore::create_section("atomic.one", "Before 🦀 after", "Author").unwrap();
    let (sid, id) = target(&source);
    let edit = ops::page_op(&source, sid, PageOp::Format { text: id, range: 2..10, set: vec![A::Bold(true), A::Color(Some([8, 64, 128]))], clear: Vec::new() })
    .unwrap();
    let before = current::current(&source);
    let after = current::current(edit.as_bytes());
    for write_limit in [17, 4096] {
        let disk = |fail_at| disk::Disk {
            visible: source.clone(),
            durable: source.clone(),
            operation: 0,
            fail_at,
            write_limit,
            random: 946,
        };
        let mut success = disk(None);
        edit.commit(&mut success).unwrap();
        assert_eq!(success.durable, edit.as_bytes());
        for at in 1..=success.operation {
            let mut interrupted = disk(Some(at));
            let failure = edit.commit(&mut interrupted).unwrap_err();
            let actual = current::current(&interrupted.durable);
            assert!(actual == before || actual == after, "operation {at}");
            if failure.state == onestore::CommitState::NotCommitted {
                assert_eq!(actual, before);
            }
        }
    }
}

#[test]
#[ignore = "exports public-API formatting candidates for cold native validation"]
fn export_native_formatting_candidates() {
    use std::{fs, path::PathBuf};
    let output = PathBuf::from(std::env::var_os("ONESTORE_FORMAT_OUTPUT").unwrap());
    assert!(output.is_absolute());
    fs::create_dir(&output).unwrap();
    let source = onestore::create_section(
        "format.one",
        "Before café 東京 🦀 after",
        "Formatting author",
    )
    .unwrap();
    let (sid, id) = target(&source);
    let mut manifest = Vec::new();
    let mut save =
        |name: &str, source: &[u8], sid, id, range: std::ops::Range<u32>, attributes: &[A]| {
            let prepared =
                ops::page_op(source, sid, PageOp::Format { text: id, range: range.clone(), set: attributes.to_vec(), clear: Vec::new() }).unwrap();
            fs::write(output.join(format!("{name}.one")), prepared.as_bytes()).unwrap();
            manifest.push(
                json!({"name":name,"space":sid,"object":id,"range":range,"attributes":attributes}),
            );
            prepared.as_bytes().to_vec()
        };
    save(
        "partial-boolean",
        &source,
        sid,
        id,
        2..18,
        &[
            A::Bold(true),
            A::Italic(true),
            A::Underline(true),
            A::Strike(true),
        ],
    );
    let colored = save(
        "partial-font-color",
        &source,
        sid,
        id,
        3..18,
        &[
            A::Font("Arial".into()),
            A::FontSize(13.5),
            A::Color(Some([24, 96, 160])),
            A::Highlight(Some([255, 255, 0])),
        ],
    );
    save(
        "clear-color",
        &colored,
        sid,
        id,
        6..14,
        &[A::Color(None), A::Highlight(None)],
    );
    let scripted = save("subscript", &source, sid, id, 0..23, &[A::Subscript(true)]);
    save(
        "superscript",
        &scripted,
        sid,
        id,
        6..18,
        &[A::Superscript(true)],
    );
    let bold = save(
        "bold",
        &source,
        sid,
        id,
        0..23,
        &[
            A::Bold(true),
            A::Italic(true),
            A::Underline(true),
            A::Strike(true),
        ],
    );
    save(
        "clear-boolean",
        &bold,
        sid,
        id,
        7..14,
        &[
            A::Bold(false),
            A::Italic(false),
            A::Underline(false),
            A::Strike(false),
        ],
    );
    let empty = onestore::create_section("empty.one", "", "Author").unwrap();
    let (empty_sid, empty_id) = target(&empty);
    let formatted = ops::page_op(&empty, empty_sid, PageOp::Format { text: empty_id, range: 0..0, set: vec![A::FontSize(18.0), A::Italic(true)], clear: Vec::new() })
    .unwrap();
    let typed = ops::page_op(formatted.as_bytes(), empty_sid, PageOp::Text { text: empty_id, range: 0..0, with: "Typed café 🦀".into() })
    .unwrap();
    fs::write(output.join("empty-then-type.one"), typed.as_bytes()).unwrap();
    let native = include_bytes!("../../../corpus/native-ink/20260905-ui/notebook/synthetic.one");
    let store = Store::parse(native).unwrap();
    let index = RevisionIndex::parse(&store).unwrap();
    let document = Document::parse(&index).unwrap();
    let (native_sid, native_id) = document
        .spaces
        .iter()
        .find_map(|(sid, s)| {
            let r = &s.revisions[&s.contexts[&ExGuid::default()]];
            r.nodes.iter().find_map(|(id, n)| {
                matches!(&n.kind,Kind::RichText{text,..} if text.starts_with("Fictitious:" ))
                    .then_some((*sid, *id))
            })
        })
        .unwrap();
    save(
        "native-cross-runs",
        native,
        native_sid,
        native_id,
        2..26,
        &[A::Bold(false), A::Italic(true), A::Underline(true)],
    );
    save(
        "native-partial",
        native,
        native_sid,
        native_id,
        3..8,
        &[A::FontSize(14.0), A::Color(Some([16, 112, 48]))],
    );
    save(
        "native-font",
        native,
        native_sid,
        native_id,
        0..27,
        &[A::Font("Arial".into())],
    );
    save("small-font", &source, sid, id, 0..23, &[A::FontSize(6.0)]);
    save("large-font", &source, sid, id, 0..23, &[A::FontSize(130.0)]);
    for points in [129.5, 130.0] {
        save(
            &format!("font-boundary-{points}"),
            &source,
            sid,
            id,
            0..23,
            &[A::FontSize(points)],
        );
    }
    fs::write(output.join("basic-baseline.one"), &source).unwrap();
    fs::write(output.join("native-baseline.one"), native).unwrap();
    fs::write(
        output.join("manifest.json"),
        serde_json::to_vec_pretty(&manifest).unwrap(),
    )
    .unwrap();
}
