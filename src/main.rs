#[macro_use]
extern crate anyhow;
extern crate lz_fear;

use anyhow::{Context, Result};
use clap::{crate_authors, crate_version, Clap};
use lz_fear::CompressionSettings;

use std::default::Default;
use std::fs::File;
use std::io::{BufRead, BufReader, Read, Seek, SeekFrom, Write};
use std::process::Command;
use std::time::{Duration, SystemTime};

#[derive(Clap)]
#[clap(version = crate_version!(), author = crate_authors!())]
pub struct Opts {
    #[clap(
        long,
        name = "address",
        short = 'c',
        about = "Establish a new unsecure connection to send the data to which reduces some load on the reMarkable and improves fps."
    )]
    connect: Option<String>,

    #[clap(
        long,
        short = 'f',
        about = "Limit framerate to the given one. Reduces bandwidth and prevents capping out the cpu all the time."
    )]
    fps_cap: Option<f32>,

    #[clap(
        long,
        short = 'm',
        about = "Always transcode the framebuffer to monow (instead streaming the native pix_fmt)"
    )]
    monow: bool,
}

fn main() -> Result<()> {
    let ref opts: Opts = Opts::parse();

    let version = remarkable_version()?;
    let streamer: Box<dyn Read> = if version == "reMarkable 1.0\n" {
        let width = 1408;
        let height = 1872;
        let bytes_per_pixel = 2;

        let restreamer =
            ReStreamer::init("/dev/fb0", 0, width, height, bytes_per_pixel, opts.fps_cap)?;
        if opts.monow {
            Box::new(MonowTranscoder::new(
                width,
                height,
                bytes_per_pixel,
                restreamer,
            )?)
        } else {
            Box::new(restreamer)
        }
    } else if version == "reMarkable 2.0\n" {
        let width = 1404;
        let height = 1872;
        let bytes_per_pixel = 1;

        let pid = xochitl_pid()?;
        let offset = rm2_fb_offset(pid)?;
        let mem = format!("/proc/{}/mem", pid);

        let restreamer =
            ReStreamer::init(&mem, offset, width, height, bytes_per_pixel, opts.fps_cap)?;
        if opts.monow {
            Box::new(MonowTranscoder::new(
                width,
                height,
                bytes_per_pixel,
                restreamer,
            )?)
        } else {
            Box::new(restreamer)
        }
    } else {
        Err(anyhow!(
            "Unknown reMarkable version: {}\nPlease open a feature request to support your device.",
            version
        ))?
    };

    let stdout = std::io::stdout();
    let data_target: Box<dyn Write> = if let Some(ref address) = opts.connect {
        let conn = std::net::TcpStream::connect(address)?;
        conn.set_write_timeout(Some(std::time::Duration::from_secs(3)))?;
        Box::new(conn)
    } else {
        Box::new(stdout.lock())
    };

    let mut lz4: CompressionSettings = CompressionSettings::default();
    if opts.monow {
        // The default block size will make the monow transcoding seem extremly
        // laggy since the frames a lot smaller and better compressable.
        lz4.block_size(64 * 1024);
    }
    lz4.compress(streamer, data_target)
        .context("Error while compressing framebuffer stream")
}

