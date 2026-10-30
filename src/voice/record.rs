//! Microphone capture for voice input. Recording runs on its own thread and ends when the stop
//! flag is set or the maximum length is reached; the thread then hands back a 16 kHz mono 16-bit
//! WAV file. macOS and Windows capture with cpal. Other systems record with the first of
//! `pw-record`, `parecord` or `arecord` that is installed, because cpal needs ALSA headers that
//! the Linux release builds don't link.

use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::Duration;

/// What the transcription endpoints are sent: 16 kHz, mono, 16-bit PCM.
#[cfg_attr(not(any(target_os = "macos", target_os = "windows")), allow(dead_code))]
pub const SAMPLE_RATE: u32 = 16_000;

/// A WAV file (PCM, 16-bit, mono) holding `samples` at `rate` Hz.
#[cfg_attr(not(any(target_os = "macos", target_os = "windows")), allow(dead_code))]
pub fn encode_wav(samples: &[i16], rate: u32) -> Vec<u8> {
    let data_len = u32::try_from(samples.len().saturating_mul(2)).unwrap_or(u32::MAX);
    let mut out = Vec::with_capacity(44 + data_len as usize);
    out.extend_from_slice(b"RIFF");
    out.extend_from_slice(&(36u32.saturating_add(data_len)).to_le_bytes());
    out.extend_from_slice(b"WAVE");
    out.extend_from_slice(b"fmt ");
    out.extend_from_slice(&16u32.to_le_bytes());
    out.extend_from_slice(&1u16.to_le_bytes()); // PCM
    out.extend_from_slice(&1u16.to_le_bytes()); // mono
    out.extend_from_slice(&rate.to_le_bytes());
    out.extend_from_slice(&rate.saturating_mul(2).to_le_bytes()); // bytes per second
    out.extend_from_slice(&2u16.to_le_bytes()); // bytes per frame
    out.extend_from_slice(&16u16.to_le_bytes()); // bits per sample
    out.extend_from_slice(b"data");
    out.extend_from_slice(&data_len.to_le_bytes());
    for s in samples {
        out.extend_from_slice(&s.to_le_bytes());
    }
    out
}

/// One sample in [-1, 1] as 16-bit PCM, clipped.
#[cfg_attr(not(any(target_os = "macos", target_os = "windows")), allow(dead_code))]
pub fn to_i16(x: f32) -> i16 {
    (x.clamp(-1.0, 1.0) * 32767.0).round() as i16
}

/// Interleaved frames to one channel by averaging each frame.
#[cfg_attr(not(any(target_os = "macos", target_os = "windows")), allow(dead_code))]
pub fn mix_to_mono(frames: &[f32], channels: usize) -> Vec<f32> {
    if channels <= 1 {
        return frames.to_vec();
    }
    frames
        .chunks(channels)
        .map(|f| f.iter().sum::<f32>() / f.len() as f32)
        .collect()
}

/// Linear-interpolation resampling of one channel. Good enough for speech.
#[cfg_attr(not(any(target_os = "macos", target_os = "windows")), allow(dead_code))]
pub fn resample_linear(input: &[f32], from: u32, to: u32) -> Vec<f32> {
    if input.is_empty() || from == 0 || to == 0 || from == to {
        return input.to_vec();
    }
    let ratio = f64::from(from) / f64::from(to);
    let len = ((input.len() as f64) / ratio).floor() as usize;
    (0..len)
        .map(|i| {
            let pos = i as f64 * ratio;
            let idx = pos.floor() as usize;
            let frac = (pos - pos.floor()) as f32;
            let a = input[idx.min(input.len() - 1)];
            let b = input[(idx + 1).min(input.len() - 1)];
            a + (b - a) * frac
        })
        .collect()
}

/// Recorders tried on Linux, in order of preference.
#[cfg(not(any(target_os = "macos", target_os = "windows")))]
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum LinuxRecorder {
    PwRecord,
    Parecord,
    Arecord,
}

/// The first recorder that is installed, given a check for each program name.
#[cfg(not(any(target_os = "macos", target_os = "windows")))]
pub fn pick_recorder(has: impl Fn(&str) -> bool) -> Option<LinuxRecorder> {
    [
        ("pw-record", LinuxRecorder::PwRecord),
        ("parecord", LinuxRecorder::Parecord),
        ("arecord", LinuxRecorder::Arecord),
    ]
    .into_iter()
    .find(|(name, _)| has(name))
    .map(|(_, r)| r)
}

