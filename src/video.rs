use std::{
    mem,
    sync::{Arc, mpsc},
    thread,
    time::Duration,
};

use ffmpeg_next::{self as ffmpeg};
use parking_lot::RwLock;
use tessera_ui::{ComputedData, Constraint, DimensionValue, tessera};
use uuid::Uuid;

mod decoder;
pub mod pipeline;

pub struct VideoPlayerArgs {
    pub width: DimensionValue,
    pub height: DimensionValue,
}

enum DecodeThreadCommand {
    Exit,
    Pause,
    Resume,
}

pub struct VideoPlayerState {
    id: Uuid,
    width: u32,
    height: u32,
    frame_duration: Duration,
    decode_thread: Option<thread::JoinHandle<()>>,
    sx_commander: mpsc::Sender<DecodeThreadCommand>,
    rx_data: flume::Receiver<Vec<u8>>,
    playing: bool,
}

impl Drop for VideoPlayerState {
    fn drop(&mut self) {
        self.rx_data.drain(); // Clear the channel
        let _ = self.sx_commander.send(DecodeThreadCommand::Resume); // Ensure it's not paused
        let _ = self.sx_commander.send(DecodeThreadCommand::Exit);
        self.decode_thread.take().unwrap().join().unwrap();
    }
}

impl VideoPlayerState {
    pub fn new(path: &str) -> Self {
        let (sx_commander, rx_commander) = mpsc::channel();
        let (sx_data, rx_data) = flume::bounded(30); // Buffer up to 30 frames
        let path = path.to_string();
        let decoder = decoder::VideoDecoder::new(&path).expect("Failed to open video");
        let width = decoder.width();
        let height = decoder.height();
        let frame_rate = decoder.frame_rate().expect("Failed to get frame rate");
        let frame_duration = Duration::from_secs_f64(
            frame_rate.denominator() as f64 / frame_rate.numerator() as f64,
        );

        let rx_data_clone = rx_data.clone();
        let decode_thread = thread::spawn(move || {
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
                while let Ok(cmd) = rx_commander.try_recv() {
                    match cmd {
                        DecodeThreadCommand::Exit => return,
                        DecodeThreadCommand::Pause => {
                            // Take all frames from the channel to avoid delays when resuming
                            let datas = rx_data_clone.drain();

                            while let Ok(cmd) = rx_commander.recv() {
                                if let DecodeThreadCommand::Resume = cmd {
                                    // Put back all frames to the channel
                                    for data in datas {
                                        let _ = sx_data.send(data);
                                    }
                                    break;
                                }
                            }
                        }
                        DecodeThreadCommand::Resume => {}
                    }
                }
                scaler
                    .run(&frame, &mut scaled_frame)
                    .expect("Failed to scale frame");
                let mut data = scaled_frame.data(0).to_vec();
                while let Err(e) =
                    sx_data.send_timeout(mem::take(&mut data), Duration::from_millis(100))
                {
                    match e {
                        flume::SendTimeoutError::Timeout(unsent_data) => {
                            // Check if the thread should pause or exit
                            while let Ok(cmd) = rx_commander.try_recv() {
                                match cmd {
                                    DecodeThreadCommand::Exit => return,
                                    DecodeThreadCommand::Pause => {
                                        // Take all frames from the channel to avoid delays when resuming
                                        let datas = rx_data_clone.drain();

                                        while let Ok(cmd) = rx_commander.recv() {
                                            if let DecodeThreadCommand::Resume = cmd {
                                                // Put back all frames to the channel
                                                for data in datas {
                                                    let _ = sx_data.send(data);
                                                }
                                                break;
                                            }
                                        }
                                    }
                                    DecodeThreadCommand::Resume => {}
                                }
                            }
                            data = unsent_data;
                        }
                        flume::SendTimeoutError::Disconnected(_) => todo!(),
                    }
                }
            }
        });

        Self {
            width,
            height,
            frame_duration,
            id: Uuid::new_v4(),
            decode_thread: Some(decode_thread),
            sx_commander,
            rx_data,
            playing: true,
        }
    }

    pub fn pause(&mut self) {
        let _ = self.sx_commander.send(DecodeThreadCommand::Pause);
        self.playing = false;
    }

    pub fn resume(&mut self) {
        let _ = self.sx_commander.send(DecodeThreadCommand::Resume);
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
                frame_duration: state.read().frame_duration,
                receiver: state.read().rx_data.clone(),
            });
        let size = Constraint::new(args.width, args.height).merge(input.parent_constraint);
        Ok(ComputedData {
            width: size.width.get_max().unwrap(),
            height: size.height.get_max().unwrap(),
        })
    }))
}
