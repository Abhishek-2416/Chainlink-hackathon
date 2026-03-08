import { spawn } from "child_process";
import path from "path";
import { fileURLToPath } from "url";

const __dirname = path.dirname(fileURLToPath(import.meta.url));

// ── Config ──────────────────────────────────────────────────
const BACKEND_URL = process.env.BACKEND_URL || "http://localhost:3001";
const POLL_INTERVAL_MS = parseInt(process.env.POLL_INTERVAL_MS || "60000"); // 1 minute
const WORKFLOW_DIR = path.resolve(__dirname, "../my-workflow");
const HELLO_TEE_DIR = path.resolve(__dirname, "..");
const CRE_TARGET = process.env.CRE_TARGET || "staging-settings";

let isRunning = false;

// ── Poll backend for markets ready to resolve ───────────────
async function checkForMarkets() {
  try {
    const res = await fetch(`${BACKEND_URL}/api/markets/ready-to-resolve`);
    if (!res.ok) {
      console.error(`[POLL] Backend returned ${res.status}`);
      return null;
    }
    const data = await res.json();
    if (data.market === null || !data.marketId) {
      return null;
    }
    return data;
  } catch (err) {
    console.error(`[POLL] Failed to reach backend: ${err.message}`);
    return null;
  }
}

// ── Run CRE workflow simulate ───────────────────────────────
function runWorkflow() {
  return new Promise((resolve, reject) => {
    console.log(`[CRE] Running: cre workflow simulate my-workflow --target ${CRE_TARGET} --broadcast`);

    const child = spawn(
      "cre",
      ["workflow", "simulate", "my-workflow", "--target", CRE_TARGET, "--broadcast"],
      {
        cwd: HELLO_TEE_DIR,
        stdio: "pipe",
        env: { ...process.env, PATH: process.env.PATH },
      }
    );

    let stdout = "";
    let stderr = "";

    child.stdout.on("data", (data) => {
      const line = data.toString();
      stdout += line;
      process.stdout.write(`[CRE] ${line}`);
    });

    child.stderr.on("data", (data) => {
      const line = data.toString();
      stderr += line;
      process.stderr.write(`[CRE:ERR] ${line}`);
    });

    child.on("close", (code) => {
      if (code === 0) {
        console.log(`[CRE] Workflow completed successfully`);
        resolve(stdout);
      } else {
        console.error(`[CRE] Workflow exited with code ${code}`);
        reject(new Error(`CRE exited with code ${code}: ${stderr}`));
      }
    });

    child.on("error", (err) => {
      console.error(`[CRE] Failed to spawn: ${err.message}`);
      reject(err);
    });
  });
}

// ── Main loop ───────────────────────────────────────────────
async function tick() {
  if (isRunning) {
    console.log("[RUNNER] Previous run still in progress, skipping...");
    return;
  }

  const timestamp = new Date().toISOString();
  console.log(`\n[RUNNER] ${timestamp} — Checking for markets ready to resolve...`);

  const market = await checkForMarkets();

  if (!market) {
    console.log("[RUNNER] No markets ready. Sleeping...");
    return;
  }

  console.log(`[RUNNER] Found market ${market.marketId}: "${market.question}"`);
  console.log(`[RUNNER] Resolution date: ${market.resolutionDate}`);

  isRunning = true;
  try {
    await runWorkflow();
  } catch (err) {
    console.error(`[RUNNER] Workflow failed: ${err.message}`);
  } finally {
    isRunning = false;
  }
}

// ── Start ───────────────────────────────────────────────────
console.log("=== CRE Workflow Runner ===");
console.log(`Backend URL:    ${BACKEND_URL}`);
console.log(`Poll interval:  ${POLL_INTERVAL_MS / 1000}s`);
console.log(`Workflow dir:   ${WORKFLOW_DIR}`);
console.log(`CRE target:     ${CRE_TARGET}`);
console.log("");

// Run immediately on start, then on interval
tick();
setInterval(tick, POLL_INTERVAL_MS);