/// The program and arguments that record 16 kHz mono 16-bit WAV into `path`, from `device`
/// (a name from [`input_devices`]) or, when it is empty, the default input.
#[cfg(not(any(target_os = "macos", target_os = "windows")))]
pub fn recorder_args(
    recorder: LinuxRecorder,
    path: &str,
    device: &str,
) -> (&'static str, Vec<String>) {
    let s = |v: &str| v.to_owned();
    let (program, mut args) = match recorder {
        LinuxRecorder::PwRecord => (
            "pw-record",
            vec![
                s("--rate"),
                s("16000"),
                s("--channels"),
                s("1"),
                s("--format"),
                s("s16"),
            ],
        ),
        LinuxRecorder::Parecord => (
            "parecord",
            vec![
                s("--rate=16000"),
                s("--channels=1"),
                s("--format=s16le"),
                s("--file-format=wav"),
            ],
        ),
        LinuxRecorder::Arecord => (
            "arecord",
            vec![
                s("-q"),
                s("-t"),
                s("wav"),
                s("-f"),
                s("S16_LE"),
                s("-r"),
                s("16000"),
                s("-c"),
                s("1"),
            ],
        ),
    };
    if !device.is_empty() {
        match recorder {
            LinuxRecorder::PwRecord => args.extend([s("--target"), s(device)]),
            LinuxRecorder::Parecord => args.push(format!("--device={device}")),
            LinuxRecorder::Arecord => args.extend([s("-D"), s(device)]),
        }
    }
    args.push(s(path));
    (program, args)
}

/// The capture sources in `pactl list short sources` output, without the monitors of outputs.
#[cfg(not(any(target_os = "macos", target_os = "windows")))]
pub fn parse_pactl_sources(out: &str) -> Vec<String> {
    out.lines()
        .filter_map(|line| line.split('\t').nth(1))
        .map(str::trim)
        .filter(|name| !name.is_empty() && !name.ends_with(".monitor"))
        .map(str::to_owned)
        .collect()
}

/// The PCM names in `arecord -L` output: the unindented lines, without `null`.
#[cfg(not(any(target_os = "macos", target_os = "windows")))]
pub fn parse_arecord_list(out: &str) -> Vec<String> {
    out.lines()
        .filter(|line| !line.is_empty() && !line.starts_with(char::is_whitespace))
        .map(str::trim)
        .filter(|name| *name != "null")
        .map(str::to_owned)
        .collect()
}

/// The microphones that can be picked for voice input, by name. Empty when they can't be
/// listed; recording then uses the default input. Runs a short command on Linux, so call it
/// off the UI thread or rarely.
pub fn input_devices() -> Vec<String> {
    #[cfg(any(target_os = "macos", target_os = "windows"))]
    {
        use cpal::traits::{DeviceTrait, HostTrait};
        cpal::default_host()
            .input_devices()
            .map(|devices| {
                devices
                    .filter_map(|d| d.description().ok().map(|d| d.name().to_owned()))
                    .collect()
            })
            .unwrap_or_default()
    }
    #[cfg(not(any(target_os = "macos", target_os = "windows")))]
    {
        let run = |program: &str, args: &[&str]| {
            std::process::Command::new(program)
                .args(args)
                .stdin(std::process::Stdio::null())
                .stderr(std::process::Stdio::null())
                .output()
                .ok()
                .filter(|o| o.status.success())
                .map(|o| String::from_utf8_lossy(&o.stdout).into_owned())
        };
        match pick_recorder(|name| which::which(name).is_ok()) {
            Some(LinuxRecorder::PwRecord | LinuxRecorder::Parecord) => {
                run("pactl", &["list", "short", "sources"])
                    .map(|out| parse_pactl_sources(&out))
                    .unwrap_or_default()
            }
            Some(LinuxRecorder::Arecord) => run("arecord", &["-L"])
                .map(|out| parse_arecord_list(&out))
                .unwrap_or_default(),
            None => Vec::new(),
        }
    }
}

