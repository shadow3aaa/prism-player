use ffmpeg_next::{self as ffmpeg, util::frame::Video as FrameVideo};
use tracing::error;

pub struct VideoDecoder {
    ictx: ffmpeg::format::context::Input,
    stream_index: usize,
    decoder: ffmpeg::codec::decoder::Video,
    sent_eof: bool,
    time_base: ffmpeg::Rational,
}

impl VideoDecoder {
    pub fn new(path: &str) -> Self {
        // open input file; panic on failure because caller cannot recover at this layer
        let ictx = ffmpeg::format::input(path).expect("Failed to open input file");

        // ensure a video stream exists; panic if missing because higher layers cannot recover
        let stream = ictx
            .streams()
            .best(ffmpeg::media::Type::Video)
            .expect("no video stream found in input (unsupported format)");
        let stream_index = stream.index();
        let time_base = stream.time_base();

        // build codec context; panic on invalid stream parameters since this indicates unrecoverable input
        let context_decoder = ffmpeg::codec::context::Context::from_parameters(stream.parameters())
            .expect("failed to build codec context from stream parameters");

        // obtain video decoder; panic if unsupported because higher layers cannot recover
        let decoder = context_decoder
            .decoder()
            .video()
            .expect("failed to obtain video decoder (unsupported codec)");

        VideoDecoder {
            ictx,
            stream_index,
            decoder,
            sent_eof: false,
            time_base,
        }
    }

    pub fn width(&self) -> u32 {
        self.decoder.width()
    }

    pub fn height(&self) -> u32 {
        self.decoder.height()
    }

    pub fn format(&self) -> ffmpeg::format::Pixel {
        self.decoder.format()
    }

    pub fn time_base(&self) -> ffmpeg::Rational {
        self.time_base
    }
}

impl Iterator for VideoDecoder {
    type Item = FrameVideo;

    fn next(&mut self) -> Option<Self::Item> {
        let mut decoded_frame = FrameVideo::empty();

        loop {
            // try receiving a frame from the decoder
            match self.decoder.receive_frame(&mut decoded_frame) {
                Ok(()) => {
                    return Some(decoded_frame);
                }
                Err(ffmpeg::Error::Other { errno }) if errno == ffmpeg::sys::EAGAIN => {
                    // decoder needs more packets; continue to packet reading logic
                }
                Err(ffmpeg::Error::Eof) => {
                    // stream ended; return None
                    return None;
                }
                Err(e) => {
                    // other error during decode; report and return None
                    error!("decode error: {:?}", e);
                    return None;
                }
            }

            // if EOF already sent we are flushing the decoder; do not send new packets
            if self.sent_eof {
                continue;
            }

            // read next packet from input
            if let Some((stream, packet)) = self.ictx.packets().next() {
                if stream.index() == self.stream_index
                    && let Err(e) = self.decoder.send_packet(&packet)
                {
                    error!("error sending packet: {:?}", e);
                    return None;
                }
            } else {
                // no more packets; send EOF to flush the decoder
                if let Err(e) = self.decoder.send_eof() {
                    error!("error sending EOF: {:?}", e);
                    return None;
                }
                self.sent_eof = true;
            }
        }
    }
}
