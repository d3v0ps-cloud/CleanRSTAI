---
title: Background
parent: Settings
nav_order: 9
---

# Background
{: .no_toc }

How replies that keep running in the background are notified, how many run at once, how long
they may run, and the scheduled tasks that send a prompt at set times. See
[Using the App → Running chats in the background](../using/chats.html#running-chats-in-the-background)
for how it looks in a chat, and [Scheduled tasks](../using/chats.html#scheduled-tasks) for how the
tasks behave.

| Setting | Default | Description |
|---|---|---|
| **Notify when a reply finishes, fails or needs approval** | on | Sends a desktop notification titled CleanRSTAI, such as "Trip plan: reply finished", "Trip plan: needs approval for write_file" or "Trip plan: failed". Each chat can switch its own notifications on or off from the tools menu (**Notifications**), which starts from this setting. |
| **Only while the CleanRSTAI window is in the background** | on | Notify only when the window does not have focus. When off, a notification is also sent for a chat other than the one open. |
| **Replies at once** | 4 | How many chats can have a reply running at the same time (1–32). |
| **Time limit** | 0 min | Stop a reply that has run this many minutes (0–1440). 0 means no limit. |

## Scheduled tasks

A scheduled task sends a saved prompt into a chat at set times. Tasks run only while CleanRSTAI is
open. **Run scheduled tasks** is the master switch; each task has its own **On** switch.

| Setting | Default | Description |
|---|---|---|
| **Name** | Scheduled task | The name of the task. New chats it makes are titled with the name and the time they started. |
| **Prompt** | empty | The message the task sends. A task with no prompt never runs. |
| **Repeats** | Every day | **Every so many minutes** (5–10080, the minutes between runs), **Every day** or **Every week** (on a weekday), at a time of day in local time. |
| **Runs in** | A new chat each time | **A new chat each time**, or a chat you picked from the list. A new chat takes the task's folder and provider; a picked chat keeps its own folder and provider, and its approval mode is set to the task's on each run. |
| **Approvals** | Read only | The approval mode of the run's chat: read only, default, auto in workspace, or allow all. Read only is the default so a task cannot change files or run commands unless you choose it. Other tools still ask, so a run can wait for your answer; the notification says so. |
| **Folder** | None | The working folder of a new chat (see [Built-in tools and workspace](../using/tools.html#workspace)). |
| **Provider** | The active provider | The provider the run uses, for example a local model for a frequent task. |
| **Catch up after the app was closed** | off | A run that was due while the app was closed runs once when the app starts. When off, it is skipped to the next time. |

**Run now** starts the task at once, using what the form shows (unsaved edits included), and counts
as its last run. Each task shows its next run under it.

The timing works like this. An interval counts from the task's last run, or, before its first run,
from when it was saved, so the first run is one interval after saving. A daily or weekly task runs
at its time on its days. A run that is due is started within about a minute and a half; a run that
is missed while the app was closed or the computer slept is skipped, unless **Catch up** is on. On
a day when the clocks go forward, a time that does not exist (such as 02:30) runs an hour later.

Runs go through the same queue, parallel limit and time limit as replies you send (see below). A
run that is still replying when its time comes is skipped that time. If a task's chat has been
deleted, the task is switched off and CleanRSTAI says so.

Each run sends its prompt to the provider without anyone watching. Read the approval mode and the
folder before you turn a task on.

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
