use crate::{
    Error, ExGuid,
    create::{current_timestamps, properties, string},
    document::{Document, Kind},
    write::{PropertyObject, RevisionEdit, fresh_guid, write_revisions},
};
use serde::{Deserialize, Serialize};
use std::{collections::BTreeMap, sync::Arc};

fn invalid(message: &'static str) -> Error {
    Error { offset: 0, message }
}

/// An empty top-level page with stable identities and creation time.
/// Retain the intent across retries; an existing page identity rejects duplicate creation.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct PageCreation {
    guid: [u8; 16],
    series_guid: [u8; 16],
    before: Option<ExGuid>,
    title: Option<String>,
    author: String,
    created: u32,
}

impl PageCreation {
    /// Appends a page, or inserts before the first page space of an existing series.
    /// `Some("")` creates an empty title field; `None` creates a page without a title node.
    /// The page has no body outlines or generated date/time text and does not apply a template.
    pub fn new(before: Option<ExGuid>, title: Option<&str>, author: &str) -> Result<Self, Error> {
        let page = Self {
            guid: fresh_guid()?,
            series_guid: fresh_guid()?,
            before,
            title: title.map(str::to_owned),
            author: author.to_owned(),
            created: current_timestamps()?.0,
        };
        page.validate()?;
        Ok(page)
    }

    pub fn space(&self) -> ExGuid {
        ExGuid {
            guid: self.guid,
            n: 1,
        }
    }

    pub fn object(&self) -> ExGuid {
        ExGuid {
            guid: self.guid,
            n: 12,
        }
    }

    pub fn title_object(&self) -> Option<ExGuid> {
        self.title.as_ref().map(|_| ExGuid {
            guid: self.guid,
            n: 16,
        })
    }

    /// Changes the insertion anchor while retaining all identities and creation metadata.
    pub fn reposition(&self, before: Option<ExGuid>) -> Result<Self, Error> {
        let mut page = self.clone();
        page.before = before;
        page.validate()?;
        Ok(page)
    }

    fn validate(&self) -> Result<(), Error> {
        if self.guid == [0; 16]
            || self.series_guid == [0; 16]
            || self.guid == self.series_guid
            || self.author.contains('\0')
            || self.before.is_some_and(|id| id.guid == [0; 16])
            || self
                .title
                .as_ref()
                .is_some_and(|title| title.contains(['\0', '\r', '\n', '\u{fffc}', '\u{fddf}']))
        {
            return Err(invalid(
                "Use a new page identity, an existing page anchor, and ordinary single-line title text",
            ));
        }
        Ok(())
    }

