FROM node:26-bookworm-slim AS frontend
WORKDIR /build/web
COPY web/package.json web/package-lock.json ./
RUN npm ci
COPY web ./
RUN npm run build

FROM rust:1.95.0-slim-bookworm AS build
RUN apt-get update && apt-get install -y --no-install-recommends gcc libc6-dev \
    && rm -rf /var/lib/apt/lists/*
WORKDIR /build
COPY Cargo.toml Cargo.lock ./
COPY src ./src
COPY --from=frontend /build/web/dist ./web/dist
RUN cargo build --release --locked && strip target/release/mdc

FROM debian:bookworm-slim
RUN apt-get update && apt-get install -y --no-install-recommends \
    ca-certificates curl git build-essential python3 python3-venv zstd unzip \
    && rm -rf /var/lib/apt/lists/*

# Keep Elan's executable in the image and downloaded toolchains in a volume.
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
    rm /tmp/elan.tar.gz /tmp/elan-init

COPY src/latex/requirements.txt /tmp/latex-requirements.txt
RUN python3 -m venv /opt/latex \
    && /opt/latex/bin/pip install --no-cache-dir -r /tmp/latex-requirements.txt \
    && rm /tmp/latex-requirements.txt \
    && useradd --create-home --uid 10001 mdc \
    && mkdir -p /var/lib/mdc/cache /var/lib/mdc/elan \
    && chown -R mdc:mdc /var/lib/mdc
COPY --from=build /build/target/release/mdc /usr/local/bin/mdc
ENV HOME=/home/mdc \
    ELAN_HOME=/var/lib/mdc/elan \
    MDC_CACHE_DIR=/var/lib/mdc/cache \
    MDC_LATEX_PYTHON=/opt/latex/bin/python \
    PATH=/opt/elan/bin:$PATH
USER mdc
WORKDIR /home/mdc
EXPOSE 17843
ENTRYPOINT ["mdc"]
CMD ["start", "--foreground"]
