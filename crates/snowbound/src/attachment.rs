//! Files on the page, as OneNote 2010 attaches, opens and saves them.

use crate::{State, platform};
use onestore::page::Attachment;
use std::{
    error::Error,
    path::{Path, PathBuf},
};

impl State {
    /// Inserts a file at the caret or window point `at`, as a picture when requested.
    pub(crate) fn import_file(&mut self, path: &Path, at: Option<[f32; 2]>, picture: bool) {
        if self.session.as_ref().is_some_and(crate::Session::read_only) {
            return;
        }
        let loading = self.loading;
        let reply = self.reply(
            move |state, (path, read): (PathBuf, std::io::Result<Vec<u8>>)| {
                let bytes = match read {
                    Ok(bytes) => bytes,
                    Err(error) if error.kind() == std::io::ErrorKind::FileTooLarge => {
                        too_large();
                        return Ok(());
                    }
                    Err(_) => {
                        platform::alert("Couldn't insert the file", "Choose a file you can open.");
                        return Ok(());
                    }
                };
                if state.loading != loading {
                    platform::alert(
                        "File wasn't inserted",
                        "Return to the page and insert the file again.",
                    );
                    return Ok(());
                }
                if state
                    .session
                    .as_ref()
                    .is_some_and(crate::Session::read_only)
                {
                    return Ok(());
                }
                if picture && draw::RasterImage::measure(&bytes).is_ok() {
                    return state.insert_picture(bytes, at);
                }
                let name = path
                    .file_name()
                    .and_then(|name| name.to_str())
                    .ok_or("No file name")?;
                let file = Attachment {
                    id: onestore::page::text::new_id()?,
                    filename: name.to_owned(),
                    source_path: path.to_str().map(str::to_owned),
                    size: Some(canvas::gpu::page::ICON_SIZE),
                    layout: Default::default(),
                    preview: platform::file_icon(&path).map(Into::into),
                    recording: canvas::recording::attached(name, &bytes),
                    bytes: Some(bytes.into()),
                    tags: Vec::new(),
                };
                let response = match at {
                    Some(point) => state.view.drop_attachment(state.page_point(point), file)?,
                    None => state.view.insert_attachment(file)?,
                };
                state.respond(response);
                Ok(())
            },
        );
        let path = path.to_owned();
        crate::spawn(move || {
            let bytes = notebook::fs::read_limited(&path, notebook::MAX_FILE_BYTES);
            reply.send((path, bytes));
        });
    }

    /// Open: a recording plays; another file opens as a copy, in a folder of its own, with
    /// the system's application for it.
    pub(crate) fn open_attachment(&mut self, file: &Attachment) -> Result<(), Box<dyn Error>> {
        if file.recording.is_some() {
            return self.play(file, 0);
        }
        if let Some(path) = self.copy_attachment(file)? {
            platform::open_file(&path);
        }
        Ok(())
    }

    /// A copy of the file, in a folder of its own; none, once said, where it is not stored
    /// in the section.
    pub(crate) fn copy_attachment(
        &self,
        file: &Attachment,
    ) -> Result<Option<std::path::PathBuf>, Box<dyn Error>> {
        let Some(bytes) = &file.bytes else {
            unstored();
            return Ok(None);
        };
        let root = std::env::temp_dir().join("Snowbound Attachments");
        notebook::fs::create_dir_all(&root)?;
        // A copy opened earlier may still be open, edited, in its application.
        let folder = (0..1000)
            .map(|index| root.join(index.to_string()))
            .find(|folder| notebook::fs::create_dir(folder).is_ok())
            .ok_or("No folder is free for a copy of the file")?;
        let path = folder.join(&file.filename);
        notebook::fs::write(&path, bytes)?;
        Ok(Some(path))
    }

    /// Save As: the file's bytes where the user chooses, under its name by default.
    pub(crate) fn save_attachment(&self, file: &Attachment) -> Result<(), Box<dyn Error>> {
        let Some(bytes) = &file.bytes else {
            unstored();
            return Ok(());
        };
        let bytes = bytes.clone();
        let reply = self.reply(move |_, path| Ok(notebook::fs::write(path, bytes)?));
        platform::pick_new("Save As", &file.filename, "Save", None, reply);
        Ok(())
    }
}

pub(crate) fn too_large() {
    platform::alert(
        "File is too large",
        &format!(
            "Choose files totaling at most {} MiB.",
            notebook::MAX_FILE_BYTES >> 20
        ),
    );
}

/// A file another program stored beside the section rather than in it.
fn unstored() {
    platform::alert(
        "Couldn't open the file",
        "Open this page in OneNote to open or save the file.",
    );
}