fn remarkable_version() -> Result<String> {
    let content = std::fs::read("/sys/devices/soc0/machine")
        .context("Failed to read /sys/devices/soc0/machine")?;
    Ok(String::from_utf8(content)?)
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

pub struct FrameCapper {
    last_frame: SystemTime,
    frame_duration: Duration,
    missed_frames: u32,
}

impl FrameCapper {
    pub fn new(fps_cap: f32) -> Self {
        Self {
            last_frame: SystemTime::now(),
            frame_duration: Duration::from_micros((1000000.0 / fps_cap) as u64),
            missed_frames: 0,
        }
    }

    pub fn sync_framerate(&mut self) -> Result<()> {
        // Delay until next frame should occur
        let elapsed = self.last_frame.elapsed().unwrap();
        if self.frame_duration > elapsed {
            std::thread::sleep(self.frame_duration - elapsed);
            self.missed_frames = (self.missed_frames as i32 - 1).max(0) as u32;
        } else {
            self.missed_frames += 1;
        }

        // Allow more frames in a small time window to catch up (1s here).
        // After that window the missed frames are forgotten to restore a
        // resonable framerate again.
        if self.missed_frames * self.frame_duration > Duration::from_secs(1) {
            self.last_frame = SystemTime::now();
        } else {
            self.last_frame = self
                .last_frame
                .checked_add(self.frame_duration)
                .context("Error calculating next frame time")?;
        }

        Ok(())
    }
}

pub struct ReStreamer {
    file: File,
    start: u64,
    cursor: usize,
    size: usize,
    framecapper: Option<FrameCapper>,
}

impl ReStreamer {
    pub fn init(
        path: &str,
        offset: usize,
        width: usize,
        height: usize,
        bytes_per_pixel: usize,
        fps_cap: Option<f32>,
    ) -> Result<ReStreamer> {
        let start = offset as u64;
        let size = width * height * bytes_per_pixel;
        let cursor = 0;
        let file = File::open(path)?;
        let framecapper = fps_cap.map(|fps_cap| FrameCapper::new(fps_cap));
        let mut streamer = ReStreamer {
            file,
            start: start,
            cursor,
            size,
            framecapper,
        };
        streamer.next_frame()?;
        Ok(streamer)
    }

    pub fn next_frame(&mut self) -> std::io::Result<()> {
        self.file.seek(SeekFrom::Start(self.start))?;
        self.cursor = 0;
        if let Some(ref mut framecapper) = self.framecapper {
            framecapper.sync_framerate().ok();
        }
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

struct MonowTranscoder<R> {
    inner: R,
    bytes_per_pixel: usize,
    native_size: usize,
    monow_data: Option<std::io::Cursor<Vec<u8>>>,
}

impl<R: Read> MonowTranscoder<R> {
    pub fn new(width: usize, height: usize, bytes_per_pixel: usize, inner: R) -> Result<Self> {
        let monow_size = (width * height) / 8;
        let mut instance = Self {
            inner,
            native_size: width * height * bytes_per_pixel,
            bytes_per_pixel,
            monow_data: Some(std::io::Cursor::new(vec![0u8; monow_size])),
        };
        instance.refill_bw_data()?;
        Ok(instance)
    }

    fn refill_bw_data(&mut self) -> Result<()> {
        match self.bytes_per_pixel {
            1 => self.refill_bw_data_1byte_per_pixel(),
            2 => self.refill_bw_data_2bytes_per_pixel(),
            _ => Err(anyhow!("Unsupported bytes_per_pixel value!")),
        }
    }

    /// Using a lot of unsafe to improve performance (probably not best but works).
    fn refill_bw_data_2bytes_per_pixel(&mut self) -> Result<()> {
        let mut fb_data = vec![0u8; self.native_size];
        self.inner.read_exact(&mut fb_data)?;

        let mut monow_data_vec: Vec<u8> = self
            .monow_data
            .take()
            .ok_or(anyhow!("monow_data is empty!"))?
            .into_inner();

        // Still a poc. Basicially convert every 16 bytes into
        // one byte of just 8 black and white pixels (2 bytes each are pixel on the rm1).
        // Using pointers gives some signifiant perf improment since no bounds checking
        // is done. I'm not aware of a faster better solution rn and it's just a poc anyway.
        unsafe {
            let mut fb_data_ptr = fb_data.as_ptr();
            let mut bw_data_ptr = monow_data_vec.as_mut_ptr();

            let mut i = 0;
            while i < self.native_size {
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

        self.monow_data = Some(std::io::Cursor::new(monow_data_vec));
        Ok(())
    }

    /// Using a lot of unsafe to improve performance (probably not best but works).
    fn refill_bw_data_1byte_per_pixel(&mut self) -> Result<()> {
        let mut fb_data = vec![0u8; self.native_size];
        self.inner.read_exact(&mut fb_data)?;

        let mut monow_data_vec: Vec<u8> = self
            .monow_data
            .take()
            .ok_or(anyhow!("monow_data is empty!"))?
            .into_inner();

        // Still a poc. Basicially convert every 8 bytes into
        // one byte of just 8 black and white pixels (8 bytes each are pixel on the rm2, I guess).
        // Using pointers gives some signifiant perf improment since no bounds checking
        // is done. I'm not aware of a faster better solution rn and it's just a poc anyway.
        unsafe {
            let mut fb_data_ptr = fb_data.as_ptr();
            let mut bw_data_ptr = monow_data_vec.as_mut_ptr();

            let mut i = 0;
            while i < self.native_size {
                let pixels = (
                    *fb_data_ptr.add(0) == 0,
                    *fb_data_ptr.add(1) == 0,
                    *fb_data_ptr.add(2) == 0,
                    *fb_data_ptr.add(3) == 0,
                    *fb_data_ptr.add(4) == 0,
                    *fb_data_ptr.add(5) == 0,
                    *fb_data_ptr.add(6) == 0,
                    *fb_data_ptr.add(7) == 0,
                );
                fb_data_ptr = fb_data_ptr.add(8);

                *bw_data_ptr = ((pixels.0 as u8) << 7)
                    | ((pixels.1 as u8) << 6)
                    | ((pixels.2 as u8) << 5)
                    | ((pixels.3 as u8) << 4)
                    | ((pixels.4 as u8) << 3)
                    | ((pixels.5 as u8) << 2)
                    | ((pixels.6 as u8) << 1)
                    | ((pixels.7 as u8) << 0);
                bw_data_ptr = bw_data_ptr.add(1);

                i += 8;
            }
        }

        self.monow_data = Some(std::io::Cursor::new(monow_data_vec));
        Ok(())
    }
}

impl<R: Read> Read for MonowTranscoder<R> {
    fn read(&mut self, buf: &mut [u8]) -> std::io::Result<usize> {
        if let Some(ref mut monow_data) = self.monow_data {
            let bytes_read = monow_data.read(buf)?;
            if bytes_read < buf.len() {
                if let Err(e) = self.refill_bw_data() {
                    eprintln!("Err while refilling cached monow_data: {}", e);
                    return Err(std::io::Error::from(std::io::ErrorKind::Other));
                }
            }

            Ok(bytes_read)
        } else {
            Err(std::io::Error::from(std::io::ErrorKind::Other))
        }
    }
}