/// Starts a recording. `done` gets the WAV bytes (or why it failed) from the recording thread
/// once `stop` is set or `max_secs` has passed. Errors that are known up front (no microphone
/// tool on Linux) are returned here instead.
pub fn start(
    max_secs: u32,
    device: String,
    stop: Arc<AtomicBool>,
    done: impl FnOnce(Result<Vec<u8>, String>) + Send + 'static,
) -> Result<(), String> {
    let max = Duration::from_secs(u64::from(max_secs.max(1)));
    #[cfg(any(target_os = "macos", target_os = "windows"))]
    {
        let reporter =
            super::Reporter::<Vec<u8>, _>::new("The recording stopped unexpectedly", done);
        std::thread::Builder::new()
            .name("voice-record".into())
            .spawn(move || reporter.finish(capture_cpal(max, &device, &stop)))
            .map(|_| ())
            .map_err(|e| format!("Couldn't start recording: {e}"))
    }
    #[cfg(not(any(target_os = "macos", target_os = "windows")))]
    {
        let recorder = pick_recorder(|name| which::which(name).is_ok())
            .ok_or_else(|| "Install PipeWire, PulseAudio or ALSA utils to record".to_owned())?;
        let path =
            std::env::temp_dir().join(format!("cleanrstai-voice-{}.wav", uuid::Uuid::new_v4()));
        let (program, args) = recorder_args(recorder, &path.to_string_lossy(), &device);
        let child = std::process::Command::new(program)
            .args(&args)
            .stdin(std::process::Stdio::null())
            .stdout(std::process::Stdio::null())
            .stderr(std::process::Stdio::null())
            .spawn()
            .map_err(|e| format!("Couldn't start {program}: {e}"))?;
        let reporter =
            super::Reporter::<Vec<u8>, _>::new("The recording stopped unexpectedly", done);
        std::thread::Builder::new()
            .name("voice-record".into())
            .spawn(move || reporter.finish(finish_process(child, &path, max, &stop)))
            .map(|_| ())
            .map_err(|e| format!("Couldn't start recording: {e}"))
    }
}

/// Waits for the recorder process, stops it with SIGINT so it closes the WAV header, and reads
/// the file back.
#[cfg(not(any(target_os = "macos", target_os = "windows")))]
fn finish_process(
    mut child: std::process::Child,
    path: &std::path::Path,
    max: Duration,
    stop: &AtomicBool,
) -> Result<Vec<u8>, String> {
    let started = std::time::Instant::now();
    loop {
        if stop.load(Ordering::Relaxed) || started.elapsed() >= max {
            break;
        }
        if let Ok(Some(status)) = child.try_wait() {
            let _ = std::fs::remove_file(path);
            return Err(format!("The recorder stopped early ({status})"));
        }
        std::thread::sleep(Duration::from_millis(50));
    }
    // SAFETY: `kill` only sends a signal to the child process we spawned and still own.
    unsafe {
        libc::kill(child.id() as libc::pid_t, libc::SIGINT);
    }
    let _ = child.wait();
    let bytes = std::fs::read(path).map_err(|e| format!("Couldn't read the recording: {e}"));
    let _ = std::fs::remove_file(path);
    match bytes {
        Ok(b) if b.len() > 44 => Ok(b),
        Ok(_) => Err("Nothing was recorded".into()),
        Err(e) => Err(e),
    }
}

