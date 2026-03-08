import { useEffect, useState } from "react";
import { useAccount } from "wagmi";
import { useTradeStore } from "../../stores/tradeStore";
import { marketService } from "../../services/marketService";
import { redeemWinning } from "../../services/redeemService";
import { formatCurrency } from "../../utils/format";
import { consolidateErrorMessage } from "../../utils/errorMessage";
import { useToastStore } from "../../stores/toastStore";
import { PositionTable } from "../../components/trade/PositionTable";
import { Card } from "../../components/ui/Card";
import { ConnectWalletButton } from "../../components/ui/ConnectWalletButton";

const SCALE = 1e6;

export function PortfolioPage() {
  const { address, isConnected } = useAccount();
  const [balance, setBalance] = useState<{ eth: number; usdc: number } | null>(null);
  const { positions, setPositions } = useTradeStore();
  const [openOrders, setOpenOrders] = useState<
    Array<{ orderId: number; marketId: number; token: string; shares: number; cost: number; price: number }>
  >([]);
  const [totalPnl, setTotalPnl] = useState(0);
  const [tradeHistory, setTradeHistory] = useState<
    Array<{ tradeId: number; question: string; token: string; shares: number; cost: number; priceCents: number; txHash: string; timestamp: string | null }>
  >([]);
  const [redeemable, setRedeemable] = useState<
    Array<{ marketId: number; question: string; winningOutcome: string; redeemableShares: number; profit: number; contractAddress?: string }>
  >([]);
  const [loading, setLoading] = useState(true);
  const [redeemingMarketId, setRedeemingMarketId] = useState<number | null>(null);

  useEffect(() => {
    if (!address) {
      setPositions([]);
      setOpenOrders([]);
      setBalance(null);
      setLoading(false);
      return;
    }
    setLoading(true);
    Promise.all([
      marketService.getPortfolio(address),
      marketService.getTradeHistory(address),
      marketService.getRedemptionStatus(address),
      marketService.getBalance(address),
    ]).then(([portfolio, historyRes, redemptionRes, balanceRes]) => {
      setPositions(portfolio.positions);
      setOpenOrders(portfolio.openOrders);
      setTotalPnl(portfolio.totalPnl / SCALE);
      setTradeHistory(historyRes);
      setRedeemable(redemptionRes.redeemableMarkets);
      setBalance(
        balanceRes
          ? { eth: balanceRes.ethFormatted, usdc: balanceRes.usdcFormatted }
          : null
      );
      setLoading(false);
    });
  }, [address, setPositions]);

  const handleRedeem = async (r: { marketId: number; question: string; winningOutcome: string; redeemableShares: number; profit: number; contractAddress?: string }) => {
    if (!address) return;
    setRedeemingMarketId(r.marketId);
    try {
      const claimData = await marketService.claimRewards(address, r.marketId);
      const contractAddr = (claimData.contractAddress ?? r.contractAddress) as `0x${string}`;
      await redeemWinning(claimData.marketId, BigInt(claimData.redeemableShares), contractAddr);
      useToastStore.getState().success("Redeemed successfully.");
      const redemptionRes = await marketService.getRedemptionStatus(address);
      setRedeemable(redemptionRes.redeemableMarkets);
      const balanceRes = await marketService.getBalance(address);
      if (balanceRes) setBalance({ eth: balanceRes.ethFormatted, usdc: balanceRes.usdcFormatted });
    } catch (e) {
      const message = consolidateErrorMessage(e, "Redeem failed.");
      useToastStore.getState().error(message, "Redeem failed");
    } finally {
      setRedeemingMarketId(null);
    }
  };

  if (!isConnected) {
    return (
      <div className="mx-auto max-w-md space-y-6 py-12 text-center">
        <h1 className="text-2xl font-bold text-white">Portfolio</h1>
        <p className="text-muted-foreground">
          Connect your wallet to view positions and balance.
        </p>
        <ConnectWalletButton />
      </div>
    );
  }

  return (
    <div className="space-y-6">
      <header>
        <h1 className="text-2xl font-bold text-white">Portfolio</h1>
        <p className="mt-1 text-muted-foreground">
          Your balance and open positions
        </p>
      </header>
      <div className="grid gap-6 md:grid-cols-3">
        <Card>
          <h3 className="text-sm font-medium text-muted-foreground">
            USDC balance
          </h3>
          <p className="mt-2 text-3xl font-bold text-yes">
            {balance ? formatCurrency(balance.usdc) : "—"}
          </p>
        </Card>
        <Card>
          <h3 className="text-sm font-medium text-muted-foreground">
            ETH (gas)
          </h3>
          <p className="mt-2 text-2xl font-bold text-white">
            {balance ? formatCurrency(balance.eth) : "—"}
          </p>
        </Card>
        <Card>
          <h3 className="text-sm font-medium text-muted-foreground">
            Total P/L
          </h3>
          <p className={`mt-2 text-3xl font-bold ${totalPnl >= 0 ? "text-yes" : "text-no"}`}>
            {totalPnl >= 0 ? "+" : ""}{formatCurrency(totalPnl)}
          </p>
        </Card>
      </div>

      {redeemable.length > 0 && (
        <Card>
          <h3 className="text-sm font-medium text-muted-foreground">
            Redeemable winnings
          </h3>
          <ul className="mt-2 space-y-2 text-sm">
            {redeemable.map((r) => (
              <li key={r.marketId} className="flex flex-wrap items-center justify-between gap-2">
                <span className="truncate text-muted-foreground">{r.question}</span>
                <span className="flex items-center gap-2">
                  <span className="text-yes">
                    {r.winningOutcome} · {formatCurrency(r.profit / SCALE)} profit
                  </span>
                  <button
                    type="button"
                    onClick={() => handleRedeem(r)}
                    disabled={redeemingMarketId === r.marketId}
                    className="rounded-md bg-yes px-2 py-1 text-xs font-medium text-black hover:bg-yes/90 disabled:opacity-50"
                  >
                    {redeemingMarketId === r.marketId ? "Redeeming…" : "Redeem"}
                  </button>
                </span>
              </li>
            ))}
          </ul>
        </Card>
      )}

      {openOrders.length > 0 && (
        <Card>
          <h3 className="text-sm font-medium text-muted-foreground">
            Open orders
          </h3>
          <ul className="mt-2 space-y-2 text-sm">
            {openOrders.map((o) => (
              <li key={o.orderId} className="flex justify-between">
                <span>Market #{o.marketId} · {o.token}</span>
                <span>{formatCurrency(o.cost / SCALE)} @ {o.price}¢</span>
              </li>
            ))}
          </ul>
        </Card>
      )}

      {loading ? (
        <p className="py-8 text-center text-muted-foreground">Loading...</p>
      ) : (
        <>
          <PositionTable positions={positions} />
          {tradeHistory.length > 0 && (
            <Card>
              <h3 className="text-sm font-medium text-muted-foreground">
                Trade history
              </h3>
              <ul className="mt-2 max-h-64 space-y-2 overflow-y-auto text-sm">
                {tradeHistory.slice(0, 20).map((t) => (
                  <li key={t.tradeId} className="flex justify-between">
                    <span className="truncate text-muted-foreground">{t.question}</span>
                    <span className={t.token === "YES" ? "text-yes-muted" : "text-no-muted"}>
                      {t.token} {formatCurrency(t.cost / SCALE)} @ {t.priceCents}¢
                    </span>
                  </li>
                ))}
              </ul>
            </Card>
          )}
        </>
      )}
    </div>
  );
}
