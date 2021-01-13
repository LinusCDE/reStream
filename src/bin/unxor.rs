//! This reverts the xoring done by the reMarkable.
//! It is supposed to be compiled for the host architecture to run on the PC.

use restream::xor::UnxorStream; // restream her refers to the lib part. Possible includes are in lib.rs.
use std::io::{stdin, stdout, Read, Result, Write};

use clap::{crate_authors, crate_version, Clap};

#[derive(Clap)]
#[clap(version = crate_version!(), author = crate_authors!())]
pub struct Opts {
    #[clap(about = "Block size used for unxoring. Should be same as framebuffer size.")]
    block_size: usize,
}

fn main() -> Result<()> {
    let opts: Opts = Opts::parse();

    let stdin = stdin();
    let stdout = stdout();
    let mut stdin_wrapper = UnxorStream::new(opts.block_size, stdin.lock());
    let mut stdout = stdout.lock();

    let mut buf = [0u8; 1024 * 1024 * 4];
    loop {
        let bytes = stdin_wrapper.read(&mut buf)?;
        stdout.write(&buf[0..bytes])?;
    }
}