/// Captures the named input device (the default one when `name` is empty or no longer there)
/// until `stop` or `max`, then encodes it as 16 kHz WAV.
#[cfg(any(target_os = "macos", target_os = "windows"))]
fn capture_cpal(max: Duration, name: &str, stop: &AtomicBool) -> Result<Vec<u8>, String> {
    use cpal::traits::{DeviceTrait, HostTrait, StreamTrait};
    use std::sync::Mutex;

    let host = cpal::default_host();
    let named = (!name.is_empty())
        .then(|| host.input_devices().ok())
        .flatten()
        .and_then(|mut devices| {
            devices.find(|d| d.description().is_ok_and(|desc| desc.name() == name))
        });
    let device = named
        .or_else(|| host.default_input_device())
        .ok_or("No microphone found")?;
    let config = device
        .default_input_config()
        .map_err(|e| format!("Couldn't read the microphone: {e}"))?;
    let rate = config.sample_rate();
    let channels = usize::from(config.channels());
    let buf: Arc<Mutex<Vec<f32>>> = Arc::default();
    let err = |e: cpal::Error| log::warn!("microphone: {e}");
    let stream = match config.sample_format() {
        cpal::SampleFormat::F32 => build_input::<f32>(&device, &config, buf.clone(), channels, err),
        cpal::SampleFormat::I16 => build_input::<i16>(&device, &config, buf.clone(), channels, err),
        cpal::SampleFormat::U16 => build_input::<u16>(&device, &config, buf.clone(), channels, err),
        other => Err(cpal::Error::with_message(
            cpal::ErrorKind::UnsupportedConfig,
            format!("unsupported microphone format {other:?}"),
        )),
    }
    .map_err(|e| format!("Couldn't open the microphone: {e}"))?;
    stream
        .play()
        .map_err(|e| format!("Couldn't start the microphone: {e}"))?;
    let started = std::time::Instant::now();
    while !stop.load(Ordering::Relaxed) && started.elapsed() < max {
        std::thread::sleep(Duration::from_millis(50));
    }
    drop(stream);
    let captured = std::mem::take(&mut *buf.lock().unwrap_or_else(|e| e.into_inner()));
    if captured.is_empty() {
        return Err("Nothing was recorded".into());
    }
    let resampled = resample_linear(&captured, rate, SAMPLE_RATE);
    let pcm: Vec<i16> = resampled.iter().map(|x| to_i16(*x)).collect();
    Ok(encode_wav(&pcm, SAMPLE_RATE))
}

