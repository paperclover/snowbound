use super::*;
use crate::{Error, discover};
use onestore::Transaction;
use std::{
    collections::{BTreeMap, BTreeSet},
    sync::Mutex,
};

fn mapping(name: &str, shape: u16, art: &str, mapped: u64) -> TagMapping {
    TagMapping {
        name: name.into(),
        shape,
        art: art_name(art.as_bytes(), "png"),
        mapped,
    }
}

#[test]
fn merging_keeps_every_tag_and_each_tags_later_mapping() {
    let ours = vec![
        mapping("Launch", 13, "rocket", 10),
        mapping("Bug", 17, "bug", 5),
    ];
    let theirs = vec![
        mapping("Launch", 13, "flame", 20),
        mapping("Launch", 36, "circle", 1),
        mapping("Bug", 17, "beetle", 5),
    ];
    let mut here = ours.clone();
    merge(&mut here, theirs.clone());
    let mut there = theirs;
    merge(&mut there, ours);
    let sorted = |mut mappings: Vec<TagMapping>| {
        mappings.sort_by(|a, b| (&a.name, a.shape).cmp(&(&b.name, b.shape)));
        mappings
    };
    assert_eq!(sorted(here.clone()), sorted(there));
    let art = |name: &str, shape| {
        here.iter()
            .find(|mapping| mapping.name == name && mapping.shape == shape)
            .map(|mapping| mapping.art.clone())
    };
    assert_eq!(here.len(), 3);
    assert_eq!(art("Launch", 13), Some(art_name(b"flame", "png")));
    assert_eq!(art("Launch", 36), Some(art_name(b"circle", "png")));
    // Mapped at once, the greater name wins on either side.
    let tied = [art_name(b"bug", "png"), art_name(b"beetle", "png")];
    assert_eq!(art("Bug", 17).as_ref(), tied.iter().max());
    let before = here.clone();
    merge(&mut here, before.clone());
    assert_eq!(here, before);
}

/// A notebook folder in memory, as a share would hold it.
#[derive(Default)]
struct Folder {
    files: Mutex<BTreeMap<String, Vec<u8>>>,
    folders: Mutex<BTreeSet<String>>,
    hidden: Mutex<BTreeSet<String>>,
    /// Pictures renamed into place.
    kept: Mutex<Vec<String>>,
    /// A mapping another writer, having read before this one wrote, puts in place just after.
    race: Mutex<Option<Vec<u8>>>,
}

fn missing() -> Error {
    io::Error::from(io::ErrorKind::NotFound).into()
}

impl Folder {
    fn parent_exists(&self, path: &str) -> bool {
        path.rsplit_once('/')
            .is_none_or(|(parent, _)| self.folders.lock().unwrap().contains(parent))
    }
}

impl Storage for Folder {
    fn discover(&self, _: &mut discover::Cache, _: discover::Limits) -> Result<discover::Folder> {
        unimplemented!()
    }

    fn location(&self) -> String {
        "memory".into()
    }

    fn exists(&self, path: &str) -> bool {
        self.files.lock().unwrap().contains_key(path) || self.folders.lock().unwrap().contains(path)
    }

    fn read(&self, _: &str) -> Result<Vec<u8>> {
        unimplemented!()
    }

    fn read_file(&self, path: &str, limit: usize) -> Result<Vec<u8>> {
        let bytes = self
            .files
            .lock()
            .unwrap()
            .get(path)
            .cloned()
            .ok_or_else(missing)?;
        assert!(bytes.len() <= limit);
        Ok(bytes)
    }

    fn create(&self, path: &str, bytes: &[u8]) -> Result<()> {
        if !self.parent_exists(path) {
            return Err(missing());
        }
        let mut files = self.files.lock().unwrap();
        if files.contains_key(path) {
            return Err(io::Error::from(io::ErrorKind::AlreadyExists).into());
        }
        files.insert(path.into(), bytes.to_vec());
        Ok(())
    }

    fn create_directory(&self, path: &str) -> Result<()> {
        if !self.folders.lock().unwrap().insert(path.into()) {
            return Err(io::Error::from(io::ErrorKind::AlreadyExists).into());
        }
        Ok(())
    }

    fn hide(&self, path: &str) -> Result<()> {
        assert!(self.exists(path));
        self.hidden.lock().unwrap().insert(path.into());
        Ok(())
    }

