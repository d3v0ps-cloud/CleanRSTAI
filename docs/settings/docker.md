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
- Each chat has one container, named `cleanrstai-<chat id>` and labelled
  `net.bukn.cleanrstai.sandbox=1`. Every command in that chat reuses it. Packages a command
  installs stay available to later commands in the same chat until the container is removed.
- A container is removed when:
  - it has not been used for 15 minutes (the app checks once a minute);
  - a command in it runs past its time limit, or the command is stopped;
  - its chat is deleted;
  - the sandbox settings change (a container still running a command is removed when that
    command ends);
  - the app quits;
  - its chat's workspace folder changes. The next command in that chat starts a new container.
- Containers left behind by an earlier run of the app are removed when the app starts, if the
  sandbox is on.
- A command that runs past its time limit (120 seconds by default, at most 600) stops its
  container. Files in the folder are not touched. The next command in that chat starts a fresh
  container, so packages installed in it are gone.

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
| **Image name** | `python:3.13-slim` | Shown for **Docker image**. Any image name and tag. An empty name is replaced by the default. |
| **Get the image** | Pull when missing | Shown for **Docker image**. **Pull when missing**, **Pull every time the app starts**, or **Only use an image already on this computer**. |
| **Dockerfile** | An example | Shown for **Custom Dockerfile**. The Dockerfile text, with **Reset to example** to put back the default. |
| **Network** | None (no network) | **None (no network)**, **Normal network (can reach this computer, your local network and the internet)** or **Only the hosts and ports below**. See [Custom network](#custom-network) and [Limits and hardening](#limits-and-hardening). |
| **Allowed outbound** | none | With **Only the hosts and ports below**: the rules, each a host, a protocol and ports. See [Custom network](#custom-network). |
| **CPUs** | 2 | How many CPUs a container may use, from 0.25 to 64. |
| **Memory** | 2048 MB | How much memory a container may use, from 256 to 262144 MB. |
| **Processes** | 512 | How many processes a container may run at once, from 32 to 65536. |
| **Workspace folder** | Read-write | Tick **Mount read-only (commands can't change files)** to stop commands changing the folder. |
| **docker** | found on `PATH` | The path to the `docker` command. Leave it empty to find `docker` on `PATH`, or type a path or use **Browse…**. |
| **Status** | | **Test**, and **Pull now** or **Build now**. See [Status](#status). |

Values outside a range are kept within it. The sandbox applies to new commands; settings
changes remove the running containers.

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
  the app starts. If that pull fails (for example, when you are offline) and the image is
  already on this computer, the app uses that copy.
- **Only use an image already on this computer**: the app never pulls. If the image is missing,
  commands are refused until you pull it yourself.

**Pull now** pulls the image straight away.

### Custom Dockerfile

Write the Dockerfile in the editor. The app builds it as `cleanrstai-sandbox:<hash>`, where
`<hash>` is the first 12 hex digits of the SHA-256 of the Dockerfile text. The image is built the
first time it is needed, and again automatically whenever the text changes.

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

Two examples are a click away under the editor:

- **Small example**: the default, which adds `git`, `curl` and `ca-certificates` to
  `python:3.13-slim`.
- **Full example**: Ubuntu 24.04 with `bash`, Python 3 (with `requests`, `httpx`, `pyyaml`,
  `numpy` and `pandas` in a virtual environment), Node.js and npm, build tools (`gcc`, `make`,
  `cmake`, `ninja`, autotools, `gdb` and common `-dev` libraries), everyday tools (`git`, `curl`,
  `wget`, `jq`, `zip`, `sqlite3`, `rsync`, `ssh`, …) and network tools (`ping`, `traceroute`,
  `nmap`, `dig`, `nc`, `socat`, `ip`, `whois`). The image is about 1.5 GB, and its first build
  takes a few minutes. The same file is in the repository as
  [`assets/sandbox/full.Dockerfile`](https://github.com/d3v0ps-cloud/CleanRSTAI/blob/main/assets/sandbox/full.Dockerfile).

In the full example, `pip install` and `npm install -g` work as the sandbox user: they install
into `/opt/venv` and `/opt/npm`, which anyone in the container can write. `apt-get install`
doesn't work at run time (on Linux commands run as your user), so add system packages to the
Dockerfile. Because commands run without Linux capabilities, `nmap` can only do connect scans
(`-sT`, with `-Pn` to skip its ping) and `tcpdump` can't capture.

**Build now** builds the image again from scratch,
pulling the base image and ignoring the cache (`docker build --pull --no-cache`). Use it to
update an old base image.

## Custom network

**Only the hosts and ports below** lets commands make only the outbound connections you list,
such as `pypi.org` on port 443 to install packages. Everything else is refused at once
(connections fail with "connection refused" rather than hanging).

Click **Add rule** for each connection to allow. A rule has:

| Field | What to enter |
|---|---|
| Host | A domain such as `pypi.org` (it covers its subdomains too, so `*.pypi.org` means the same), an IPv4 address such as `192.168.1.20`, a range such as `10.0.0.0/8`, or `*` for any address. |
| Protocol | **TCP**, **UDP**, **ICMP** or **Any**. **ICMP** allows `ping` (and the replies `traceroute` needs) and takes no ports. **Any** with ports means TCP and UDP on those ports; **Any** without ports allows every protocol, ICMP included. |
| Ports | Ports and ranges separated by commas, such as `80, 443, 8000-8100`. Empty allows every port. |

A rule with a mistake shows what is wrong under it, and commands are refused with "Network rule
<n>: …" until it is fixed. With no rules at all, the container has no network, like **None**.

Some useful rules:

| To | Host | Protocol | Ports |
|---|---|---|---|
| Install Python packages with pip | `pypi.org` and `files.pythonhosted.org` | TCP | 443 |
| Install npm packages | `registry.npmjs.org` | TCP | 443 |
| Clone from GitHub | `github.com` and `codeload.github.com` | TCP | 443 |
| Reach a server on your network | `192.168.1.20` | TCP | 8080 |
| Ping a host | `1.1.1.1` (or a domain) | ICMP | (none) |
| Allow the web, nothing else | `*` | TCP | 80, 443 |

### How it works

The app starts a small companion container, named like the sandbox container with `-net` at the
end, from an image it builds itself (`cleanrstai-sandbox-net:<hash>`, Ubuntu with `iptables`,
`ipset` and `dnsmasq`; the first build needs internet access and takes a minute). The sandbox
container shares its network, and:

- the companion's firewall drops every outbound connection except the rules, and refuses
  incoming connections;
- the companion answers DNS for the sandbox. Only the domains in the rules resolve; other names
  get "not found", unless a rule allows `*`. Each address a domain resolves to is added to that
  rule, so it keeps working when the address changes;
- commands can't reach any DNS server themselves, so they can't send data out through DNS
  lookups of other names;
- the sandbox has no Linux capabilities, so commands can't change the firewall. Only the
  companion has the network capabilities it needs to set it up.

The companion is removed with the sandbox container. **Test** also says whether its image is
built, and **Pull now** / **Build now** rebuilds it too.

Limits:

- IPv4 only. IPv6 is blocked, and domains resolve to IPv4 addresses only.
- `ping` works without extra permissions in the container (Docker allows unprivileged ICMP),
  but the image must have it: `python:3.13-slim` doesn't. The **Full example** Dockerfile
  includes it. In your own Dockerfile, install `iputils-ping` and then run
  `setcap -r /usr/bin/ping` (from `libcap2-bin`): a program with file capabilities can't start
  in the sandbox, which runs with none.
- Rules work on addresses, not names. A domain served from a shared address (a CDN, for
  example) also lets commands reach other sites on that same address.
- A domain resolves through your computer's DNS servers, as Docker sets them up.
- The rules apply when a chat's container is created. After changing them, click **Apply**; the
  running containers are removed and the next command starts a new one.

## Subagent containers

With **The workspace tools, with commands in their own sandbox container** on in
[Settings → Subagents](subagents.html), each subagent that runs a command gets a container of
its own, named like the chat's with `-s` and a short id at the end. It uses the same image,
limits and **Mount read-only** setting, has the chat's folder at `/workspace`, and is removed
when the subagent finishes, fails or runs out of time. Subagents run their commands in
parallel, each in its own container.

On a [custom network](#custom-network) all of a chat's containers, its own and its subagents',
share the chat's one network companion, so the same rules apply to all of them. The companion
is removed with the chat's last container (after 15 idle minutes at the latest).

## Limits and hardening

In plain words, each container:

- has **no network access** by default. With **Normal network (can reach this computer, your local network and the internet)** it can reach the internet; with **Only the hosts and ports below** only what the [rules](#custom-network) allow;
- is limited to the **CPUs**, **Memory** and **Processes** you set. Swap is not extra: the
  memory limit is also the swap limit;
- has no Linux capabilities (`--cap-drop ALL`), and cannot gain privileges through `setuid`
  programs (`no-new-privileges`);
- runs as your user on Linux, so files it creates in the folder belong to you;
- sees only the workspace folder, mounted at `/workspace`. Nothing else from your computer is
  mounted;
- gets no environment variables from your computer, so API keys and other secrets are not
  passed in. Only `HOME` is set, to `/tmp`.

## When Docker isn't available

Commands are never run on your computer instead. The command is refused with a message that
says what to do:

- Docker isn't installed: "Docker isn't installed: install Docker Desktop or Docker Engine, or
  set its location in Settings → Docker."
- The **docker** path points at no file: "docker not found at <path>"
- Docker isn't running: "Docker isn't running: start Docker Desktop (or the Docker service) and
  try again."
- The image is missing and **Get the image** is **Only use an image already on this
  computer**: "The image <image> isn't on this computer: pull it, or choose "Pull when missing"
  in Settings → Docker."
- Pulling fails: "Couldn't pull <image>: <last line of Docker's error>"
- Building fails: "Couldn't build the sandbox image: <last line of Docker's error>"
- The container can't be created: "Couldn't start the sandbox container: <last line of Docker's
  error>"
- A network rule is wrong: "Network rule <n>: <what is wrong>"
- The network image can't be built: "Couldn't build the sandbox network image: <last line of
  Docker's error>"
- The custom network can't be set up: "Couldn't set up the sandbox network: <the reason>"
- The command runs past its time limit: "the command didn't finish within <n> s; its container
  was stopped and removed"

The message is returned as the command's error, so the model sees it and can tell you.

## Status

The **Status** row at the bottom of the page has these buttons:

- **Test** checks that Docker is running and the image is present. A successful result looks
  like "Docker 27.3.1 is running · image python:3.13-slim is present". For a Dockerfile it reads
  "image <tag> is built" or "isn't built yet". A failure shows the error message.
- **Pull now** (for **Docker image**) or **Build now** (for **Custom Dockerfile**) prepares the
  image. A spinner shows while it works. The result appears under the buttons: "Pulled <image>"
  or "Built <image>", or the error.

Pulling and building have their own limit of 15 minutes. That time does not count against a
command's time limit.

## What the sandbox protects against, and what it doesn't

The sandbox limits what a command can reach. It does not make an untrusted command safe.

**It protects against:**

- commands reading or changing files outside the workspace folder;
- commands using your network, when the network is set to **None (no network)**, or using more
  of it than the rules allow, with **Only the hosts and ports below**;
- commands using more than the CPU, memory and processes you allow.

**It doesn't protect against:**

- files inside the workspace folder. Commands can read and change them, unless **Mount
  read-only** is ticked;
- your network, when **Normal network (can reach this computer, your local network and the internet)** is on. Commands can reach anything your computer can;
- what the image itself contains. Use images you trust;
- bugs in Docker or the Linux kernel that let a program escape a container;
- the other tools. The file tools, MCP servers and web fetch still run on your computer and are
  not sandboxed.

On macOS and Windows, Docker runs in a virtual machine, so containers run inside that machine
rather than on your computer's own system.

See also [Tools and approvals](../using/tools.html) and
[Data and privacy](../reference/data-and-privacy.html).
