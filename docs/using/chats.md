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

The window is dark first and quiet: a sidebar on the left and a large canvas holding the chat,
with hairline dividers and greys rather than colour. On macOS the window has no title bar of its
own: the traffic lights sit at the top of the sidebar, and the strip along the top (the *title
row*) moves the window when dragged and zooms it on a double click.

- **Title row**: show or hide the sidebar (**⌘/Ctrl + B**), **Back** and **Forward**
  (**⌘/Ctrl + [** and **⌘/Ctrl + ]**) through the chats and pages you opened, and the
  **Chat / Code** switch ([below](#chat-and-code)). At the top right of the canvas, the ghost
  starts an [incognito chat](memory.html#incognito-chats) (it is lit while one is open), with the
  [artifact panel](#artifacts) button beside it in a chat that has artifacts, an **Update to …**
  link when a new version is out, and the MCP status when servers are set up.
- **Sidebar**: **New** (a new chat), then **Search** (a box that filters chats by title and text,
  in both modes), **Projects** and **Artifacts** (pages in the canvas, below), **Scheduled**
  ([Background](../settings/background.html#scheduled-tasks) settings), **Design**
  ([Appearance](../settings/appearance.html)) and **Customize**
  ([Skills & hooks](../settings/skills-and-hooks.html)). **Recents** (click the label to fold it
  away) lists the chats, pinned ones first; a long title fades out on the right, and a mark at the
  right edge shows a chat's state ([below](#running-chats-in-the-background)) or, with a document
  icon, that it has artifacts. The footer shows your name and the active provider, with a menu for
  Settings, the command palette, shortcuts, the documentation and **Check for updates**; the
  laptop button beside it opens [remote access](remote-access.html) (green while it runs).
- **Home**: a new chat opens on a greeting (with your name from
  [Appearance](../settings/appearance.html), or your account's), the message box in the middle,
  and your three newest artifacts under it. The app starts here. Once the chat has a message, the
  greeting and cards give way to the conversation: a narrow centred column, with the message box at
  the bottom.
- **Chat**: your messages sit in bubbles on the right; replies run the column's width, and tool
  calls show as collapsible cards.
- **Message box**: **+** opens a menu to attach files, this chat's **Tools** (switches,
  permissions, compaction and review), MCP **Resources & prompts**, and **Commands and skills**.
  On the right, the microphone (with a menu to pick the input device), then the waveform for
  [voice mode](voice.html#voice-mode); once you type, the send button takes the waveform's place.
  Typing **/** lists the [slash commands](../settings/skills-and-hooks.html) and skills that
  match. The box grows with what you type, then scrolls.
- **Under the message box**: **Project or folder** picks the chat's working folder (recent
  folders, **Choose a folder…**, **No folder**) and its [project](projects.html); on the right,
  the model, its reasoning effort (**· Medium**, or **· Default**) and the permission mode
  (**Ask**, **Auto**, **Allow all**, **Read only** or **Plan**), each opening its own menu.
- **Artifact panel** (right): web pages, drawings and longer code from replies.
- **Settings** opens over the window, which dims behind it; click outside or press **Esc** to
  close it when nothing is unsaved.

### Chat and Code

The switch in the title row splits your chats in two: **Chat** lists the chats without a working
folder, and **Code** the ones with one. A new chat in Code mode starts in the folder you used last
(change it under the message box), and one in Chat mode has none. Opening a chat (from search,
the palette or a project) switches to its mode, and the switch is remembered between runs.

### Projects and Artifacts pages

**Projects** lists each [project](projects.html) as a card with its folder, the first line of
its instructions and its newest chats (**Show all** for the rest), **New chat** and a menu to
**Edit**, **Export** or **Delete** it; **New project** and **Import…** sit at the top.
**Artifacts** shows the artifacts of your recent chats (the 200 most recent) as cards with a
preview, newest first; click one to open it in its chat's panel.

## Sending messages

Press **Enter** to send and **Shift+Enter** for a new line, or swap them in
[Settings → General](../settings/general.html#sending). **⌘/Ctrl + Enter** always sends.
While a reply is streaming, the send button becomes **Stop**.

### Choosing a model

The model's name under the message box (shown without an `org/` prefix) opens the picker: your
provider profiles and the models fetched for each, and you can type a model name. **⌘/Ctrl + K** opens the command palette, where you
can also switch provider or model.

### Model options

The effort label under the message box (**· Default** until one is set) opens the model
options for this chat: **Reasoning effort**, **Thinking budget**,
**Temperature**, **Top-p**, **Max tokens** and **Stop sequences** for this chat only. Unset
options show the default they inherit from the provider and model
([Settings → Providers](../settings/providers.html#reasoning-and-sampling)), or "server default".
The button shows the effort and temperature in use, and turns the accent colour when the chat
changes any; **Reset to defaults** clears the chat's own. Changes apply from the next message.

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
start a new chat before it gets there, or let the app compact it (below). Other server types
don't report their context size, so no figure is shown for them.

### Slash commands

A saved prompt can be sent by typing its name after a slash: `/explain fn main() {}` sends the
*explain* prompt with `fn main() {}` filled in. Type `/` in the message box and the matching
commands are listed above it; click one to put its name in the box. The message in the chat is
the expanded prompt, not the command. Commands come from
[Settings → Skills & hooks](../settings/skills-and-hooks.html#slash-commands), and from
`.cleanrstai/commands/` in the chat's working folder: each `<name>.md` file there is a command,
and a first line `description: …` gives its description. A folder command replaces a setting
with the same name. A `/name` that matches no command is sent as typed.

## Long chats

When a request would fill most of the model's context, the app *compacts* the chat: a model
summarises the older messages, and from then on only that summary (in the system prompt) and
the latest messages are sent. A marker in the chat, **Earlier messages summarised**, shows
where; it says how many messages it stands for and how many tokens the request had, and
expands to show the summary. Everything above it stays in the chat for you to read, but the
model no longer sees it. While the summary is written, *Summarising older messages…* shows. The
request that writes the summary counts in the chat's totals and cost.

**Compact now** in the tools menu (under **Chat features**) or **Compact this chat** in the
command palette does it straight away. Switch automatic compaction off for a chat in the tools
menu, and set when it happens, how much is kept and which model writes the summary in
[Context & files](../settings/context.html#context-compaction). For servers that don't report
their context size, set **Context size** there, or compaction only happens when you ask.

## Editing and retrying

Each message has small action icons:

- Your messages: **Edit and resend**, **Copy**, and **Fork from here**.
- Replies: **Copy**, **Retry** to generate the reply again (the latest reply only), and
  **Fork from here**. A reply with other versions shows the **‹ k/N ›** arrows.

**Fork from here** copies the chat up to that message into a new chat, titled with
"(fork)" after the original title. The new chat keeps the original's workspace folder, tool
switches, model options and permissions, and the original is left as it was. The workspace
folder is shared, not copied, so file edits made in either chat change the same files. The
file edits copied into the fork can't be undone from the fork, only from the original chat.
Forking is not available while a reply is being generated in that chat. Only the version
of the chat that is shown is copied (see [Branches](#branches)).

### Branches

**Retry** and **Edit and resend** keep what they replace as a branch instead of losing it.
A message with other versions shows arrows, **‹ k/N ›**: N is the number of versions and k
the one shown. The arrows switch to the previous or next version, and the replies after that
message then come from the version you picked. Editing keeps the turn from the edited message
on (its reply and what followed) as a version; the edit takes effect when you send it, so the
chat still shows the turn until then. Stopping a retry puts back the reply it was replacing.

Only the version shown is sent to the model, exported, compacted, forked and shown in the
browser client. Switching doesn't change files, and **Undo file edit** only undoes the edits
of the version shown. Arrows are unavailable while a reply runs or waits in the chat.

Settings → General → Branches keeps up to **Versions kept per message** (default 10) older
versions per message and drops the oldest beyond that. Turn it off there, or with **Branches**
under **Chat features** in the tools menu for one chat: off, Retry and Edit replace the
version as they always did (versions already kept stay switchable).

## Replies

Replies are rendered as Markdown with syntax-highlighted code blocks (with a copy button),
tables and images.

- ```` ```mermaid ```` blocks are drawn as diagrams.
- ```` ```svg ```` blocks and inline `<svg>` markup are shown as images.
- Click a drawing to enlarge it, copy its source or save it as an SVG file.
- Images from the web show as links unless **Load web images in replies** is on in
  [General](../settings/general.html#images). Loading them tells the image's server that the
  chat was opened, so it is off by default.
- **Save as…** (the download icon under the reply) saves it as a Word document, a PDF or an
  Excel workbook; see [Saving as files](#saving-as-files).

## Artifacts

Web pages, drawings, documents and longer code in a reply are *artifacts*. They show in the
panel beside the chat; the reply keeps only a link to each, with its name, type and size.

- **Click the link** to expand the artifact in the chat as well, and again to fold it away.
- **Open in panel** after the link shows that artifact in the panel ("in the panel" marks
  the one shown).
- **The panel button** at the top right, beside the ghost, shows and hides the panel (it opens
  on the newest artifact, and only shows in a chat that has one).
- **Cards** for the newest artifacts sit under the message box on the home screen, and the
  **Artifacts** page in the sidebar lists them all; a card opens its artifact in its chat.

- **What counts**: closed top-level code blocks marked ```` ```html ````, ```` ```svg ````,
  ```` ```mermaid ```` or ```` ```markdown ```` (an SVG or web page in an ```` ```xml ```` or
  unmarked block counts too), and other code blocks of 10 lines or more.
  Shorter code stays in the reply. A block still being written shows as code until it ends.
- **Names** come from a file name or `title="…"` after the language (```` ```python build.py ````),
  a page's or SVG's `<title>`, a document's first heading or a Mermaid diagram's title.
  Otherwise the name says what it is: "Sequence diagram", "Python code".
- **The panel** shows drawings and documents with **Preview** and **Code** tabs, and pages and
  code with syntax highlighting. **Copy**, **Save as…** and **Close** sit at the top. Click the
  title to pick any artifact in the chat.
- **Versions**: artifacts with the same given name and type are versions of one another (the
  same file name in a later reply, say); the arrows step between them, keeping the tab you are
  on. Made-up names like "Flowchart" never make versions.
- **Web pages** show live in the panel's **Preview** tab on macOS and Windows, in a web view
  that runs the page's scripts but, by default, blocks everything from the network (set in
  [General](../settings/general.html#web-page-preview)); links open in your browser. On Linux,
  **Open live preview** shows the page in a window of its own, with the same settings, when
  WebKitGTK is installed; the window follows the page as a reply streams. Without WebKitGTK it
  opens in your browser, with a notice. **Open in browser** writes the page to a temporary file
  and opens it in your default browser, where its scripts run as in any local file and it can
  load anything. SVGs can open there too.
- The panel opens by itself when a reply makes an artifact, and follows each new one while
  the newest is shown. Turn that off with **Open web pages, drawings and long code in a side
  panel** in [General](../settings/general.html#artifacts); **Open in panel** and the top-bar
  button still open it.
- **Save as…** in the panel's title row saves Markdown documents, code, and CSV or TSV
  artifacts as a Word document, a PDF or (for CSV and TSV) an Excel workbook.

## Saving as files

A reply or a Markdown, code or CSV artifact can be saved as a file from its **Save as…** menu.
Each format is switched on or off in [General](../settings/general.html#save-as-files); the menu
offers only the enabled ones that suit the content. The file dialog suggests the chat's name (or
the artifact's title) with the new extension.

- **Word (.docx)**: headings, paragraphs with bold, italic and `code`, links as their text
  followed by the address, bulleted and numbered lists (nested too), code blocks in a monospace
  font on a grey background, block quotes, tables with a bold header row, and rules.
- **PDF (.pdf)**: the same elements on A4 pages, with word wrap and page breaks. Text is set in
  Instrument Sans and code in IBM Plex Mono, both embedded. The PDF has no italic face, so
  italic text is upright. Characters the fonts lack (most non-Latin scripts) are left out.
- **Excel (.xlsx)**: offered when the reply has a table, and for CSV and TSV artifacts. Each
  Markdown table becomes a sheet named "Table 1", "Table 2" and so on, with a bold header row;
  a CSV or TSV file becomes one sheet, "Sheet1". Cells that read as numbers (plain digits with
  an optional sign and point) are stored as numbers, so they sum and sort; codes with leading
  zeros, numbers with separators or exponents, and numbers longer than 15 digits stay text.

Images in a reply are saved as their alt text and address, and raw HTML is left out. The files
are written only where you choose; nothing is sent anywhere.

## Attachments

Attach files with the **+** button or by dropping them on the window. Files up to 50 MB can be
attached; text beyond a million characters is cut, with a note saying so.

| Kind | Sent as |
|---|---|
| Images (PNG, JPEG, GIF, WebP) | images for vision models; click one to enlarge it |
| PDF (`.pdf`) | its text; a scanned PDF without a text layer is read by OCR when it is on ([Context & files](../settings/context.html#ocr-for-scanned-pdfs)), or attached with a note saying so |
| Word (`.docx`, `.doc`) | its text |
| Excel (`.xlsx`, `.xlsm`, `.xlsb`, `.xls`) | one CSV block per sheet |
| PowerPoint (`.pptx`, `.ppt`) | numbered slides, with speaker notes |
| OpenDocument (`.odt`, `.ods`, `.odp`) and RTF | their text |
| Any text file (code, Markdown, CSV, JSON, logs…) | inline, read as UTF-8, UTF-16 or Windows-1252 |

Documents are recognised by their content, not their extension. Password-protected files
can't be read.

## Running chats in the background

A reply keeps running when you open another chat, so you can start several and come back to them.
The sidebar row shows what each one is doing:

- a spinning hourglass: the reply is running;
- a shield: the reply is waiting for your approval of a tool;
- a clock: the reply is **queued**, because the limit on replies at once is reached;
- a check mark: the reply ended while you were in another chat, until you open it;
- a pin: the chat is pinned; a document: the chat has artifacts.

A [scheduled](#scheduled-tasks) run that starts when the limit is reached waits in the queue the
same way, and a run whose chat is still replying is skipped that time.

The chat shows **Queued** with how many replies are ahead of it. Stop cancels a queued reply.
When the window is in the background, or another chat is open, a desktop notification says when a
reply finishes, fails or needs approval. Each chat can switch its notifications off from the tools
menu. How many run at once, the notifications and a time limit are set in
[Background](../settings/background.html) settings.

With **Keep running in the menu bar or system tray when the window is closed** on (macOS and
Windows), closing the window hides it and replies keep running. Show the window again from the
icon's menu (on Windows, a left click on the icon also works); Quit there ends the app.

## Scheduled tasks

A scheduled task is a saved prompt that CleanRSTAI sends at set times: every so many minutes, every
day, or every week. Each run opens a new chat titled with the task's name and the time (or adds the
prompt to a chat you picked), and the reply starts like any other. Tasks run only while the app is
open; a run that was due while it was closed is skipped unless the task catches up.

Tasks are made in [Background](../settings/background.html#scheduled-tasks) settings, which lists
each task's prompt, schedule, approval mode, folder and provider. **Run scheduled task: …** in the
command palette starts one at once. Runs share the queue and the limits of background replies, and
a new chat starts read only unless the task says otherwise.

## Managing chats

Chats are saved automatically. In the sidebar:

- Click **Search** and type to filter chats (titles and text, in Chat and Code mode alike);
  **Esc** closes the box.
- Right-click a chat for **Pin** / **Unpin**, **Rename**, **Export as Markdown…**,
  **Share as HTML…** and **Delete**.

**Export as Markdown** includes tool calls, results and images. **Share as HTML…** writes the
same chat as a single, self-contained `.html` file styled like the app: it opens in any browser
with no network, fonts or scripts, so it is easy to send to someone. To back up every chat at
once, use **Export conversations…** in [General](../settings/general.html#export--import).

## Command palette

**⌘/Ctrl + K** opens a searchable list of commands: new chat, attach files, export this chat,
compact this chat, undo the last turn's file edits, show files changed in this chat, fork this
chat, set this chat's permissions (one entry per [mode](tools.html#permissions)), share this chat
as HTML, toggle the sidebar, go to Projects or Artifacts, back and forward, switch to Chat or
Code mode, voice mode on or off, change theme, open each settings page, export and import, check
for updates, switch provider or model, run a scheduled task now, and open any chat.

See [Keyboard shortcuts](../reference/keyboard-shortcuts.html) for the rest.
