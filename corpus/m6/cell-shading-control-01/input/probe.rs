use onestore::{Store, RevisionIndex, ObjectData, PropertySets, Value, Reference, document::Document};
fn main() -> Result<(), Box<dyn std::error::Error>> {
 let source=std::fs::read("corpus/m6/native-structure-01/notebook/synthetic.one")?;
 let store=Store::parse(&source)?;let index=RevisionIndex::parse(&store)?;let document=Document::parse(&index)?;
 let mut bytes=source.clone();let mut changed=false;
 'spaces: for (sid,space) in &document.spaces {for rid in space.revisions.keys() {let revision=index.resolve(*sid,*rid)?;
 for object in revision.objects.values().filter(|o| o.jcid==0x60024) {if let ObjectData::Properties(data)=object.data {
 let sets=PropertySets::parse(data)?;let property=sets.sets[0].iter().find(|p|p.id==0x14001d7a).unwrap();
 let Value::Bytes(value)=property.value else {panic!("Expected a four-byte timestamp")};assert_eq!(value.len(),4);
 let start=data.as_ptr().addr()-source.as_ptr().addr();let at=value.as_ptr().addr()-source.as_ptr().addr();
 let offsets:Vec<_>=data.windows(4).enumerate().filter(|(_,b)|*b==0x14001d7au32.to_le_bytes()).map(|(i,_)|i).collect();assert_eq!(offsets.len(),1);
 bytes[start+offsets[0]..start+offsets[0]+4].copy_from_slice(&0x14001e26u32.to_le_bytes());
 bytes[at..at+4].copy_from_slice(&[255,255,0,0]);
 let digest=md5::compute(&bytes[start..start+data.len()]).0;
 for node in store.lists.values().flat_map(|list|&list.nodes) {if matches!(node.id,0xc2|0xc4|0xc5) && let Some(Reference::Data(chunk))=node.reference && chunk.offset==start as u64 && chunk.length==data.len() as u64 {
 let hash=node.payload.as_ptr().addr()-source.as_ptr().addr()+node.payload.len()-16;bytes[hash..hash+16].copy_from_slice(&digest);
 }}
 println!("Changed one cell timestamp property to CellShadingColor yellow at object offset {start}");changed=true;break 'spaces;
 }}}}
 assert!(changed);let store=Store::parse(&bytes)?;assert!(store.checksum_mismatches.is_empty());let index=RevisionIndex::parse(&store)?;Document::parse(&index)?;
 let path="evidence/m6/cell-shading-input/notebook/synthetic.one";assert!(!std::path::Path::new(path).exists());std::fs::write(path,bytes)?;Ok(())
}
