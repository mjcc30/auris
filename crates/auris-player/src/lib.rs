//! Moteur de lecture Auris: decodage PCM (symphonia) + DSD (DSF/DFF -> DoP),
//! sortie cross-plateforme via cpal (ALSA / CoreAudio / WASAPI).

pub mod decoder;
pub mod dsd;
pub mod engine;
pub mod eq;
pub mod ffmpeg;
pub mod output;
pub mod wasapi_dop;
pub mod wasapi_exclusive;

pub use decoder::{open_decoder, probe_meta, Chunk, ChunkData, Decoder, OutputSpec, TrackMeta};
pub use engine::{Command, PlayerEngine, WorkerMsg};
pub use eq::{Biquad, EqBandParams, EqChain, EqConfig, EqProcessor};
pub use ffmpeg::SharedReader;
pub use output::{resolve_device_name, AudioOutput};
pub use wasapi_dop::{
    all_mix_formats, device_mix_format, dop_default_format_bytes, ensure_dop_carrier, MixFormat,
};
pub use wasapi_exclusive::{
    probe_any as wasapi_exclusive_supported, DeviceFormat, ExclusiveStream,
};
