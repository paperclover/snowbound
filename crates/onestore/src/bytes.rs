use crate::Error;

type Result<T> = std::result::Result<T, Error>;

pub(crate) struct Cursor<'a> {
    pub(crate) bytes: &'a [u8],
    pub(crate) offset: usize,
}

impl<'a> Cursor<'a> {
    pub(crate) fn take(&mut self, len: usize) -> Result<&'a [u8]> {
        let Some((head, tail)) = self.bytes.split_at_checked(len) else {
            return Err(Error {
                offset: self.offset,
                message: "Truncated structure",
            });
        };
        self.bytes = tail;
        self.offset += len;
        Ok(head)
    }

    pub(crate) fn read<const N: usize>(&mut self) -> Result<[u8; N]> {
        let mut value = [0; N];
        value.copy_from_slice(self.take(N)?);
        Ok(value)
    }
}
