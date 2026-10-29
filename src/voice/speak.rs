//! Reading replies aloud. The reply's Markdown becomes plain prose (code blocks and tables are
//! announced, not read). The OS voice is a speech program the system has (`say`, PowerShell's
//! System.Speech, or spd-say / espeak); the text goes in on stdin, and never on a command line on
//! Windows. Endpoint speech is WAV audio from `POST /audio/speech`, played on the default output
//! device with cpal on macOS and Windows, or with pw-play, paplay or aplay elsewhere.

use std::io::Write as _;
use std::process::{Command, Stdio};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::Duration;

use pulldown_cmark::{Event, Options, Parser, Tag, TagEnd};

use super::record::resample_linear;

/// Words per minute at speed 1.0 for the OS voices.
const BASE_WPM: f32 = 175.0;

/// The prose a reply reads as: its text in order, one line per block, with code blocks and
/// tables replaced by a short note, links read as their text (never their address), and the
/// result cut to `max_chars` at a word boundary when it is longer.
pub fn prose(markdown: &str, max_chars: usize) -> String {
    let mut out = String::new();
    // Set while inside a block that is announced rather than read; the value is its end tag.
    let mut skip: Option<TagEnd> = None;
    let mut image_depth = 0usize;
    for event in Parser::new_ext(
        markdown,
        Options::ENABLE_TABLES | Options::ENABLE_STRIKETHROUGH,
    ) {
        if let Some(end) = &skip {
            if let Event::End(e) = &event
                && e == end
            {
                skip = None;
            }
            continue;
        }
        match event {
            Event::Start(Tag::CodeBlock(_)) => {
                out.push_str("\n(code block omitted)\n");
                skip = Some(TagEnd::CodeBlock);
            }
            Event::Start(Tag::Table(_)) => {
                out.push_str("\n(table omitted)\n");
                skip = Some(TagEnd::Table);
            }
            Event::Start(Tag::Image { .. }) => image_depth += 1,
            Event::End(TagEnd::Image) => image_depth = image_depth.saturating_sub(1),
            // Image descriptions are alt text, not something the reply says.
            Event::Text(t) if image_depth == 0 => out.push_str(&t),
            Event::Code(c) if image_depth == 0 => out.push_str(&c),
            Event::SoftBreak if image_depth == 0 => out.push(' '),
            Event::HardBreak | Event::Rule => out.push('\n'),
            Event::End(
                TagEnd::Paragraph | TagEnd::Heading(_) | TagEnd::Item | TagEnd::BlockQuote(_),
            ) => out.push('\n'),
            Event::Start(Tag::Item) => out.push('\n'),
            _ => {}
        }
    }
    let lines: Vec<&str> = out
        .lines()
        .map(str::trim)
        .filter(|l| !l.is_empty())
        .collect();
    cut_at_word(&lines.join("\n"), max_chars)
}

/// The first `max` characters, cut back to the last space when one is near the end.
fn cut_at_word(text: &str, max: usize) -> String {
    if text.chars().count() <= max {
        return text.to_owned();
    }
    let head: String = text.chars().take(max).collect();
    match head.rfind(char::is_whitespace) {
        Some(i) if i > head.len() / 2 => head[..i].trim_end().to_owned(),
        _ => head,
    }
}

/// The body of `POST {base}/audio/speech`. WAV is asked for, because it is the one format
/// every playback path here reads.
pub fn tts_body(model: &str, voice: &str, speed: f32, text: &str) -> serde_json::Value {
    serde_json::json!({
        "model": model,
        "input": text,
        "voice": voice,
        "response_format": "wav",
        "speed": speed,
    })
}

/// A speech program and what to give it.
#[derive(Debug, Clone, PartialEq)]
pub struct SpeechCommand {
    pub program: String,
    pub args: Vec<String>,
    /// Text written to the program's stdin (the program reads it all, then speaks).
    pub stdin: Option<String>,
    /// Run after the program is stopped early, to silence the speech server (spd-say).
    pub cancel: Option<(String, Vec<String>)>,
}

