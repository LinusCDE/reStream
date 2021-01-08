#[macro_use]
extern crate anyhow;

use anyhow::{Context, Result};
use libremarkable::device::{Model, CURRENT_DEVICE};
use lz_fear::CompressionSettings;

use std::default::Default;
use std::fs::File;
use std::io::{BufRead, BufReader, Read, Seek, SeekFrom, Write};
use std::process::Command;
use std::thread;
use std::time::{Duration, SystemTime};

fn main() -> Result<()> {
    let mut streamer = match CURRENT_DEVICE.model {
        Model::Gen1 => {
            let width = 1408;
            let height = 1872;
            let bytes_per_pixel = 2;
            ReStreamer::init("/dev/fb0", 0, width, height, bytes_per_pixel)?
        }
        Model::Gen2 => {
            let width = 1404;
            let height = 1872;
            let bytes_per_pixel = 1;

            let pid = xochitl_pid()?;
            let offset = rm2_fb_offset(pid)?;
            let mem = format!("/proc/{}/mem", pid);
            ReStreamer::init(&mem, offset, width, height, bytes_per_pixel)?
        }
        Model::Unknown => unreachable!(),
    };

    /*
    let lz4: CompressionSettings = Default::default();
    lz4.compress(streamer, std::io::stdout().lock())
        .context("Error while compressing framebuffer stream")
    */

    let stdout = std::io::stdout();
    let mut stdout = stdout.lock();

    let frame_duration: Duration = Duration::from_micros(1000000 / 10 as u64);

    let mut last_frame = SystemTime::now();
    let mut fb_data = vec![0u8; streamer.size];
    loop {
        streamer.read_exact(&mut fb_data)?;
        let mut bw_data = vec![0u8; streamer.size / 16];

        unsafe {
            let mut fb_data_ptr = fb_data.as_ptr();
            let mut bw_data_ptr = bw_data.as_mut_ptr();

            let mut i = 0;
            while i < streamer.size {
                let pix1 = *fb_data_ptr == 0 && *fb_data_ptr.add(1) == 0;
                fb_data_ptr = fb_data_ptr.add(2);
                let pix2 = *fb_data_ptr == 0 && *fb_data_ptr.add(1) == 0;
                fb_data_ptr = fb_data_ptr.add(2);
                let pix3 = *fb_data_ptr == 0 && *fb_data_ptr.add(1) == 0;
                fb_data_ptr = fb_data_ptr.add(2);
                let pix4 = *fb_data_ptr == 0 && *fb_data_ptr.add(1) == 0;
                fb_data_ptr = fb_data_ptr.add(2);
                let pix5 = *fb_data_ptr == 0 && *fb_data_ptr.add(1) == 0;
                fb_data_ptr = fb_data_ptr.add(2);
                let pix6 = *fb_data_ptr == 0 && *fb_data_ptr.add(1) == 0;
                fb_data_ptr = fb_data_ptr.add(2);
                let pix7 = *fb_data_ptr == 0 && *fb_data_ptr.add(1) == 0;
                fb_data_ptr = fb_data_ptr.add(2);
                let pix8 = *fb_data_ptr == 0 && *fb_data_ptr.add(1) == 0;
                fb_data_ptr = fb_data_ptr.add(2);

                *bw_data_ptr = ((pix1 as u8) << 7)
                    | ((pix2 as u8) << 6)
                    | ((pix3 as u8) << 5)
                    | ((pix4 as u8) << 4)
                    | ((pix5 as u8) << 3)
                    | ((pix6 as u8) << 2)
                    | ((pix7 as u8) << 1)
                    | ((pix8 as u8) << 0);
                bw_data_ptr = bw_data_ptr.add(1);

                i += 16;
            }
        }

        stdout.write_all(&bw_data)?;
        stdout.flush()?;

        let elapsed = last_frame.elapsed().unwrap();
        if frame_duration > elapsed {
            thread::sleep(frame_duration - elapsed);
        }
        last_frame = last_frame.checked_add(frame_duration).unwrap();
    }
}

fn xochitl_pid() -> Result<usize> {
    let output = Command::new("/bin/pidof")
        .args(&["xochitl"])
        .output()
        .context("Failed to run `/bin/pidof xochitl`")?;
    if output.status.success() {
        let pid = &output.stdout;
        let pid_str = std::str::from_utf8(pid)?.trim();
        pid_str
            .parse()
            .with_context(|| format!("Failed to parse xochitl's pid: {}", pid_str))
    } else {
        Err(anyhow!(
            "Could not find pid of xochitl, is xochitl running?"
        ))
    }
}

fn rm2_fb_offset(pid: usize) -> Result<usize> {
    let file = File::open(format!("/proc/{}/maps", &pid))?;
    let line = BufReader::new(file)
        .lines()
        .skip_while(|line| matches!(line, Ok(l) if !l.ends_with("/dev/fb0")))
        .skip(1)
        .next()
        .with_context(|| format!("No line containing /dev/fb0 in /proc/{}/maps file", pid))?
        .with_context(|| format!("Error reading file /proc/{}/maps", pid))?;

    let addr = line
        .split("-")
        .next()
        .with_context(|| format!("Error parsing line in /proc/{}/maps", pid))?;

    let address = usize::from_str_radix(addr, 16).context("Error parsing framebuffer address")?;
    Ok(address + 8)
}

pub struct ReStreamer {
    file: File,
    start: u64,
    cursor: usize,
    size: usize,
}

impl ReStreamer {
    pub fn init(
        path: &str,
        offset: usize,
        width: usize,
        height: usize,
        bytes_per_pixel: usize,
    ) -> Result<ReStreamer> {
        let start = offset as u64;
        let size = width * height * bytes_per_pixel;
        let cursor = 0;
        let file = File::open(path)?;
        let mut streamer = ReStreamer {
            file,
            start: start,
            cursor,
            size,
        };
        streamer.next_frame()?;
        Ok(streamer)
    }

    pub fn next_frame(&mut self) -> std::io::Result<()> {
        self.file.seek(SeekFrom::Start(self.start))?;
        self.cursor = 0;
        Ok(())
    }
}

impl Read for ReStreamer {
    fn read(&mut self, buf: &mut [u8]) -> std::io::Result<usize> {
        let requested = buf.len();
        let bytes_read = if self.cursor + requested < self.size {
            self.file.read(buf)?
        } else {
            let rest = self.size - self.cursor;
            self.file.read(&mut buf[0..rest])?
        };
        self.cursor += bytes_read;
        if self.cursor == self.size {
            self.next_frame()?;
        }
        Ok(bytes_read)
    }
}
