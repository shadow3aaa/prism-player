use ffmpeg_next::{self as ffmpeg, util::frame::Video as FrameVideo};

pub struct VideoDecoder {
    ictx: ffmpeg::format::context::Input,
    stream_index: usize,
    decoder: ffmpeg::codec::decoder::Video,
    sent_eof: bool,
}

impl VideoDecoder {
    pub fn new(path: &str) -> Result<Self, ffmpeg::Error> {
        // 初始化 ffmpeg，这步很重要，最好在应用启动时调用一次
        ffmpeg::init().unwrap();

        let ictx = ffmpeg::format::input(path)?;
        let stream = ictx
            .streams()
            .best(ffmpeg::media::Type::Video)
            .ok_or(ffmpeg::Error::StreamNotFound)?;
        let stream_index = stream.index();
        let context_decoder =
            ffmpeg::codec::context::Context::from_parameters(stream.parameters())?;
        let decoder = context_decoder.decoder().video()?;

        Ok(VideoDecoder {
            ictx,
            stream_index,
            decoder,
            sent_eof: false,
        })
    }

    pub fn width(&self) -> u32 {
        self.decoder.width()
    }

    pub fn height(&self) -> u32 {
        self.decoder.height()
    }

    pub fn frame_rate(&self) -> Option<ffmpeg::Rational> {
        self.decoder.frame_rate()
    }

    pub fn format(&self) -> ffmpeg::format::Pixel {
        self.decoder.format()
    }
}

impl Iterator for VideoDecoder {
    type Item = FrameVideo;

    fn next(&mut self) -> Option<Self::Item> {
        let mut decoded_frame = FrameVideo::empty();

        loop {
            // 1. 尝试从解码器接收一帧
            match self.decoder.receive_frame(&mut decoded_frame) {
                Ok(()) => {
                    return Some(decoded_frame);
                }
                Err(ffmpeg::Error::Other { errno }) if errno == ffmpeg::sys::EAGAIN => {
                    // 解码器需要更多数据包，继续下面的逻辑
                }
                Err(ffmpeg::Error::Eof) => {
                    // 流已结束，返回 None
                    return None;
                }
                Err(e) => {
                    // 发生其他错误
                    eprintln!("解码错误: {:?}", e);
                    return None;
                }
            }

            // 2. 如果已发送 EOF，说明正在冲刷解码器，不再发送新包
            if self.sent_eof {
                continue;
            }

            // 3. 读取下一个数据包
            if let Some((stream, packet)) = self.ictx.packets().next() {
                if stream.index() == self.stream_index
                    && let Err(e) = self.decoder.send_packet(&packet)
                {
                    eprintln!("发送包时出错: {:?}", e);
                    return None;
                }
            } else {
                // 4. 数据包读取完毕，发送一个空包以冲刷解码器
                if let Err(e) = self.decoder.send_eof() {
                    eprintln!("发送 EOF 时出错: {:?}", e);
                    return None;
                }
                self.sent_eof = true;
            }
        }
    }
}
