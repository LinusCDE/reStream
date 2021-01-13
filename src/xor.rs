use std::collections::VecDeque;
use std::io::Read;
use std::io::Result;

/// Use xoring to improve the ability to be compressed.
/// This stream starts with an assumed block of 0s and
/// only ever reads out the xor diff from the inner source.
/// To revert this action use UnxorStream
pub struct XorStream<R> {
    inner: R,
    last_cache: Vec<u8>,
    last_cache_index: usize,
}

impl<R> XorStream<R> {
    pub fn new(block_size: usize, inner: R) -> Self {
        Self {
            inner,
            last_cache: vec![0u8; block_size],
            last_cache_index: 0,
        }
    }
}

impl<R: Read> Read for XorStream<R> {
    fn read(&mut self, buf: &mut [u8]) -> Result<usize> {
        let bytes_read = self.inner.read(buf)?;
        for i in 0..bytes_read {
            let prev_byte = self.last_cache[self.last_cache_index];
            self.last_cache[self.last_cache_index] = buf[i];
            buf[i] ^= prev_byte;

            self.last_cache_index = (self.last_cache_index + 1) % self.last_cache.len();
        }

        Ok(bytes_read)
    }
}

/// Use xoring to improve the ability to be compressed.
/// This takes the diffs from XorStream and reads the correct
/// blocks out of it.
/// It starts with the same assumed block of 0s.
pub struct UnxorStream<R> {
    inner: R,
    diff_cache: VecDeque<u8>,
}

impl<R> UnxorStream<R> {
    pub fn new(block_size: usize, inner: R) -> Self {
        let mut diff_cache = VecDeque::with_capacity(block_size);
        for _ in 0..block_size {
            diff_cache.push_back(0u8);
        }
        Self { inner, diff_cache }
    }
}

impl<R: Read> Read for UnxorStream<R> {
    fn read(&mut self, buf: &mut [u8]) -> Result<usize> {
        let bytes_read = self.inner.read(buf)?;
        for i in 0..bytes_read {
            buf[i] = (self.diff_cache.pop_front().unwrap()) ^ buf[i];
            self.diff_cache.push_back(buf[i]);
        }
        Ok(bytes_read)
    }
}
