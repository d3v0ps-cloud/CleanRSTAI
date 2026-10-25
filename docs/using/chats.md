---
title: Chats
parent: Using the App
nav_order: 1
---

# Chats
{: .no_toc }

1. TOC
{:toc}

## The window

- **Sidebar** (left): **New chat**, a search box, pinned chats and recent chats, and
  **Settings** at the bottom. Hide or show it with **⌘/Ctrl + B**.
- **Top bar**: the sidebar, settings and command palette buttons, plus an **Update to …**
  link when a new version is out and a warning when an MCP server needs attention.
- **Chat**: your messages sit in bubbles on the right; replies run full width, and tool calls
  show as collapsible cards.
- **Message box** (bottom, or centred under the greeting in a new chat): **+** to attach
  files, the tools menu (sliders icon), the MCP menu (books icon), the model picker and
  **Set working directory**.

## Sending messages

Press **Enter** to send and **Shift+Enter** for a new line, or swap them in
[Settings → General](../settings/general.html#sending). **⌘/Ctrl + Enter** always sends.
While a reply is streaming, the send button becomes **Stop**.

### Choosing a model

The model picker under the message box lists your provider profiles and the models fetched
for each, and you can type a model name. **⌘/Ctrl + K** opens the command palette, where you
can also switch provider or model.

### Reasoning

Models that return `reasoning_content` or `reasoning` show it in a **Thought process** section
above the answer. It starts collapsed; [Settings → General → Reasoning](../settings/general.html#reasoning)
can keep it expanded, or expanded only while the model is thinking.

### Stats

Under every reply you see prefill and decode speed, prompt time and tokens in and out, plus a
running total for the chat. llama.cpp's own `timings` are used when the server reports them,
and the cost when OpenRouter or a LiteLLM proxy reports it; otherwise speeds are measured from
the stream. Hover over the line for the full breakdown.

A LiteLLM proxy doesn't report the cost of streamed replies, so the app estimates it from
the per-token prices the proxy lists for the model. An estimated cost shows with a "~", such
as *~$0.0123*, and so does the chat's total when it includes one. Turn the stats off in
[General](../settings/general.html#stats).

#### Prompt progress

A long prompt can take a local model a minute or more to read before the reply starts, for
example with many MCP tools switched on or a large attachment. With a llama.cpp server (set
**Server type** on the provider to *llama.cpp server*, or click **Detect**), a bar above the
message box shows how far it has got: tokens read out of the prompt, how many came from the
server's cache, the time so far and an estimate of the time left. It gives way to the reply
when the first words arrive, and shows even with stats turned off.

#### llama.cpp server details

With a llama.cpp provider, an info icon appears beside the model picker under the message
box. Hover over it for what the server reports about the model:

- **Status**: ready, loading the model, or asleep;
- **Model file**, **Parameters** and **Size**;
- **Context**: per slot, with the number of slots when it serves several requests at once,
  and **Trained context**, the most the model was trained for;
- **Images**: whether the model can read them;
- **Requests** being processed and queued, and **Draft acceptance** with speculative
  decoding, when the server runs with `--metrics`;
- **Build** of the server.

While the server loads the model, *Loading model…* shows beside the picker. *Asleep* means
the server unloaded it after idling (`--sleep-idle-seconds`); your next message reloads it,
so its reply starts later. The details refresh every 15 seconds, and every 3 seconds while
the model loads.

While a reply streams in, the live readout above the message box shows the server's own
prefill and decode speeds.

#### LiteLLM details

With a LiteLLM proxy (set **Server type** to *LiteLLM proxy*, or click **Detect**):

- **Under each reply**, the hover over the stats line adds what the proxy reports for that
  request: the deployment that served it (such as `azure/gpt-4o-2024-08-06` behind a
  `gpt-4o` alias) and its endpoint, retries and fallbacks, whether LiteLLM's response cache
  was used, the time LiteLLM itself added, your key's spend against its budget, the rate
  limit left, and for non-streamed replies the cost split into input, output, cache and
  reasoning.
- **Beside the model picker**, an info icon shows the model as the proxy lists it: the
  provider behind it, context window, longest reply, price per million tokens, whether it
  reads images and PDFs, calls tools and reasons, and your key's spend against its budget.
  It refreshes every minute.
- **Context use** works with LiteLLM too, against the context window the proxy lists.

#### Ollama, LM Studio, vLLM and OpenRouter

With **Server type** set to one of these (or found by **Detect**), an info icon beside the
model picker shows what the server reports, and context use works against the context size
the model actually runs with:

- **Ollama**: whether the model is loaded (*Not loaded* shows beside the picker; Ollama loads
  it for your next message), parameters, quantization, family, size and how much of it is in
  GPU memory, context size (as loaded, or `num_ctx` from the Modelfile), trained context,
  capabilities and the Ollama version.
- **LM Studio**: whether the model is loaded, its type, architecture, quantization, format and
  publisher, and the context size it was loaded with next to its maximum.
- **vLLM**: context size (`max_model_len`), the weights behind the served name, requests
  running and waiting, KV cache use, prefix cache hits and the vLLM version.
- **OpenRouter**: context size, longest reply, price per million tokens, what the model
  reads, and your key's spend, limit and credit left. Each reply's stats hover also names the
  provider OpenRouter sent it to, and the cost comes from OpenRouter's usage accounting.

#### Several models on one llama.cpp server

A llama.cpp server started in router mode (without `-m`) serves several models and loads
them as they are asked for. The model picker then lists the router's models with their
state (*loaded*, *loading…*, *asleep*, *not loaded*, *failed to load*) and a **Load** or
**Unload** link, so you can load a model before you need it or free the memory one uses.
When the selected model isn't loaded, *Not loaded* shows beside the picker; your next
message loads it.

#### Context use

With a llama.cpp server, including one running several models in router mode, the stats
also show how full the model's context is:

- under each reply, as a percentage, such as *context 38%*;
- in the chat's summary at the top, as tokens used out of the context size, such as
  *context 12.4k / 32.8k (38%)*. It turns to the warning colour from 80%.

The figure counts the prompt, the tokens reused from the server's cache and the reply. When
the context is full, the server may drop older parts of the chat or refuse the request, so
start a new chat before it gets there. Other server types don't report their context size,
so no figure is shown for them.

## Editing and retrying

Each message has small action icons:

- Your messages: **Edit and resend**, **Copy**.
- Replies: **Copy**, and **Retry** to generate the reply again.

## Replies

Replies are rendered as Markdown with syntax-highlighted code blocks (with a copy button),
tables and images.

- ```` ```mermaid ```` blocks are drawn as diagrams.
- ```` ```svg ```` blocks and inline `<svg>` markup are shown as images.
- Click a drawing to enlarge it, copy its source or save it as an SVG file.
- Images from the web show as links unless **Load web images in replies** is on in
  [General](../settings/general.html#images). Loading them tells the image's server that the
  chat was opened, so it is off by default.

## Artifacts

Web pages, drawings, documents and longer code in a reply are *artifacts*: they show as a card
in the reply and open in a panel beside the chat.

- **What counts**: closed top-level code blocks marked ```` ```html ````, ```` ```svg ````,
  ```` ```mermaid ```` or ```` ```markdown ````, and other code blocks of 10 lines or more.
  Shorter code stays in the reply. A block still being written shows as code until it ends.
- **Names** come from a file name or `title="…"` after the language (```` ```python build.py ````),
  a page's `<title>`, a document's first heading, or else what it is ("Python code").
- **The panel** shows drawings and documents with **Preview** and **Code** tabs, and pages and
  code with syntax highlighting. **Copy**, **Save as…** and **Close** sit at the top. The
  menu on the right lists every artifact in the chat.
- **Versions**: artifacts with the same name and type are versions of one another; the arrows
  step between them, keeping the tab you are on.
- **Web pages** don't run inside the app: **Open in browser** writes the page to a temporary
  file and opens it in your default browser, where its scripts run as in any local file. SVGs
  can open there too.
- Drawings stay drawn in the reply, with an extra button to open them in the panel.
- The panel opens by itself when a reply makes an artifact, and follows each new one while
  the newest is shown. Turn that off with **Open web pages, drawings and long code in a side
  panel** in [General](../settings/general.html#artifacts); the cards still open it.

## Attachments

Attach files with the **+** button or by dropping them on the window. Files up to 50 MB can be
attached; text beyond a million characters is cut, with a note saying so.

| Kind | Sent as |
|---|---|
| Images (PNG, JPEG, GIF, WebP) | images for vision models; click one to enlarge it |
| PDF (`.pdf`) | its text; scanned PDFs without a text layer can't be read |
| Word (`.docx`, `.doc`) | its text |
| Excel (`.xlsx`, `.xlsm`, `.xlsb`, `.xls`) | one CSV block per sheet |
| PowerPoint (`.pptx`, `.ppt`) | numbered slides, with speaker notes |
| OpenDocument (`.odt`, `.ods`, `.odp`) and RTF | their text |
| Any text file (code, Markdown, CSV, JSON, logs…) | inline, read as UTF-8, UTF-16 or Windows-1252 |

Documents are recognised by their content, not their extension. Password-protected files
can't be read.

## Managing chats

Chats are saved automatically. In the sidebar:

- Type in the search box to filter chats.
- Right-click a chat for **Pin** / **Unpin**, **Rename**, **Export as Markdown…** and
  **Delete**.

**Export as Markdown** includes tool calls, results and images. To back up every chat at once,
use **Export conversations…** in [General](../settings/general.html#export--import).

## Command palette

**⌘/Ctrl + K** opens a searchable list of commands: new chat, attach files, export this chat,
toggle the sidebar, change theme, open each settings page, export and import, check for
updates, switch provider or model, and open any chat.

See [Keyboard shortcuts](../reference/keyboard-shortcuts.html) for the rest.