/// The words-per-minute an OS voice speaks at for a speed setting.
fn wpm(speed: f32) -> u32 {
    (BASE_WPM * speed).round().clamp(90.0, 720.0) as u32
}

/// The PowerShell script for Windows. The text is read from stdin, so it is never part of the
/// command line; the rate is a number from the speed setting.
pub fn windows_script(speed: f32) -> String {
    let rate = ((speed - 1.0) * 10.0).round().clamp(-10.0, 10.0) as i32;
    format!(
        "Add-Type -AssemblyName System.Speech; \
         [Console]::InputEncoding = [System.Text.Encoding]::UTF8; \
         $s = New-Object System.Speech.Synthesis.SpeechSynthesizer; \
         $s.Rate = {rate}; \
         $s.Speak([Console]::In.ReadToEnd())"
    )
}

/// The OS voice command for an operating system (`std::env::consts::OS`), or None when there is
/// no speech program. `has` says whether a program is installed (Linux only).
pub fn os_voice_command(
    os: &str,
    speed: f32,
    text: &str,
    has: impl Fn(&str) -> bool,
) -> Option<SpeechCommand> {
    let stdin = Some(text.to_owned());
    match os {
        "macos" => Some(SpeechCommand {
            program: "say".into(),
            args: vec!["-r".into(), wpm(speed).to_string()],
            stdin,
            cancel: None,
        }),
        "windows" => Some(SpeechCommand {
            program: "powershell".into(),
            args: vec![
                "-NoProfile".into(),
                "-NonInteractive".into(),
                "-Command".into(),
                windows_script(speed),
            ],
            stdin,
            cancel: None,
        }),
        _ if has("spd-say") => {
            let rate = ((speed - 1.0) * 100.0).round().clamp(-100.0, 100.0) as i32;
            Some(SpeechCommand {
                program: "spd-say".into(),
                // Passed as an argument after `--` (no shell), since spd-say takes no stdin.
                args: vec![
                    "--wait".into(),
                    "-r".into(),
                    rate.to_string(),
                    "--".into(),
                    text.to_owned(),
                ],
                stdin: None,
                cancel: Some(("spd-say".into(), vec!["-S".into()])),
            })
        }
        _ => ["espeak-ng", "espeak"]
            .into_iter()
            .find(|p| has(p))
            .map(|program| SpeechCommand {
                program: program.into(),
                args: vec!["-s".into(), wpm(speed).to_string()],
                stdin,
                cancel: None,
            }),
    }
}

/// Runs a speech command until it finishes or `stop` is set (then it is killed).
pub fn run_os_voice(cmd: &SpeechCommand, stop: &AtomicBool) -> Result<(), String> {
    let mut child = Command::new(&cmd.program)
        .args(&cmd.args)
        .stdin(if cmd.stdin.is_some() {
            Stdio::piped()
        } else {
            Stdio::null()
        })
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()
        .map_err(|e| format!("Couldn't start {}: {e}", cmd.program))?;
    if let (Some(text), Some(mut input)) = (&cmd.stdin, child.stdin.take()) {
        // A write error means the program quit early; its exit status says the rest.
        let _ = input.write_all(text.as_bytes());
    }
    loop {
        if stop.load(Ordering::Relaxed) {
            let _ = child.kill();
            let _ = child.wait();
            if let Some((program, args)) = &cmd.cancel {
                let _ = Command::new(program)
                    .args(args)
                    .stdout(Stdio::null())
                    .stderr(Stdio::null())
                    .status();
            }
            return Ok(());
        }
        match child.try_wait() {
            Ok(Some(status)) if status.success() => return Ok(()),
            Ok(Some(status)) => return Err(format!("{} failed ({status})", cmd.program)),
            Ok(None) => std::thread::sleep(Duration::from_millis(50)),
            Err(e) => return Err(format!("{}: {e}", cmd.program)),
        }
    }
}

