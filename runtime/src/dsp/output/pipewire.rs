//! PipeWire audio output backend.
//!
//! # PCM output (`PipeWireOutput`)
//! Streams interleaved stereo F32LE audio.  The tokio DSP pipeline sends
//! `Vec<f32>` frames over a bounded crossbeam channel; a dedicated thread
//! runs the PipeWire main loop and feeds them to the RT process callback.
//!
//! # DoP output (`PipeWireDsdOutput`)
//! Streams DSD-over-PCM (DoP v1.1) as S32LE.  Raw 1-bit DSD samples (±1.0
//! f32, interleaved L/R) are encoded into 32-bit DoP words by `DopEncoder`
//! before being enqueued.  Each word carries eight DSD bits per channel plus
//! an alternating marker byte (0x05 / 0xFA) in the MSB.
//!
//! PCM carrier rate = DSD rate ÷ 16 (sixteen DSD bits packed per S32LE frame
//! per channel):
//! * DSD64  → 176 400 Hz PCM
//! * DSD128 → 352 800 Hz PCM
//! * DSD256 → 705 600 Hz PCM
//! * DSD512 → 1 411 200 Hz PCM
//!
//! # Threading model
//! All PipeWire objects are `Rc`-based (`!Send`).  They are created and owned
//! entirely on a dedicated background thread.  `write()` / `write_dsd()` are
//! non-blocking (`try_send`); frames are silently dropped on overflow.

use crossbeam_channel::{bounded, Receiver as CbReceiver, Sender as CbSender, TrySendError};
use pipewire as pw;
use pw::properties::properties;
use pw::spa;
use spa::pod::Pod;
use tracing::{debug, info, warn};

use super::{AudioOutput, DsdAudioOutput, OutputError};
use crate::dsp::config::DspConfig;
use crate::dsp::output::dsd::DopEncoder;

/// Frames buffered between the DSP pipeline and the RT callback.
const CHANNEL_CAPACITY: usize = 4;

// ── PCM output ────────────────────────────────────────────────────────────────

pub struct PipeWireOutput {
    sender: CbSender<Vec<f32>>,
    sample_rate: u32,
    quit_tx: pw::channel::Sender<()>,
    thread: Option<std::thread::JoinHandle<()>>,
}

impl PipeWireOutput {
    pub fn new(config: &DspConfig) -> Result<Self, OutputError> {
        let sample_rate = config.output_sample_rate;
        let role = config.pipewire_role.clone();

        let (audio_tx, audio_rx) = bounded::<Vec<f32>>(CHANNEL_CAPACITY);
        let (quit_tx, quit_rx) = pw::channel::channel::<()>();
        let (init_tx, init_rx) = std::sync::mpsc::channel::<Result<(), String>>();

        let thread = std::thread::spawn(move || {
            run_pipewire_thread(sample_rate, role, audio_rx, quit_rx, init_tx);
        });

        let thread = recv_init(init_rx, thread)?;
        info!(rate = sample_rate, "PipeWire output ready");
        Ok(Self {
            sender: audio_tx,
            sample_rate,
            quit_tx,
            thread: Some(thread),
        })
    }
}

impl AudioOutput for PipeWireOutput {
    fn sample_rate(&self) -> u32 {
        self.sample_rate
    }
    fn channels(&self) -> u16 {
        2
    }

    fn write(&mut self, samples: &[f32]) -> Result<(), OutputError> {
        match self.sender.try_send(samples.to_vec()) {
            Ok(()) => {
                debug!(frames = samples.len() / 2, "PipeWire enqueued");
                Ok(())
            }
            Err(TrySendError::Full(_)) => {
                warn!("PipeWire channel full — dropping frame (RT thread behind)");
                Ok(())
            }
            Err(TrySendError::Disconnected(_)) => Err(OutputError::WriteError(
                "PipeWire channel disconnected".into(),
            )),
        }
    }

    fn close(mut self: Box<Self>) {
        let _ = self.quit_tx.send(());
        if let Some(t) = self.thread.take() {
            let _ = t.join();
        }
        info!("PipeWire output closed");
    }
}

