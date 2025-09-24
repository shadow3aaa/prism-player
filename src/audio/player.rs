use crate::media::clock::GlobalClock;
use std::sync::{
    Arc,
    atomic::{AtomicBool, Ordering},
};
use std::thread;
use std::time::Duration;

use cpal::traits::{DeviceTrait, HostTrait, StreamTrait};
use flume;
use tracing::error;

use crate::audio::decoder::AudioDecoder;
use ffmpeg_next as ffmpeg;
use ringbuf::RingBuffer;

pub struct AudioHandle {
    pub play_buf_thread: Option<thread::JoinHandle<()>>,
    pub decode_thread: Option<thread::JoinHandle<()>>,
    #[allow(unused)]
    stream: cpal::Stream, // we need to keep the stream alive
    shutdown: Arc<AtomicBool>,
}

impl Drop for AudioHandle {
    fn drop(&mut self) {
        // signal threads to stop
        self.shutdown.store(true, Ordering::Relaxed);

        // join threads; ignore panics because panicking in Drop must be avoided
        if let Some(handle) = self.decode_thread.take() {
            let _ = handle.join().ok();
        }

        if let Some(handle) = self.play_buf_thread.take() {
            let _ = handle.join().ok();
        }
    }
}

pub fn spawn_audio(path: String, clock: GlobalClock) -> AudioHandle {
    // use pre-resampled frames to match device sample rate and simplify playback
    let (sx, rx) = flume::bounded::<(Vec<f32>, f64, u32, u16)>(100);

    // shutdown flag used to signal threads to stop
    let shutdown = Arc::new(AtomicBool::new(false));

    // query device config once to determine target sample rate and channels for resampling
    let host = cpal::default_host();
    let device = host
        .default_output_device()
        .expect("no output device available");
    let config = device
        .default_output_config()
        .expect("Failed to get default output config");

    let (target_sample_rate, target_channels) = match config.sample_format() {
        cpal::SampleFormat::F32 => {
            let stream_config: cpal::StreamConfig = config.clone().into();
            (
                stream_config.sample_rate.0 as u32,
                stream_config.channels as u16,
            )
        }
        other => panic!(
            "Unsupported sample format for target detection: {:?}",
            other
        ),
    };

    let decode_path = path.clone();
    let sx2 = sx.clone();
    let decode_shutdown = shutdown.clone(); // clone for decode thread
    let decode_thread = thread::spawn(move || {
        let decoder = AudioDecoder::new(&decode_path).expect("Failed to open audio decoder");
        let time_base = decoder.time_base();
        let mut resampler: Option<ffmpeg::software::resampling::Context> = None;

        for frame in decoder {
            // check shutdown flag each iteration to allow timely exit of decode thread
            if decode_shutdown.load(Ordering::Relaxed) {
                break;
            }

            let pts_seconds = frame.pts().unwrap() as f64 * time_base.numerator() as f64
                / time_base.denominator() as f64;

            if resampler.is_none() {
                let in_format = frame.format();
                let in_layout = frame.channel_layout();
                let in_rate = frame.rate();

                let out_format = ffmpeg::format::Sample::F32(ffmpeg::format::sample::Type::Packed);
                let out_layout = match target_channels {
                    1 => ffmpeg::channel_layout::ChannelLayout::MONO,
                    2 => ffmpeg::channel_layout::ChannelLayout::STEREO,
                    _ => ffmpeg::channel_layout::ChannelLayout::STEREO_DOWNMIX,
                };
                let out_rate = target_sample_rate;

                resampler = Some(ffmpeg::software::resampling::Context::get(
                    in_format, in_layout, in_rate, out_format, out_layout, out_rate,
                )
                .expect("Failed to create resampler"));
            }

            if let Some(ref mut r) = resampler {
                let mut resampled = ffmpeg::util::frame::Audio::empty();
                match r.run(&frame, &mut resampled) {
                    Ok(_) => {
                        let data = resampled.data(0);
                        let mut samples: Vec<f32> = Vec::new();
                        if !data.is_empty() {
                            samples.reserve(data.len() / 4);
                            for chunk in data.chunks_exact(4) {
                                samples.push(f32::from_ne_bytes([
                                    chunk[0], chunk[1], chunk[2], chunk[3],
                                ]));
                            }
                        }

                        let sample_rate = resampled.rate();
                        let channels = resampled.channels();

                        if sx2
                            .send((samples, pts_seconds, sample_rate, channels))
                            .is_err()
                        {
                            // stop because playback thread likely terminated (channel closed)
                            break;
                        }
                    }
                    Err(e) => {
                        error!("Resampling error: {:?}", e);
                        break;
                    }
                }
            }
        }
    });

    // PLAYBACK & CPAL STREAM
    let rb_capacity = (target_sample_rate as usize) * (target_channels as usize) * 2;
    let rb = RingBuffer::<f32>::new(rb_capacity.max(1024));
    let (producer, mut consumer) = rb.split();

    match config.sample_format() {
        cpal::SampleFormat::F32 => {
            let stream_config: cpal::StreamConfig = config.into();
            let err_fn = |err| error!("an error occurred on stream: {}", err);
            let stream = device
                .build_output_stream(
                    &stream_config,
                    move |data: &mut [f32], _| {
                        for sample in data.iter_mut() {
                            if let Some(s) = consumer.pop() {
                                *sample = s;
                            } else {
                                *sample = 0.0;
                            }
                        }
                    },
                    err_fn,
                    None::<Duration>,
                )
                .expect("Failed to build output stream");
            stream.play().expect("Failed to play stream");

            let play_buf_thread = {
                let rx = rx.clone();
                thread::spawn(move || {
                    let target_latency = 0.1_f64;
                    let mut prod = producer;

                    // loop exits when sender is dropped by decode thread, making rx.recv return Err
                    loop {
                        match rx.recv() {
                            Ok((samples, pts, sample_rate, channels)) => {
                                if sample_rate != target_sample_rate || channels != target_channels
                                {
                                    error!(
                                        "Warning: frame sample_rate/channels mismatch: {} {} vs target {} {}",
                                        sample_rate, channels, target_sample_rate, target_channels
                                    );
                                }

                                while pts > clock.now() + target_latency {
                                    thread::sleep(Duration::from_millis(4));
                                }

                                if pts + target_latency < clock.now() {
                                    continue;
                                }

                                for s in samples {
                                    let _ = prod.push(s);
                                }
                            }
                            Err(_) => {
                                // sender dropped -> exit loop
                                break;
                            }
                        }
                    }
                })
            };

            AudioHandle {
                play_buf_thread: Some(play_buf_thread),
                decode_thread: Some(decode_thread),
                stream,
                // transfer ownership of the shutdown flag to the handle so it can signal threads
                shutdown,
            }
        }
        other => panic!("Unsupported sample format: {:?}", other),
    }
}
