use onestore::CommitIo;
use std::io;

pub enum Event {
    Write(usize, Vec<u8>),
    Flush,
}

pub struct Trace {
    pub bytes: Vec<u8>,
    pub events: Vec<Event>,
}

impl CommitIo for Trace {
    fn read_at(&mut self, offset: u64, output: &mut [u8]) -> io::Result<usize> {
        let offset = offset as usize;
        let count = output.len().min(self.bytes.len().saturating_sub(offset));
        if count != 0 {
            output[..count].copy_from_slice(&self.bytes[offset..offset + count]);
        }
        Ok(count)
    }
    fn write_at(&mut self, offset: u64, bytes: &[u8]) -> io::Result<usize> {
        let offset = offset as usize;
        let count = bytes.len().min(4096);
        self.bytes.resize(self.bytes.len().max(offset + count), 0);
        self.bytes[offset..offset + count].copy_from_slice(&bytes[..count]);
        self.events
            .push(Event::Write(offset, bytes[..count].to_vec()));
        Ok(count)
    }
    fn flush(&mut self) -> io::Result<()> {
        self.events.push(Event::Flush);
        Ok(())
    }
}
