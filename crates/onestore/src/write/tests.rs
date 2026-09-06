use super::patch_properties;
use crate::{PropertySets, Value, create::properties};

#[test]
fn document_insertions_and_formatting_respect_readonly_ancestors() {
    use super::{PropertyObject, write_revision};
    use crate::{ExGuid, Insertion, PreparedEdit, RevisionIndex, Store};
    use std::collections::BTreeMap;
    let source = crate::create_section("readonly.one", "Original", "Author").unwrap();
    let store = Store::parse(&source).unwrap();
    let index = RevisionIndex::parse(&store).unwrap();
    let (sid, page, outline, paragraph) = index
        .spaces
        .iter()
        .find_map(|(sid, s)| {
            let raw = index
                .resolve(*sid, s.labels[&(ExGuid::default(), 1)])
                .unwrap();
            let by_type = |jcid| {
                raw.objects
                    .iter()
                    .find_map(|(id, o)| (o.jcid == jcid).then_some(*id))
            };
            Some((
                *sid,
                by_type(0x6000b)?,
                by_type(0x6000c)?,
                by_type(0x6000d)?,
            ))
        })
        .unwrap();
    for blocked in [page, outline, paragraph] {
        let protected = write_revision(&source, sid, |raw| {
            let mut object = PropertyObject::from_object(&raw.objects[&blocked])?;
            object.set(&[(0x88001cde, &[])])?;
            Ok(BTreeMap::from([(blocked, object)]))
        })
        .unwrap();
        let child = Insertion::paragraph(paragraph, None, "Nested", "Author").unwrap();
        assert!(PreparedEdit::insert(&protected, sid, &child).is_err());
        let raw = index
            .resolve(sid, index.spaces[&sid].labels[&(ExGuid::default(), 1)])
            .unwrap();
        let text = raw
            .objects
            .iter()
            .find_map(|(id, object)| (object.jcid == 0x6000e).then_some(*id))
            .unwrap();
        assert!(
            PreparedEdit::format(
                &protected,
                sid,
                text,
                1..3,
                &[crate::TextAttribute::Bold(true)]
            )
            .is_err()
        );
        if blocked == page {
            let outline = Insertion::outline(page, 36.0, 36.0, "Outline", "Author").unwrap();
            assert!(PreparedEdit::insert(&protected, sid, &outline).is_err());
        }
    }
}

fn add_paragraph(source: &[u8], number: u32) -> Vec<u8> {
    use super::{PropertyObject, compact, write_revision};
    use crate::{ExGuid, ObjectData, RevisionIndex, Store};
    use std::{collections::BTreeMap, sync::Arc};
    let store = Store::parse(source).unwrap();
    let index = RevisionIndex::parse(&store).unwrap();
    let space = index
        .spaces
        .iter()
        .find_map(|(sid, s)| {
            let revision = index
                .resolve(*sid, s.labels[&(ExGuid::default(), 1)])
                .unwrap();
            revision
                .objects
                .values()
                .any(|o| o.jcid == 0x6000c)
                .then_some(*sid)
        })
        .unwrap();
    write_revision(source, space, |revision| {
        let (&outline_id, outline) = revision
            .objects
            .iter()
            .find(|(_, o)| o.jcid == 0x6000c)
            .unwrap();
        let (&author, _) = revision
            .objects
            .iter()
            .find(|(_, o)| o.jcid == 0x120001)
            .unwrap();
        let (&style, _) = revision
            .objects
            .iter()
            .find(|(_, o)| o.jcid == 0x12004d)
            .unwrap();
        let mut guid = [0x69; 16];
        guid[..4].copy_from_slice(&number.to_le_bytes());
        let paragraph = ExGuid { guid, n: 1 };
        let text = ExGuid { guid, n: 2 };
        let mut table = (*outline.global_ids).clone();
        for guid in [guid, author.guid, style.guid] {
            if !table.values().any(|previous| *previous == guid) {
                table.insert(table.last_key_value().map_or(0, |(i, _)| i + 1), guid);
            }
        }
        let table = Arc::new(table);
        let ObjectData::Properties(blob) = outline.data else {
            unreachable!()
        };
        let parsed = PropertySets::parse(blob).unwrap();
        let Value::References { compact_ids, .. } = parsed.sets[0]
            .iter()
            .find(|p| p.id == 0x24001c20)
            .unwrap()
            .value
        else {
            unreachable!()
        };
        let mut children = compact_ids.to_vec();
        children.extend_from_slice(&compact(paragraph, &table).unwrap());
        let modified = crate::create::current_timestamps()?.0.to_le_bytes();
        let mut changed = BTreeMap::new();
        changed.insert(
            outline_id,
            PropertyObject {
                jcid: outline.jcid,
                bytes: patch_properties(
                    blob,
                    &[(0x24001c20, &children), (0x14001d7a, &modified)],
                    &[],
                )?,
                global_ids: Arc::clone(&table),
            },
        );
        changed.insert(
            paragraph,
            PropertyObject {
                jcid: 0x6000d,
                bytes: properties(&[
                    (0x14001d7a, modified.to_vec()),
                    (0x14001d09, modified.to_vec()),
                    (0x0c001c03, vec![1]),
                    (0x24001c1f, compact(text, &table)?.to_vec()),
                    (0x20001d78, compact(author, &table)?.to_vec()),
                    (0x20001d79, compact(author, &table)?.to_vec()),
                ])?,
                global_ids: Arc::clone(&table),
            },
        );
        changed.insert(
            text,
            PropertyObject {
                jcid: 0x6000e,
                bytes: properties(&[
                    (0x14001d7a, modified.to_vec()),
                    (
                        0x1c001c22,
                        format!("Paragraph {number}\0")
                            .encode_utf16()
                            .flat_map(u16::to_le_bytes)
                            .collect(),
                    ),
                    (0x24001e13, compact(style, &table)?.to_vec()),
                ])?,
                global_ids: table,
            },
        );
        Ok(changed)
    })
    .unwrap()
}

