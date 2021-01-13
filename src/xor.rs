use std::collections::VecDeque;
use std::io::Result;
use std::io::{Read, Write};

/// Use xoring to improve the ability to be compressed.
/// Writing xor's with previous data.
/// Reading un-xor's with previous data.
/// The block size has to match on both ends for this to work.
pub struct XorStream<I> {
    inner: I,
    block_cache: VecDeque<u8>,
}

impl<I> XorStream<I> {
    pub fn new(block_size: usize, inner: I) -> Self {
        let mut block_cache = VecDeque::with_capacity(block_size);
        for _ in 0..block_size {
            block_cache.push_back(0u8);
        }
        Self { inner, block_cache }
    }
}

/*impl<I: Write> Write for XorStream<I> {
    fn write(&mut self, buf: &[u8]) -> Result<usize> {
        let mut xored_buf: Vec<u8> = Vec::with_capacity(buf.len());
        for i in 0..buf.len() {
            xored_buf[i] = buf[i] ^ self.block_cache.pop_front().unwrap();
            self.block_cache.push_back(xored_buf[i]);
        }

        self.inner.write_all(&mut xored_buf)?;
        Ok(buf.len())
    }

    fn flush(&mut self) -> Result<()> {
        Ok(())
    }
}*/

impl<I: Read> Read for XorStream<I> {
    fn read(&mut self, buf: &mut [u8]) -> Result<usize> {
        self.inner.read_exact(buf)?;
        for i in 0..buf.len() {
            buf[i] = buf[i] ^ self.block_cache.pop_front().unwrap();
            self.block_cache.push_back(buf[i]);
        }

        Ok(buf.len())
    }
}
