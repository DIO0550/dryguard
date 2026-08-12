FROM rust:1-trixie

RUN apt-get update \
  && apt-get install -y --no-install-recommends \
     pkg-config \
     libssl-dev \
     zip \
     unzip \
  && useradd -m -s /bin/bash vscode \
  && mkdir -p /workspaces \
  && mkdir -p /usr/local/cargo/registry /usr/local/cargo/git \
  && chown -R vscode:vscode /workspaces /usr/local/cargo \
  && rm -rf /var/lib/apt/lists/*

RUN curl -fsSL https://gist.githubusercontent.com/DIO0550/1830ced050035c34668aa426f7258346/raw/install-gh.sh | bash

# Node.js公式バイナリをインストール（メジャーバージョンの最新を取得）
ENV NODE_MAJOR=24

RUN set -eux; \
  arch="$(uname -m)"; \
  case "$arch" in \
    x86_64) node_arch="x64" ;; \
    aarch64|arm64) node_arch="arm64" ;; \
    *) echo "unsupported arch: $arch" >&2; exit 1 ;; \
  esac; \
  NODE_VERSION="$(curl -fsSL "https://nodejs.org/dist/latest-v${NODE_MAJOR}.x/" \
    | grep -oE "node-v${NODE_MAJOR}\\.[0-9]+\\.[0-9]+" | head -1 | sed 's/node-v//')"; \
  curl -fsSL "https://nodejs.org/dist/v${NODE_VERSION}/node-v${NODE_VERSION}-linux-${node_arch}.tar.xz" \
    | tar -xJ -C /usr/local --strip-components=1; \
  node -v; npm -v
  

USER vscode

SHELL ["/bin/bash", "-l", "-c"]
RUN curl -fsSL https://gist.githubusercontent.com/DIO0550/dcc09dc9d2dcefceb9b0cda183e7ef8e/raw/pnpm-safe-chain-setup.sh | sh
RUN curl -fsSL https://gist.githubusercontent.com/DIO0550/148b1392cf435ba3562da0d398bee442/raw/install-ai-tools.sh | bash