fn restyle(source: &[u8]) -> Vec<u8> {
    use super::{PropertyObject, compact, write_revision};
    use crate::{ExGuid, ObjectData, RevisionIndex, Store};
    use std::{collections::BTreeMap, sync::Arc};
    let store = Store::parse(source).unwrap();
    let index = RevisionIndex::parse(&store).unwrap();
    let space = index
        .spaces
        .iter()
        .find_map(|(sid, s)| {
            let revision = index
                .resolve(*sid, s.labels[&(ExGuid::default(), 1)])
                .unwrap();
            revision
                .objects
                .values()
                .any(|o| o.jcid == 0x6000c)
                .then_some(*sid)
        })
        .unwrap();
    let style = ExGuid {
        guid: [0x74; 16],
        n: 1,
    };
    write_revision(source, space, |revision| {
        let previous = revision
            .objects
            .values()
            .find(|o| o.jcid == 0x12004d)
            .unwrap();
        let ObjectData::Properties(blob) = previous.data else {
            unreachable!()
        };
        let mut table = (*previous.global_ids).clone();
        table.insert(table.last_key_value().unwrap().0 + 1, style.guid);
        let mut changed = BTreeMap::from([(
            style,
            PropertyObject {
                jcid: previous.jcid,
                bytes: patch_properties(blob, &[], &[(0x88001c04, &[])])?,
                global_ids: Arc::new(table),
            },
        )]);
        for (id, text) in revision.objects.iter().filter(|(_, o)| o.jcid == 0x6000e) {
            let ObjectData::Properties(blob) = text.data else {
                unreachable!()
            };
            let mut table = (*text.global_ids).clone();
            table.insert(table.last_key_value().unwrap().0 + 1, style.guid);
            changed.insert(
                *id,
                PropertyObject {
                    jcid: text.jcid,
                    bytes: patch_properties(blob, &[(0x24001e13, &compact(style, &table)?)], &[])?,
                    global_ids: Arc::new(table),
                },
            );
        }
        Ok(changed)
    })
    .unwrap()
}

