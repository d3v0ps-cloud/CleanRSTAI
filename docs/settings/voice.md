---
title: Voice
parent: Settings
nav_order: 10
---

# Voice
{: .no_toc }

Dictating into the message box and reading replies aloud. How they work is on the
[Voice](../using/voice.html) page.

## Voice input

| Setting | Default | What it does |
|---|---|---|
| **Voice input** | Off | Lets the microphone in the message box record (and ⌘/Ctrl + Shift + M), and turns on voice mode |
| **Provider** | Active provider | The provider whose base URL, API key and headers the transcription call uses |
| **Model** | `whisper-1` | The transcription model, or the name a local server gives its model |
| **Language** | Detect | A language code such as `de`; empty lets the model detect it |
| **Longest recording (seconds)** | 120 | A recording stops by itself after this long. From 5 to 600 |
| **Microphone** | System default | The input device to record from. macOS and Windows list the system's microphones; Linux lists PipeWire or PulseAudio sources (`pactl`), or ALSA devices (`arecord -L`) when only `arecord` is installed. A saved microphone that isn't plugged in stays chosen, and recording falls back to the default (on macOS and Windows) until it is back. The chevron beside the microphone in the message box picks it too |

## Reading replies aloud

| Setting | Default | What it does |
|---|---|---|
| **Read replies aloud** | Off | Shows the speaker button under replies and enables ⌘/Ctrl + Shift + S |
| **Speech engine** | System voice | **System voice** uses the computer's own voice and sends nothing anywhere. **Speech endpoint** sends the text to a text-to-speech endpoint |
| **Provider** | Active provider | Speech endpoint only: the provider whose base URL, API key and headers the call uses |
| **Model** | `tts-1` | Speech endpoint only: the speech model |
| **Voice** | `alloy` | Speech endpoint only: the voice the server speaks with |
| **Speed** | 1.0 | From 0.5 to 2.0. For the system voice it sets the speaking rate too |
| **Longest reply read (characters)** | 4000 | Longer replies are cut short before they are read. From 200 to 20,000 |

Voice settings are not part of a chat: they apply to every chat, and there is no per-chat switch.
