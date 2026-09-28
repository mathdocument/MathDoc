FROM node:22-bookworm-slim AS build
WORKDIR /app
COPY package.json package-lock.json ./
COPY coordinator/package.json ./coordinator/
COPY app/package.json ./app/
RUN npm ci
COPY coordinator ./coordinator
COPY app ./app
RUN npm run build

FROM node:22-bookworm-slim
ENV NODE_ENV=production MDC_HOST=0.0.0.0 MDC_APP_DIR=/app/app/dist
WORKDIR /app
COPY package.json package-lock.json ./
COPY coordinator/package.json ./coordinator/
COPY app/package.json ./app/
RUN npm ci --omit=dev && npm cache clean --force
COPY --from=build /app/coordinator/dist ./coordinator/dist
COPY --from=build /app/app/dist ./app/dist
USER node
EXPOSE 17843
ENTRYPOINT ["node", "coordinator/dist/main.js"]
CMD ["serve"]