/// Decoded WAV audio: interleaved samples in [-1, 1].
#[derive(Debug, Clone, PartialEq)]
pub struct Wav {
    pub rate: u32,
    pub channels: usize,
    pub samples: Vec<f32>,
}

/// Reads PCM (8, 16, 24 or 32 bit) or 32-bit float WAV bytes.
pub fn parse_wav(bytes: &[u8]) -> Result<Wav, String> {
    if bytes.len() < 12 || &bytes[0..4] != b"RIFF" || &bytes[8..12] != b"WAVE" {
        return Err("The speech endpoint didn't return WAV audio".into());
    }
    let u16_at = |i: usize| u16::from_le_bytes([bytes[i], bytes[i + 1]]);
    let u32_at =
        |i: usize| u32::from_le_bytes([bytes[i], bytes[i + 1], bytes[i + 2], bytes[i + 3]]);
    let mut fmt: Option<(u16, usize, u32, u16)> = None;
    let mut data: Option<&[u8]> = None;
    let mut pos = 12usize;
    while pos + 8 <= bytes.len() {
        let id = &bytes[pos..pos + 4];
        let size = u32_at(pos + 4) as usize;
        let start = pos + 8;
        let end = start.saturating_add(size).min(bytes.len());
        if id == b"fmt " && end - start >= 16 {
            let tag = u16_at(start);
            let channels = u16_at(start + 2) as usize;
            let rate = u32_at(start + 4);
            let bits = u16_at(start + 14);
            fmt = Some((tag, channels, rate, bits));
        } else if id == b"data" {
            data = Some(&bytes[start..end]);
        }
        pos = start.saturating_add(size).saturating_add(size & 1);
    }
    let (tag, channels, rate, bits) = fmt.ok_or("The WAV audio has no format chunk")?;
    let data = data.ok_or("The WAV audio has no data")?;
    if channels == 0 || rate == 0 {
        return Err("The WAV audio has no channels or sample rate".into());
    }
    // 0xFFFE (extensible) carries PCM or float in a sub-format; the sample size decides here.
    let float = tag == 3 && bits == 32;
    if tag != 1 && tag != 3 && tag != 0xFFFE {
        return Err(format!("Unsupported WAV encoding {tag}"));
    }
    let samples: Vec<f32> = match (float, bits) {
        (true, 32) => data
            .as_chunks::<4>()
            .0
            .iter()
            .map(|c| f32::from_le_bytes(*c))
            .collect(),
        (false, 8) => data
            .iter()
            .map(|b| (f32::from(*b) - 128.0) / 128.0)
            .collect(),
        (false, 16) => data
            .as_chunks::<2>()
            .0
            .iter()
            .map(|c| f32::from(i16::from_le_bytes(*c)) / 32768.0)
            .collect(),
        (false, 24) => data
            .as_chunks::<3>()
            .0
            .iter()
            .map(|c| {
                let v = i32::from_le_bytes([0, c[0], c[1], c[2]]) >> 8;
                v as f32 / 8_388_608.0
            })
            .collect(),
        (false, 32) => data
            .as_chunks::<4>()
            .0
            .iter()
            .map(|c| i32::from_le_bytes(*c) as f32 / 2_147_483_648.0)
            .collect(),
        _ => return Err(format!("Unsupported WAV sample size {bits} bits")),
    };
    Ok(Wav {
        rate,
        channels,
        samples,
    })
}

/// The audio laid out for an output device: resampled to its rate, with its channel count
/// (a mono track is copied to every channel).
#[cfg_attr(not(any(target_os = "macos", target_os = "windows")), allow(dead_code))]
pub fn render(wav: &Wav, out_rate: u32, out_channels: usize) -> Vec<f32> {
    let ch = wav.channels.max(1);
    let frames = wav.samples.len() / ch;
    let per_channel: Vec<Vec<f32>> = (0..ch)
        .map(|c| {
            let lane: Vec<f32> = (0..frames).map(|f| wav.samples[f * ch + c]).collect();
            resample_linear(&lane, wav.rate, out_rate)
        })
        .collect();
    let out_frames = per_channel.first().map_or(0, Vec::len);
    (0..out_frames)
        .flat_map(|f| (0..out_channels).map(move |oc| (oc, f)))
        .map(|(oc, f)| per_channel[oc.min(ch - 1)][f])
        .collect()
}

