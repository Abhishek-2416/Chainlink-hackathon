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

// ── Config Schema ───────────────────────────────────────
const configSchema = z.object({
  schedule: z.string(),
  inputData: z.string(),
  apiUrl: z.string(),
  predictionMarketAddress: z.string(),
  marketId: z.string(),
  chainSelectorName: z.string(),
})

type Config = z.infer<typeof configSchema>

// ── Deterministic Hash ──────────────────────────────────
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

// ── Fetch Live Data ─────────────────────────────────────
const fetchMarketData = (sendRequester: HTTPSendRequester, config: Config): string => {
  const response = sendRequester.sendRequest({
    url: config.apiUrl,
    method: "GET" as const,
  }).result()
  return new TextDecoder().decode(response.body)
}

// ── Call LLM ────────────────────────────────────────────
const callLLM = (nodeRuntime: NodeRuntime<Config>, apiKey: string): string => {
  const bodyObj = {
    contents: [
      {
        parts: [
          {
            text: `You are a prediction market resolver. Determine the outcome.\n\nQuestion: ${nodeRuntime.config.inputData}\n\nRespond ONLY with JSON: {"outcome": "YES", "confidence": 0.95, "reasoning": "one sentence"}`,
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
    throw new Error(`LLM request failed: ${resp.statusCode}`)
  }

  const parsed = JSON.parse(responseBody)
  return parsed.candidates[0].content.parts[0].text
}

// ── Parse LLM outcome to uint8 ─────────────────────────
function parseOutcome(llmResponse: string): number {
  // Clean markdown fences if present
  const cleaned = llmResponse.replace(/```json\n?/g, "").replace(/```/g, "").trim()
  try {
    const parsed = JSON.parse(cleaned)
    if (parsed.outcome === "YES") return 1
    if (parsed.outcome === "NO") return 2
  } catch {
    // If JSON parsing fails, look for YES/NO in raw text
    if (llmResponse.toUpperCase().includes("YES")) return 1
    if (llmResponse.toUpperCase().includes("NO")) return 2
  }
  return 0 // Unresolved
}

// ── Trigger Callback ────────────────────────────────────
const onCronTrigger = (runtime: Runtime<Config>): string => {
  runtime.log("=== Prediction Market Resolution Workflow ===")

  // Step 1: Fetch live market data
  runtime.log("[STEP 1] Fetching market data...")
  const httpClient = new HTTPClient()
  const marketData = httpClient
    .sendRequest(
      runtime,
      fetchMarketData,
      consensusIdenticalAggregation<string>()
    )(runtime.config)
    .result()
  runtime.log(`[STEP 1] Done: ${marketData}`)

  // Step 2: Call LLM for resolution
  runtime.log("[STEP 2] Calling LLM...")
  const secret = runtime.getSecret({ id: "GEMINI_API_KEY" }).result()
  const llmResponse = runtime.runInNodeMode(
    callLLM,
    consensusIdenticalAggregation<string>()
  )(secret.value).result()
  runtime.log(`[STEP 2] LLM says: ${llmResponse}`)

  // Step 3: Parse outcome
  const outcome = parseOutcome(llmResponse)
  const marketId = BigInt(runtime.config.marketId)
  runtime.log(`[STEP 3] Parsed outcome: ${outcome} (1=YES, 2=NO) for market ${marketId}`)

  if (outcome === 0) {
    runtime.log("[STEP 3] Could not determine outcome. Skipping on-chain write.")
    return JSON.stringify({ status: "unresolved", llmResponse })
  }

  // Step 4: Encode resolution data for on-chain
  runtime.log("[STEP 4] Encoding report...")
  const encodedPayload = encodeAbiParameters(
    parseAbiParameters("uint256 marketId, uint8 outcome"),
    [marketId, outcome]
  )
  runtime.log(`[STEP 4] Encoded: ${encodedPayload}`)

  // Step 5: Generate DON-signed report
  runtime.log("[STEP 5] Generating signed report...")
  const reportResponse = runtime.report({
    encodedPayload: hexToBase64(encodedPayload),
    encoderName: "evm",
    signingAlgo: "ecdsa",
    hashingAlgo: "keccak256",
  }).result()
  runtime.log("[STEP 5] Report signed by DON")

  // Step 6: Write to PredictionMarket contract via Forwarder
  runtime.log("[STEP 6] Writing to chain...")
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
  runtime.log(`[STEP 6] TX submitted: ${txHash}`)

  // Step 7: Return full result
  const result = {
    marketId: runtime.config.marketId,
    outcome: outcome === 1 ? "YES" : "NO",
    confidence: llmResponse,
    evidenceHash: deterministicHash(marketData),
    txHash,
  }
  runtime.log(`Result: ${JSON.stringify(result)}`)

  return JSON.stringify(result)
}

// ── Init + Entry ────────────────────────────────────────
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