//! OneNote packages (`.onepkg`) and the other copies OneNote 2010's Save As writes. A
//! package is a cabinet holding a notebook's tables of contents and sections at their paths
//! in its folder, the recycle bin left out though its entry stays; OneNote writes each file
//! outside any notebook (no `guidAncestor` or `crcName`). Its Unpack Notebook gives each file
//! an identity of its own and lists it anew beside its stale entry; here the entry follows
//! the file instead (`corpus/notebook-package`). OneNote compresses with LZX; this writes
//! MSZIP, which every cabinet reader takes, and reads either.

use crate::{
    Error, Result,
    session::{Notebook, lists, moved},
};
use onestore::{TocEdit, op::Edit, page::Page};
use std::{
    collections::BTreeMap,
    io::{self, Cursor, Read, Write},
    path::Path,
};

/// A package's files: each one's path in the notebook's folder, `/`-separated, and bytes.
pub type Files = Vec<(String, Vec<u8>)>;

fn invalid(message: &str) -> Error {
    io::Error::new(io::ErrorKind::InvalidData, message.to_owned()).into()
}

/// The files of `notebook` a package holds, as OneNote 2010 packs them: each folder's table
/// of contents and sections, then its groups', the recycle bin and copies left out. `image`
/// gives a section the caller holds edits for, by catalog path, as its replica leaves it.
pub fn notebook_files(
    notebook: &Notebook,
    mut image: impl FnMut(&str) -> Option<Vec<u8>>,
) -> Result<Files> {
    let mut files = Vec::new();
    let mut folders = vec![notebook.catalog()];
    while let Some(folder) = folders.pop() {
        let Some(toc) = &folder.toc else {
            continue;
        };
        files.push((
            join(&folder.path, &toc.filename),
            notebook.read_toc(&folder.path)?,
        ));
        for section in folder.sections.iter().filter(|section| !section.copy) {
            let bytes = match image(&section.path) {
                Some(bytes) => bytes,
                None => notebook.read_section(&section.path)?,
            };
            files.push((section.path.clone(), bytes));
        }
        folders.extend(
            (folder.groups.iter().rev())
                .filter(|group| group.path.rsplit('/').next() != Some("OneNote_RecycleBin")),
        );
    }
    Ok(files)
}

fn join(folder: &str, name: &str) -> String {
    if folder.is_empty() {
        name.to_owned()
    } else {
        format!("{folder}/{name}")
    }
}

/// A package holding `files`, each taken out of its notebook as OneNote writes them.
pub fn pack(files: Files) -> Result<Vec<u8>> {
    let mut builder = cab::CabinetBuilder::new();
    let folder = builder.add_folder(cab::CompressionType::MsZip);
    for (path, _) in &files {
        folder.add_file(path.replace('/', "\\"));
    }
    let mut writer = builder.build(Cursor::new(Vec::new()))?;
    let mut files = files.into_iter();
    while let Some(mut file) = writer.next_file()? {
        let (_, mut bytes) = files.next().ok_or_else(|| invalid("A file went missing"))?;
        onestore::place_image(&mut bytes, None)?;
        file.write_all(&bytes)?;
    }
    Ok(writer.finish()?.into_inner())
}

/// The files the package `bytes` holds, refusing a path that leaves the notebook's folder
/// and more than `limit` bytes unpacked.
pub fn read(bytes: &[u8], limit: u64) -> Result<Files> {
    let mut cabinet = cab::Cabinet::new(Cursor::new(bytes))?;
    let mut listed = Vec::new();
    let mut total = 0u64;
    for folder in cabinet.folder_entries() {
        for file in folder.file_entries() {
            total += u64::from(file.uncompressed_size());
            listed.push(file.name().to_owned());
        }
    }
    if total > limit {
        return Err(io::Error::from(io::ErrorKind::FileTooLarge).into());
    }
    let mut files = Vec::new();
    for name in listed {
        let path = name.replace('\\', "/");
        if !path.split('/').all(component) {
            return Err(invalid("The package names a file outside its notebook"));
        }
        let mut bytes = Vec::new();
        cabinet.read_file(&name)?.read_to_end(&mut bytes)?;
        files.push((path, bytes));
    }
    Ok(files)
}

fn component(name: &str) -> bool {
    !name.is_empty() && !name.contains([':', '\0']) && name != "." && name != ".."
}

