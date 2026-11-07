//! H.264 and HEVC through VA-API: cros-codecs' stateless decoders (its
//! parsers, and what a decoder keeps between pictures: the decoded
//! picture buffer, picture order, reference lists, which picture shows
//! next) driving a backend of ours, which hands each picture to the
//! driver (`h264.rs`, `h265.rs` fill its parameters) and copies the
//! pictures shown out as `Va::read` does for AV1 and VP9.
//!
//! A picture is decoded into one of the session's surfaces, taken from
//! the free ones and given back when nothing holds it any more: not the
//! decoder's references, not a picture waiting to be shown.

use std::{cell::RefCell, rc::Rc, sync::Arc};

use cros_codecs::{
    DecodedFormat, Fourcc, Resolution,
    decoder::{
        DecodedHandle, DecoderEvent, StreamInfo,
        stateless::{DecodeError, NewPictureError, NewPictureResult, StatelessBackendError, StatelessBackendResult, StatelessDecoderBackend, StatelessVideoDecoder},
    },
    video_frame::{ReadMapping, VideoFrame, WriteMapping},
};
use va_sys as va;

use super::{Picture as Shown, Va};

/// The "frame" cros-codecs gives each picture: only its size (the
/// picture is in a surface of the session's, `Surface`).
#[derive(Debug)]
pub(super) struct Token(Resolution);

impl VideoFrame for Token {
    fn fourcc(&self) -> Fourcc {
        Fourcc::from(b"NV12")
    }

    fn resolution(&self) -> Resolution {
        self.0
    }

    fn get_plane_size(&self) -> Vec<usize> {
        Vec::new()
    }

    fn get_plane_pitch(&self) -> Vec<usize> {
        Vec::new()
    }

    fn map<'a>(&'a self) -> Result<Box<dyn ReadMapping<'a> + 'a>, String> {
        Err("a surface's picture is read through VA-API".into())
    }

    fn map_mut<'a>(&'a mut self) -> Result<Box<dyn WriteMapping<'a> + 'a>, String> {
        Err("a surface's picture is read through VA-API".into())
    }
}

/// The session's surfaces free to decode into, by index.
type Free = Rc<RefCell<Vec<usize>>>;

/// A surface holding a picture, free again when it goes.
pub(super) struct Surface {
    pub index: usize,
    pub id: va::VASurfaceID,
    free: Free,
}

impl Drop for Surface {
    fn drop(&mut self) {
        self.free.borrow_mut().push(self.index);
    }
}

/// A decoded picture, as cros-codecs keeps it (its references, its
/// pictures waiting to show).
#[derive(Clone)]
pub(super) struct Handle(Rc<Decoded>);

pub(super) struct Decoded {
    pub surface: Rc<Surface>,
    timestamp: u64,
    size: Resolution,
    frame: Arc<Token>,
}

impl Handle {
    /// Its surface's id.
    pub fn id(&self) -> va::VASurfaceID {
        self.0.surface.id
    }

    pub fn surface(&self) -> &Rc<Surface> {
        &self.0.surface
    }
}

impl DecodedHandle for Handle {
    type Frame = Token;

    fn video_frame(&self) -> Arc<Token> {
        self.0.frame.clone()
    }

    fn timestamp(&self) -> u64 {
        self.0.timestamp
    }

    fn coded_resolution(&self) -> Resolution {
        self.0.size
    }

    fn display_resolution(&self) -> Resolution {
        self.0.size
    }

    // (Read only once decoded: `Va::read` waits for the GPU.)
    fn is_ready(&self) -> bool {
        true
    }

    fn sync(&self) -> anyhow::Result<()> {
        Ok(())
    }
}

/// A picture being decoded: its surface and the buffers for it so far.
pub(super) struct Picture {
    pub surface: Rc<Surface>,
    pub timestamp: u64,
    pub buffers: Vec<va::VABufferID>,
}

/// What a stream's pictures look like (from its sequence parameters).
#[derive(Clone, Copy, Default)]
pub(super) struct Look {
    /// The visible picture (the decoded one is cropped to it).
    pub width: u32,
    pub height: u32,
    pub matrix: u8,
    pub full_range: bool,
}

/// Ours for cros-codecs: the session, its free surfaces, and what the
/// stream is.
pub(super) struct Backend {
    pub va: Rc<RefCell<Va>>,
    free: Free,
    info: Option<StreamInfo>,
    pub look: Rc<RefCell<Look>>,
}

impl Backend {
    pub fn new(va: Rc<RefCell<Va>>, look: Rc<RefCell<Look>>) -> Backend {
        Backend { va, free: Rc::default(), info: None, look }
    }

    /// A new sequence: a session for it (the one there is, if it's the
    /// same).
    pub fn sequence(&mut self, profile: va::VAProfile, deep: bool, coded: Resolution, look: Look, surfaces: usize) -> StatelessBackendResult<()> {
        let new = self.va.borrow_mut().session(profile, coded.width, coded.height, deep, surfaces).map_err(other)?;
        if new {
            // (Surfaces of the old session give themselves back to the
            // old list, which goes with them.)
            self.free = Rc::new(RefCell::new((0..surfaces).rev().collect()));
        }
        *self.look.borrow_mut() = look;
        self.info = Some(StreamInfo {
            format: if deep { DecodedFormat::I010 } else { DecodedFormat::NV12 },
            coded_resolution: coded,
            display_resolution: Resolution { width: look.width, height: look.height },
            min_num_frames: surfaces,
        });
        Ok(())
    }

