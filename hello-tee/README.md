# Chainlink CRE — AI-Powered Prediction Market Resolution

## Overview

This directory contains the **Chainlink Confidential Runtime Environment (CRE)** workflow that resolves prediction markets using an LLM (Gemini 2.5 Flash) running inside a **Trusted Execution Environment (TEE)**.

The workflow is TypeScript compiled to WASM and executed inside Chainlink's DON (Decentralized Oracle Network). It fetches market data, gathers evidence, calls an LLM for resolution, and writes the result on-chain — all while keeping API keys and evidence confidential.

## Architecture

```
Runner (Node.js)                    CRE Workflow (TEE / WASM)
┌─────────────┐                     ┌───────────────────────────────────────┐
│ Polls       │──── triggers ──────>│ Step 1: Fetch market from backend    │
│ backend     │                     │ Step 2: Fetch live evidence          │
│ every 10s   │                     │ Step 3: Call Gemini LLM (secret key) │
│             │                     │ Step 4: Parse YES/NO outcome         │
│             │                     │ Step 5: ABI-encode result            │
│             │                     │ Step 6: DON nodes sign report        │
│             │<── result ──────────│ Step 7: Write to contract on-chain   │
└─────────────┘                     └───────────────────────────────────────┘
```

## How We Use Chainlink CRE