/// Players tried on Linux, in order of preference.
#[cfg(not(any(target_os = "macos", target_os = "windows")))]
pub fn pick_player(has: impl Fn(&str) -> bool) -> Option<&'static str> {
    ["pw-play", "paplay", "aplay"].into_iter().find(|p| has(p))
}

#[cfg(not(any(target_os = "macos", target_os = "windows")))]
fn player_args(program: &str, path: &str) -> Vec<String> {
    match program {
        "aplay" => vec!["-q".into(), path.into()],
        _ => vec![path.into()],
    }
}

/// Plays WAV bytes until they end or `stop` is set.
pub fn play_wav(bytes: &[u8], stop: &Arc<AtomicBool>) -> Result<(), String> {
    let wav = parse_wav(bytes)?;
    #[cfg(any(target_os = "macos", target_os = "windows"))]
    {
        play_cpal(&wav, stop)
    }
    #[cfg(not(any(target_os = "macos", target_os = "windows")))]
    {
        let _ = wav;
        play_process(bytes, stop)
    }
}

/// Plays through the default output device, until the samples are out or `stop` is set.
#[cfg(any(target_os = "macos", target_os = "windows"))]
fn play_cpal(wav: &Wav, stop: &AtomicBool) -> Result<(), String> {
    use cpal::traits::{DeviceTrait, HostTrait, StreamTrait};
    use std::sync::atomic::AtomicUsize;

    let device = cpal::default_host()
        .default_output_device()
        .ok_or("No audio output device found")?;
    let config = device
        .default_output_config()
        .map_err(|e| format!("Couldn't read the audio output: {e}"))?;
    let channels = usize::from(config.channels());
    let samples = Arc::new(render(wav, config.sample_rate(), channels));
    let pos = Arc::new(AtomicUsize::new(0));
    let done = Arc::new(AtomicBool::new(false));
    let stream = match config.sample_format() {
        cpal::SampleFormat::F32 => build_output::<f32>(&device, &config, &samples, &pos, &done),
        cpal::SampleFormat::I16 => build_output::<i16>(&device, &config, &samples, &pos, &done),
        cpal::SampleFormat::U16 => build_output::<u16>(&device, &config, &samples, &pos, &done),
        other => Err(cpal::Error::with_message(
            cpal::ErrorKind::UnsupportedConfig,
            format!("unsupported output format {other:?}"),
        )),
    }
    .map_err(|e| format!("Couldn't open the audio output: {e}"))?;
    stream
        .play()
        .map_err(|e| format!("Couldn't start the audio output: {e}"))?;
    while !done.load(Ordering::Relaxed) && !stop.load(Ordering::Relaxed) {
        std::thread::sleep(Duration::from_millis(30));
    }
    // The device still holds its last buffer; let it play out before the stream closes.
    if !stop.load(Ordering::Relaxed) {
        std::thread::sleep(Duration::from_millis(150));
    }
    Ok(())
}

#[cfg(any(target_os = "macos", target_os = "windows"))]
fn build_output<T>(
    device: &cpal::Device,
    config: &cpal::SupportedStreamConfig,
    samples: &Arc<Vec<f32>>,
    pos: &Arc<std::sync::atomic::AtomicUsize>,
    done: &Arc<AtomicBool>,
) -> Result<cpal::Stream, cpal::Error>
where
    T: cpal::SizedSample + cpal::FromSample<f32>,
{
    use cpal::traits::DeviceTrait;
    let (samples, pos, done) = (samples.clone(), pos.clone(), done.clone());
    device.build_output_stream::<T, _, _>(
        config.config(),
        move |out: &mut [T], _: &cpal::OutputCallbackInfo| {
            let mut p = pos.load(Ordering::Relaxed);
            for slot in out.iter_mut() {
                let v = samples.get(p).copied().unwrap_or(0.0);
                *slot = cpal::Sample::to_sample::<T>(v);
                p = p.saturating_add(1);
            }
            pos.store(p, Ordering::Relaxed);
            if p >= samples.len() {
                done.store(true, Ordering::Relaxed);
            }
        },
        |e| log::warn!("audio output: {e}"),
        None,
    )
}