/// Writes package `files` to the new folder `root` as OneNote 2010's Unpack Notebook does:
/// each table of contents and section takes a new identity, placed under its folder's table
/// of contents, whose entries follow.
pub fn unpack(mut files: Files, root: &Path) -> Result<()> {
    let folder = |path: &str| {
        path.rsplit_once('/')
            .map_or("", |(folder, _)| folder)
            .to_owned()
    };
    let toc = |path: &str| path.to_ascii_lowercase().ends_with(".onetoc2");
    let mut tocs: BTreeMap<String, usize> = BTreeMap::new();
    for (at, (path, _)) in files.iter().enumerate() {
        if toc(path) && tocs.insert(folder(path), at).is_some() {
            return Err(invalid(
                "A folder of the package holds two tables of contents",
            ));
        }
    }
    // Parents first: a group's table of contents is placed under its parent's new identity.
    let mut identities: BTreeMap<String, ([u8; 16], [u8; 16])> = BTreeMap::new();
    let mut ordered: Vec<(&String, &usize)> = tocs.iter().collect();
    ordered.sort_by_key(|(folder, _)| folder.split('/').count() * usize::from(!folder.is_empty()));
    for (path, at) in ordered {
        let image = &mut files[*at].1;
        let old = onestore::Store::parse(image)?.header.file_id;
        let new = onestore::reidentify(image)?;
        let parent = (!path.is_empty()).then(|| folder(path));
        let placed = match &parent {
            Some(parent) => {
                let (_, ancestor) = identities
                    .get(parent)
                    .ok_or_else(|| invalid("A section group's folder has no parent"))?;
                Some((*ancestor, path.rsplit('/').next().unwrap_or_default()))
            }
            None => None,
        };
        onestore::place_image(image, placed)?;
        identities.insert(path.clone(), (old, new));
    }
    // The entries each table of contents lists, by their files' old identities.
    let mut followed: BTreeMap<String, Vec<TocEdit>> = BTreeMap::new();
    for (path, image) in &mut files {
        if toc(path) {
            continue;
        }
        let home = folder(path);
        let Some((_, ancestor)) = identities.get(&home) else {
            return Err(invalid("A section's folder has no table of contents"));
        };
        let old = onestore::Store::parse(image)?.header.file_id;
        let new = onestore::reidentify(image)?;
        onestore::place_image(
            image,
            Some((*ancestor, path.rsplit('/').next().unwrap_or_default())),
        )?;
        (followed.entry(home).or_default()).push(TocEdit::Reidentify {
            identity: old,
            with: new,
        });
    }
    for (path, (old, new)) in &identities {
        if !path.is_empty() {
            let edit = TocEdit::Reidentify {
                identity: *old,
                with: *new,
            };
            followed.entry(folder(path)).or_default().push(edit);
        }
    }
    for (home, edits) in followed {
        let image = &mut files[tocs[&home]].1;
        let mut listed = Vec::new();
        for edit in edits {
            if let TocEdit::Reidentify { identity, .. } = &edit
                && lists(image, *identity)?
            {
                listed.push(edit);
            }
        }
        if let Some(transaction) = onestore::edit_table_of_contents(image, &listed)? {
            transaction.apply(image)?;
        }
    }
    std::fs::create_dir(root)?;
    for (path, bytes) in &files {
        let file = root.join(path);
        if let Some(parent) = file.parent() {
            std::fs::create_dir_all(parent)?;
        }
        std::fs::File::create_new(file)?.write_all(bytes)?;
    }
    Ok(())
}

/// The section `image` holds as Save As writes a copy of it: an identity of its own, outside
/// any notebook.
pub fn section_copy(mut image: Vec<u8>) -> Result<Vec<u8>> {
    onestore::reidentify(&mut image)?;
    onestore::place_image(&mut image, None)?;
    Ok(image)
}

/// A new section, named `name` and coloured `color` (COLORREF), holding `page` as Save As
/// writes a page: the page keeps its identity, title, date and creation time.
pub fn page_section(page: &Page, name: &str, color: Option<u32>, author: &str) -> Result<Vec<u8>> {
    let mut image = onestore::create_empty_section(name, color)?;
    let arena = onestore::Arena::default();
    let mut section = onestore::Section::open(&arena, image.clone())?;
    section.apply(
        author,
        &Edit {
            at: crate::now(),
            ops: vec![moved(page, author)?],
        },
    )?;
    if let Some(transaction) = section.seal()? {
        transaction.apply(&mut image)?;
    }
    onestore::place_image(&mut image, None)?;
    Ok(image)
}