    fn rename(&self, from: &str, to: &str) -> Result<()> {
        let mut files = self.files.lock().unwrap();
        if files.contains_key(to) {
            return Err(io::Error::from(io::ErrorKind::AlreadyExists).into());
        }
        let bytes = files.remove(from).ok_or_else(missing)?;
        files.insert(to.into(), bytes);
        self.kept.lock().unwrap().push(to.into());
        Ok(())
    }

    fn rename_root(&self, _: &str, _: &[String]) -> Result<String> {
        unreachable!()
    }

    fn replace(&self, from: &str, to: &str) -> Result<()> {
        let mut files = self.files.lock().unwrap();
        let bytes = files.remove(from).ok_or_else(missing)?;
        files.insert(to.into(), bytes);
        if let Some(theirs) = self.race.lock().unwrap().take() {
            files.insert(to.into(), theirs);
        }
        Ok(())
    }

    fn delete(&self, path: &str) -> Result<()> {
        self.files
            .lock()
            .unwrap()
            .remove(path)
            .ok_or_else(missing)?;
        Ok(())
    }

    fn place(&self, _: &str, _: [u8; 16], _: &str) -> Result<()> {
        unimplemented!()
    }

    fn commit(&self, _: &str, _: &Transaction) -> Result<()> {
        unimplemented!()
    }

    fn supersede(&self, _: &str, _: &onestore::Stamp, _: &str) -> Result<()> {
        unimplemented!()
    }
}

#[test]
fn mapping_hides_the_folder_and_keeps_each_picture_once() {
    let folder = Folder::default();
    assert!(mappings(&folder).unwrap().is_empty());
    let mapped = map(&folder, "Launch", 13, b"rocket", "png").unwrap();
    assert_eq!(mapped.len(), 1);
    assert_eq!(
        *folder.hidden.lock().unwrap(),
        BTreeSet::from([FOLDER.to_owned()])
    );
    let rocket = art_name(b"rocket", "png");
    assert_eq!(art(&folder, &rocket).unwrap(), b"rocket");
    // A second tag with the same picture, and a drawing.
    map(&folder, "Ship it", 127, b"rocket", "png").unwrap();
    let mapped = map(&folder, "Idea", 21, b"<svg/>", "svg").unwrap();
    assert_eq!(
        mapped
            .iter()
            .map(|mapping| (mapping.name.as_str(), mapping.shape))
            .collect::<Vec<_>>(),
        [("Launch", 13), ("Ship it", 127), ("Idea", 21)]
    );
    assert_eq!(
        *folder.kept.lock().unwrap(),
        [
            format!("{ART}/{rocket}"),
            format!("{ART}/{}", art_name(b"<svg/>", "svg"))
        ]
    );
    // Only the pictures and the mapping are left: no written file stays behind.
    let files: Vec<_> = folder.files.lock().unwrap().keys().cloned().collect();
    assert_eq!(files.len(), 3);
    assert!(files.iter().all(|file| !file.ends_with(".tmp")));
    // The same tag mapped again takes the new picture.
    let mapped = map(&folder, "Launch", 13, b"flame", "png").unwrap();
    assert_eq!(mapped[0].art, art_name(b"flame", "png"));
    assert_eq!(mappings(&folder).unwrap(), mapped);
}

/// Another writer read the mapping before this one replaced it and replaced it just after,
/// without this one's tag: this one reads it back and merges again.
#[test]
fn a_writer_overtaken_merges_again() {
    let folder = Folder::default();
    map(&folder, "Theirs before", 13, b"one", "png").unwrap();
    let theirs = vec![
        mappings(&folder).unwrap()[0].clone(),
        mapping("Theirs", 17, "two", crate::now()),
    ];
    *folder.race.lock().unwrap() = Some(serde_json::to_vec(&theirs).unwrap());
    let mapped = map(&folder, "Ours", 21, b"three", "png").unwrap();
    let names: BTreeSet<_> = mapped.iter().map(|mapping| mapping.name.as_str()).collect();
    assert_eq!(names, BTreeSet::from(["Theirs before", "Theirs", "Ours"]));
    assert!(folder.race.lock().unwrap().is_none());
}

