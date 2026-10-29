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

![A chat working in a project folder: the model searches the code, reads a file and suggests a fix]({{ '/screenshot.png' | relative_url }})

## Highlights

- **Any OpenAI-compatible API**: OpenAI, OpenRouter, Groq, Mistral, DeepSeek, Together, Gemini,
  or local servers such as Ollama, LM Studio, llama.cpp and vLLM.
- **Streaming replies** with speed and token stats, and a *Thought process* section for
  reasoning models.
- **Skills**: folders of instructions the model loads when a task matches, from your skills folder or a chat's workspace
- **Built-in tools**: time and date, calculator, task list and web fetch, with no MCP server
  needed.
- **Workspace**: give a chat a folder and the model can read, search, edit and run commands in
  it, asking before anything that writes.
- **MCP client**: stdio and Streamable HTTP servers, OAuth sign-in, resources and prompts.
- **Subagents**: the model can hand independent tasks to agents that work in parallel.
- **Artifacts**: web pages, SVGs, Mermaid diagrams, Markdown documents and longer code from a
  reply open in a side panel, with a link in the chat.
- **Attachments**: images for vision models; PDF, Office, OpenDocument, RTF and text files as
  text.
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
| Let the model use tools or a folder | [Tools and approvals](using/tools.html) |
| Add MCP servers | [MCP servers](using/mcp.html) |
| Run the model's commands in a container | [Docker sandbox](settings/docker.html) |
| Use your chats from a phone or another computer | [Remote access](using/remote-access.html) |
| Reach your chats from anywhere | [Cloudflare Tunnel](using/cloudflare-tunnel.html) |
| Look up a setting | [Settings](settings/) |