#[test]
fn replaced_readonly_styles_retain_history_with_zero_current_references() {
    use crate::{ExGuid, RevisionIndex, Store};
    let source = add_paragraph(
        &crate::create_section("style.one", "Original", "Author").unwrap(),
        1,
    );
    let changed = restyle(&source);
    let store = Store::parse(&changed).unwrap();
    let index = RevisionIndex::parse(&store).unwrap();
    index.validate_current().unwrap();
    let mut styles = Vec::new();
    for (sid, s) in &index.spaces {
        let revision = index
            .resolve(*sid, s.labels[&(ExGuid::default(), 1)])
            .unwrap();
        for (id, object) in &revision.objects {
            if object.jcid == 0x12004d {
                styles.push((object.reference_count, id.guid));
            }
        }
    }
    styles.sort();
    assert_eq!(styles.len(), 2);
    assert_eq!(styles[0].0, 0);
    assert_eq!(styles[1], (2, [0x74; 16]));
    let document = crate::document::Document::parse(&index).unwrap();
    let mut count = 0;
    for space in document.spaces.values() {
        for revision in space.revisions.values() {
            for (id, node) in &revision.nodes {
                if matches!(node.kind, crate::document::Kind::RichText { .. }) {
                    for run in revision.text_runs(*id).unwrap() {
                        assert_eq!(run.format.bold, Some(true));
                    }
                    count += 1;
                }
            }
        }
    }
    assert_eq!(count, 2);
}

#[test]
#[ignore = "Writes cold-native candidates to ONESTORE_GROWTH_OUTPUT"]
fn export_native_growth_candidates() {
    let output = std::path::PathBuf::from(std::env::var_os("ONESTORE_GROWTH_OUTPUT").unwrap());
    std::fs::create_dir(&output).unwrap();
    let mut source = crate::create_section("growth.one", "Original", "Author").unwrap();
    for number in 1..=24 {
        source = add_paragraph(&source, number);
    }
    std::fs::write(output.join("growth.one"), &source).unwrap();
    std::fs::write(output.join("restyled.one"), restyle(&source)).unwrap();
}

#[test]
fn atomic_graph_growth_preserves_history_and_counts_shared_readonly_objects() {
    use crate::{ExGuid, RevisionIndex, Store};
    let initial = crate::create_section("growth.one", "Original", "Author").unwrap();
    let mut source = initial.clone();
    for number in 1..=24 {
        source = add_paragraph(&source, number);
        let store = Store::parse(&source).unwrap();
        let index = RevisionIndex::parse(&store).unwrap();
        index.validate_current().unwrap();
        assert!(store.checksum_mismatches.is_empty());
        assert_eq!(store.header.transaction_count, number + 1);
        let mut text_count = 0;
        for (sid, s) in &index.spaces {
            let revision = index
                .resolve(*sid, s.labels[&(ExGuid::default(), 1)])
                .unwrap();
            let reachable = revision.reachable().unwrap();
            for id in reachable {
                let object = &revision.objects[&id];
                match object.jcid {
                    0x6000e => text_count += 1,
                    0x120001 => assert_eq!(object.reference_count, (number + 1) * 2),
                    0x12004d => assert_eq!(object.reference_count, number + 1),
                    _ => {}
                }
            }
        }
        assert_eq!(text_count, number + 1);
    }
    let initial_store = Store::parse(&initial).unwrap();
    let initial_index = RevisionIndex::parse(&initial_store).unwrap();
    let store = Store::parse(&source).unwrap();
    let index = RevisionIndex::parse(&store).unwrap();
    for (sid, s) in &initial_index.spaces {
        let rid = s.labels[&(ExGuid::default(), 1)];
        let before = initial_index.resolve(*sid, rid).unwrap();
        let after = index.resolve(*sid, rid).unwrap();
        assert_eq!(before.roots, after.roots);
        for (id, object) in &before.objects {
            assert_eq!(object.data, after.objects[id].data);
            assert_eq!(object.reference_count, after.objects[id].reference_count);
        }
    }
}

