---
title: Providers
parent: Settings
nav_order: 2
---

# Providers
{: .no_toc }

Endpoints the app can talk to: OpenAI-compatible Chat Completions, OpenAI's Responses API or
Anthropic's Messages API. You can keep several provider profiles and
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

### API

Which API the provider speaks:

| API | Request | Notes |
|---|---|---|
| **Chat Completions (OpenAI-compatible)** (default) | `POST /chat/completions` | Works with OpenAI and almost every other server. |
| **OpenAI Responses** | `POST /responses` | OpenAI's newer API, also served by some local servers. Shows reasoning summaries and keeps the model's (encrypted) reasoning between tool calls when a reasoning effort is set. Nothing is stored on OpenAI's side (`store: false`). Stop sequences aren't supported. |
| **Anthropic Messages** | `POST /messages` | Base URL `https://api.anthropic.com/v1`. Extended thinking (kept between tool calls, as the API requires) and prompt caching. The key goes in the `x-api-key` header. Without **Max tokens**, replies may use up to 8,192 tokens (plus the thinking budget). |

The server type's extra statistics still apply when they come from the server's own endpoints.

### Prompt caching

Shown for **Anthropic Messages** (default on). Marks the system prompt, the tools and the
conversation so far for caching, so later requests in the chat read them at a tenth of the
input price; writing the cache costs a quarter more, once. Cached tokens show in the stats.

### API key

Sent as a bearer token (or as `x-api-key` for the Anthropic Messages API). Leave it empty for local servers. **Show** reveals it. It is stored in
the keychain (see [Secrets](general.html#secrets)).

### Model

The model name to request. **Fetch models** calls `GET /models` and fills a drop-down with the
server's models; the list is also offered in the model picker.

### Server type

Decides which extra statistics are read:

| Type | What you get |
|---|---|
| **Generic OpenAI-compatible** | Token usage from the API; speeds measured by the app. |
| **llama.cpp server** | Prefill/decode speeds, context size and slot usage reported by llama.cpp, how full the context is ([context use](../using/chats.html#context-use)), a progress bar while it reads a long prompt ([prompt progress](../using/chats.html#prompt-progress)), and the model's details and loading or asleep status ([server details](../using/chats.html#llamacpp-server-details)). |
| **LiteLLM proxy** | The deployment that served each request and its cost (estimated from the proxy's listed prices for streamed replies), the model's context window and capabilities, and your key's spend and rate limits ([LiteLLM details](../using/chats.html#litellm-details)). |
| **Ollama** | Whether the model is loaded, its size and share in GPU memory, parameters, quantization, context size and capabilities ([details](../using/chats.html#ollama-lm-studio-vllm-and-openrouter)). |
| **LM Studio** | Whether the model is loaded, its quantization and format, and the context size it was loaded with. |
| **vLLM** | Context size, and the server's load: running and waiting requests, KV cache use and prefix cache hits. |
| **OpenRouter** | The provider that served each request and its cost, the model's context, prices and inputs, and your key's spend and credit left. |

**Detect** probes the server and picks the type for you.

### Context size

**Context size** (off by default) is the model's context size in tokens, for a server that
doesn't report it. Tick it and enter the size, from 1,024 to 10,000,000. A size the server
reports (llama.cpp, LiteLLM, Ollama, LM Studio, vLLM and OpenRouter) always wins over this one.
The size is used for [context compaction](../settings/context.html#context-compaction) and the
context-use figure in the chat's stats. When neither this nor the server gives a size, the
**Context size** setting under Context & files is used.

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

## Reasoning and sampling

Below the fields, options sent with each request. All are unset by default, so the server's own
default applies; tick an option's box to set it.

| Option | Range | Sent as |
|---|---|---|
| **Reasoning effort** | Off, Minimal, Low, Medium, High | `reasoning_effort` (Generic, LiteLLM, Ollama, LM Studio); `reasoning.effort` for OpenRouter and the Responses API; `chat_template_kwargs.enable_thinking` (plus `reasoning_effort`) for llama.cpp and vLLM; a thinking budget for Anthropic (1,024 / 2,048 / 8,192 / 24,576 tokens). **Off** asks the model not to think where the server can. |
| **Thinking budget** | 1,024 to 128,000 tokens | `thinking.budget_tokens` (Anthropic, LiteLLM and servers that take `thinking`), `reasoning.max_tokens` for OpenRouter. Not used by the Responses API, llama.cpp or vLLM. |
| **Temperature** | 0 to 2 | `temperature` (left out with Anthropic extended thinking, which doesn't allow it) |
| **Top-p** | 0 to 1 | `top_p` (also left out with Anthropic extended thinking) |
| **Max tokens** | 1 to 1,000,000 | `max_tokens` (`max_output_tokens` for Responses) |
| **Stop sequences** | one per line | `stop` (`stop_sequences` for Anthropic; not supported by Responses) |

Models that don't reason may reject a reasoning effort or budget: leave them unset for those.

**Only for *model*** sets options for the provider's current model alone; they win over the
provider's. Each chat can change them again from the sparkle menu under the message box (see
[Model options](../using/chats.html#model-options)). Subagents and compaction use the provider
and model options, not a chat's.

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

### Anthropic

1. **Base URL** `https://api.anthropic.com/v1`, **API → Anthropic Messages**.
2. Paste your Anthropic key.
3. **Fetch models**, then pick a model such as `claude-sonnet-4-5`. Set a **Reasoning effort**
   or **Thinking budget** to turn on extended thinking.
