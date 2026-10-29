---
title: Background
parent: Settings
nav_order: 9
---

# Background
{: .no_toc }

How replies that keep running in the background are notified, how many run at once, and how long
they may run. See [Using the App → Running chats in the background](../using/chats.html#running-chats-in-the-background)
for how it looks in a chat.

| Setting | Default | Description |
|---|---|---|
| **Notify when a reply finishes, fails or needs approval** | on | Sends a desktop notification titled CleanRSTAI, such as "Trip plan: reply finished", "Trip plan: needs approval for write_file" or "Trip plan: failed". Each chat can switch its own notifications on or off from the tools menu (**Notifications**), which starts from this setting. |
| **Only while the CleanRSTAI window is in the background** | on | Notify only when the window does not have focus. When off, a notification is also sent for a chat other than the one open. |
| **Replies at once** | 4 | How many chats can have a reply running at the same time (1–32). |
| **Time limit** | 0 min | Stop a reply that has run this many minutes (0–1440). 0 means no limit. |

## Queueing

A reply that starts while **Replies at once** replies are already running waits in a queue. Its
chat shows **Queued** where the reply would appear, and its row in the sidebar shows a clock. Queued
replies start in the order they were sent, as running ones finish or stop. The queue also holds
[Compact now](../using/chats.html#long-chats) and review runs.

Stop on a queued chat takes it out of the queue; its prompt stays in the chat, marked as stopped,
as it does for a reply that was stopped while running. Deleting a chat does the same.

## Time limit

The time counts from when a reply starts, including any time it waits for your approval. When it
runs out, the reply is stopped and the chat shows the error "Stopped after 15 minutes (Settings →
Background)" with a **Retry** button.

## Notifications

Notifications go through the system: Notification Center on macOS, Windows notifications, and the
desktop notification service on Linux (D-Bus). The text is the chat's title and what happened, not
the reply. On macOS and Windows the notification may be shown on your other devices if you have the
system set up to mirror notifications. A notification the system refuses, or a Linux desktop with no
notification service, is logged and skipped; the app keeps working.