All CRE-specific code lives in [`my-workflow/main.ts`](https://github.com/Abhishek-2416/Chainlink-hackathon/blob/feature/runnerService/hello-tee/my-workflow/main.ts).

### 1. Consensus-Based HTTP Fetching

Every DON node independently fetches the same data. `consensusIdenticalAggregation` ensures all nodes received the **exact same response** — if any node's data was tampered with, the workflow fails.

**Market data fetch** — [`main.ts#L188-L194`](https://github.com/Abhishek-2416/Chainlink-hackathon/blob/feature/runnerService/hello-tee/my-workflow/main.ts#L188-L194)
```typescript
const marketDataRaw = httpClient
  .sendRequest(
    runtime,
    fetchMarketData,
    consensusIdenticalAggregation<string>()
  )(runtime.config)
  .result()
```

**Evidence fetch** — [`main.ts#L210-L216`](https://github.com/Abhishek-2416/Chainlink-hackathon/blob/feature/runnerService/hello-tee/my-workflow/main.ts#L210-L216)
```typescript
const evidence = httpClient
  .sendRequest(
    runtime,
    fetchEvidence,
    consensusIdenticalAggregation<string>()
  )(runtime.config)
  .result()
```

### 2. Secret Management Inside TEE

The Gemini API key is stored in CRE's encrypted secrets store and is **only accessible inside the TEE**. It never leaves the secure enclave.

[`main.ts#L221`](https://github.com/Abhishek-2416/Chainlink-hackathon/blob/feature/runnerService/hello-tee/my-workflow/main.ts#L221)
```typescript
const secret = runtime.getSecret({ id: "GEMINI_API_KEY" }).result()
```

### 3. LLM Call in Confidential Node Mode

The LLM call runs inside `runtime.runInNodeMode()` — a confidential execution context where the API key is used but never exposed. Consensus ensures all nodes agree on the LLM result.

[`main.ts#L222-L225`](https://github.com/Abhishek-2416/Chainlink-hackathon/blob/feature/runnerService/hello-tee/my-workflow/main.ts#L222-L225)
```typescript
const llmResponse = runtime.runInNodeMode(
  callLLM,
  consensusIdenticalAggregation<string>()
)(secret.value, marketData.question, evidence).result()
```

The `callLLM` function uses the secret API key inside TEE memory — [`main.ts#L130-L136`](https://github.com/Abhishek-2416/Chainlink-hackathon/blob/feature/runnerService/hello-tee/my-workflow/main.ts#L130-L136)
```typescript
headers: {
  "Content-Type": "application/json",
  "x-goog-api-key": apiKey,  // secret.value — only exists in TEE
}
```

### 4. DON-Signed Report

Each DON node signs the encoded payload `(marketId, outcome)` with their private key. The Chainlink Forwarder verifies these multi-node signatures before allowing on-chain settlement.

[`main.ts#L252-L257`](https://github.com/Abhishek-2416/Chainlink-hackathon/blob/feature/runnerService/hello-tee/my-workflow/main.ts#L252-L257)
```typescript
const reportResponse = runtime.report({
  encodedPayload: hexToBase64(encodedPayload),
  encoderName: "evm",
  signingAlgo: "ecdsa",
  hashingAlgo: "keccak256",
}).result()
```

### 5. On-Chain Write via Forwarder

The CRE Forwarder submits the DON-signed report to the PredictionMarket contract. The contract verifies `msg.sender == forwarder` before resolving the market.

[`main.ts#L273-L277`](https://github.com/Abhishek-2416/Chainlink-hackathon/blob/feature/runnerService/hello-tee/my-workflow/main.ts#L273-L277)
```typescript
const writeResult = evmClient.writeReport(runtime, {
  receiver: runtime.config.predictionMarketAddress,
  report: reportResponse,
  gasConfig: { gasLimit: "500000" },
}).result()
```

### 6. CRE Workflow Registration

The workflow is registered with Chainlink's CRE runtime using a cron trigger. The TypeScript is compiled to WASM and sandboxed inside the TEE.

[`main.ts#L297-L305`](https://github.com/Abhishek-2416/Chainlink-hackathon/blob/feature/runnerService/hello-tee/my-workflow/main.ts#L297-L305)
```typescript
function initWorkflow(config: Config, secretsProvider: SecretsProvider) {
  const cron = new cre.capabilities.CronCapability()
  const trigger = cron.trigger({ schedule: config.schedule })
  return [cre.handler(trigger, onCronTrigger)]
}

const runner = await Runner.newRunner<Config>({ configSchema })
await runner.run(initWorkflow)
```

## On-Chain Contract

The smart contract that receives the CRE report — [`contracts/src/PredictionMarket.sol#L152-L163`](https://github.com/Abhishek-2416/Chainlink-hackathon/blob/feature/runnerService/contracts/src/PredictionMarket.sol#L152-L163)

```solidity
function onReport(bytes calldata metadata, bytes calldata report) external onlyForwarder {
    (uint256 marketId, uint8 outcomeRaw) = abi.decode(report, (uint256, uint8));
    Outcome outcome = Outcome(outcomeRaw);
    Market storage market = markets[marketId];
    require(market.status == MarketStatus.Open, "Market not open");
    require(outcome == Outcome.Yes || outcome == Outcome.No, "Invalid outcome");
    market.status = MarketStatus.Resolved;
    market.outcome = outcome;
    emit MarketResolved(marketId, outcome);
}
```

## File Structure

```
hello-tee/
├── my-workflow/
│   ├── main.ts                          # CRE workflow (all 7 steps)
│   ├── config/
│   │   └── config.staging.json          # Contract address, chain, backend URL
│   ├── workflow.yaml                    # CRE target settings
│   └── package.json                     # @chainlink/cre-sdk, viem
├── runner/
│   └── index.js                         # Node.js scheduler — polls backend, triggers CRE
├── project.yaml                         # RPC endpoints per chain
├── secrets.yaml                         # Secret name mappings
├── Dockerfile                           # Docker image for deployment
└── .env                                 # CRE_ETH_PRIVATE_KEY, GEMINI_API_KEY_ALL
```

## Running Locally

```bash
# 1. Install workflow dependencies
cd my-workflow && npm install

# 2. Set environment variables in .env
CRE_TARGET=staging-settings
GEMINI_API_KEY_ALL=your-gemini-key
CRE_ETH_PRIVATE_KEY=your-private-key

# 3. Run CRE workflow manually
cre workflow simulate my-workflow --target staging-settings --broadcast

# 4. Or start the runner (auto-polls backend)
node runner/index.js
```
