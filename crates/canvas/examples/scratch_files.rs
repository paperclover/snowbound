use onestore::{RevisionIndex, Store, document::{Document, Kind}};
fn main() {
    let bytes = std::fs::read(std::env::args().nth(1).unwrap()).unwrap();
    let store = Store::parse(&bytes).unwrap();
    let index = RevisionIndex::parse(&store).unwrap();
    let document = Document::parse(&index).unwrap();
    for (space, _) in document.pages().unwrap() {
        let revision = document.active(space).unwrap();
        for (id, node) in &revision.nodes {
            if let Kind::File { extension, payload, .. } = &node.kind {
                println!("{id:?} {extension} {:?}", payload.map(|p| (p.len(), &p[..4.min(p.len())])));
            }
        }
    }
}
