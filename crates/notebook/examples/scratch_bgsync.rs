//! Scratch lab writer (delete before reporting): appends TEXT to the first body paragraph of
//! the first page of a share-relative section. `scratch_bgsync ADDRESS PATH TEXT`
use notebook::{
    Replica, SmbRemote,
    smb::{Client, Credentials},
};
use onestore::op::{Edit, Op, PageOp};
use std::time::Duration;

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let args: Vec<_> = std::env::args().skip(1).collect();
    let [address, path, with] = &args[..] else {
        return Err("scratch_bgsync ADDRESS PATH TEXT".into());
    };
    let connect = || Client::connect(address, "agent", Credentials::default(), Duration::from_secs(5));
    let source = connect()?.read_storage(path, 1 << 26)?;
    let directory = tempfile::tempdir()?;
    let replica = Replica::create(directory.path().join("cache.sqlite"), &source)?;
    let space = replica.pages()?[0].0;
    let page = replica.page(space)?;
    let (text, len) = page
        .objects
        .iter()
        .find_map(|object| match object {
            onestore::page::PageObject::Outline(outline) => outline
                .paragraphs
                .iter()
                .find_map(|p| p.text().map(|t| (t.id, t.text.text().encode_utf16().count() as u32))),
            _ => None,
        })
        .ok_or("no text")?;
    replica.apply(
        "Lab",
        Edit {
            at: 134_000_000_000_000_000,
            ops: vec![Op::Page { space, op: PageOp::Text { text, range: len..len, with: with.clone() } }],
        },
    )?;
    let mut remote = SmbRemote::new(connect()?, path.clone(), 1 << 26);
    println!("{:?}", replica.sync_once(&mut remote)?.edit);
    Ok(())
}
