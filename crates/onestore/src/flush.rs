use std::{fs::File, io};

pub(crate) fn flush(file: &File) -> io::Result<()> {
    let result = file.sync_all();
    #[cfg(target_os = "macos")]
    if result
        .as_ref()
        .is_err_and(|error| error.raw_os_error() == Some(nix::libc::ENOTSUP))
    {
        // SMB uses standard FLUSH when Apple's F_FULLFSYNC extension is unavailable.
        return nix::unistd::fsync(file).map_err(io::Error::from);
    }
    result
}
