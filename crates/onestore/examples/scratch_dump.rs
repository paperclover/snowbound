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
        if page.title != needle { continue; }
        let revision = document.spaces[&space].active().unwrap();
        println!("{}", serde_json::to_string_pretty(revision).unwrap());
        if let Ok(out) = std::env::var("PAYLOAD_OUT") {
            for object in &page.objects {
                if let onestore::page::PageObject::Outline(o) = object {
                    for p in &o.paragraphs {
                        if let onestore::page::ParagraphContent::Attachment(a) = &p.content {
                            std::fs::write(format!("{out}/{}", a.filename), a.bytes.as_deref().unwrap()).unwrap();
                            if let Some(i) = &a.preview { std::fs::write(format!("{out}/{}.png", a.filename), i).unwrap(); }
                        }
                    }
                }
            }
        }
    }
}
#[allow(dead_code)]
fn unused() {}
