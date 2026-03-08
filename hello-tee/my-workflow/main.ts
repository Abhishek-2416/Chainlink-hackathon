import {
  cre,
  Runner,
  HTTPClient,
  EVMClient,
  ok,
  getNetwork,
  hexToBase64,
  bytesToHex,
  consensusIdenticalAggregation,
  type Runtime,
  type NodeRuntime,
  type HTTPSendRequester,
  type SecretsProvider,
} from "@chainlink/cre-sdk"
import { z } from "zod"
import { encodeAbiParameters, parseAbiParameters } from "viem"

// ── Config Schema ───────────────────────────────────────────
// The workflow dynamically discovers which market to resolve by calling
// the backend's /ready-to-resolve endpoint. No hardcoded marketId needed.
const configSchema = z.object({
  schedule: z.string(),
  backendUrl: z.string(),                  // e.g. "https://your-api.com"
  predictionMarketAddress: z.string(),     // deployed contract
  chainSelectorName: z.string(),           // e.g. "ethereum-testnet-sepolia"
})

type Config = z.infer<typeof configSchema>

// ── Types ───────────────────────────────────────────────────
interface MarketResolveData {
  marketId: number
  question: string
  category: string
  resolutionDate: string
  status: string
}

// ── Deterministic Hash ──────────────────────────────────────
function deterministicHash(str: string): string {
  let h1 = 0xdeadbeef
  let h2 = 0x41c6ce57
  for (let i = 0; i < str.length; i++) {
    const ch = str.charCodeAt(i)
    h1 = Math.imul(h1 ^ ch, 2654435761)
    h2 = Math.imul(h2 ^ ch, 1597334677)
  }
  h1 = Math.imul(h1 ^ (h1 >>> 16), 2246822507)
  h1 ^= Math.imul(h2 ^ (h2 >>> 13), 3266489909)
  h2 = Math.imul(h2 ^ (h2 >>> 16), 2246822507)
  h2 ^= Math.imul(h1 ^ (h1 >>> 13), 3266489909)
  const combined = 4294967296 * (2097151 & h2) + (h1 >>> 0)
  return combined.toString(16).padStart(16, "0")
}

// ── Step 1: Fetch next market ready for resolution ──────────
// High-level sendRequest pattern — all nodes fetch, consensus on identical response.
// Hits: GET {backendUrl}/api/markets/ready-to-resolve
const fetchMarketData = (sendRequester: HTTPSendRequester, config: Config): string => {
  const url = `${config.backendUrl}/api/markets/ready-to-resolve`
  const response = sendRequester.sendRequest({
    url,
    method: "GET" as const,
  }).result()

  if (!ok(response)) {
    throw new Error(`Backend returned ${response.statusCode} for ready-to-resolve`)
  }

  return new TextDecoder().decode(response.body)
}

// ── Step 2: Fetch live evidence (price data, news, etc.) ────
// For crypto markets we still hit CoinGecko. For other categories
// the backend could provide evidence URLs in the market data.
const fetchEvidence = (sendRequester: HTTPSendRequester, config: Config): string => {
  // Default: fetch BTC price. In production, you'd pick the right
  // evidence source based on the market category/question.
  const response = sendRequester.sendRequest({
    url: "https://api.coingecko.com/api/v3/simple/price?ids=bitcoin&vs_currencies=usd",
    method: "GET" as const,
  }).result()

  return new TextDecoder().decode(response.body)
}

// ── Step 3: Call LLM for resolution ─────────────────────────
// Low-level runInNodeMode — needs secrets (API key).
// Takes the question + evidence as arguments.
const callLLM = (
  nodeRuntime: NodeRuntime<Config>,
  apiKey: string,
  question: string,
  evidence: string,
): string => {
  const today = new Date().toISOString().split("T")[0]

  const bodyObj = {
    contents: [
      {
        parts: [
          {
            text: `You are a prediction market resolver. Your job is to determine whether the outcome of a market question is YES or NO based on the evidence provided.

Market Question: ${question}

Live Evidence Data: ${evidence}

Today's Date: ${today}

Rules:
- Answer based ONLY on the evidence provided and your knowledge up to today's date
- If the evidence clearly supports one outcome, state it with high confidence
- If the evidence is insufficient or ambiguous, state the more likely outcome with lower confidence
- Do NOT hedge — you must pick YES or NO

Respond ONLY with this exact JSON format. No markdown, no backticks, nothing else:
{"outcome": "YES", "confidence": 0.95, "reasoning": "one sentence"}`,
          },
        ],
      },
    ],
  }

  const bodyBytes = new TextEncoder().encode(JSON.stringify(bodyObj))
  const body = Buffer.from(bodyBytes).toString("base64")

  const httpClient = new HTTPClient()
  const resp = httpClient.sendRequest(nodeRuntime, {
    url: "https://generativelanguage.googleapis.com/v1beta/models/gemini-2.5-flash-lite:generateContent",
    method: "POST" as const,
    body,
    headers: {
      "Content-Type": "application/json",
      "x-goog-api-key": apiKey,
    },
    cacheSettings: {
      store: true,
      maxAge: "60s",
    },
  }).result()

  nodeRuntime.log(`[LLM] Status: ${resp.statusCode}`)
  const responseBody = new TextDecoder().decode(resp.body)

  if (!ok(resp)) {
    throw new Error(`LLM request failed: ${resp.statusCode} — ${responseBody}`)
  }

  const parsed = JSON.parse(responseBody)
  return parsed.candidates[0].content.parts[0].text
}

