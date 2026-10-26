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
 && pip install --no-cache-dir \
      # data analysis
      numpy pandas polars openpyxl xlrd tabulate \
      # documents and templates
      python-docx python-pptx jinja2 pyyaml \
      # web and HTTP
      requests httpx beautifulsoup4 lxml scrapy \
      # PDF
      pypdf reportlab pdfplumber \
      # images
      pillow opencv-python-headless \
      # charts and terminal output
      matplotlib seaborn plotly rich \
      # text and NLP
      nltk textblob regex rapidfuzz \
      # databases
      sqlalchemy psycopg2-binary pymysql \
      # utilities
      tqdm click python-dotenv arrow pytz loguru pytest \
 && mkdir -p /opt/npm \
 && chmod -R a+rwX /opt/venv /opt/npm

# The NLTK data TextBlob needs (tokenizers, tagger, WordNet, stopwords, corpora), where NLTK
# looks without NLTK_DATA set. The downloader exits 0 even when a download fails, so the
# build checks that TextBlob and NLTK can use it.
RUN python -m nltk.downloader -q -d /usr/share/nltk_data \
      punkt punkt_tab stopwords wordnet omw-1.4 averaged_perceptron_tagger \
      averaged_perceptron_tagger_eng brown conll2000 movie_reviews vader_lexicon \
 && python -c "from textblob import TextBlob; from nltk.corpus import stopwords, wordnet; \
from nltk.sentiment import SentimentIntensityAnalyzer as S; \
b = TextBlob('It works. It is good.'); b.sentences; b.tags; b.noun_phrases; \
stopwords.words('english'); wordnet.synsets('dog'); S().polarity_scores('good')"

WORKDIR /workspace
