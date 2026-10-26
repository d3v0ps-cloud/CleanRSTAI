# A full CleanRSTAI sandbox: Python, Node.js, build tools and network tools.
#
# Use it in Settings → Docker → Custom Dockerfile ("Full example"), or build it yourself:
#   docker build -t cleanrstai-sandbox-full - < full.Dockerfile
#
# The sandbox runs commands without Linux capabilities and, on Linux, as your own user:
# - ping and traceroute work (unprivileged ICMP); nmap runs unprivileged, so use connect
#   scans (-sT) rather than SYN scans or raw-socket pings (-sS, -PE need root and NET_RAW),
#   and tcpdump can't capture;
# - apt-get can't install at run time as your user; add packages here instead;
# - pip and npm install into /opt/venv and /opt/npm, which any user can write.
FROM ubuntu:24.04

ENV DEBIAN_FRONTEND=noninteractive \
    LANG=C.UTF-8 \
    LC_ALL=C.UTF-8 \
    VIRTUAL_ENV=/opt/venv \
    NPM_CONFIG_PREFIX=/opt/npm \
    PATH=/opt/venv/bin:/opt/npm/bin:/usr/local/sbin:/usr/local/bin:/usr/sbin:/usr/bin:/sbin:/bin

RUN apt-get update \
 && apt-get install -y --no-install-recommends \
      # shells and everyday tools
      bash ca-certificates coreutils curl wget git less nano vim-tiny file tree jq \
      zip unzip xz-utils bzip2 p7zip-full rsync procps psmisc time sqlite3 openssh-client \
      # Python
      python3 python3-dev python3-pip python3-venv python-is-python3 \
      # build tools
      build-essential pkg-config cmake ninja-build autoconf automake libtool gdb \
      libssl-dev libffi-dev zlib1g-dev libbz2-dev libreadline-dev libsqlite3-dev liblzma-dev \
      # Node.js
      nodejs npm \
      # network tools
      iputils-ping traceroute nmap dnsutils netcat-openbsd iproute2 net-tools \
      socat whois tcpdump openssl libcap2-bin \
 && rm -rf /var/lib/apt/lists/*

# The sandbox runs with no capabilities and no-new-privileges, so a program with file
# capabilities can't start there. ping has cap_net_raw; without it, it uses unprivileged ICMP.
RUN getcap -r /usr /bin /sbin 2>/dev/null | cut -d' ' -f1 | xargs -r -n1 setcap -r

# A Python environment and an npm prefix that the (unprivileged) sandbox user can install to.
RUN python3 -m venv /opt/venv \
 && pip install --no-cache-dir --upgrade pip \
 && pip install --no-cache-dir requests httpx pyyaml numpy pandas \
 && mkdir -p /opt/npm \
 && chmod -R a+rwX /opt/venv /opt/npm

WORKDIR /workspace
