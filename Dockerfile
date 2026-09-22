FROM node:26-trixie-slim AS frontend
WORKDIR /build/web
COPY web/package.json web/package-lock.json ./
RUN npm ci
COPY web ./
RUN npm run build

FROM rust:1.95.0-slim-trixie AS build
RUN apt-get update && apt-get install -y --no-install-recommends gcc libc6-dev \
    && rm -rf /var/lib/apt/lists/*
WORKDIR /build
COPY Cargo.toml Cargo.lock ./
COPY src ./src
COPY --from=frontend /build/web/dist ./web/dist
RUN cargo build --release --locked && strip target/release/mdc

FROM debian:trixie-slim AS runtime
RUN apt-get update && apt-get install -y --no-install-recommends \
    ca-certificates curl git build-essential python3 python3-venv zstd unzip \
    && rm -rf /var/lib/apt/lists/*

# Elan and the default Lean toolchain are immutable image contents. Additional
# toolchains live under /var/lib/mdc/tools, alongside future Rocq installations.
RUN set -eu; \
    arch="$(uname -m)"; \
    case "$arch" in \
      aarch64) checksum=cb69af0803b04157bc30201c29c12fca882bb3ad8b43476b8d2d3064810bc3ac ;; \
      x86_64) checksum=df0b2b3a439961ffcbb3985214365ffe40f49bc871df04dff268c7d8e21ca8b2 ;; \
      *) echo "Unsupported architecture: $arch" >&2; exit 1 ;; \
    esac; \
    curl --fail --location --retry 3 \
      "https://github.com/leanprover/elan/releases/download/v4.2.3/elan-$arch-unknown-linux-gnu.tar.gz" -o /tmp/elan.tar.gz; \
    echo "$checksum  /tmp/elan.tar.gz" | sha256sum -c -; \
    tar -xzf /tmp/elan.tar.gz -C /tmp; \
    ELAN_HOME=/opt/elan /tmp/elan-init -y --no-modify-path --default-toolchain none; \
    ELAN_HOME=/opt/elan /opt/elan/bin/elan toolchain install leanprover/lean4:v4.33.1; \
    mkdir -p /opt/lean; \
    mv /opt/elan/toolchains/leanprover--lean4---v4.33.1 /opt/lean/4.33.1; \
    rm /tmp/elan.tar.gz /tmp/elan-init

COPY src/latex/requirements.txt /tmp/latex-requirements.txt
RUN python3 -m venv /opt/latex \
    && /opt/latex/bin/pip install --no-cache-dir -r /tmp/latex-requirements.txt \
    && rm /tmp/latex-requirements.txt \
    && useradd --create-home --uid 10001 mdc \
    && mkdir -p /var/lib/mdc/cache /var/lib/mdc/tools/lean \
    && chown -R mdc:mdc /var/lib/mdc
COPY --from=build /build/target/release/mdc /usr/local/bin/mdc
COPY --chmod=755 docker/runtime-entrypoint.sh /usr/local/bin/mathdoc-entrypoint
ENV HOME=/home/mdc \
    ELAN_HOME=/var/lib/mdc/tools/lean \
    MDC_CACHE_DIR=/var/lib/mdc/cache \
    MDC_LATEX_PYTHON=/opt/latex/bin/python \
    PATH=/opt/elan/bin:$PATH
USER mdc
WORKDIR /home/mdc
LABEL org.opencontainers.image.source="https://github.com/mathdocument/MathDoc" \
    org.opencontainers.image.title="MathDoc runtime" \
    org.opencontainers.image.licenses="MIT"
EXPOSE 17843
ENTRYPOINT ["mathdoc-entrypoint"]
CMD ["start", "--foreground"]
