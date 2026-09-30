use onestore::{Arena, Section, page::{PageObject}};
fn main() {
    let path = std::env::args().nth(1).unwrap();
    let want = std::env::args().nth(2).unwrap_or_default();
    let arena = Arena::default();
    let mut section = Section::open(&arena, std::fs::read(path).unwrap()).unwrap();
    for (space, _, _) in section.pages().unwrap() {
        let page = section.page(space).unwrap();
        for object in &page.objects {
            let PageObject::Outline(outline) = object else { continue };
            for p in &outline.paragraphs {
                let Some(text) = p.text() else { continue };
                if !text.text.text().contains(want.as_str()) { continue }
                let mut start = 0;
                println!("-- {:?}", p.id);
                for span in text.text.spans() {
                    println!("  {:?} math={:?} obj={:?} emb={:?}", &text.text.text()[start..span.end], span.format.math, span.format.math_object, span.format.embedded_object);
                    start = span.end;
                }
            }
        }
    }
}
