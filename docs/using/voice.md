---
title: Voice
parent: Using the App
nav_order: 9
---

# Voice
{: .no_toc }

Speak into the message box instead of typing, and have replies read aloud. Both are off until
you turn them on in [Settings → Voice](../settings/voice.html).

## Voice input

The microphone sits at the right of the message box's bottom row. With **Voice input** on, click
it, or press **⌘/Ctrl + Shift + M**, to start recording (with it off, it says where to turn it
on). The button turns
red and counts the seconds. Click it again, or press the shortcut again, to stop. The clip is
then transcribed, and the text appears in the message box at the cursor (or at the end), so you
can read and edit it before you send. Nothing is sent for you.

A recording stops by itself after the **Longest recording** you set (120 seconds by default).

### Where the audio goes

The clip is sent as WAV to the provider chosen under **Provider**, at
`POST {base URL}/audio/transcriptions`, with the provider's API key and headers and the
**Model** (`whisper-1` by default) and **Language** you set. If no provider is chosen, the active
provider is used, so pick one whose service has a transcription endpoint. The audio is not kept
after the text comes back.

### A local Whisper model

Any server that answers `POST /audio/transcriptions` in the OpenAI format works, so a Whisper
model that runs on your own computer can be used with no cloud service:

1. Run a transcription server, such as whisper.cpp's server, faster-whisper-server, speaches or
   LocalAI. Each has its own setup and its own address; check its documentation for the route
   and the model name it serves.
2. Add a provider in [Providers](../settings/providers.html) whose **Base URL** is the server's
   OpenAI-compatible address (usually ending in `/v1`), and whose API key is anything the server
   accepts (or empty when it takes none).
3. In [Settings → Voice](../settings/voice.html), choose that provider and set **Model** to the
   name the server gives its Whisper model.

Local servers never see the clip leave your network, but the clip still goes to the address you
set, so use a server on this computer or on a network you trust.

### Microphone

The small chevron beside the microphone picks the input device (**Microphone** in
[Settings → Voice](../settings/voice.html) does the same); **Look for microphones again** reads the
list afresh after you plug one in. **System default** follows the system's choice.

On macOS and Windows the app records with the system's audio API, and the system asks for
microphone permission the first time. On Linux it uses the first of these that is installed:

- `pw-record` (PipeWire),
- `parecord` (PulseAudio),
- `arecord` (ALSA utils).

If none is installed, the button reports "Install PipeWire, PulseAudio or ALSA utils to record".
The device list comes from `pactl list short sources` for the first two (without the monitors of
outputs) and `arecord -L` for ALSA; a chosen device is passed as `--target`, `--device=` or `-D`.

## Voice mode

The waveform at the right end of the message box (shown while the box is empty) starts voice
mode, a spoken back-and-forth:

1. it records, as the microphone does; click the microphone (or press the shortcut) when you have
   finished speaking, or let the **Longest recording** end it;
2. the transcript is sent at once, without stopping in the message box;
3. when the reply is done it is read aloud, if **Read replies aloud** is on;
4. then it records again.

The waveform is lit while voice mode is on; click it again (or use **Voice mode on or off** in the
command palette) to end it, which also stops a recording or reading. Voice mode needs voice input
on, and it ends by itself when a transcription, a reading or the reply fails. It waits while the
reply waits for your approval of a tool.

## Reading replies aloud

With **Read replies aloud** on, each reply that has text gets a speaker button under it, next to
Copy and Retry. Click it to read that reply; click it again, or the **Stop reading** button that
replaces it, to stop. **⌘/Ctrl + Shift + S** reads the chat's latest reply, or stops the reading
if one is going on.

Only one reply is read at a time: starting another stops the first.

Before a reply is read, the app turns its Markdown into plain prose:

- code blocks are replaced by "(code block omitted)" and tables by "(table omitted)", so they are
  not read out;
- links are read as their text, never as their address;
- headings, lists and emphasis are read as their words;
- images are skipped;
- a reply longer than **Longest reply read** is cut short at a word boundary.

### Voices

- **System voice** (the default) uses what the computer already has, and nothing leaves it:
  - macOS: `say`, with the system voice.
  - Windows: the Windows speech synthesizer (System.Speech), with the default voice installed.
    The text is passed through standard input, never on the command line.
  - Linux: `spd-say` (Speech Dispatcher) or, failing that, `espeak-ng` or `espeak`. The window
    says so when none is installed.
- **Speech endpoint** sends the reply's text to the provider chosen under **Provider**, at
  `POST {base URL}/audio/speech`, with the **Model** (`tts-1` by default), the **Voice**
  (`alloy` by default) and the **Speed**. The app asks for WAV audio and plays it on this
  computer: through the default output device on macOS and Windows, and through `pw-play`,
  `paplay` or `aplay` on Linux (one of these must be installed). A server that answers with
  anything other than WAV gets an error.

## Limits

- The switches are in Settings rather than per chat, because the microphone and the speaker are
  shared by every chat.
- Recording and reading run in the background, so the window stays responsive. The clip and the
  reply's audio are kept only while they are used; the temporary WAV files on Linux are deleted
  after use.
- Command-line runs (`cleanrstai exec`) don't use voice.

For what each feature sends and where it is kept, see
[Data and privacy](../reference/data-and-privacy.html#what-leaves-your-computer).
