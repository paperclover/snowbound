use onestore::{
    Arena, ExGuid, RevisionIndex, Section, Store,
    op::{Edit, Op, PageOp},
};

/// `source` with revisions of `space` appended until its chain is 512 deep, so its next
/// revision is a checkpoint. Each retouches the text object `text`, typing a character and
/// removing it again.
pub fn pending(source: &[u8], space: ExGuid, text: ExGuid) -> Vec<u8> {
    let depth = |image: &[u8]| {
        let store = Store::parse(image).unwrap();
        let index = RevisionIndex::parse(&store).unwrap();
        let rid = index.spaces[&space].labels[&(ExGuid::default(), 1)];
        std::iter::successors(Some(rid), |id| {
            index.spaces[&space].revisions[id].dependency
        })
        .count()
    };
    let arena = Arena::default();
    let mut section = Section::open(&arena, source.to_vec()).unwrap();
    let retouch = |range, with: &str| Op::Page {
        space,
        op: PageOp::Text {
            text,
            range,
            with: with.into(),
        },
    };
    for at in 0..512 - depth(source) as u64 {
        let ops = vec![retouch(0..0, "x"), retouch(0..1, "")];
        let at = 134_000_000_000_000_000 + at * 10_000_000;
        section.apply("Author", &Edit { at, ops }).unwrap();
        section.seal().unwrap().unwrap();
    }
    let image = section.image();
    assert_eq!(depth(&image), 512);
    image
}
