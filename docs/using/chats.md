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
the stream. Turn the stats off in [General](../settings/general.html#stats).

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
