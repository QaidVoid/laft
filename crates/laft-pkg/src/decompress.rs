//! Streaming zstd decompression that accepts only the frames package files
//! may contain.

use std::io::{self, Read};

use zstd::stream::raw::{DParameter, Decoder, InBuffer, Operation, OutBuffer};

/// The largest window a package file may need: 128 MiB.
pub(crate) const MAX_WINDOW_LOG: u32 = 27;

const FRAME_MAGIC: [u8; 4] = [0x28, 0xb5, 0x2f, 0xfd];
const CHECKSUM_FLAG: u8 = 0x04;
const INPUT_LEN: usize = 128 * 1024;

/// Decompresses a sequence of zstd frames.
///
/// Every frame must be a zstd frame (skippable frames are rejected) with the
/// content checksum flag set, and none may need a window above
/// [`MAX_WINDOW_LOG`]. The checksum of each frame is verified when the frame
/// ends. An empty input is an error, because a package has at least one
/// frame.
pub(crate) struct Decompressor<R> {
    input: R,
    decoder: Decoder<'static>,
    buf: Box<[u8]>,
    start: usize,
    end: usize,
    at_frame_start: bool,
    frames: u64,
}

impl<R: Read> Decompressor<R> {
    pub(crate) fn new(input: R) -> io::Result<Self> {
        let mut decoder = Decoder::new()?;
        decoder.set_parameter(DParameter::WindowLogMax(MAX_WINDOW_LOG))?;
        Ok(Self {
            input,
            decoder,
            buf: vec![0; INPUT_LEN].into_boxed_slice(),
            start: 0,
            end: 0,
            at_frame_start: true,
            frames: 0,
        })
    }

    /// Reads more input after the buffered bytes. Returns `false` at the end
    /// of the input.
    fn fill(&mut self) -> io::Result<bool> {
        if self.start > 0 {
            self.buf.copy_within(self.start..self.end, 0);
            self.end -= self.start;
            self.start = 0;
        }
        loop {
            match self.input.read(&mut self.buf[self.end..]) {
                Ok(0) => return Ok(false),
                Ok(n) => {
                    self.end += n;
                    return Ok(true);
                }
                Err(e) if e.kind() == io::ErrorKind::Interrupted => {}
                Err(e) => return Err(e),
            }
        }
    }

    /// Checks the header of the next frame. Returns `false` at a clean end
    /// of the input.
    fn start_frame(&mut self) -> io::Result<bool> {
        while self.end - self.start < FRAME_MAGIC.len() + 1 {
            if !self.fill()? {
                break;
            }
        }
        let header = &self.buf[self.start..self.end];
        if header.is_empty() && self.frames > 0 {
            return Ok(false);
        }
        if header.len() < FRAME_MAGIC.len() + 1 || header[..4] != FRAME_MAGIC {
            return Err(invalid("the input is not a sequence of zstd frames"));
        }
        if header[4] & CHECKSUM_FLAG == 0 {
            return Err(invalid("a zstd frame has no content checksum"));
        }
        self.frames += 1;
        self.at_frame_start = false;
        Ok(true)
    }
}

impl<R: Read> Read for Decompressor<R> {
    fn read(&mut self, out: &mut [u8]) -> io::Result<usize> {
        if out.is_empty() {
            return Ok(0);
        }
        loop {
            if self.at_frame_start && !self.start_frame()? {
                return Ok(0);
            }
            if self.start == self.end && !self.fill()? {
                return Err(io::Error::new(
                    io::ErrorKind::UnexpectedEof,
                    "the zstd stream ends inside a frame",
                ));
            }
            let mut input = InBuffer::around(&self.buf[self.start..self.end]);
            let mut output = OutBuffer::around(&mut *out);
            let hint = self.decoder.run(&mut input, &mut output)?;
            self.start += input.pos();
            if hint == 0 {
                self.at_frame_start = true;
            }
            if output.pos() > 0 {
                return Ok(output.pos());
            }
        }
    }
}

fn invalid(message: &str) -> io::Error {
    io::Error::new(io::ErrorKind::InvalidData, message)
}

#[cfg(test)]
#[path = "decompress_tests.rs"]
mod tests;