fn run_pipewire_thread(
    sample_rate: u32,
    role: String,
    audio_rx: CbReceiver<Vec<f32>>,
    quit_rx: pw::channel::Receiver<()>,
    init_tx: std::sync::mpsc::Sender<Result<(), String>>,
) {
    pw::init();

    macro_rules! try_init {
        ($expr:expr, $label:expr) => {
            match $expr {
                Ok(v) => v,
                Err(e) => {
                    let _ = init_tx.send(Err(format!("{}: {e}", $label)));
                    return;
                }
            }
        };
        ($expr:expr, $label:expr, opt) => {
            match $expr {
                Some(v) => v,
                None => {
                    let _ = init_tx.send(Err($label.to_string()));
                    return;
                }
            }
        };
    }

    let mainloop = try_init!(pw::main_loop::MainLoopRc::new(None), "MainLoop");
    let context = try_init!(pw::context::ContextRc::new(&mainloop, None), "Context");
    let core = try_init!(context.connect_rc(None), "PipeWire connect");

    let props = properties! {
        *pw::keys::MEDIA_TYPE     => "Audio",
        *pw::keys::MEDIA_CATEGORY => "Playback",
        *pw::keys::MEDIA_ROLE     => role.as_str(),
    };
    let stream = try_init!(
        pw::stream::StreamBox::new(&core, "stui-dsp", props),
        "Stream"
    );

    let mainloop_quit = mainloop.clone();
    let _quit_recv = quit_rx.attach(mainloop.loop_(), move |_| {
        mainloop_quit.quit();
    });

    // Register the RT process callback before connecting the stream.
    let registration = stream
        .add_local_listener_with_user_data(audio_rx)
        .process(|stream_ref, rx| {
            let Some(mut buf) = stream_ref.dequeue_buffer() else {
                return;
            };
            let Some(data) = buf.datas_mut().first_mut() else {
                return;
            };
            let Some(bytes) = data.data() else {
                return;
            };

            // PipeWire allocated this buffer as F32LE; bytemuck validates alignment.
            let floats: &mut [f32] = match bytemuck::try_cast_slice_mut(bytes) {
                Ok(s) => s,
                Err(_) => return,
            };

            match rx.try_recv() {
                Ok(frame) => {
                    let n = floats.len().min(frame.len());
                    floats[..n].copy_from_slice(&frame[..n]);
                    floats[n..].fill(0.0);
                    *data.chunk_mut().size_mut() = (n * 4) as u32;
                }
                Err(_) => {
                    // Underrun: output silence.
                    floats.fill(0.0);
                    *data.chunk_mut().size_mut() = (floats.len() * 4) as u32;
                }
            }
        })
        .register();

    let _listener = try_init!(registration, "listener");

    let pod_bytes = try_init!(
        build_format_pod(spa::param::audio::AudioFormat::F32LE, sample_rate),
        "format pod"
    );
    let pod = try_init!(Pod::from_bytes(&pod_bytes), "pod from_bytes", opt);
    let mut params = [pod];

    try_init!(
        stream.connect(
            spa::utils::Direction::Output,
            None,
            pw::stream::StreamFlags::AUTOCONNECT
                | pw::stream::StreamFlags::MAP_BUFFERS
                | pw::stream::StreamFlags::RT_PROCESS,
            &mut params,
        ),
        "stream connect"
    );

    let _ = init_tx.send(Ok(()));
    mainloop.run();
    // mainloop, context, core, stream, and _listener all drop here in LIFO order.
}

// ── DoP output ────────────────────────────────────────────────────────────────

pub struct PipeWireDsdOutput {
    sender: CbSender<Vec<u32>>, // DoP words encoded as S32LE
    encoder: DopEncoder,
    dsd_rate: u32,
    quit_tx: pw::channel::Sender<()>,
    thread: Option<std::thread::JoinHandle<()>>,
}

impl PipeWireDsdOutput {
    /// Open a DoP output stream.
    ///
    /// `dsd_rate` must be one of: 2 822 400 (DSD64), 5 644 800 (DSD128),
    /// 11 289 600 (DSD256), 22 579 200 (DSD512).
    pub fn new(dsd_rate: u32, role: String) -> Result<Self, OutputError> {
        match dsd_rate {
            2_822_400 | 5_644_800 | 11_289_600 | 22_579_200 => {}
            _ => {
                return Err(OutputError::ConfigError(format!(
                "invalid DSD rate {dsd_rate} — expected 2822400, 5644800, 11289600, or 22579200"
            )))
            }
        }

        // DoP carries 16 DSD bits (8 per channel) per S32LE PCM frame.
        let pcm_rate = dsd_rate / 16;

        let (audio_tx, audio_rx) = bounded::<Vec<u32>>(CHANNEL_CAPACITY);
        let (quit_tx, quit_rx) = pw::channel::channel::<()>();
        let (init_tx, init_rx) = std::sync::mpsc::channel::<Result<(), String>>();

        let thread = std::thread::spawn(move || {
            run_pipewire_dsd_thread(pcm_rate, audio_rx, quit_rx, init_tx, role);
        });

        let thread = recv_init(init_rx, thread)?;
        info!(dsd_rate, pcm_rate, "PipeWire DoP output ready");
        Ok(Self {
            sender: audio_tx,
            encoder: DopEncoder::new(),
            dsd_rate,
            quit_tx,
            thread: Some(thread),
        })
    }
}

