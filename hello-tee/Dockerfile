# ── Stage 1: Install CRE CLI ─────────────────────────────────
FROM debian:bookworm-slim AS cre-installer

RUN apt-get update && apt-get install -y curl ca-certificates && rm -rf /var/lib/apt/lists/*

# Install CRE CLI (Chainlink Runtime Environment)
RUN curl -sSL https://smartcontract.chainlinklabs.com/cre-cli/install | sh

# ── Stage 2: Install workflow dependencies ───────────────────
FROM node:20-slim AS deps

WORKDIR /app

# Copy workflow package files and install deps
COPY my-workflow/package.json my-workflow/package-lock.json* my-workflow/bun.lock* ./my-workflow/
RUN cd my-workflow && npm install --production

# ── Stage 3: Final image ────────────────────────────────────
FROM node:20-slim

RUN apt-get update && apt-get install -y ca-certificates && rm -rf /var/lib/apt/lists/*

WORKDIR /app

# Copy CRE CLI binary
COPY --from=cre-installer /root/.cre/bin/cre /usr/local/bin/cre
RUN chmod +x /usr/local/bin/cre

# Copy the parent package.json (needed for "type": "module")
COPY package.json ./

# Copy workflow source + installed deps
COPY my-workflow/ ./my-workflow/
COPY --from=deps /app/my-workflow/node_modules ./my-workflow/node_modules

# Copy CRE project files
COPY project.yaml ./
COPY secrets.yaml ./

# Copy runner
COPY runner/ ./runner/

# ── Environment variables (override at runtime) ─────────────
ENV BACKEND_URL=http://localhost:3001
ENV POLL_INTERVAL_MS=10000
ENV CRE_TARGET=staging-settings
# These MUST be provided at runtime:
# ENV GEMINI_API_KEY_ALL=...
# ENV CRE_ETH_PRIVATE_KEY=...

CMD ["node", "runner/index.js"]
