---
title: Docker sandbox
parent: Settings
nav_order: 6
---

# Docker sandbox
{: .no_toc }

1. TOC
{:toc}

The sandbox runs the workspace shell tool, **Run command** (`exec_shell_command`), in a Docker
container instead of on this computer. It is off by default. The other workspace tools, such as
`read_file` and `edit_file`, still run in the app, as before. See
[Tools and approvals](../using/tools.html#workspace) for the workspace tools.

## What it does

- Only shell commands run in the container. Each command runs with `sh -c` in the chat's
  workspace folder, which is mounted at `/workspace`.
- Each chat has one container, and every command in that chat reuses it. Packages a command
  installs stay available to later commands in the same chat until the container is removed.
- A container is removed when:
  - it has not been used for 15 minutes;
  - its chat is deleted;
  - the sandbox settings change;
  - the app quits;
  - its chat's workspace folder changes.
- Containers left behind by an earlier run of the app are removed when the app starts.
- A command that runs past the command's time limit stops its container. Files in the folder
  are not touched. The next command in that chat starts a fresh container, so packages installed
  in it are gone.

## Requirements

- Docker Desktop, Docker Engine, or a compatible runtime such as OrbStack or Colima.
- The `docker` command must work in a terminal. The app runs `docker` itself, so Docker
  contexts and the `DOCKER_HOST` variable apply.

Use **Test** in the Status row to check. It reports the Docker version and whether the image is
present.

## Settings

| Setting | Default | Description |
|---|---|---|
| **Sandbox** | off | Ticked, the box reads "Run shell commands in a Docker container". Turning it on or off applies to new commands. |
| **Image** | Docker image | **Docker image** uses an image you name. **Custom Dockerfile** builds one from the text you write. See [Images](#images). |
| **Get the image** | Pull when missing | Shown for **Docker image**. **Pull when missing**, **Pull every time the app starts**, or **Only use an image already on this computer**. |
| **Network** | None (no network) | **None (no network)** or **Normal network**. See [Limits and hardening](#limits-and-hardening). |
| **CPUs** | 2 | How many CPUs a container may use, from 0.25 to 64, in steps of 0.25. |
| **Memory** | 2048 MB | How much memory a container may use, from 256 to 262144 MB. |
| **Processes** | 512 | How many processes a container may run at once, from 32 to 65536. |
| **Workspace folder** | Mount read-write | Tick **Mount read-only (commands can't change files)** to stop commands changing the folder. |
| **docker** | found on `PATH` | The path to the `docker` command. Leave it empty to find `docker` on `PATH`, or type a path or use **Browse…**. |

Settings changes apply to new commands. Changing the sandbox settings removes the running
containers.

## Images

### Docker image

Type any image name and tag, such as `python:3.13-slim` (the default) or `node:22`. The app runs
the image as typed, so it can be:

- an image from Docker Hub or another public registry;
- a private registry image, if you have run `docker login` for that registry;
- an image that only exists on this computer.

**Get the image** sets how the app obtains it:

- **Pull when missing** (default): the app uses the image if it is on this computer, and pulls
  it when it is not.
- **Pull every time the app starts**: the app pulls the image before the first command after
  the app starts.
- **Only use an image already on this computer**: the app never pulls. If the image is missing,
  commands are refused until you pull it yourself.

**Pull now** pulls the image straight away.

### Custom Dockerfile

Write the Dockerfile in the editor. The app builds it as `cleanrstai-sandbox:<hash>`, where the
hash comes from the Dockerfile text. The image is built the first time it is needed, and again
automatically whenever the text changes.

{: .warning }
> There is no build context. `COPY` and `ADD` of files from your computer don't work, because
> the Dockerfile is sent to Docker on its own. Install what you need with `RUN` instead, and
> start from any image with `FROM`.

For example, to add Node.js and Git to the default image:

```dockerfile
FROM python:3.13-slim
RUN apt-get update \
 && apt-get install -y --no-install-recommends nodejs npm git \
 && rm -rf /var/lib/apt/lists/*
```

**Reset to example** puts back the default Dockerfile. **Build now** builds the image again
from scratch, pulling the base image and ignoring the cache (`docker build --pull --no-cache`).
Use it to update an old base image.

## Limits and hardening

In plain words, each container:

- has **no network access** by default. With **Normal network** it can reach the internet;
- is limited to the **CPUs**, **Memory** and **Processes** you set;
- gets no extra Linux capabilities, and cannot gain privileges through `setuid` programs;
- runs as your user on Linux, so files it creates in the folder belong to you;
- sees only the workspace folder, mounted at `/workspace`. Nothing else from your computer is
  mounted;
- gets no environment variables from your computer. API keys and other secrets are not passed
  in.

## When Docker isn't available

Commands are never run on your computer instead. The command is refused with a message that
says what to do:

- Docker isn't installed: "Docker isn't installed: install Docker Desktop or Docker Engine, or
  set its location in Settings → Docker."
- Docker isn't running: "Docker isn't running: start Docker Desktop (or the Docker service) and
  try again."
- The image is missing and **Get the image** is **Only use an image already on this
  computer**: "The image <image> isn't on this computer: pull it, or choose "Pull when missing"
  in Settings → Docker."
- Pulling fails: "Couldn't pull <image>: <last line of Docker's error>"
- Building fails: "Couldn't build the sandbox image: <last line of Docker's error>"

The message is returned as the command's error, so the model sees it and can tell you.

## Status

The **Status** row at the bottom of the page has these buttons:

- **Test** checks that Docker is running and the image is present. A successful result looks
  like "Docker 27.3.1 is running · image python:3.13-slim is present". A failure shows the
  error message.
- **Pull now** (for **Docker image**) or **Build now** (for **Custom Dockerfile**) prepares the
  image. A spinner shows while it works, and the result appears under the buttons.

Pulling and building have their own limit of 15 minutes. That time does not count against a
command's time limit.

## What the sandbox protects against, and what it doesn't

The sandbox limits what a command can reach. It does not make an untrusted command safe.

**It protects against:**

- commands reading or changing files outside the workspace folder;
- commands using your network, when the network is set to **None (no network)**;
- commands using more than the CPU, memory and processes you allow.

**It doesn't protect against:**

- files inside the workspace folder. Commands can read and change them, unless **Mount
  read-only** is ticked;
- your network, when **Normal network** is on. Commands can reach anything your computer can;
- what the image itself contains. Use images you trust;
- bugs in Docker or the Linux kernel that let a program escape a container;
- the other tools. The file tools, MCP servers and web fetch still run on your computer and are
  not sandboxed.

On macOS and Windows, Docker runs in a virtual machine, so containers run inside that machine
rather than on your computer's own system.

See also [Tools and approvals](../using/tools.html) and
[Data and privacy](../reference/data-and-privacy.html).
