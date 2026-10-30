---
title: Your first chat
parent: Getting Started
nav_order: 2
---

# Your first chat

CleanRSTAI starts with one provider profile called **OpenAI**, pointing at
`https://api.openai.com/v1` with the model `gpt-4o-mini` and no API key. Point it at the
service you want to use before sending a message.

## 1. Set up a provider

1. Open **Settings** from the menu on your name at the bottom of the sidebar, or with
   **⌘/Ctrl + ,**, then choose **Providers**.
2. Pick a **Base URL** from **Presets** (OpenAI, OpenRouter, Groq, Mistral, DeepSeek, Together,
   Gemini, Ollama, LM Studio, llama.cpp / vLLM) or type your own. It is the URL that ends in
   `/v1`, without `/chat/completions`.
3. Paste your **API key**. Leave it empty for local servers such as Ollama or LM Studio.
4. Click **Fetch models** to load the server's model list, then pick one, or type the model
   name.
5. Click **Save & Close**.

The key goes to the system keychain, not to `settings.json`. See
[Providers](../settings/providers.html) for every field.

{: .tip }
> Running llama.cpp or a LiteLLM proxy? Click **Detect** next to **Server type** to get
> llama.cpp's own speed figures or LiteLLM's cost and served-model information.

## 2. Send a message

![A new chat: greeting, message box and model picker]({{ '/new-chat.png' | relative_url }})

A new chat opens on a greeting with the message box in the middle. The model picker under the
box shows the current provider and model; click it to switch.

Type a message and press **Enter** (**Shift+Enter** adds a new line). The reply streams in,
and the speed and token counts appear under it when it's done.

## 3. Try the tools

The built-in tools are on by default, so you can ask straight away:

- "What's the date today, and what time is it in Tokyo?" uses **Time & date**.
- "What is 17.5% of 2,340, rounded to 2 places?" uses the **Calculator**.
- "Summarise https://example.com" uses **Web fetch**, which asks for your approval first.

To let the model work with files, click **Project or folder** under the message box and pick a
folder, or switch the title row to **Code**. See [Tools and approvals](../using/tools.html).

## Next steps

- [Chats](../using/chats.html): attachments, editing, search and export.
- [MCP servers](../using/mcp.html): connect more tools.
- [Settings](../settings/): everything else you can change.