    /// A free surface for a new picture.
    pub fn picture(&mut self, timestamp: u64) -> NewPictureResult<Picture> {
        let index = self.free.borrow_mut().pop().ok_or(NewPictureError::OutOfOutputBuffers)?;
        let id = self.va.borrow().session.as_ref().map(|s| s.surfaces[index]).ok_or_else(|| NewPictureError::BackendError(anyhow::anyhow!("no session")))?;
        Ok(Picture { surface: Rc::new(Surface { index, id, free: self.free.clone() }), timestamp, buffers: Vec::new() })
    }

    /// A buffer of one `T` for `picture`.
    pub fn add<T>(&self, picture: &mut Picture, kind: va::VABufferType, value: &mut T) -> StatelessBackendResult<()> {
        let b = self.va.borrow().buffer(kind, value, 1).map_err(other)?;
        picture.buffers.push(b);
        Ok(())
    }

    /// Bytes (a slice's data) for `picture`.
    pub fn add_bytes(&self, picture: &mut Picture, bytes: &[u8]) -> StatelessBackendResult<()> {
        let b = self.va.borrow().buffer_bytes(va::VASliceDataBufferType, bytes).map_err(other)?;
        picture.buffers.push(b);
        Ok(())
    }

    /// `picture` decoded: handed to the driver, its buffers let go.
    pub fn submit(&mut self, mut picture: Picture) -> StatelessBackendResult<Handle> {
        let va = self.va.borrow();
        let done = va.run(picture.surface.id, &picture.buffers);
        va.drop_buffers(&std::mem::take(&mut picture.buffers));
        done.map_err(other)?;
        let size = self.info.as_ref().map(|i| i.coded_resolution).unwrap_or_default();
        Ok(Handle(Rc::new(Decoded { surface: picture.surface, timestamp: picture.timestamp, size, frame: Arc::new(Token(size)) })))
    }
}

impl StatelessDecoderBackend for Backend {
    type Handle = Handle;

    fn stream_info(&self) -> Option<&StreamInfo> {
        self.info.as_ref()
    }

    fn reset_backend(&mut self) -> anyhow::Result<()> {
        Ok(())
    }
}

pub(super) fn other(e: String) -> StatelessBackendError {
    StatelessBackendError::Other(anyhow::anyhow!(e))
}

/// A stream through one of cros-codecs' decoders and our backend: its
/// samples in, the pictures it shows out (in display order, each with
/// its sample's time).
pub(super) struct Stream<D> {
    pub dec: D,
    pub va: Rc<RefCell<Va>>,
    pub look: Rc<RefCell<Look>>,
}

impl<D: StatelessVideoDecoder<Handle = Handle>> Stream<D> {
    /// `data` (Annex B: start codes before each NAL unit), shown at `pts`.
    pub fn decode(&mut self, data: &[u8], pts: i64, out: &mut Vec<Shown>) -> Result<(), String> {
        let mut rest = data;
        let mut waited = 0;
        while !rest.is_empty() {
            let size = self.dec.stream_info().map(|i| i.coded_resolution).unwrap_or_default();
            match self.dec.decode(pts as u64, rest, &mut || Some(Token(size))) {
                Ok(0) => break,
                Ok(n) => {
                    rest = rest.get(n..).unwrap_or_default();
                    waited = 0;
                }
                // Pictures to take (or the format confirmed) first.
                Err(DecodeError::CheckEvents | DecodeError::NotEnoughOutputBuffers(_)) => {
                    if !self.events(out)? {
                        waited += 1;
                        if waited > 2 {
                            return Err("the decoder has no surface left to decode into".into());
                        }
                    }
                }
                Err(e) => return Err(e.to_string()),
            }
        }
        self.events(out).map(|_| ())
    }

    /// The pictures still held, shown (the stream ended).
    pub fn flush(&mut self, out: &mut Vec<Shown>) -> Result<(), String> {
        self.dec.flush().map_err(|e| e.to_string())?;
        self.events(out).map(|_| ())
    }

    /// Everything held let go (a seek): it starts again at the next key
    /// frame.
    pub fn reset(&mut self) {
        let _ = self.dec.flush();
        while self.dec.next_event().is_some() {}
    }

    /// The decoder's events: each picture ready, read out; whether there
    /// were any.
    fn events(&mut self, out: &mut Vec<Shown>) -> Result<bool, String> {
        let mut any = false;
        while let Some(e) = self.dec.next_event() {
            any = true;
            if let DecoderEvent::FrameReady(h) = e {
                let look = *self.look.borrow();
                let picture = self.va.borrow_mut().read(h.surface().index, look.width, look.height, h.timestamp() as i64, look.matrix, look.full_range)?;
                out.push(picture);
            }
        }
        Ok(any)
    }
}
