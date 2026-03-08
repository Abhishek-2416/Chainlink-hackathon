import { writeContract, getPublicClient, getWalletClient } from "@wagmi/core";
import type { Abi } from "viem";
import { config } from "../config/wagmi";
import { PREDICTION_MARKET_ADDRESS, hasPredictionMarketAddress } from "../config/api";
import PredictionMarketAbi from "../abi/PredictionMarket.json";

/** Fallback gas limit when estimation fails */
const FALLBACK_GAS = 500_000n;
/** Cap gas to avoid "exceeds max" from wallet/RPC */
const MAX_GAS = 2_000_000n;

/**
 * Redeems winning shares for a resolved market.
 * Call marketService.claimRewards() first to get validated claim data from the API.
 * @param marketId - The market ID
 * @param amount - Amount in 6 decimals (1 share = 1e6)
 * @param contractAddress - Optional contract address; defaults to env config
 */
export async function redeemWinning(
  marketId: number,
  amount: bigint,
  contractAddress?: `0x${string}`
): Promise<`0x${string}`> {
  const address = contractAddress ?? PREDICTION_MARKET_ADDRESS;
  if (!hasPredictionMarketAddress && !contractAddress) {
    throw new Error("VITE_PREDICTION_MARKET_ADDRESS is not configured");
  }
  if (!address) {
    throw new Error("Contract address is required");
  }

  const abi = (PredictionMarketAbi as { abi: Abi }).abi;
  const publicClient = getPublicClient(config);
  const walletClient = await getWalletClient(config);

  let gas: bigint = FALLBACK_GAS;
  if (publicClient && typeof publicClient.estimateContractGas === "function") {
    try {
      const estimated = await publicClient.estimateContractGas({
        address,
        abi,
        functionName: "redeemWinning",
        args: [BigInt(marketId), amount],
        ...(walletClient?.account && { account: walletClient.account }),
      });
      gas = (estimated * 13n) / 10n;
      if (gas > MAX_GAS) gas = MAX_GAS;
    } catch {
      /* use fallback */
    }
  }

  const hash = await writeContract(config, {
    address,
    abi,
    functionName: "redeemWinning",
    args: [BigInt(marketId), amount],
    gas,
  });

  if (publicClient) {
    await publicClient.waitForTransactionReceipt({ hash });
  }
  return hash;
}