#[cfg(any(target_os = "macos", target_os = "windows"))]
fn build_input<T>(
    device: &cpal::Device,
    config: &cpal::SupportedStreamConfig,
    buf: Arc<std::sync::Mutex<Vec<f32>>>,
    channels: usize,
    err: impl FnMut(cpal::Error) + Send + 'static,
) -> Result<cpal::Stream, cpal::Error>
where
    T: cpal::SizedSample + cpal::Sample,
    f32: cpal::FromSample<T>,
{
    use cpal::traits::DeviceTrait;
    device.build_input_stream::<T, _, _>(
        config.config(),
        move |data: &[T], _: &cpal::InputCallbackInfo| {
            let frames: Vec<f32> = data.iter().map(|s| (*s).to_sample::<f32>()).collect();
            let mono = mix_to_mono(&frames, channels);
            buf.lock().unwrap_or_else(|e| e.into_inner()).extend(mono);
        },
        err,
        None,
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn wav_header_describes_16k_mono_pcm() {
        let wav = encode_wav(&[1, -2, 300], SAMPLE_RATE);
        assert_eq!(&wav[0..4], b"RIFF");
        assert_eq!(&wav[8..12], b"WAVE");
        assert_eq!(&wav[12..16], b"fmt ");
        let u16_at = |i: usize| u16::from_le_bytes([wav[i], wav[i + 1]]);
        let u32_at = |i: usize| u32::from_le_bytes(wav[i..i + 4].try_into().unwrap());
        assert_eq!(u32_at(16), 16, "fmt chunk size");
        assert_eq!(u16_at(20), 1, "PCM");
        assert_eq!(u16_at(22), 1, "mono");
        assert_eq!(u32_at(24), 16_000, "sample rate");
        assert_eq!(u32_at(28), 32_000, "byte rate");
        assert_eq!(u16_at(32), 2, "block align");
        assert_eq!(u16_at(34), 16, "bits per sample");
        assert_eq!(&wav[36..40], b"data");
        assert_eq!(u32_at(40), 6, "data bytes");
        assert_eq!(u32_at(4), 36 + 6, "RIFF size");
        assert_eq!(wav.len(), 44 + 6);
        assert_eq!(i16::from_le_bytes([wav[44], wav[45]]), 1);
        assert_eq!(i16::from_le_bytes([wav[46], wav[47]]), -2);
        assert_eq!(i16::from_le_bytes([wav[48], wav[49]]), 300);
    }

    #[test]
    fn samples_are_clipped_to_16_bit() {
        assert_eq!(to_i16(0.0), 0);
        assert_eq!(to_i16(1.0), 32767);
        assert_eq!(to_i16(-1.0), -32767);
        assert_eq!(to_i16(3.0), 32767);
        assert_eq!(to_i16(-3.0), -32767);
    }

    #[test]
    fn stereo_frames_are_averaged_to_mono() {
        let mono = mix_to_mono(&[0.2, 0.4, -1.0, 1.0], 2);
        assert_eq!(mono.len(), 2);
        assert!((mono[0] - 0.3).abs() < 1e-6 && mono[1].abs() < 1e-6);
        assert_eq!(mix_to_mono(&[0.5, 0.25], 1), vec![0.5, 0.25]);
    }

    #[test]
    fn resampling_changes_the_length_by_the_rate_ratio() {
        let input: Vec<f32> = (0..4800).map(|i| i as f32 / 4800.0).collect();
        let out = resample_linear(&input, 48_000, 16_000);
        assert_eq!(out.len(), 1600);
        // A ramp stays a ramp: each output sample is the input three samples later.
        assert!((out[10] - input[30]).abs() < 1e-5);
        assert_eq!(resample_linear(&input, 16_000, 16_000).len(), 4800);
        assert!(resample_linear(&[], 48_000, 16_000).is_empty());
    }

    #[cfg(not(any(target_os = "macos", target_os = "windows")))]
    #[test]
    fn recorder_is_the_first_installed_one() {
        let all = |_: &str| true;
        assert_eq!(pick_recorder(all), Some(LinuxRecorder::PwRecord));
        assert_eq!(
            pick_recorder(|n| n != "pw-record"),
            Some(LinuxRecorder::Parecord)
        );
        assert_eq!(
            pick_recorder(|n| n == "arecord"),
            Some(LinuxRecorder::Arecord)
        );
        assert_eq!(pick_recorder(|_| false), None);
    }

    #[cfg(not(any(target_os = "macos", target_os = "windows")))]
    #[test]
    fn recorder_arguments_ask_for_16k_mono_s16_wav() {
        for r in [
            LinuxRecorder::PwRecord,
            LinuxRecorder::Parecord,
            LinuxRecorder::Arecord,
        ] {
            let (program, args) = recorder_args(r, "/tmp/x.wav", "");
            let joined = args.join(" ");
            assert!(joined.contains("16000"), "{program}: {joined}");
            assert!(
                joined.contains("s16") || joined.contains("S16_LE"),
                "{program}"
            );
            assert!(joined.contains('1'), "{program}: mono");
            assert_eq!(args.last().map(String::as_str), Some("/tmp/x.wav"));
        }
    }

    #[cfg(not(any(target_os = "macos", target_os = "windows")))]
    #[test]
    fn a_chosen_microphone_goes_before_the_file() {
        let mic = "alsa_input.usb-Mic-00.mono-fallback";
        let (_, pw) = recorder_args(LinuxRecorder::PwRecord, "/tmp/x.wav", mic);
        assert_eq!(pw[pw.len() - 3..], ["--target", mic, "/tmp/x.wav"]);
        let (_, pa) = recorder_args(LinuxRecorder::Parecord, "/tmp/x.wav", mic);
        assert_eq!(pa[pa.len() - 2], format!("--device={mic}"));
        let (_, al) = recorder_args(LinuxRecorder::Arecord, "/tmp/x.wav", "hw:1,0");
        assert_eq!(al[al.len() - 3..], ["-D", "hw:1,0", "/tmp/x.wav"]);
    }

    #[cfg(not(any(target_os = "macos", target_os = "windows")))]
    #[test]
    fn device_lists_are_read_from_pactl_and_arecord() {
        let pactl = "0\talsa_output.pci.analog-stereo.monitor\tPipeWire\ts32le 2ch 48000Hz\tSUSPENDED\n\
                     1\talsa_input.pci.analog-stereo\tPipeWire\ts32le 2ch 48000Hz\tRUNNING\n";
        assert_eq!(parse_pactl_sources(pactl), ["alsa_input.pci.analog-stereo"]);
        let arecord = "null\n    Discard all samples\ndefault\n    Default ALSA device\nhw:CARD=PCH,DEV=0\n    HDA Intel PCH\n";
        assert_eq!(
            parse_arecord_list(arecord),
            ["default", "hw:CARD=PCH,DEV=0"]
        );
    }
}