// ── Parse LLM outcome to uint8 ─────────────────────────────
function parseOutcome(llmResponse: string): number {
  const cleaned = llmResponse.replace(/```json\n?/g, "").replace(/```/g, "").trim()
  try {
    const parsed = JSON.parse(cleaned)
    if (parsed.outcome === "YES") return 1
    if (parsed.outcome === "NO") return 2
  } catch {
    if (llmResponse.toUpperCase().includes("YES")) return 1
    if (llmResponse.toUpperCase().includes("NO")) return 2
  }
  return 0 // Unresolved
}

// ── Extract question from backend response ──────────────────
function extractQuestion(marketDataJson: string): MarketResolveData {
  const data = JSON.parse(marketDataJson)
  return {
    marketId: data.marketId,
    question: data.question,
    category: data.category || "general",
    resolutionDate: data.resolutionDate || "",
    status: data.status || "open",
  }
}

// ── Trigger Callback ────────────────────────────────────────
const onCronTrigger = (runtime: Runtime<Config>): string => {
  runtime.log("=== Prediction Market Resolution Workflow ===")

  // ── Step 1: Fetch next market ready for resolution ──
  runtime.log("[STEP 1] Checking for markets ready to resolve...")
  const httpClient = new HTTPClient()
  const marketDataRaw = httpClient
    .sendRequest(
      runtime,
      fetchMarketData,
      consensusIdenticalAggregation<string>()
    )(runtime.config)
    .result()
  runtime.log(`[STEP 1] Got response: ${marketDataRaw.substring(0, 200)}...`)

  // Check if backend returned a market or null
  const parsed = JSON.parse(marketDataRaw)
  if (parsed.market === null || parsed.marketId === undefined || parsed.marketId === null) {
    runtime.log("[STEP 1] No markets ready for resolution. Exiting.")
    return JSON.stringify({ status: "idle", message: "No markets ready for resolution" })
  }

  const marketData = extractQuestion(marketDataRaw)
  runtime.log(`[STEP 1] Resolving market ${marketData.marketId}: "${marketData.question}"`)
  runtime.log(`[STEP 1] Category: ${marketData.category}, Status: ${marketData.status}`)

  // ── Step 2: Fetch live evidence ──
  runtime.log("[STEP 2] Fetching live evidence...")
  const evidence = httpClient
    .sendRequest(
      runtime,
      fetchEvidence,
      consensusIdenticalAggregation<string>()
    )(runtime.config)
    .result()
  runtime.log(`[STEP 2] Evidence: ${evidence}`)

  // ── Step 3: Call LLM with question + evidence ──
  runtime.log("[STEP 3] Calling LLM for resolution...")
  const secret = runtime.getSecret({ id: "GEMINI_API_KEY" }).result()
  const llmResponse = runtime.runInNodeMode(
    callLLM,
    consensusIdenticalAggregation<string>()
  )(secret.value, marketData.question, evidence).result()
  runtime.log(`[STEP 3] LLM says: ${llmResponse}`)

  // ── Step 4: Parse outcome ──
  const outcome = parseOutcome(llmResponse)
  const marketId = BigInt(marketData.marketId)
  runtime.log(`[STEP 4] Parsed outcome: ${outcome} (1=YES, 2=NO) for market ${marketId}`)

  if (outcome === 0) {
    runtime.log("[STEP 4] Could not determine outcome. Skipping on-chain write.")
    return JSON.stringify({
      marketId: marketData.marketId,
      status: "unresolved",
      llmResponse,
    })
  }

  // ── Step 5: Encode resolution data ──
  runtime.log("[STEP 5] Encoding report...")
  const encodedPayload = encodeAbiParameters(
    parseAbiParameters("uint256 marketId, uint8 outcome"),
    [marketId, outcome]
  )
  runtime.log(`[STEP 5] Encoded: ${encodedPayload}`)

  // ── Step 6: Generate DON-signed report ──
  runtime.log("[STEP 6] Generating signed report...")
  const reportResponse = runtime.report({
    encodedPayload: hexToBase64(encodedPayload),
    encoderName: "evm",
    signingAlgo: "ecdsa",
    hashingAlgo: "keccak256",
  }).result()
  runtime.log("[STEP 6] Report signed by DON")

  // ── Step 7: Write to PredictionMarket contract ──
  runtime.log("[STEP 7] Writing to chain...")
  const network = getNetwork({
    chainFamily: "evm",
    chainSelectorName: runtime.config.chainSelectorName,
    isTestnet: true,
  })

  if (!network) {
    throw new Error(`Network not found: ${runtime.config.chainSelectorName}`)
  }

  const evmClient = new EVMClient(network.chainSelector.selector)
  const writeResult = evmClient.writeReport(runtime, {
    receiver: runtime.config.predictionMarketAddress,
    report: reportResponse,
    gasConfig: { gasLimit: "500000" },
  }).result()

  const txHash = bytesToHex(writeResult.txHash || new Uint8Array(32))
  runtime.log(`[STEP 7] TX submitted: ${txHash}`)

  // ── Result ──
  const result = {
    marketId: marketData.marketId,
    question: marketData.question,
    outcome: outcome === 1 ? "YES" : "NO",
    llmResponse,
    evidenceHash: deterministicHash(evidence),
    txHash,
  }
  runtime.log(`Result: ${JSON.stringify(result)}`)

  return JSON.stringify(result)
}

// ── Init + Entry ────────────────────────────────────────────
function initWorkflow(config: Config, secretsProvider: SecretsProvider) {
  const cron = new cre.capabilities.CronCapability()
  const trigger = cron.trigger({ schedule: config.schedule })
  return [cre.handler(trigger, onCronTrigger)]
}

export async function main() {
  const runner = await Runner.newRunner<Config>({ configSchema })
  await runner.run(initWorkflow)
}

main()
