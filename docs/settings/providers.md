---
title: Providers
parent: Settings
nav_order: 2
---

# Providers
{: .no_toc }

OpenAI-compatible endpoints the app can talk to. You can keep several provider profiles and
switch between them from the model picker under the message box or with **⌘/Ctrl + K**.

1. TOC
{:toc}

## The provider list

The left column lists your profiles; **●** marks the active one. Below it:

- **+** adds a provider.
- **⧉** duplicates the selected provider (handy for the same endpoint with another model or
  settings).
- **－** removes it (you always keep at least one).

**Use this provider** at the bottom of the page makes the selected profile the active one.

## Fields

### Name

Shown in the model picker and the command palette.

### Base URL

The API root, ending in `/v1` (without `/chat/completions`). **Presets** fills in a known one:

| Preset | Base URL |
|---|---|
| OpenAI | `https://api.openai.com/v1` |
| OpenRouter | `https://openrouter.ai/api/v1` |
| Groq | `https://api.groq.com/openai/v1` |
| Mistral | `https://api.mistral.ai/v1` |
| DeepSeek | `https://api.deepseek.com/v1` |
| Together | `https://api.together.xyz/v1` |
| Gemini (OpenAI compat) | `https://generativelanguage.googleapis.com/v1beta/openai` |
| Ollama (local) | `http://localhost:11434/v1` |
| LM Studio (local) | `http://localhost:1234/v1` |
| llama.cpp / vLLM (local) | `http://localhost:8000/v1` |

### API key

Sent as a bearer token. Leave it empty for local servers. **Show** reveals it. It is stored in
the keychain (see [Secrets](general.html#secrets)).

### Model

The model name to request. **Fetch models** calls `GET /models` and fills a drop-down with the
server's models; the list is also offered in the model picker.

### Temperature

Off by default, so the server's own default is used. Turn it on to send a value from 0 to 2.

### Max tokens

Off by default. Turn it on to cap the reply length (1 to 1,000,000 tokens).

### Server type

Decides which extra statistics are read:

| Type | What you get |
|---|---|
| **Generic OpenAI-compatible** | Token usage from the API; speeds measured by the app. |
| **llama.cpp server** | Prefill/decode speeds, context size and slot usage reported by llama.cpp, how full the context is ([context use](../using/chats.html#context-use)), a progress bar while it reads a long prompt ([prompt progress](../using/chats.html#prompt-progress)), and the model's details and loading or asleep status ([server details](../using/chats.html#llamacpp-server-details)). |
| **LiteLLM proxy** | The model that served each request and its cost (estimated from the proxy's listed prices for streamed replies). |

**Detect** probes the server and picks the type for you.

### Token usage

**Request usage in streamed responses (stream_options)** (default on). Needed for exact token
counts and speeds; turn it off if the server rejects the request.

### Streaming

**Stream responses** (default on). Turn it off if the server doesn't support Server-Sent
Events; replies then arrive all at once.

### Timeout

How long to wait for a response: 10 to 3600 seconds, default 300.

### Extra headers

Header names and values added to every request, for example an organisation or project header
or a proxy's own key. Values are stored in the keychain.

## Examples

### Ollama

1. **Presets → Ollama (local)**.
2. Leave **API key** empty.
3. **Fetch models** and pick one you have pulled, such as `llama3.2`.

### OpenRouter

1. **Presets → OpenRouter**.
2. Paste your OpenRouter key.
3. **Fetch models**, then pick a model such as `anthropic/claude-sonnet-4`. OpenRouter reports
   the cost of each reply, which shows in the stats.
