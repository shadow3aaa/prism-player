use ffmpeg_next::{self as ffmpeg, util::frame::Audio as AudioFrame};
use tracing::error;

/// A very small helper that wraps ffmpeg audio decoding for a file and yields resampled
/// f32 interleaved frames together with their PTS (in seconds).
pub struct AudioDecoder {
    ictx: ffmpeg::format::context::Input,
    stream_index: usize,
    decoder: ffmpeg::codec::decoder::Audio,
    sent_eof: bool,
    time_base: ffmpeg::Rational,
}

impl AudioDecoder {
    pub fn new(path: &str) -> Result<Self, ffmpeg::Error> {
        // Propagate errors opening the input so the caller can handle missing or inaccessible files
        let ictx = ffmpeg::format::input(path)?;

        // Panic if no audio stream is found because this is unrecoverable at this layer
        let stream = ictx
            .streams()
            .best(ffmpeg::media::Type::Audio)
            .expect("no audio stream found in input (unsupported format)");
        let stream_index = stream.index();
        let time_base = stream.time_base();

        // Build codec context; panic on invalid stream parameters since this indicates unrecoverable input
        let context_decoder = ffmpeg::codec::context::Context::from_parameters(stream.parameters())
            .expect("failed to build codec context from stream parameters");

        // Obtain audio decoder; panic if codec unsupported because higher layers cannot recover
        let decoder = context_decoder
            .decoder()
            .audio()
            .expect("failed to obtain audio decoder (unsupported codec)");

        Ok(AudioDecoder {
            ictx,
            stream_index,
            decoder,
            sent_eof: false,
            time_base,
        })
    }

    pub fn time_base(&self) -> ffmpeg::Rational {
        self.time_base
    }
}

impl Iterator for AudioDecoder {
    type Item = AudioFrame;

    fn next(&mut self) -> Option<Self::Item> {
        let mut decoded = AudioFrame::empty();
        loop {
            match self.decoder.receive_frame(&mut decoded) {
                Ok(()) => return Some(decoded),
                Err(ffmpeg::Error::Other { errno }) if errno == ffmpeg::sys::EAGAIN => {}
                Err(ffmpeg::Error::Eof) => return None,
                Err(e) => {
                    error!("audio decode error: {:?}", e);
                    return None;
                }
            }

            if self.sent_eof {
                continue;
            }

            if let Some((stream, packet)) = self.ictx.packets().next() {
                if stream.index() == self.stream_index
                    && let Err(e) = self.decoder.send_packet(&packet)
                {
                    error!("send packet err: {:?}", e);
                    return None;
                }
            } else {
                if let Err(e) = self.decoder.send_eof() {
                    error!("send EOF err: {:?}", e);
                    return None;
                }
                self.sent_eof = true;
            }
        }
    }
}