    pub(crate) fn apply(&self, source: &[u8]) -> Result<Vec<u8>, Error> {
        self.validate()?;
        write_revisions(source, |index| {
            if index.spaces.contains_key(&self.space()) {
                return Err(invalid(
                    "This page identity already exists; reconcile the original creation",
                ));
            }
            let document = Document::parse(index)?;
            document.pages()?;
            let section = &document.spaces[&document.root];
            let view = &section.revisions[&section.contexts[&ExGuid::default()]];
            let section_id = view.roots[&1];
            let section_node = &view.nodes[&section_id];
            if !matches!(section_node.kind, Kind::Section { .. })
                || section_node.extra[0]
                    .iter()
                    .any(|field| field.id == 0x88001cde)
            {
                return Err(invalid("Choose an editable section for the new page"));
            }
            let position = match self.before {
                None => section_node.children.len(),
                Some(before) => section_node
                    .children
                    .iter()
                    .position(|id| view.nodes[id].spaces.first() == Some(&before))
                    .ok_or_else(|| {
                        invalid("Insert before the first page of an existing series, or append")
                    })?,
            };
            let raw = index.resolve(document.root, section.contexts[&ExGuid::default()])?;
            let id = |n| ExGuid { guid: self.guid, n };
            let reference = |n: u32| n.to_le_bytes().to_vec();
            let timestamp = ((u64::from(self.created) + 315532800 + 11644473600) * 10000000)
                .to_le_bytes()
                .to_vec();
            let modified = || (0x14001d7a, self.created.to_le_bytes().to_vec());
            let mut table = BTreeMap::from([(0, self.guid)]);
            let mut metadata_id = id(1);
            for (byte, salt) in metadata_id.guid.iter_mut().zip([
                0x31, 0xc0, 0xa8, 0x22, 0, 0x36, 0xee, 0x42, 0xb7, 0x14, 0xd7, 0xac, 0xda, 0x24,
                0x35, 0xe8,
            ]) {
                *byte ^= salt;
            }
            table.insert(1, metadata_id.guid);
            let table = Arc::new(table);
            let object = |jcid, values: Vec<_>| -> Result<_, Error> {
                Ok(PropertyObject {
                    jcid,
                    bytes: properties(&values)?,
                    global_ids: Arc::clone(&table),
                })
            };
            let title = self.title.as_deref().unwrap_or_default().trim_start();
            let metadata = vec![
                (0x1c001c30, self.guid.to_vec()),
                (0x1c001cf3, string(title)),
                (0x14001d82, reference(40)),
                (0x1400348b, reference(40)),
                (0x14001dff, reference(1)),
                (0x18001c65, timestamp.clone()),
            ];
            let mut page = vec![
                modified(),
                (0x1c001d75, string(&self.author)),
                (0x1c001d3c, string("")),
            ];
            if self.title.is_some() {
                page.push((0x24001d5f, reference(13)));
            }
            let mut objects = BTreeMap::from([
                (id(10), object(0x60037, vec![(0x24001c1f, reference(12))])?),
                (id(11), object(0x20030, metadata.clone())?),
                (id(12), object(0x6000b, page)?),
            ]);
            if let Some(title) = &self.title {
                for (n, jcid, values) in [
                    (
                        13,
                        0x6002c,
                        vec![
                            modified(),
                            (0x24001c20, reference(14)),
                            (0x14001c14, 0_f32.to_le_bytes().to_vec()),
                            (0x14001c15, 0_f32.to_le_bytes().to_vec()),
                        ],
                    ),
                    (
                        14,
                        0x6000c,
                        vec![
                            modified(),
                            (0x24001c20, reference(15)),
                            (0x0c001c03, vec![1]),
                        ],
                    ),
                    (
                        15,
                        0x6000d,
                        vec![
                            modified(),
                            (0x24001c1f, reference(16)),
                            (0x0c001c03, vec![1]),
                            (0x20001d78, reference(17)),
                            (0x20001d79, reference(17)),
                            (0x14001d09, self.created.to_le_bytes().to_vec()),
                            (0x88001cb4, vec![]),
                        ],
                    ),
                    (
                        16,
                        0x6000e,
                        vec![
                            modified(),
                            (0x1c001c22, string(title)),
                            (0x24001e13, reference(19)),
                            (0x2000342c, reference(18)),
                            (0x10001cfe, 0x409_u16.to_le_bytes().to_vec()),
                            (0x88001cb4, vec![]),
                        ],
                    ),
                    (17, 0x120001, vec![(0x1c001d75, string(&self.author))]),
                    (
                        18,
                        0x12004d,
                        vec![
                            (0x1c00345a, string("PageTitle")),
                            (0x1c001c0a, string("Calibri")),
                            (0x10001c0b, 34_u16.to_le_bytes().to_vec()),
                        ],
                    ),
                    (19, 0x12004d, vec![(0x14001c3b, reference(0x409))]),
                ] {
                    objects.insert(id(n), object(jcid, values)?);
                }
            }
            let series = object(
                0x60008,
                vec![
                    (0x1c001c30, self.series_guid.to_vec()),
                    (0x18001c65, timestamp),
                    (0x2c001d63, reference(1)),
                    (0x24003442, reference(257)),
                ],
            )?;
            if raw.objects.contains_key(&id(2)) || raw.objects.contains_key(&metadata_id) {
                return Err(invalid("The new page's section identities already exist"));
            }
            let mut parent = PropertyObject::from_object(&raw.objects[&section_id])?;
            let mut children = section_node.children.clone();
            children.insert(position, id(2));
            let mut references = Vec::new();
            for child in children {
                references.extend_from_slice(&parent.reference(child)?);
            }
            parent.set(&[(0x24001c20, &references)])?;
            Ok(BTreeMap::from([
                (
                    document.root,
                    RevisionEdit::Update(BTreeMap::from([
                        (section_id, parent),
                        (id(2), series),
                        (metadata_id, object(0x20030, metadata)?),
                    ])),
                ),
                (
                    self.space(),
                    RevisionEdit::Create {
                        roots: BTreeMap::from([(1, id(10)), (2, id(11))]),
                        objects,
                    },
                ),
            ]))
        })
    }
}
