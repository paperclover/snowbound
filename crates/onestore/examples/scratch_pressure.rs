use onestore::{RevisionIndex, Store, document::{Document, Kind}};
fn mb(bytes: &[u8]) -> Vec<i64> {
    let mut out = Vec::new(); let mut v = 0u64; let mut s = 0;
    for &b in bytes { v |= u64::from(b & 0x7f) << s; s += 7; if b & 0x80 == 0 { out.push(v as i64); v = 0; s = 0; } }
    let count = (out[0] >> 1) as usize;
    out[1..].iter().take(count).map(|r| if r & 1 == 1 { -(r >> 1) } else { r >> 1 }).collect()
}
fn guid(b: &[u8]) -> String {
    let d1 = u32::from_le_bytes(b[0..4].try_into().unwrap());
    let d2 = u16::from_le_bytes(b[4..6].try_into().unwrap());
    let d3 = u16::from_le_bytes(b[6..8].try_into().unwrap());
    format!("{d1:08x}-{d2:04x}-{d3:04x}-{}", b[8..].iter().map(|x| format!("{x:02x}")).collect::<String>())
}
fn main() {
    let path = std::env::args().nth(1).unwrap();
    let bytes = std::fs::read(path).unwrap();
    let store = Store::parse(&bytes).unwrap();
    let index = RevisionIndex::parse(&store).unwrap();
    let document = Document::parse(&index).unwrap();
    for (space, _) in document.pages().unwrap() {
        let rev = document.active(space).unwrap();
        for (id, node) in &rev.nodes {
            match &node.kind {
                Kind::InkStyle { dimensions, .. } => {
                    println!("STYLE {id:?} {:?}", node.kind);
                    for d in dimensions.chunks_exact(32) {
                        let lo = i32::from_le_bytes(d[16..20].try_into().unwrap());
                        let hi = i32::from_le_bytes(d[20..24].try_into().unwrap());
                        let units = u32::from_le_bytes(d[24..28].try_into().unwrap());
                        let res = f32::from_le_bytes(d[28..32].try_into().unwrap());
                        println!("  dim {} [{lo}, {hi}] units {units} res {res}", guid(&d[..16]));
                    }
                    for set in &node.extra { for f in set { println!("  extra {:#x} {:?}", f.id, f.value); } }
                }
                Kind::InkStroke { path, style, bias, index, .. } => {
                    let v = mb(path);
                    println!("STROKE {id:?} style {style:?} bias {bias:?} index {index:?} n={} values {:?}", v.len(), v);
                    for set in &node.extra { for f in set { println!("  extra {:#x} {:?}", f.id, f.value); } }
                }
                Kind::Ink { .. } | Kind::InkData { .. } => {
                    println!("{id:?} {:?} layout {:?}", node.kind, node.layout);
                    for set in &node.extra { for f in set { println!("  extra {:#x} {:?}", f.id, f.value); } }
                }
                _ => {}
            }
        }
    }
}