impl DsdAudioOutput for PipeWireDsdOutput {
    fn dsd_rate(&self) -> u32 {
        self.dsd_rate
    }

    /// Encode `samples` as DoP and enqueue for the RT thread.
    ///
    /// `samples` must be interleaved left/right 1-bit DSD as `f32` (≥ 0.0 =
    /// DSD-1, < 0.0 = DSD-0).  Its length must be a multiple of 16.
    fn write_dsd(&mut self, samples: &[f32]) -> Result<(), OutputError> {
        let words = self.encoder.encode(samples)?;
        match self.sender.try_send(words) {
            Ok(()) => Ok(()),
            Err(TrySendError::Full(_)) => {
                warn!("PipeWire DoP channel full — dropping frame");
                Ok(())
            }
            Err(TrySendError::Disconnected(_)) => Err(OutputError::WriteError(
                "PipeWire DoP channel disconnected".into(),
            )),
        }
    }

    fn close(mut self: Box<Self>) {
        let _ = self.quit_tx.send(());
        if let Some(t) = self.thread.take() {
            let _ = t.join();
        }
        info!("PipeWire DoP output closed");
    }
}

fn run_pipewire_dsd_thread(
    pcm_rate: u32,
    audio_rx: CbReceiver<Vec<u32>>,
    quit_rx: pw::channel::Receiver<()>,
    init_tx: std::sync::mpsc::Sender<Result<(), String>>,
    role: String,
) {
    pw::init();

    macro_rules! try_init {
        ($expr:expr, $label:expr) => {
            match $expr {
                Ok(v) => v,
                Err(e) => {
                    let _ = init_tx.send(Err(format!("{}: {e}", $label)));
                    return;
                }
            }
        };
        ($expr:expr, $label:expr, opt) => {
            match $expr {
                Some(v) => v,
                None => {
                    let _ = init_tx.send(Err($label.to_string()));
                    return;
                }
            }
        };
    }

    let mainloop = try_init!(pw::main_loop::MainLoopRc::new(None), "MainLoop");
    let context = try_init!(pw::context::ContextRc::new(&mainloop, None), "Context");
    let core = try_init!(context.connect_rc(None), "PipeWire connect");

    let props = properties! {
        *pw::keys::MEDIA_TYPE     => "Audio",
        *pw::keys::MEDIA_CATEGORY => "Playback",
        *pw::keys::MEDIA_ROLE     => role.as_str(),
    };
    let stream = try_init!(
        pw::stream::StreamBox::new(&core, "stui-dsd-dop", props),
        "Stream"
    );

    let mainloop_quit = mainloop.clone();
    let _quit_recv = quit_rx.attach(mainloop.loop_(), move |_| {
        mainloop_quit.quit();
    });

    let registration = stream
        .add_local_listener_with_user_data(audio_rx)
        .process(|stream_ref, rx| {
            let Some(mut buf) = stream_ref.dequeue_buffer() else {
                return;
            };
            let Some(data) = buf.datas_mut().first_mut() else {
                return;
            };
            let Some(bytes) = data.data() else {
                return;
            };

            // Cast the S32LE buffer as u32 words (bytemuck validates 4-byte alignment).
            // Each u32 is one DoP frame: [marker | L_bits | R_bits | 0x00] big-endian.
            let ints: &mut [u32] = match bytemuck::try_cast_slice_mut(bytes) {
                Ok(s) => s,
                Err(_) => return,
            };

            match rx.try_recv() {
                Ok(words) => {
                    let n = ints.len().min(words.len());
                    ints[..n].copy_from_slice(&words[..n]);
                    // Pad remaining slots with zeros. A DoP-aware DAC treats
                    // frames with no valid marker as PCM silence, which is
                    // audibly transparent and avoids state corruption.
                    ints[n..].fill(0);
                    *data.chunk_mut().size_mut() = (n * 4) as u32;
                }
                Err(_) => {
                    // Underrun: fill with zeros (PCM silence on DoP DACs).
                    ints.fill(0);
                    *data.chunk_mut().size_mut() = (ints.len() * 4) as u32;
                }
            }
        })
        .register();

    let _listener = try_init!(registration, "listener");

    // S32LE is required for DoP: bit patterns (including marker bytes) must be
    // transmitted verbatim.  F32LE would corrupt them via float interpretation.
    let pod_bytes = try_init!(
        build_format_pod(spa::param::audio::AudioFormat::S32LE, pcm_rate),
        "format pod"
    );
    let pod = try_init!(Pod::from_bytes(&pod_bytes), "pod from_bytes", opt);
    let mut params = [pod];

    try_init!(
        stream.connect(
            spa::utils::Direction::Output,
            None,
            pw::stream::StreamFlags::AUTOCONNECT
                | pw::stream::StreamFlags::MAP_BUFFERS
                | pw::stream::StreamFlags::RT_PROCESS,
            &mut params,
        ),
        "stream connect"
    );

    let _ = init_tx.send(Ok(()));
    mainloop.run();
}

