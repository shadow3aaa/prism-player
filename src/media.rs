pub mod clock;
mod decoder;
pub mod pipeline;

use std::{
    mem,
    sync::{Arc, mpsc},
    thread,
    time::Duration,
};

use crate::audio::player as audio_player;
use ffmpeg_next::{self as ffmpeg};
use parking_lot::RwLock;
use tessera_ui::{ComputedData, Constraint, DimensionValue, tessera};
use uuid::Uuid;

pub struct VideoPlayerArgs {
    pub width: DimensionValue,
    pub height: DimensionValue,
}

enum DecodeThreadCommand {
    Exit,
}

pub struct VideoPlayerState {
    id: Uuid,
    width: u32,
    height: u32,
    decode_thread: Option<thread::JoinHandle<()>>,
    sx_commander: mpsc::Sender<DecodeThreadCommand>,
    rx_data: flume::Receiver<(Vec<u8>, f64)>,
    playing: bool,
    clock: clock::GlobalClock,
    #[allow(unused)]
    audio_handle: audio_player::AudioHandle,
}

impl Drop for VideoPlayerState {
    fn drop(&mut self) {
        // clear buffered frames so the decoder thread can exit without blocking
        self.rx_data.drain();
        let _ = self.sx_commander.send(DecodeThreadCommand::Exit);
        self.decode_thread.take().unwrap().join().unwrap();
    }
}

impl VideoPlayerState {
    pub fn new(path: &str) -> Self {
        let (sx_commander, rx_commander) = mpsc::channel();
        // buffer up to 30 frames to smooth producer/consumer bursts
        let (sx_data, rx_data) = flume::bounded(30);
        let path = path.to_string();
        let decoder = decoder::VideoDecoder::new(&path);
        let width = decoder.width();
        let height = decoder.height();

        let decode_thread = thread::spawn(move || {
            let timebase_f64: f64 = decoder.time_base().into();
            let mut scaler = ffmpeg::software::scaling::Context::get(
                decoder.format(),
                decoder.width(),
                decoder.height(),
                ffmpeg::format::Pixel::RGBA,
                decoder.width(),
                decoder.height(),
                ffmpeg::software::scaling::Flags::BILINEAR,
            )
            .expect("Failed to create video scaler");
            let mut scaled_frame = ffmpeg::util::frame::Video::empty();
            for frame in decoder {
                // poll exit command to allow responsive shutdown
                while let Ok(cmd) = rx_commander.try_recv() {
                    match cmd {
                        DecodeThreadCommand::Exit => return,
                    }
                }
                scaler
                    .run(&frame, &mut scaled_frame)
                    .expect("Failed to scale frame");
                let mut data = scaled_frame.data(0).to_vec();

                // convert frame pts to seconds for timing and scheduling
                let mut pts_seconds = frame.pts().map(|p| p as f64 * timebase_f64).unwrap();
                while let Err(e) = sx_data.send_timeout(
                    (mem::take(&mut data), pts_seconds),
                    Duration::from_millis(100),
                ) {
                    match e {
                        flume::SendTimeoutError::Timeout((unsent_data, unsent_pts)) => {
                            // check for exit to allow prompt shutdown; preserve pts and data for resend
                            if let Ok(cmd) = rx_commander.try_recv() {
                                match cmd {
                                    DecodeThreadCommand::Exit => return,
                                }
                            }
                            data = unsent_data;
                            pts_seconds = unsent_pts;
                        }
                        flume::SendTimeoutError::Disconnected(_) => return,
                    }
                }
            }
        });

        // create shared clock so audio and video share the same timing reference
        let clock = clock::GlobalClock::new();

        // spawn audio playback (best-effort)
        let audio_handle = audio_player::spawn_audio(path.to_string(), clock.clone());

        Self {
            width,
            height,
            id: Uuid::new_v4(),
            decode_thread: Some(decode_thread),
            sx_commander,
            rx_data,
            playing: true,
            clock,
            audio_handle,
        }
    }

    pub fn pause(&mut self) {
        // pause shared clock to stop playback timing
        self.clock.pause();
        self.playing = false;
    }

    pub fn resume(&mut self) {
        // resume shared clock to continue playback timing
        self.clock.resume();
        self.playing = true;
    }

    pub fn toggle(&mut self) {
        if self.playing {
            self.pause();
        } else {
            self.resume();
        }
    }

    pub fn is_playing(&self) -> bool {
        self.playing
    }
}

#[tessera]
pub fn video_player(args: VideoPlayerArgs, state: Arc<RwLock<VideoPlayerState>>) {
    measure(Box::new(move |input| {
        input
            .metadata_mut()
            .push_draw_command(pipeline::VideoCommand {
                id: state.read().id,
                width: state.read().width,
                height: state.read().height,
                receiver: state.read().rx_data.clone(),
                clock: state.read().clock.clone(),
            });
        let size = Constraint::new(args.width, args.height).merge(input.parent_constraint);
        Ok(ComputedData {
            width: size.width.get_max().unwrap(),
            height: size.height.get_max().unwrap(),
        })
    }))
}
