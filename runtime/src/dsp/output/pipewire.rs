//! PipeWire audio output backend.
//!
//! Uses a bounded crossbeam channel to decouple the tokio DSP pipeline from
//! the PipeWire realtime callback thread. write() is non-blocking (try_send);
//! frames are dropped if the RT thread falls behind.
//!
//! All PipeWire objects (MainLoopRc, ContextRc, CoreRc, StreamBox) are created
//! on a dedicated background thread because they are Rc-based and !Send.

use crossbeam_channel::{bounded, Receiver as CbReceiver, Sender as CbSender, TrySendError};
use pipewire as pw;
use pw::properties::properties;
use pw::spa;
use spa::pod::Pod;
use tracing::{debug, info, warn};

use super::{AudioOutput, OutputError};
use crate::dsp::config::DspConfig;

/// Maximum number of audio frames buffered between the DSP pipeline and the RT callback.
const CHANNEL_CAPACITY: usize = 4;

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

        // Signal successful init or first error back to the caller.
        let (init_tx, init_rx) = std::sync::mpsc::channel::<Result<(), String>>();

        // All PipeWire objects are Rc-based (!Send). Create them entirely inside the thread.
        let thread = std::thread::spawn(move || {
            run_pipewire_thread(sample_rate, role, audio_rx, quit_rx, init_tx);
        });

        match init_rx.recv() {
            Ok(Ok(())) => {}
            Ok(Err(e)) => {
                let _ = thread.join();
                return Err(OutputError::DeviceNotFound(e));
            }
            Err(_) => {
                let _ = thread.join();
                return Err(OutputError::DeviceNotFound(
                    "PipeWire thread died during init".into(),
                ));
            }
        }

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
            }
            Err(TrySendError::Full(_)) => {
                warn!("PipeWire channel full — dropping frame (RT thread behind)");
                // Non-blocking: return Ok to avoid stalling the DSP pipeline.
            }
            Err(TrySendError::Disconnected(_)) => {
                return Err(OutputError::WriteError(
                    "PipeWire channel disconnected".into(),
                ));
            }
        }
        Ok(())
    }

    fn close(mut self: Box<Self>) {
        // Signal the PipeWire thread to quit the main loop.
        let _ = self.quit_tx.send(());
        if let Some(t) = self.thread.take() {
            let _ = t.join();
        }
        info!("PipeWire output closed");
    }
}

/// Runs entirely on the PipeWire background thread.
/// Creates all PipeWire objects, connects the stream, then runs the main loop.
fn run_pipewire_thread(
    sample_rate: u32,
    role: String,
    audio_rx: CbReceiver<Vec<f32>>,
    quit_rx: pw::channel::Receiver<()>,
    init_tx: std::sync::mpsc::Sender<Result<(), String>>,
) {
    pw::init();

    macro_rules! try_init {
        ($expr:expr, $msg:expr) => {
            match $expr {
                Ok(v) => v,
                Err(e) => {
                    let _ = init_tx.send(Err(format!("{}: {e}", $msg)));
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

    // Attach quit receiver: when the sender signals, quit the main loop.
    let mainloop_quit = mainloop.clone();
    let _quit_recv = quit_rx.attach(mainloop.loop_(), move |_| {
        mainloop_quit.quit();
    });

    // Register the RT process callback. User data is the audio receiver.
    let _listener = stream
        .add_local_listener_with_user_data(audio_rx)
        .process(|stream_ref, rx| {
            let Some(mut buf) = stream_ref.dequeue_buffer() else {
                return;
            };
            let datas = buf.datas_mut();
            let Some(data) = datas.first_mut() else {
                return;
            };
            let Some(bytes): Option<&mut [u8]> = data.data() else {
                return;
            };

            // Safe cast: PipeWire allocated this buffer as F32LE; bytemuck checks alignment.
            let floats: &mut [f32] = match bytemuck::try_cast_slice_mut(bytes) {
                Ok(s) => s,
                Err(_) => return,
            };

            match rx.try_recv() {
                Ok(frame) => {
                    let copy_len = floats.len().min(frame.len());
                    floats[..copy_len].copy_from_slice(&frame[..copy_len]);
                    if copy_len < floats.len() {
                        floats[copy_len..].fill(0.0);
                    }
                    *data.chunk_mut().size_mut() = (copy_len * 4) as u32;
                }
                Err(_) => {
                    floats.fill(0.0); // underrun → silence
                    *data.chunk_mut().size_mut() = (floats.len() * 4) as u32;
                }
            }
        })
        .register();

    let _listener = match _listener {
        Ok(l) => l,
        Err(e) => {
            let _ = init_tx.send(Err(format!("listener: {e}")));
            return;
        }
    };

    // Build the SPA format pod: F32LE, output_sample_rate, 2 channels.
    let mut audio_info = spa::param::audio::AudioInfoRaw::new();
    audio_info.set_format(spa::param::audio::AudioFormat::F32LE);
    audio_info.set_rate(sample_rate);
    audio_info.set_channels(2);

    let obj = pw::spa::pod::Object {
        type_: pw::spa::utils::SpaTypes::ObjectParamFormat.as_raw(),
        id: pw::spa::param::ParamType::EnumFormat.as_raw(),
        properties: audio_info.into(),
    };
    let values = pw::spa::pod::serialize::PodSerializer::serialize(
        std::io::Cursor::new(Vec::new()),
        &pw::spa::pod::Value::Object(obj),
    )
    .unwrap()
    .0
    .into_inner();

    let mut params = [Pod::from_bytes(&values).unwrap()];

    match stream.connect(
        spa::utils::Direction::Output,
        None,
        pw::stream::StreamFlags::AUTOCONNECT
            | pw::stream::StreamFlags::MAP_BUFFERS
            | pw::stream::StreamFlags::RT_PROCESS,
        &mut params,
    ) {
        Ok(()) => {}
        Err(e) => {
            let _ = init_tx.send(Err(format!("stream connect: {e}")));
            return;
        }
    }

    // Signal successful init before blocking in run().
    let _ = init_tx.send(Ok(()));

    mainloop.run();
    // PipeWire cleanup: mainloop, context, core, stream, listeners all drop here in reverse order.
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::dsp::config::DspConfig;

    #[test]
    fn pipewire_write_or_skip() {
        // Skip when PipeWire is not available in the test environment.
        // Set STUI_TEST_PIPEWIRE=1 in CI environments that have a running daemon.
        if std::env::var("STUI_TEST_PIPEWIRE").is_err() {
            eprintln!("skipping PipeWire test (set STUI_TEST_PIPEWIRE=1 to run)");
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
}
