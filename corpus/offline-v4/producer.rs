use onestore::{CommitError, CommitState, ExGuid, PreparedEdit, RevisionIndex, Store, document::{Document, Kind}};
use onestore_offline::{Remote, Replica, Recovery};
use std::{fs, io, path::Path};
struct LostReply(Vec<u8>);
impl Remote for LostReply {
    fn read(&mut self) -> io::Result<Vec<u8>> { Ok(self.0.clone()) }
    fn publish(&mut self, edit: &PreparedEdit<'_>) -> Result<(), CommitError> {
        self.0 = edit.as_bytes().to_vec();
        Err(CommitError { state: CommitState::Unknown, error: io::ErrorKind::ConnectionReset.into() })
    }
    fn confirm(&mut self, _: &[u8]) -> Result<(), CommitError> { panic!("No confirmation is expected") }
}
fn main() -> Result<(), Box<dyn std::error::Error>> {
    let root = Path::new("corpus/offline-v4"); fs::create_dir(root)?;
    let source = fs::read("corpus/native-external-assets/notebook/synthetic.one")?;
    let store = Store::parse(&source)?; let index = RevisionIndex::parse(&store)?; let document = Document::parse(&index)?;
    let (space, object) = document.spaces.iter().find_map(|(sid, space)| {
        let view = &space.revisions[&space.contexts[&ExGuid::default()]];
        view.nodes.iter().find_map(|(oid,node)| matches!(&node.kind,Kind::RichText { text, .. } if text == "Native before 🦀").then_some((*sid,*oid)))
    }).unwrap();
    let cache = Replica::create(root.join("live.sqlite"), &source)?;
    let first = cache.edit_text(&source, space, object, 0..0, "queued ")?.unwrap();
    assert!(cache.sync_once(&mut LostReply(source.clone())).is_err());
    let working = cache.snapshot()?;
    let second = cache.edit_text(&working, space, object, 0..0, "dependent ")?.unwrap();
    cache.export_recovery(root.join("recovery.sqlite"))?;
    let recovery = Recovery::open(root.join("recovery.sqlite"))?;
    assert_eq!(cache.pending()?, recovery.pending()?);
    assert_eq!(recovery.summary()?.queued_edits, 2);
    assert_eq!(recovery.summary()?.uncertain_edits, 1);
    println!("Original schema-4 producer: IDs {first}, {second}; {:?}", recovery.summary()?);
    Ok(())
}
