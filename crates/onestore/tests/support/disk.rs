pub struct Disk {
    pub visible: Vec<u8>,
    pub durable: Vec<u8>,
    pub operation: usize,
    pub fail_at: Option<usize>,
    pub write_limit: usize,
    pub random: u64,
}

impl Disk {
    fn interrupt(&mut self) -> std::io::Result<()> {
        self.operation += 1;
        if self.fail_at != Some(self.operation) {
            return Ok(());
        }
        self.durable.resize(self.visible.len(), 0);
        for (durable, visible) in self.durable.iter_mut().zip(&self.visible) {
            self.random ^= self.random << 13;
            self.random ^= self.random >> 7;
            self.random ^= self.random << 17;
            if self.random & 1 != 0 {
                *durable = *visible;
            }
        }
        Err(std::io::Error::other("Injected storage interruption"))
    }
}

impl onestore::CommitIo for Disk {
    fn read_at(&mut self, offset: u64, bytes: &mut [u8]) -> std::io::Result<usize> {
        self.interrupt()?;
        let offset = offset as usize;
        let size = bytes
            .len()
            .min(193)
            .min(self.visible.len().saturating_sub(offset));
        bytes[..size].copy_from_slice(&self.visible[offset..offset + size]);
        Ok(size)
    }
    fn write_at(&mut self, offset: u64, bytes: &[u8]) -> std::io::Result<usize> {
        let offset = offset as usize;
        let size = bytes.len().min(self.write_limit);
        self.visible
            .resize(self.visible.len().max(offset + size), 0);
        self.visible[offset..offset + size].copy_from_slice(&bytes[..size]);
        self.interrupt()?;
        Ok(size)
    }
    fn flush(&mut self) -> std::io::Result<()> {
        self.interrupt()?;
        self.durable.clone_from(&self.visible);
        Ok(())
    }
}
