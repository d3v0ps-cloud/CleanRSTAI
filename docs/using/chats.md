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
the stream. Hover over the line for the full breakdown. Turn the stats off in
[General](../settings/general.html#stats).

#### Prompt progress

A long prompt can take a local model a minute or more to read before the reply starts, for
example with many MCP tools switched on or a large attachment. With a llama.cpp server (set
**Server type** on the provider to *llama.cpp server*, or click **Detect**), a bar above the
message box shows how far it has got: tokens read out of the prompt, how many came from the
server's cache, the time so far and an estimate of the time left. It gives way to the reply
when the first words arrive, and shows even with stats turned off.

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
