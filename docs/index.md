---
title: Home
nav_order: 1
---

# CleanRSTAI

A fast, native desktop AI chat client written in Rust. It talks to any **OpenAI-compatible**
endpoint and gives the model tools through **MCP servers**, built-in tools, subagents and a
per-chat workspace folder.

macOS is the primary platform and Linux is fully supported. Windows builds are released
too, as an installer and a portable `.exe`.

![A chat in Code mode: the model searches the code, reads a file and suggests a fix]({{ '/screenshot.png' | relative_url }})

## Highlights

- **A calm, dark-first design**: a sidebar with Search, Projects, Artifacts, Scheduled agents
  and your recent chats; a home screen with a greeting, the message box in the middle and your
  newest artifacts; back and forward through what you opened; a **Chat / Code** switch (Code
  lists the chats that work in a folder); Settings as a dimmed window over the app, with an
  Appearance page and **Reduce motion**.
- **Scheduled agents**: prompts that run on a schedule (every so many minutes, daily or
  weekly), each in a chat of its own, with their recent runs on the **Scheduled** page.
- **Projects**: group chats with shared instructions, files, a folder, a model and tool
  switches.
- **Memory**: facts the model saves about you and uses in later chats (off by default), and
  incognito chats that use none.
- **Permission modes per chat**: ask, auto in the folder, allow all, read only, or **Plan**
  (the model proposes a plan, and it runs once you approve).
- **Branches**: regenerate or edit a message and keep every version, with arrows to switch.
- **Voice**: dictate with the microphone (and pick the input device), have replies read aloud,
  or talk back and forth in **voice mode**.
- **Save replies as files**: Word, PDF or Excel from a reply or an artifact.
- **Any OpenAI-compatible API**: OpenAI, OpenRouter, Groq, Mistral, DeepSeek, Together, Gemini,
  or local servers such as Ollama, LM Studio, llama.cpp and vLLM.
- **Streaming replies** with speed and token stats, and a *Thought process* section for
  reasoning models.
- **Skills**: folders of instructions the model loads when a task matches, from your skills folder or a chat's workspace
- **Hooks**: commands that run before or after a tool call, or when a reply is done, set in Settings and switched per chat
- **Built-in tools**: time and date, calculator, task list, web fetch and web search (or the
  provider's own search with Anthropic, OpenAI and OpenRouter), with no MCP server needed.
- **Workspace**: give a chat a folder and the model can read, search, edit and run commands in
  it, asking before anything that writes; every edit shows a diff and can be undone, and the
  model can commit the changes (off by default, never pushes).
- **MCP client**: stdio and Streamable HTTP servers, OAuth sign-in, resources and prompts.
- **Subagents**: the model can hand independent tasks to agents that work in parallel.
- **Background replies**: keep working in other chats while one replies; desktop notifications
  when a reply finishes or needs approval; a limit on replies at once, and a time limit.
- **Artifacts**: web pages, SVGs, Mermaid diagrams, Markdown documents and longer code from a
  reply open in a side panel, with a link in the chat.
- **Attachments**: images for vision models; PDF, Office, OpenDocument, RTF and text files as
  text, and scanned PDFs read with OCR.
- **Docker sandbox**: run the model's shell commands in a container with only the chat's
  folder, resource limits, and no network or only the hosts, ports and protocols you allow;
  subagents can get containers of their own.
- **Remote access in the browser**: use your chats from a phone or another computer on your
  network, with a password and HTTPS; models, tools and approvals stay on this computer. A
  Cloudflare Tunnel makes it reachable from anywhere.
- **Private by default**: API keys and secrets live in the system keychain, and conversations
  are plain JSON files on your computer.

## Where to start

| If you want to… | Read |
|---|---|
| Install the app | [Installation](getting-started/installation.html) |
| Connect a model and send a first message | [Your first chat](getting-started/first-chat.html) |
| Learn the chat window | [Chats](using/chats.html) |
| Run a prompt on a schedule | [Scheduled agents](using/chats.html#scheduled-agents) |
| Group chats with shared instructions | [Projects](using/projects.html) |
| Talk instead of typing | [Voice](using/voice.html) |
| Change the theme, size or motion | [Appearance](settings/appearance.html) |
| Let the model use tools or a folder | [Tools and approvals](using/tools.html) |
| Add MCP servers | [MCP servers](using/mcp.html) |
| Run the model's commands in a container | [Docker sandbox](settings/docker.html) |
| Use your chats from a phone or another computer | [Remote access](using/remote-access.html) |
| Reach your chats from anywhere | [Cloudflare Tunnel](using/cloudflare-tunnel.html) |
| Look up a setting | [Settings](settings/) |
