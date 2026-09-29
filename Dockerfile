# MathDoc: the TypeScript backend (API and worker), the web/ knowledge editor and the
# LaTeX renderer. Lean is checked by an external LeanGround; the image has no Lean.
FROM node:26-bookworm-slim AS build
WORKDIR /build
COPY package.json package-lock.json ./
COPY coordinator/package.json coordinator/
COPY app/package.json app/
COPY web/package.json web/
RUN npm ci
COPY coordinator coordinator
COPY web web
RUN npm run build -w @mathdoc/coordinator && npm run build -w mdc-web

# Production dependencies of the backend only.
FROM node:26-bookworm-slim AS runtime-deps
WORKDIR /app
COPY package.json package-lock.json ./
COPY coordinator/package.json coordinator/
COPY app/package.json app/
COPY web/package.json web/
RUN npm ci --omit=dev -w @mathdoc/coordinator --include-workspace-root=false

FROM node:26-bookworm-slim
RUN apt-get update && apt-get install -y --no-install-recommends python3 python3-venv \
    && rm -rf /var/lib/apt/lists/*
COPY renderer/requirements.txt /tmp/latex-requirements.txt
RUN python3 -m venv /opt/latex \
    && /opt/latex/bin/pip install --no-cache-dir -r /tmp/latex-requirements.txt \
    && rm /tmp/latex-requirements.txt \
    && mkdir -p /var/lib/mdc/cache && chown node:node /var/lib/mdc/cache
WORKDIR /app
COPY --from=runtime-deps /app/node_modules ./node_modules
COPY coordinator/package.json coordinator/
COPY --from=build /build/coordinator/dist coordinator/dist
COPY --from=build /build/web/dist web/dist
COPY renderer renderer
RUN chmod +x coordinator/dist/cli.js && ln -s /app/coordinator/dist/cli.js /usr/local/bin/mdc
ENV NODE_ENV=production \
    MDC_HOST=0.0.0.0 \
    MDC_PORT=17843 \
    MDC_WEB_DIR=/app/web/dist \
    MDC_RENDERER_DIR=/app/renderer \
    MDC_LATEX_PYTHON=/opt/latex/bin/python \
    MDC_CACHE_DIR=/var/lib/mdc/cache
USER node
EXPOSE 17843
ENTRYPOINT ["node", "/app/coordinator/dist/main.js"]
CMD ["serve"]