// ── Shared helpers ────────────────────────────────────────────────────────────

/// Serialise a stereo SPA audio format pod into bytes.
///
/// Returns the raw pod bytes suitable for `Pod::from_bytes` and
/// `stream.connect(&mut params)`.
fn build_format_pod(format: spa::param::audio::AudioFormat, rate: u32) -> Result<Vec<u8>, String> {
    let mut info = spa::param::audio::AudioInfoRaw::new();
    info.set_format(format);
    info.set_rate(rate);
    info.set_channels(2);

    let obj = pw::spa::pod::Object {
        type_: pw::spa::utils::SpaTypes::ObjectParamFormat.as_raw(),
        id: pw::spa::param::ParamType::EnumFormat.as_raw(),
        properties: info.into(),
    };

    pw::spa::pod::serialize::PodSerializer::serialize(
        std::io::Cursor::new(Vec::new()),
        &pw::spa::pod::Value::Object(obj),
    )
    .map(|(cursor, _)| cursor.into_inner())
    .map_err(|e| format!("pod serialize: {e}"))
}

/// Wait for the init handshake from the PipeWire thread.
///
/// Returns the thread handle on success so the caller can store it for `join`
/// on close.  Joins and drops the thread on any error.
fn recv_init(
    init_rx: std::sync::mpsc::Receiver<Result<(), String>>,
    thread: std::thread::JoinHandle<()>,
) -> Result<std::thread::JoinHandle<()>, OutputError> {
    match init_rx.recv() {
        Ok(Ok(())) => Ok(thread),
        Ok(Err(msg)) => {
            let _ = thread.join();
            Err(OutputError::DeviceNotFound(msg))
        }
        Err(_) => {
            let _ = thread.join();
            Err(OutputError::DeviceNotFound(
                "PipeWire thread died during init".into(),
            ))
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Set STUI_TEST_PIPEWIRE=1 in environments that have a live PipeWire daemon.
    fn pipewire_available() -> bool {
        std::env::var("STUI_TEST_PIPEWIRE").is_ok()
    }

    #[test]
    fn pipewire_pcm_write_or_skip() {
        if !pipewire_available() {
            eprintln!("skipping PipeWire PCM test (set STUI_TEST_PIPEWIRE=1 to run)");
            return;
        }
        let config = DspConfig {
            output_sample_rate: 48000,
            pipewire_role: "Music".to_string(),
            ..Default::default()
        };
        let mut output = PipeWireOutput::new(&config).expect("PipeWire available");
        let silence = vec![0.0f32; 2048];
        output.write(&silence).expect("write");
        Box::new(output).close();
    }

    #[test]
    fn pipewire_dop_write_or_skip() {
        if !pipewire_available() {
            eprintln!("skipping PipeWire DoP test (set STUI_TEST_PIPEWIRE=1 to run)");
            return;
        }
        // DSD64 → pcm_rate 176400 Hz
        let mut output =
            PipeWireDsdOutput::new(2_822_400, "Music".to_string()).expect("PipeWire DoP available");
        // 16 DSD-1 samples = one DoP word
        let dsd = vec![1.0f32; 16];
        output.write_dsd(&dsd).expect("write_dsd");
        Box::new(output).close();
    }

    #[test]
    fn rejects_invalid_dsd_rate() {
        let err = PipeWireDsdOutput::new(44100, "Music".to_string());
        assert!(
            matches!(err, Err(OutputError::ConfigError(_))),
            "44100 is not a valid DSD rate"
        );
    }

    #[test]
    fn dop_write_rejects_non_multiple_of_16() {
        if !pipewire_available() {
            eprintln!("skipping (set STUI_TEST_PIPEWIRE=1 to run)");
            return;
        }
        let mut output =
            PipeWireDsdOutput::new(2_822_400, "Music".to_string()).expect("PipeWire DoP available");
        let bad_samples = vec![1.0f32; 15];
        assert!(
            output.write_dsd(&bad_samples).is_err(),
            "15 samples is not a multiple of 16"
        );
        Box::new(output).close();
    }
}
