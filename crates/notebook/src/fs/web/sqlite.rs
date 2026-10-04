//! SQLite's default VFS in the browser: the files of `fs`, so that a replica, its write-ahead
//! log and the notebook it caches are kept and written out alike.
#![allow(unsafe_code)]

use super::{FILES, Node, Shared, normal, with};
use rsqlite_vfs::{
    OsCallback, SQLiteIoMethods, SQLiteVfs, SQLiteVfsFile, VfsError, VfsFile, VfsResult, VfsStore,
    ffi::{SQLITE_CANTOPEN, SQLITE_IOERR, SQLITE_IOERR_DELETE, sqlite3_vfs},
};
use std::{path::Path, rc::Rc, sync::Once, time::Duration};

/// Registers the VFS as SQLite's default, once.
pub(crate) fn install() {
    static INSTALLED: Once = Once::new();
    INSTALLED.call_once(|| {
        rsqlite_vfs::register_vfs::<Methods, Vfs>("snowbound", (), true)
            .expect("SQLite takes the browser's VFS");
    });
}

struct File(Shared);

impl VfsFile for File {
    fn read(&self, output: &mut [u8], offset: usize) -> VfsResult<bool> {
        let data = self.0.borrow();
        let rest = data.bytes.get(offset..).unwrap_or_default();
        let count = rest.len().min(output.len());
        output[..count].copy_from_slice(&rest[..count]);
        output[count..].fill(0);
        Ok(count == output.len())
    }

    fn write(&mut self, input: &[u8], offset: usize) -> VfsResult<()> {
        let mut data = self.0.borrow_mut();
        let end = offset + input.len();
        if data.bytes.len() < end {
            data.bytes.resize(end, 0);
        }
        data.bytes[offset..end].copy_from_slice(input);
        data.wrote(offset..end);
        Ok(())
    }

    fn truncate(&mut self, size: usize) -> VfsResult<()> {
        let mut data = self.0.borrow_mut();
        data.bytes.truncate(size);
        data.wrote(size..size);
        Ok(())
    }

    fn flush(&mut self) -> VfsResult<()> {
        self.0.borrow_mut().flush();
        Ok(())
    }

    fn size(&self) -> VfsResult<usize> {
        Ok(self.0.borrow().bytes.len())
    }
}

fn error(code: i32, error: std::io::Error) -> VfsError {
    VfsError::new(code, error.to_string())
}

struct Store;

impl Store {
    fn data(file: &SQLiteVfsFile) -> VfsResult<Shared> {
        // SAFETY: SQLite hands back the file `xOpen` named, whose name lives until `xClose`.
        let name = unsafe { file.name() };
        with(|files| files.file(&normal(Path::new(name)))).map_err(|io| error(SQLITE_IOERR, io))
    }
}

impl VfsStore<File, ()> for Store {
    fn add_file(_: *mut sqlite3_vfs, name: &str, _: i32) -> VfsResult<()> {
        // SQLite's own files beside a replica, such as its write-ahead log, need no folder made.
        let path = normal(Path::new(name));
        with(|files| files.create(path))
            .map(drop)
            .map_err(|io| error(SQLITE_CANTOPEN, io))
    }

    fn contains_file(_: *mut sqlite3_vfs, name: &str) -> VfsResult<bool> {
        let path = normal(Path::new(name));
        Ok(FILES.with_borrow(|files| matches!(files.nodes.get(&path), Some(Node::File(_)))))
    }

    fn delete_file(_: *mut sqlite3_vfs, name: &str) -> VfsResult<()> {
        super::remove_file(name).map_err(|io| error(SQLITE_IOERR_DELETE, io))
    }

    fn with_file<F: Fn(&File) -> VfsResult<i32>>(file: &SQLiteVfsFile, act: F) -> VfsResult<i32> {
        act(&File(Self::data(file)?))
    }

    fn with_file_mut<F: Fn(&mut File) -> VfsResult<i32>>(
        file: &SQLiteVfsFile,
        act: F,
    ) -> VfsResult<i32> {
        act(&mut File(Rc::clone(&Self::data(file)?)))
    }
}

struct Methods;

impl SQLiteIoMethods for Methods {
    type File = File;
    type AppData = ();
    type Store = Store;

    const VERSION: i32 = 1;
}

struct Vfs;

impl OsCallback for Vfs {
    /// Nothing else holds a lock to wait for.
    fn sleep(_: Duration) {}

    fn random(bytes: &mut [u8]) {
        let _ = getrandom::fill(bytes);
    }

    fn epoch_timestamp_in_ms() -> i64 {
        js_sys::Date::now() as i64
    }
}

impl SQLiteVfs<Methods> for Vfs {
    const VERSION: i32 = 1;

    fn sleep(duration: Duration) {
        <Self as OsCallback>::sleep(duration)
    }

    fn random(bytes: &mut [u8]) {
        <Self as OsCallback>::random(bytes)
    }

    fn epoch_timestamp_in_ms() -> i64 {
        <Self as OsCallback>::epoch_timestamp_in_ms()
    }
}