#[test]
fn changed_graphs_reject_cycles_dangling_and_unreachable_additions() {
    use super::{PropertyObject, compact, write_revision};
    use crate::{ExGuid, ObjectData, RevisionIndex, Store};
    use std::{collections::BTreeMap, sync::Arc};
    let source = crate::create_section("invalid.one", "Original", "Author").unwrap();
    let store = Store::parse(&source).unwrap();
    let index = RevisionIndex::parse(&store).unwrap();
    for (sid, s) in &index.spaces {
        let revision = index
            .resolve(*sid, s.labels[&(ExGuid::default(), 1)])
            .unwrap();
        let Some((&oid, outline)) = revision.objects.iter().find(|(_, o)| o.jcid == 0x6000c) else {
            continue;
        };
        let ObjectData::Properties(blob) = outline.data else {
            unreachable!()
        };
        for target in [
            oid,
            ExGuid {
                guid: oid.guid,
                n: 255,
            },
        ] {
            assert!(
                write_revision(&source, *sid, |_| Ok(BTreeMap::from([(
                    oid,
                    PropertyObject {
                        jcid: outline.jcid,
                        bytes: patch_properties(
                            blob,
                            &[(0x24001c20, &compact(target, &outline.global_ids)?)],
                            &[]
                        )?,
                        global_ids: Arc::clone(&outline.global_ids),
                    }
                )])))
                .is_err()
            );
        }
        let orphan = ExGuid {
            guid: oid.guid,
            n: 255,
        };
        assert!(
            write_revision(&source, *sid, |_| Ok(BTreeMap::from([(
                orphan,
                PropertyObject {
                    jcid: 0x6000e,
                    bytes: properties(&[])?,
                    global_ids: Arc::clone(&outline.global_ids),
                }
            )])))
            .is_err()
        );
    }
}

#[test]
fn property_splices_match_independently_encoded_flat_sets() {
    let mut seed = 0x749391acb66327d5_u64;
    let mut next = || {
        seed ^= seed << 13;
        seed ^= seed >> 7;
        seed ^= seed << 17;
        seed
    };
    for case in 0..20_000 {
        let mut original = Vec::new();
        let mut expected = Vec::new();
        let mut updates = Vec::new();
        let mut inserts = Vec::new();
        for index in 0..next() % 30 {
            let kind = 2 + (next() % 8) as u32;
            let mut id = (kind << 26) | index as u32;
            let length = |random: u64| match kind {
                2 => 0,
                3..=6 => 1 << (kind - 3),
                7 => (random % 20) as usize,
                8 => 4,
                9 => (random % 5) as usize * 4,
                _ => unreachable!(),
            };
            let value = (0..length(next()))
                .map(|_| next() as u8)
                .collect::<Vec<_>>();
            if kind == 2 && next() & 1 != 0 {
                id |= 0x80000000;
            }
            original.push((id, value.clone()));
            if next() & 1 != 0 {
                if kind == 2 {
                    id ^= 0x80000000;
                }
                let value = (0..length(next()))
                    .map(|_| next() as u8)
                    .collect::<Vec<_>>();
                expected.push((id, value.clone()));
                updates.push((id, value));
            } else {
                expected.push((id, value));
            }
        }
        for index in 0..next() % 6 {
            let kind = [2, 7, 8, 9][(next() % 4) as usize];
            let mut id = (kind << 26) | (100 + index as u32);
            if kind == 2 && next() & 1 != 0 {
                id |= 0x80000000;
            }
            let length = match kind {
                2 => 0,
                7 => next() as usize % 10,
                8 => 4,
                9 => (next() as usize % 4) * 4,
                _ => unreachable!(),
            };
            let value = (0..length).map(|_| next() as u8).collect::<Vec<_>>();
            inserts.push((id, value.clone()));
            expected.push((id, value));
        }
        updates.reverse();
        let updates: Vec<_> = updates
            .iter()
            .map(|(id, value)| (*id, value.as_slice()))
            .collect();
        let inserts: Vec<_> = inserts
            .iter()
            .map(|(id, value)| (*id, value.as_slice()))
            .collect();
        let original = properties(&original).unwrap();
        let actual = patch_properties(&original, &updates, &inserts).unwrap();
        assert_eq!(actual, properties(&expected).unwrap(), "case {case}");
    }
}