#[test]
fn unreadable_mappings_and_pictures_are_passed_over() {
    let folder = Folder::default();
    map(&folder, "Launch", 13, b"rocket", "png").unwrap();
    let rocket = art_name(b"rocket", "png");
    let entry = |art: &str| serde_json::json!({"name": "Odd", "shape": 1, "art": art, "mapped": 1});
    let stored = serde_json::json!([
        mappings(&folder).unwrap()[0],
        entry("../../Open Notebook.onetoc2"),
        entry(&rocket.replace(".png", ".exe")),
        {"name": "No art"},
        7,
    ]);
    folder
        .files
        .lock()
        .unwrap()
        .insert(MAPPING.into(), stored.to_string().into_bytes());
    assert_eq!(mappings(&folder).unwrap().len(), 1);
    folder
        .files
        .lock()
        .unwrap()
        .insert(MAPPING.into(), b"{not json".to_vec());
    assert!(mappings(&folder).unwrap().is_empty());
    let kind = |result: Result<Vec<u8>>| match result {
        Err(Error::Io(error)) => error.kind(),
        other => panic!("{other:?}"),
    };
    assert_eq!(
        kind(art(&folder, "../tags.json")),
        io::ErrorKind::InvalidInput
    );
    folder
        .files
        .lock()
        .unwrap()
        .insert(format!("{ART}/{rocket}"), b"rock".to_vec());
    assert_eq!(kind(art(&folder, &rocket)), io::ErrorKind::InvalidData);
    assert!(map(&folder, "Tag", 13, b"x", "gif").is_err());
}

/// On this computer the folder takes the hidden attribute Windows or macOS keeps, and the
/// notebook's catalog never lists it.
#[test]
fn a_notebook_folder_keeps_the_art_hidden() {
    let temporary = tempfile::tempdir().unwrap();
    let root = temporary.path().join("Notebook");
    let page = onestore::PageCreation::new(None, Some(""), "Author").unwrap();
    let mut notebook =
        crate::session::Notebook::create(&root, temporary.path().join("cache"), 0x00f0_c090, &page)
            .unwrap();
    let mapped = notebook
        .map_tag_art("Launch", 13, b"rocket", "png")
        .unwrap();
    assert_eq!(notebook.tag_art().unwrap(), mapped);
    assert_eq!(notebook.tag_art_file(&mapped[0].art).unwrap(), b"rocket");
    let folder = root.join(FOLDER);
    assert!(folder.join("tags.json").is_file());
    #[cfg(target_vendor = "apple")]
    {
        use nix::sys::stat::{FileFlag, stat};
        let flags = FileFlag::from_bits_retain(stat(&folder).unwrap().st_flags);
        assert!(flags.contains(FileFlag::UF_HIDDEN));
    }
    #[cfg(windows)]
    {
        use std::os::windows::fs::MetadataExt;
        // FILE_ATTRIBUTE_HIDDEN.
        assert_ne!(folder.metadata().unwrap().file_attributes() & 2, 0);
    }
    notebook.refresh().unwrap();
    assert!(notebook.catalog().groups.is_empty());
    assert_eq!(notebook.catalog().sections.len(), 1);
}

/// Themes go in the hidden folder beside the tag art, and a writer another overtook merges
/// again until its change holds; entries it can't read are passed over, built-ins refused.
#[test]
fn themes_merge_into_the_hidden_folder() {
    use themes::{Assignment, Scope, Themes};
    let folder = Folder::default();
    let assign = |scope, theme: &str, assigned| Themes {
        assignments: vec![Assignment {
            scope,
            theme: Some(theme.into()),
            assigned,
        }],
        ..Default::default()
    };
    let theirs = assign(Scope::Notebook, "modern", 1);
    *folder.race.lock().unwrap() = Some(serde_json::to_vec(&theirs).unwrap());
    let kept = themes::write(&folder, assign(Scope::section([7; 16]), "editorial", 2)).unwrap();
    assert!(folder.hidden.lock().unwrap().contains(".snowbound"));
    assert_eq!(kept.assignments.len(), 2);
    assert_eq!(kept, themes::read(&folder).unwrap());
    assert_eq!(kept.effective(Some([7; 16]), None).unwrap().id, "editorial");
    folder.files.lock().unwrap().insert(
        ".snowbound/themes.json".into(),
        br#"{"themes": [{"id": 3}], "assignments": [{"scope": "notebook", "theme": "manuscript", "assigned": 4}, "junk"]}"#.to_vec(),
    );
    let read = themes::read(&folder).unwrap();
    assert!(read.themes.is_empty());
    assert_eq!(read.effective(None, None).unwrap().id, "manuscript");
    let built_in = Themes {
        themes: vec![themes::built_in().remove(0)],
        ..Default::default()
    };
    assert!(themes::write(&folder, built_in).is_err());
}
