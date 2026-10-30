---
title: Projects
parent: Using the App
nav_order: 9
---

# Projects

A project groups related chats in the sidebar and gives them shared context: instructions
and reference files that every chat in the project gets, and defaults for new chats (a
working folder, a provider, a model, tool switches and a permission mode).

Projects are on by default. Turn them off under **Settings → Context & files → Projects**:
the project list leaves the sidebar, and no project context is added to requests. The
projects and their chats stay on disk.

## Make a project

Click the **+** beside **Projects** in the sidebar, use **New project** in the command palette (⌘K),
or right-click the **Projects** heading. The editor has:

- **Name**, which the sidebar shows.
- **Instructions**: text added to the system prompt of every chat in the project, such as
  "Answer in French" or "This is a Rust crate; run `cargo fmt` after edits".
- **Files**: reference files whose text is sent with every request in the project. Documents
  (PDF, Office, OpenDocument, RTF) are read as text, as in a chat's attachments. Images are left
  out, and the model is told so.
- **Folder**: the working folder a new chat in the project starts with.
- **Provider** and **Model**: what a new chat starts with. Leave the provider on
  "Settings' active provider" to use the one you picked under the message box.
- **Permissions**: the permission mode a new chat starts with (see [Tools and approvals](tools.html)).
- **Tool switches**: the switches a new chat starts with. They are set from a chat, see below.

The defaults for a new project (its instructions, folder and provider) come from
**Settings → Context & files → Projects**.

## Chats in a project

- A **+** on the project's row starts a new chat in it. The chat takes the project's folder,
  provider, model, tool switches and permission mode when it is created, as a copy: changing
  them in the chat afterwards doesn't change the project.
- Click a project's name to fold or unfold its chats. The state is kept until the app quits.
- A chat's right-click menu has **Move to project** (and **Remove from project**), which moves
  the chat into or out of a project. The chat keeps its own folder, provider and switches.
- A chat with a provider or model of its own shows it in place of the model picker, with the
  provider's name. Pick another provider or model there, or **Use Settings' provider and
  model** to go back. The model picker's menu has **Use this model for this chat only**, which
  does the same for any chat.
- **Use this chat's tool switches for its project** (the chat's right-click menu) copies the
  chat's tool switches to its project, for the chats that start in it afterwards.
- In a chat in a project, the tools menu has a **Project context** switch, to leave the
  project's instructions and files out of that chat only.

Chats that aren't in a project stay in the list under **Pinned** and **Recents**, as before.
While you search, the sidebar lists every matching chat together.

## What the model gets

For each request in a project's chat, the system prompt gets a section that starts
`Project: <name>`, then the instructions, then the text of each file, each in a
`<project-file>` tag. The total text of the files is limited by **Files size limit** (256 KB by
default, 16 to 4096 KB); when the files are longer, the shorter ones keep their whole text and
the longer ones are cut. The model is told which files were left out or cut.

The files are read again for each request, so an edit to a file shows up in the next reply. A
file that has been moved or deleted is noted as unreadable.

## Projects on the command line

`cleanrstai exec --project NAME` (a name, ignoring case, or the project's id) starts the run in
the project: its folder, provider, model, tool switches, permission mode and context. Flags
given with it win: `--workspace`, `--provider`, `--model` and `--mode` replace the project's.
See [Command line](command-line.html).

Scheduled tasks that run in a chat of a project get the project's context the same way.

## Export and import

Right-click a project and choose **Export…** to save it as one `.cleanrstai-project.json` file.
The file holds the project and all its chats, with their messages, in plain JSON, so keep it
as private as the chats themselves. Its file paths are kept as they are: a file or folder of the
project may not exist on another computer.

**Import project…** (right-click the **Projects** heading, or the command palette) reads such a file.
The project and its chats get new ids, so importing the same file twice gives two projects,
and the chats keep their titles and messages. The file edits in an imported chat can no longer
be undone from there, since their undo checkpoints stayed on the other computer; their diffs
are kept.

## Delete a project

Right-click a project and choose **Delete**, then confirm. The project's instructions, files
and defaults are removed, but its chats stay in the list, outside any project.