#[test]
fn nested_fields_and_other_reference_streams_remain_byte_exact() {
    let mut original = 0x40000003_u32.to_le_bytes().to_vec();
    original.extend_from_slice(&[1, 0, 0, 0, 2, 0, 0, 0, 3, 0, 0, 0]);
    original.extend_from_slice(&0x40000001_u32.to_le_bytes());
    original.extend_from_slice(&[4, 0, 0, 0]);
    original.extend_from_slice(&1_u32.to_le_bytes());
    original.extend_from_slice(&[5, 0, 0, 0]);
    original.extend_from_slice(&4_u16.to_le_bytes());
    for id in [0x24000001_u32, 0x40000002, 0x24000003, 0x1c000004] {
        original.extend_from_slice(&id.to_le_bytes());
    }
    original.extend_from_slice(&1_u32.to_le_bytes());
    let nested_start = original.len();
    original.extend_from_slice(&1_u32.to_le_bytes());
    original.extend_from_slice(&0x44003456_u32.to_le_bytes());
    original.extend_from_slice(&4_u16.to_le_bytes());
    for id in [0x20000001_u32, 0x28000002, 0x30000003, 0x1c000004] {
        original.extend_from_slice(&id.to_le_bytes());
    }
    original.extend_from_slice(&3_u32.to_le_bytes());
    original.extend_from_slice(&[91, 92, 93]);
    let nested_end = original.len();
    original.extend_from_slice(&1_u32.to_le_bytes());
    original.extend_from_slice(&2_u32.to_le_bytes());
    original.extend_from_slice(&[94, 95]);
    original.resize(original.len().next_multiple_of(8), 0);
    let changed = patch_properties(
        &original,
        &[
            (0x24000003, &[]),
            (0x1c000004, &[96, 97, 98, 99]),
            (0x24000001, &[6, 0, 0, 0, 7, 0, 0, 0]),
        ],
        &[(0x24000005, &[8, 0, 0, 0]), (0x88000006, &[])],
    )
    .unwrap();
    let parsed = PropertySets::parse(&changed).unwrap();
    let previous = PropertySets::parse(&original).unwrap();
    assert_eq!(parsed.sets[1], previous.sets[1]);
    let nested = &original[nested_start..nested_end];
    assert_eq!(
        changed
            .windows(nested.len())
            .filter(|bytes| *bytes == nested)
            .count(),
        1
    );
    assert_eq!(
        &changed[4..20],
        &[6, 0, 0, 0, 7, 0, 0, 0, 2, 0, 0, 0, 8, 0, 0, 0]
    );
    assert_eq!(&changed[20..36], &original[16..32]);
    assert_eq!(parsed.sets[0][3].value, Value::Bytes(&[96, 97, 98, 99]));
}

#[test]
fn deep_property_splices_do_not_use_the_call_stack() {
    let mut bytes = 0x80000000_u32.to_le_bytes().to_vec();
    for _ in 0..100_000 {
        bytes.extend_from_slice(&1_u16.to_le_bytes());
        bytes.extend_from_slice(&0x44000001_u32.to_le_bytes());
    }
    bytes.extend_from_slice(&0_u16.to_le_bytes());
    let changed = patch_properties(&bytes, &[], &[(0x88000002, &[])]).unwrap();
    let parsed = PropertySets::parse(&changed).unwrap();
    assert_eq!(parsed.sets.len(), 100_001);
    assert_eq!(parsed.sets[0].len(), 2);
    assert_eq!(
        &changed[14..changed.len() - parsed.padding.len()],
        &bytes[10..]
    );
}

#[test]
fn invalid_property_splices_are_rejected() {
    let bytes = properties(&[(0x08000001, vec![]), (0x24000002, vec![])]).unwrap();
    for (updates, inserts) in [
        (vec![(0x88000001, &[][..]), (0x08000001, &[])], vec![]),
        (vec![(0x14000003, &[0; 4][..])], vec![]),
        (vec![(0x24000002, &[0; 3][..])], vec![]),
        (vec![(0x88000001, &[1][..])], vec![]),
        (vec![], vec![(0x88000001, &[][..])]),
        (vec![], vec![(0x20000003, &[][..])]),
    ] {
        assert!(patch_properties(&bytes, &updates, &inserts).is_err());
    }
}

