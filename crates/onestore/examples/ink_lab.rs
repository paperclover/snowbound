use onestore::{RevisionIndex, Store, document::Document};
fn main() {
    let path = std::env::args().nth(1).unwrap();
    let needle = std::env::args().nth(2).unwrap();
    let bytes = std::fs::read(path).unwrap();
    let store = Store::parse(&bytes).unwrap();
    let index = RevisionIndex::parse(&store).unwrap();
    let document = Document::parse(&index).unwrap();
    for (space, _) in document.pages().unwrap() {
        let page = onestore::page::Page::from_space(&document, space).unwrap();
        if page.title != needle {
            continue;
        }
        if std::env::var_os("INK_MODEL").is_some() {
            for object in &page.objects {
                if let onestore::page::PageObject::Ink(ink) = object {
                    println!("{}", serde_json::to_string(ink).unwrap());
                }
            }
            continue;
        }
        let revision = document.spaces[&space].active().unwrap();
        println!("{}", serde_json::to_string_pretty(revision).unwrap());
    }
}