/// Plays through the first installed Linux player, from a temporary file.
#[cfg(not(any(target_os = "macos", target_os = "windows")))]
fn play_process(bytes: &[u8], stop: &AtomicBool) -> Result<(), String> {
    let program = pick_player(|p| which::which(p).is_ok()).ok_or_else(|| {
        "Install PipeWire, PulseAudio or ALSA utils to play replies aloud".to_owned()
    })?;
    let path = std::env::temp_dir().join(format!("cleanrstai-speech-{}.wav", uuid::Uuid::new_v4()));
    std::fs::write(&path, bytes).map_err(|e| format!("Couldn't write the speech audio: {e}"))?;
    let result = (|| {
        let mut child = Command::new(program)
            .args(player_args(program, &path.to_string_lossy()))
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .spawn()
            .map_err(|e| format!("Couldn't start {program}: {e}"))?;
        loop {
            if stop.load(Ordering::Relaxed) {
                let _ = child.kill();
                let _ = child.wait();
                return Ok(());
            }
            match child.try_wait() {
                Ok(Some(status)) if status.success() => return Ok(()),
                Ok(Some(status)) => return Err(format!("{program} failed ({status})")),
                Ok(None) => std::thread::sleep(Duration::from_millis(50)),
                Err(e) => return Err(format!("{program}: {e}")),
            }
        }
    })();
    let _ = std::fs::remove_file(&path);
    result
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn prose_reads_text_and_drops_markup_and_urls() {
        let md = "# Title\n\nSome **bold** and a [link](https://example.com/secret) here.\n\n- one\n- two\n";
        let text = prose(md, 4000);
        assert!(text.contains("Title"));
        assert!(text.contains("Some bold and a link here."), "{text}");
        assert!(!text.contains("https://"), "{text}");
        assert!(!text.contains('*'), "{text}");
        assert!(text.contains("one\ntwo"), "{text}");
    }

    #[test]
    fn code_blocks_and_tables_are_announced_not_read() {
        let md =
            "Before.\n\n```rust\nfn secret() {}\n```\n\n| a | b |\n|---|---|\n| 1 | 2 |\n\nAfter.";
        let text = prose(md, 4000);
        assert!(text.contains("(code block omitted)"), "{text}");
        assert!(text.contains("(table omitted)"), "{text}");
        assert!(!text.contains("secret"), "{text}");
        assert!(!text.contains('|'), "{text}");
        assert!(
            text.contains("Before.") && text.contains("After."),
            "{text}"
        );
    }

    #[test]
    fn images_read_nothing_and_inline_code_reads_its_text() {
        let text = prose("See ![a cat](cat.png) and `cargo test`.", 4000);
        assert!(!text.contains("cat"), "{text}");
        assert!(text.contains("cargo test"), "{text}");
    }

    #[test]
    fn long_prose_is_cut_at_a_word_boundary() {
        let md = "word ".repeat(100);
        let text = prose(&md, 23);
        assert!(text.chars().count() <= 23);
        assert!(text.ends_with("word"), "{text:?}");
        assert_eq!(prose("short", 4000), "short");
    }

    #[test]
    fn tts_body_asks_for_wav_with_the_settings() {
        let v = tts_body("tts-1", "alloy", 1.25, "Hi");
        assert_eq!(v["model"], "tts-1");
        assert_eq!(v["voice"], "alloy");
        assert_eq!(v["input"], "Hi");
        assert_eq!(v["response_format"], "wav");
        assert_eq!(v["speed"], 1.25);
    }

    #[test]
    fn windows_voice_never_puts_the_text_in_argv() {
        let secret = "launch the rocket; rm -rf / \"quoted\" $(whoami)";
        let cmd = os_voice_command("windows", 1.0, secret, |_| false).unwrap();
        assert_eq!(cmd.program, "powershell");
        assert!(
            cmd.args.iter().all(|a| !a.contains("rocket")),
            "{:?}",
            cmd.args
        );
        assert_eq!(cmd.stdin.as_deref(), Some(secret));
        assert!(cmd.args.last().unwrap().contains("ReadToEnd"));
    }

    #[test]
    fn macos_say_reads_stdin_at_the_chosen_rate() {
        let cmd = os_voice_command("macos", 2.0, "hello", |_| false).unwrap();
        assert_eq!(cmd.program, "say");
        assert_eq!(cmd.args, vec!["-r", "350"]);
        assert_eq!(cmd.stdin.as_deref(), Some("hello"));
    }

    #[test]
    fn linux_prefers_spd_say_then_espeak() {
        let all = |_: &str| true;
        assert_eq!(
            os_voice_command("linux", 1.0, "x", all).unwrap().program,
            "spd-say"
        );
        let espeak =
            os_voice_command("linux", 1.0, "x", |p| p != "spd-say" && p != "espeak").unwrap();
        assert_eq!(espeak.program, "espeak-ng");
        assert_eq!(espeak.stdin.as_deref(), Some("x"));
        let plain = os_voice_command("linux", 1.0, "x", |p| p == "espeak").unwrap();
        assert_eq!(plain.program, "espeak");
        assert!(os_voice_command("linux", 1.0, "x", |_| false).is_none());
    }

    #[test]
    fn spd_say_gets_the_text_after_a_separator_and_a_cancel() {
        let cmd = os_voice_command("linux", 1.0, "-not an option", |p| p == "spd-say").unwrap();
        assert_eq!(cmd.args.last().map(String::as_str), Some("-not an option"));
        assert_eq!(&cmd.args[cmd.args.len() - 2], "--");
        assert_eq!(cmd.cancel, Some(("spd-say".into(), vec!["-S".into()])));
    }

    #[test]
    fn wav_round_trips_through_the_parser() {
        let pcm = [0i16, 16_384, -16_384, 32_767];
        let wav = parse_wav(&super::super::record::encode_wav(&pcm, 16_000)).unwrap();
        assert_eq!(wav.rate, 16_000);
        assert_eq!(wav.channels, 1);
        assert_eq!(wav.samples.len(), 4);
        assert!((wav.samples[1] - 0.5).abs() < 1e-4);
        assert!((wav.samples[2] + 0.5).abs() < 1e-4);
    }

    #[test]
    fn parser_rejects_what_is_not_wav() {
        assert!(parse_wav(b"ID3\x04 mp3 bytes").is_err());
        assert!(parse_wav(b"").is_err());
        let mut no_data = super::super::record::encode_wav(&[1, 2], 16_000);
        no_data.truncate(36); // header with no data chunk
        assert!(parse_wav(&no_data).is_err());
    }

    #[test]
    fn render_copies_mono_to_stereo_and_resamples() {
        let wav = Wav {
            rate: 16_000,
            channels: 1,
            samples: vec![0.5; 1600],
        };
        let out = render(&wav, 48_000, 2);
        assert_eq!(out.len(), 4800 * 2);
        assert!(out.iter().all(|s| (*s - 0.5).abs() < 1e-6));
    }

    #[cfg(not(any(target_os = "macos", target_os = "windows")))]
    #[test]
    fn player_is_the_first_installed_one() {
        assert_eq!(pick_player(|_| true), Some("pw-play"));
        assert_eq!(pick_player(|p| p == "aplay"), Some("aplay"));
        assert_eq!(pick_player(|_| false), None);
        assert_eq!(player_args("aplay", "/x.wav"), vec!["-q", "/x.wav"]);
    }
}
