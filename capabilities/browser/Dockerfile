# Kura browser worker image: Node + Playwright + Chromium (headless shell),
# with worker.mjs. A tenant browser (agent-runtime-program.md 3.3,
# docs/harness/tenant-browser.md) runs it in the tenant's isolated
# container, as the tenant's uid, with a read-only root. So everything here
# is world-readable and nothing is written outside HOME and /tmp.
#
# Build context is the REPO ROOT:
#   docker build -f capabilities/browser/Dockerfile -t kura-browser-worker .
#
# Chromium only (the headless shell): the stock Playwright image ships every
# browser and is 3.7 GB.

FROM node:22-bookworm-slim
ARG PLAYWRIGHT_VERSION=1.64.1
ENV PLAYWRIGHT_BROWSERS_PATH=/opt/ms-playwright
WORKDIR /opt/kura-browser
RUN npm init -y >/dev/null \
 && npm install --no-audit --no-fund --omit=dev "playwright@${PLAYWRIGHT_VERSION}" \
 && npx playwright install --with-deps --only-shell chromium \
 && rm -rf /var/lib/apt/lists/* /root/.npm /root/.cache \
 && chmod -R a+rX /opt/kura-browser /opt/ms-playwright
COPY capabilities/browser/worker.mjs /opt/kura-browser/worker.mjs
COPY capabilities/browser/PROTOCOL.md /opt/kura-browser/PROTOCOL.md
ENV KURA_BROWSER_HEADLESS=1 \
    HOME=/tmp
USER 10001:10001
# The daemon runs `node /opt/kura-browser/worker.mjs` itself; this is for
# running the image by hand.
CMD ["node", "/opt/kura-browser/worker.mjs"]