#[test]
fn character_formatting_preserves_inheritance_and_associated_data() {
    use super::{PropertyObject, write_revision};
    use crate::{
        ExGuid, PreparedEdit, RevisionIndex, Store, TextAttribute,
        document::{Document, Kind},
    };
    use std::collections::BTreeMap;
    let source = crate::create_section("inherited.one", "abcdef", "Author").unwrap();
    let store = Store::parse(&source).unwrap();
    let index = RevisionIndex::parse(&store).unwrap();
    let (sid, text) = index
        .spaces
        .iter()
        .find_map(|(sid, s)| {
            let raw = index
                .resolve(*sid, s.labels[&(ExGuid::default(), 1)])
                .unwrap();
            raw.objects
                .iter()
                .find_map(|(id, o)| (o.jcid == 0x6000e).then_some((*sid, *id)))
        })
        .unwrap();
    for variant in 0..7 {
        let fixture = write_revision(&source, sid, |raw| {
            let mut target = PropertyObject::from_object(&raw.objects[&text])?;
            target.bytes = properties(&[
                (0x1c001c22, crate::create::string("abcdef")),
                (0x14001d7a, vec![0; 4]),
                (0x1c001c0a, crate::create::string("Georgia")),
                (0x88001c05, Vec::new()),
            ])?;
            match variant {
                1 | 4 | 5 | 6 => {
                    let flag = match variant {
                        1 => 0x88001e16,
                        4 => 0x88001e14,
                        5 => 0x88003401,
                        _ => 0x88001e22,
                    };
                    target.set(&[(flag, &[])])?;
                }
                2 => {
                    let author = raw
                        .objects
                        .iter()
                        .find_map(|(id, o)| (o.jcid == 0x120001).then_some(*id))
                        .unwrap();
                    let reference = target.reference(author)?;
                    target.set(&[(0x24003458, &reference)])?;
                }
                3 => {
                    // An unknown nested run property must survive a style-only edit byte for byte.
                    let parsed = PropertySets::parse(&target.bytes)?;
                    let at = parsed.root_ids.as_ptr().addr() - target.bytes.as_ptr().addr();
                    let count = parsed.sets[0].len();
                    let end = target.bytes.len() - parsed.padding.len();
                    let mut bytes = target.bytes[..end].to_vec();
                    bytes[at - 2..at].copy_from_slice(&((count + 1) as u16).to_le_bytes());
                    bytes.splice(at + count * 4..at + count * 4, 0x40003499_u32.to_le_bytes());
                    bytes.extend_from_slice(&1_u32.to_le_bytes());
                    bytes.extend_from_slice(&0x44001234_u32.to_le_bytes());
                    bytes.extend_from_slice(&1_u16.to_le_bytes());
                    bytes.extend_from_slice(&0x14001234_u32.to_le_bytes());
                    bytes.extend_from_slice(&0xdeadbeef_u32.to_le_bytes());
                    bytes.resize(bytes.len().next_multiple_of(8), 0);
                    target.bytes = bytes;
                }
                _ => {}
            }
            Ok(BTreeMap::from([(text, target)]))
        })
        .unwrap();
        let edit = PreparedEdit::format(&fixture, sid, text, 0..6, &[TextAttribute::Bold(true)]);
        if matches!(variant, 1 | 2 | 4 | 5 | 6) {
            assert!(edit.is_err());
            continue;
        }
        let edit = edit.unwrap();
        let store = Store::parse(edit.as_bytes()).unwrap();
        let index = RevisionIndex::parse(&store).unwrap();
        let doc = Document::parse(&index).unwrap();
        let s = &doc.spaces[&sid];
        let view = &s.revisions[&s.contexts[&ExGuid::default()]];
        let runs = view.text_runs(text).unwrap();
        assert_eq!(runs.len(), 1);
        assert_eq!(runs[0].format.font.as_deref(), Some("Georgia"));
        assert_eq!(runs[0].format.italic, Some(true));
        assert_eq!(runs[0].format.bold, Some(true));
        if variant == 3 {
            let Kind::RichText { runs, .. } = &view.nodes[&text].kind else {
                panic!()
            };
            let data = &view.nodes[&text].extra[runs[0].extra_set.unwrap()];
            assert_eq!(data.len(), 1);
            assert_eq!(data[0].id, 0x14001234);
            assert!(
                PreparedEdit::format(&fixture, sid, text, 1..3, &[TextAttribute::Bold(true)])
                    .is_err()
            );
            let previous = Store::parse(&fixture).unwrap();
            let previous = RevisionIndex::parse(&previous).unwrap();
            let previous_doc = Document::parse(&previous).unwrap();
            let s = &previous_doc.spaces[&sid];
            let previous_view = &s.revisions[&s.contexts[&ExGuid::default()]];
            assert_eq!(
                format!("{:?}", view.nodes[&text].extra),
                format!("{:?}", previous_view.nodes[&text].extra)
            );
        }
    }
}
