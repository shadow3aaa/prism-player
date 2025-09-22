mod color;
mod video;

use std::sync::Arc;

use mimalloc::MiMalloc;

#[global_allocator]
static GLOBAL: MiMalloc = MiMalloc;

use clap::Parser;
use parking_lot::RwLock;
use tessera_ui::{Color, DimensionValue, Dp, Renderer, shard, tessera};
use tessera_ui_basic_components::{
    RippleState,
    alignment::Alignment,
    boxed::{BoxedArgs, boxed},
    fluid_glass::{FluidGlassArgs, fluid_glass},
    surface::{SurfaceArgs, surface},
    text::text,
};
use tracing::error;

use crate::{
    color::BACKGROUND_COLOR,
    video::{VideoPlayerArgs, VideoPlayerState, pipeline::VideoPipeline, video_player},
};

/// Simple video player application
#[derive(Parser, Debug)]
#[command(version, about, long_about = None)]
struct Args {
    /// Path to the video file to play
    #[arg(short, long)]
    video_path: String,
}

fn main() {
    let args = Args::parse();
    ffmpeg_next::init().unwrap();
    let filter = tracing_subscriber::EnvFilter::try_from_default_env()
        .or_else(|_| {
            tracing_subscriber::EnvFilter::try_new("off,prism_player=info,tessera_ui=info")
        })
        .unwrap();
    tracing_subscriber::fmt()
        .pretty()
        .with_env_filter(filter)
        .with_span_events(tracing_subscriber::fmt::format::FmtSpan::CLOSE)
        .init();
    let video_player_state = VideoPlayerState::new(&args.video_path);
    let video_player_state = Arc::new(RwLock::new(video_player_state));
    Renderer::run(
        || app(video_player_state.clone()),
        |app| {
            tessera_ui_basic_components::pipelines::register_pipelines(app);
            let video_pipeline = VideoPipeline::new(app.sample_count);
            app.drawer.pipeline_registry.register(video_pipeline);
        },
    )
    .unwrap_or_else(|e| error!("App failed to run: {e}"));
}

struct AppState {
    scrim_ripple_state: Arc<RippleState>,
}

impl Default for AppState {
    fn default() -> Self {
        let scrim_ripple_state = Default::default();
        Self { scrim_ripple_state }
    }
}

#[tessera]
#[shard]
fn app(#[state] state: AppState, video_player_state: Arc<RwLock<VideoPlayerState>>) {
    background(move || {
        operation_scrim(state.clone(), video_player_state.clone(), move || {
            boxed(
                BoxedArgs {
                    alignment: Alignment::Center,
                    width: DimensionValue::FILLED,
                    height: DimensionValue::FILLED,
                },
                move |scope| {
                    let video_player_state_clone = video_player_state.clone();
                    scope.child(move || {
                        video_player(
                            VideoPlayerArgs {
                                width: DimensionValue::FILLED,
                                height: DimensionValue::FILLED,
                            },
                            video_player_state_clone,
                        );
                    });

                    if !video_player_state.read().is_playing() {
                        scope.child(|| {
                            fluid_glass(
                                FluidGlassArgs {
                                    width: Dp(200.0).into(),
                                    height: Dp(200.0).into(),
                                    refraction_height: 50.0,
                                    refraction_amount: 100.0,
                                    blur_radius: 30.0,
                                    ..Default::default()
                                },
                                None,
                                || {
                                    boxed(
                                        BoxedArgs {
                                            alignment: Alignment::Center,
                                            width: DimensionValue::FILLED,
                                            height: DimensionValue::FILLED,
                                        },
                                        |scope| {
                                            scope.child(|| {
                                                text("Play");
                                            });
                                        },
                                    );
                                },
                            );
                        });
                    }
                },
            );
        });
    });
}

#[tessera]
fn background(child: impl FnOnce()) {
    surface(
        SurfaceArgs {
            width: DimensionValue::FILLED,
            height: DimensionValue::FILLED,
            style: BACKGROUND_COLOR.into(),
            ..Default::default()
        },
        None,
        move || {
            child();
        },
    );
}

#[tessera]
fn operation_scrim(
    state: Arc<AppState>,
    video_player_state: Arc<RwLock<VideoPlayerState>>,
    child: impl FnOnce() + Send + Sync + 'static,
) {
    let scrim_ripple_state = state.scrim_ripple_state.clone();
    boxed(
        BoxedArgs {
            alignment: Alignment::Center,
            width: DimensionValue::FILLED,
            height: DimensionValue::FILLED,
        },
        |scope| {
            scope.child(move || {
                child();
            });

            scope.child(move || {
                surface(
                    SurfaceArgs {
                        width: DimensionValue::FILLED,
                        height: DimensionValue::FILLED,
                        style: Color::TRANSPARENT.into(),
                        on_click: Some(Arc::new(move || {
                            video_player_state.write().toggle();
                        })),
                        ..Default::default()
                    },
                    Some(scrim_ripple_state),
                    || {},
                );
            });
        },
    );
}
