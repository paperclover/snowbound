use super::*;

/// A mounted notebook where another writer commits `revision` to the section file just
/// before Snowbound's rewritten file takes its place.
struct Racing {
    inner: Directory,
    revision: Mutex<Option<(String, Transaction)>>,
}

impl Racing {
    fn race(&self, path: &str) {
        let revision = self.revision.lock().unwrap().take();
        if let Some((section, transaction)) = revision {
            assert_eq!(section, path);
            self.inner.commit(&section, &transaction).unwrap();
        }
    }
}

impl Storage for Racing {
    fn discover(
        &self,
        cache: &mut discover::Cache,
        limits: discover::Limits,
    ) -> Result<discover::Folder> {
        self.inner.discover(cache, limits)
    }
    fn location(&self) -> String {
        self.inner.location()
    }
    fn entries(&self, folder: &str) -> io::Result<Vec<discover::Entry>> {
        self.inner.entries(folder)
    }
    fn exists(&self, path: &str) -> bool {
        self.inner.exists(path)
    }
    fn stamp(&self, path: &str) -> io::Result<Stamp> {
        self.inner.stamp(path)
    }
    fn read(&self, path: &str) -> Result<Vec<u8>> {
        self.inner.read(path)
    }
    fn read_file(&self, path: &str, limit: usize) -> Result<Vec<u8>> {
        self.inner.read_file(path, limit)
    }
    fn create(&self, path: &str, bytes: &[u8]) -> Result<()> {
        self.inner.create(path, bytes)
    }
    fn create_directory(&self, path: &str) -> Result<()> {
        self.inner.create_directory(path)
    }
    fn hide(&self, path: &str) -> Result<()> {
        self.inner.hide(path)
    }
    fn rename(&self, from: &str, to: &str) -> Result<()> {
        self.inner.rename(from, to)
    }
    fn rename_root(&self, name: &str, files: &[String]) -> Result<String> {
        self.inner.rename_root(name, files)
    }
    fn replace(&self, from: &str, to: &str) -> Result<()> {
        self.inner.replace(from, to)
    }
    fn delete(&self, path: &str) -> Result<()> {
        self.inner.delete(path)
    }
    fn place(&self, path: &str, ancestor: [u8; 16], name: &str) -> Result<()> {
        self.inner.place(path, ancestor, name)
    }
    fn commit(&self, path: &str, transaction: &Transaction) -> Result<()> {
        self.inner.commit(path, transaction)
    }
    fn confirm(&self, path: &str, base: &Stamp) -> std::result::Result<(), CommitError> {
        self.inner.confirm(path, base)
    }
    fn supersede(&self, path: &str, base: &Stamp, with: &str) -> Result<()> {
        self.race(path);
        self.inner.supersede(path, base, with)
    }
}

/// A revision another writer commits while a password is being set is never overwritten:
/// the section keeps it, and the password waits for a later try.
#[test]
fn a_password_never_overwrites_a_concurrent_revision() {
    let temporary = tempfile::tempdir().unwrap();
    let folder = temporary.path().join("notebook");
    let cache = temporary.path().join("cache");
    let creation = PageCreation::new(None, Some("Mine"), "Fixture").unwrap();
    let path = Notebook::create(&folder, &cache, 0x00f0_a0c0, &creation)
        .unwrap()
        .catalog()
        .sections[0]
        .path
        .clone();
    let file = folder.canonicalize().unwrap().join(&path);
    let image = std::fs::read(&file).unwrap();
    let arena = onestore::Arena::default();
    let mut section = onestore::Section::open(&arena, image).unwrap();
    let theirs = PageCreation::new(None, Some("Theirs"), "OneNote").unwrap();
    section
        .apply(
            "OneNote",
            &Edit {
                at: crate::now(),
                ops: vec![Op::Section(SectionOp::Create(theirs))],
            },
        )
        .unwrap();
    let revision = section.seal().unwrap().unwrap();

    let root = folder.canonicalize().unwrap();
    let storage = Racing {
        inner: Directory(root.clone()),
        revision: Mutex::new(Some((path.clone(), revision))),
    };
    let mut notebook = Notebook::with(Box::new(storage), Some(root), &cache).unwrap();
    let set = notebook.set_password(&path, None, Some("secret"));
    let titles = |image: &[u8]| -> Vec<String> {
        stored_pages(image)
            .unwrap()
            .into_iter()
            .map(|page| page.page.title)
            .collect()
    };
    let stored = std::fs::read(&file).unwrap();
    match set {
        Ok(key) => {
            let key = key.unwrap();
            let pages = stored_pages_unlocked(&stored, &key).unwrap();
            assert!(pages.iter().any(|page| page.page.title == "Theirs"));
        }
        Err(_) => assert_eq!(titles(&stored), ["Mine", "Theirs"]),
    }
}
