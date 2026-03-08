import { API_BASE } from "../config/api";
import type { Market } from "../types/market";
import type { OrderBookEntry } from "../types/trade";
import type { Position } from "../types/trade";
import type { ChartDataPoint } from "../stores/chartStore";

// ── Backend response types ───────────────────────────────────────────────────

interface BackendMarketItem {
  marketId: number;
  question: string;
  category: string;
  yesPrice: number;
  noPrice: number;
  resolutionDate: string;
  status: string;
  outcome: string | null;
  createdAt: string | null;
  totalVolume?: number;
}

interface BackendMarketsResponse {
  markets: BackendMarketItem[];
  total: number;
}

interface BackendOrderbookLevel {
  price: number;
  shares: number;
  orders: number;
}

interface BackendOrderbookResponse {
  market_id: number;
  bids: BackendOrderbookLevel[];
  no_bids: BackendOrderbookLevel[];
  yes_price: number;
  no_price: number;
}

interface BackendPriceHistoryRow {
  yes_price: number;
  no_price: number;
  timestamp: string | null;
}

interface BackendMarketDetailResponse {
  marketId: number;
  question: string;
  category: string;
  yesPrice: number;
  noPrice: number;
  totalVolume: number;
  resolutionDate: string;
  status: string;
  createdAt: string | null;
  priceHistory: BackendPriceHistoryRow[];
}

interface BackendPortfolioPosition {
  marketId: number;
  question: string;
  token: string;
  shares: number;
  totalCost: number;
  avgBuyPrice: number;
  currentValue: number;
  unrealizedPnl: number;
}

interface BackendPortfolioResponse {
  positions: BackendPortfolioPosition[];
}

export interface BackendCategory {
  category: string;
  marketCount: number;
  openCount: number;
}

export interface BackendActivityItem {
  type: string;
  address: string;
  token: string;
  shares: number;
  cost: number;
  priceCents: number;
  txHash: string;
  timestamp: string | null;
}

// USDC/shares use 6 decimals
const SCALE = 1e6;

function mapBackendMarketToFrontend(m: BackendMarketItem, volume?: number): Market {
  const vol = volume ?? (typeof m.totalVolume === "number" ? m.totalVolume / SCALE : 0);
  return {
    id: String(m.marketId),
    title: m.question,
    description: "",
    yesProbability: m.yesPrice / 100,
    noProbability: m.noPrice / 100,
    volume: vol,
    liquidity: 0,
    participants: 0,
    endDate: typeof m.resolutionDate === "string" ? m.resolutionDate : new Date((m.resolutionDate as number) * 1000).toISOString(),
    category: m.category,
    createdAt: m.createdAt ?? new Date().toISOString(),
  };
}

export const marketService = {
  async getMarkets(params?: {
    status?: string;
    category?: string;
    limit?: number;
    offset?: number;
  }): Promise<{ markets: Market[]; total: number }> {
    try {
      const searchParams = new URLSearchParams();
      searchParams.set("status", params?.status ?? "open");
      if (params?.category && params.category !== "all")
        searchParams.set("category", params.category);
      searchParams.set("limit", String(params?.limit ?? 100));
      if (params?.offset) searchParams.set("offset", String(params.offset));
      const res = await fetch(
        `${API_BASE}/api/markets?${searchParams.toString()}`
      );
      if (!res.ok) return { markets: [], total: 0 };
      const data = (await res.json()) as BackendMarketsResponse;
      const markets = (data.markets ?? []).map((m) =>
        mapBackendMarketToFrontend(m)
      );
      return { markets, total: data.total ?? 0 };
    } catch {
      return { markets: [], total: 0 };
    }
  },

  async getTrendingMarkets(limit = 10): Promise<Market[]> {
    try {
      const res = await fetch(
        `${API_BASE}/api/markets/trending?limit=${limit}`
      );
      if (!res.ok) return [];
      const data = (await res.json()) as { markets: BackendMarketItem[] };
      return (data.markets ?? []).map((m) => mapBackendMarketToFrontend(m));
    } catch {
      return [];
    }
  },

  async getCategories(): Promise<BackendCategory[]> {
    try {
      const res = await fetch(`${API_BASE}/api/markets/categories`);
      if (!res.ok) return [];
      const data = (await res.json()) as { categories: BackendCategory[] };
      return data.categories ?? [];
    } catch {
      return [];
    }
  },

  async searchMarkets(q: string, limit = 20): Promise<Market[]> {
    if (!q.trim()) return [];
    try {
      const res = await fetch(
        `${API_BASE}/api/markets/search?q=${encodeURIComponent(q.trim())}&limit=${limit}`
      );
      if (!res.ok) return [];
      const data = (await res.json()) as { markets: BackendMarketItem[] };
      return (data.markets ?? []).map((m) => mapBackendMarketToFrontend(m));
    } catch {
      return [];
    }
  },

  async getMarketActivity(marketId: string): Promise<BackendActivityItem[]> {
    const id = parseInt(marketId, 10);
    if (Number.isNaN(id)) return [];
    try {
      const res = await fetch(`${API_BASE}/api/markets/${id}/activity`);
      if (!res.ok) return [];
      const data = (await res.json()) as { activity: BackendActivityItem[] };
      return data.activity ?? [];
    } catch {
      return [];
    }
  },

  async getMarketById(id: string): Promise<Market | null> {
    const marketId = parseInt(id, 10);
    if (Number.isNaN(marketId)) return null;
    const res = await fetch(`${API_BASE}/api/markets/${marketId}`);
    if (!res.ok) {
      if (res.status === 404) return null;
      throw new Error(`Failed to fetch market: ${res.status}`);
    }
    const data = (await res.json()) as BackendMarketDetailResponse;
    return {
      id: String(data.marketId),
      title: data.question,
      description: "",
      yesProbability: data.yesPrice / 100,
      noProbability: data.noPrice / 100,
      volume: data.totalVolume / SCALE,
      liquidity: 0,
      participants: 0,
      endDate: data.resolutionDate,
      category: data.category,
      createdAt: data.createdAt ?? new Date().toISOString(),
    };
  },

  async getChartData(marketId: string): Promise<ChartDataPoint[]> {
    const id = parseInt(marketId, 10);
    if (Number.isNaN(id)) return [];
    const res = await fetch(`${API_BASE}/api/markets/${id}`);
    if (!res.ok) return [];
    const data = (await res.json()) as BackendMarketDetailResponse;
    const history = data.priceHistory ?? [];
    return history.map((p) => ({
      time: p.timestamp
        ? String(Math.floor(new Date(p.timestamp).getTime() / 1000))
        : "",
      value: p.yes_price / 100,
    }));
  },

  async getMarketPrice(marketId: string): Promise<{
    yesPrice: number;
    noPrice: number;
    yesProbability: number;
    noProbability: number;
  } | null> {
    const id = parseInt(marketId, 10);
    if (Number.isNaN(id)) return null;
    try {
      const res = await fetch(`${API_BASE}/api/markets/${id}/price`);
      if (!res.ok) return null;
      return res.json();
    } catch {
      return null;
    }
  },

  async getOrderBook(marketId: string): Promise<OrderBookEntry[]> {
    const id = parseInt(marketId, 10);
    if (Number.isNaN(id)) return [];
    const res = await fetch(`${API_BASE}/api/orders/${id}`);
    if (!res.ok) return [];
    const ob = (await res.json()) as BackendOrderbookResponse;
    const bids: OrderBookEntry[] = (ob.bids ?? []).map((b) => ({
      price: b.price / 100,
      amount: b.shares / SCALE,
      side: "YES" as const,
    }));
    const asks: OrderBookEntry[] = (ob.no_bids ?? []).map((b) => ({
      price: b.price / 100,
      amount: b.shares / SCALE,
      side: "NO" as const,
    }));
    return [...bids, ...asks];
  },

  async createMarket(params: {
    question: string;
    category: string;
    resolutionDate: string;
    creatorAddress?: string;
  }): Promise<{ marketId: number; txHash: string }> {
    const res = await fetch(`${API_BASE}/api/markets`, {
      method: "POST",
      headers: { "Content-Type": "application/json" },
      body: JSON.stringify({
        question: params.question,
        category: params.category,
        resolution_date: params.resolutionDate,
        creator_address: params.creatorAddress ?? "",
      }),
    });
    const data = await res.json().catch(() => ({})) as { error?: string; marketId?: number; txHash?: string };
    if (!res.ok) {
      throw new Error(data.error ?? `Failed to create market: ${res.status}`);
    }
    return { marketId: data.marketId!, txHash: data.txHash ?? "" };
  },

  async getBalance(address: string | null): Promise<{
    eth: string;
    usdc: string;
    ethFormatted: number;
    usdcFormatted: number;
  } | null> {
    if (!address) return null;
    try {
      const res = await fetch(
        `${API_BASE}/api/portfolio/${encodeURIComponent(address)}/balance`
      );
      if (!res.ok) return null;
      const data = (await res.json()) as {
        eth: string;
        usdc: string;
        ethFormatted: number;
        usdcFormatted: number;
      };
      return data;
    } catch {
      return null;
    }
  },

  async getPositions(address: string | null): Promise<Position[]> {
    if (!address) return [];
    const res = await fetch(`${API_BASE}/api/portfolio/${encodeURIComponent(address)}`);
    if (!res.ok) return [];
    const data = (await res.json()) as BackendPortfolioResponse;
    const positions = data.positions ?? [];
    return positions.map((p) => ({
      id: `${p.marketId}-${p.token}`,
      marketId: String(p.marketId),
      marketTitle: p.question,
      side: p.token as "YES" | "NO",
      shares: p.shares / SCALE,
      avgPrice: p.avgBuyPrice / 100,
      currentValue: p.currentValue / SCALE,
      pnl: p.unrealizedPnl / SCALE,
    }));
  },

  async getPortfolio(address: string | null): Promise<{
    positions: Position[];
    openOrders: Array<{
      orderId: number;
      marketId: number;
      token: string;
      shares: number;
      cost: number;
      price: number;
      createdAt: string | null;
    }>;
    totalCost: number;
    totalPnl: number;
  }> {
    if (!address) return { positions: [], openOrders: [], totalCost: 0, totalPnl: 0 };
    const res = await fetch(`${API_BASE}/api/portfolio/${encodeURIComponent(address)}`);
    if (!res.ok) return { positions: [], openOrders: [], totalCost: 0, totalPnl: 0 };
    const data = (await res.json()) as {
      positions: BackendPortfolioPosition[];
      openOrders: Array<{
        orderId: number;
        marketId: number;
        token: string;
        shares: number;
        cost: number;
        price: number;
        createdAt: string | null;
      }>;
      totalCost: number;
      totalPnl: number;
    };
    const positions = (data.positions ?? []).map((p) => ({
      id: `${p.marketId}-${p.token}`,
      marketId: String(p.marketId),
      marketTitle: p.question,
      side: p.token as "YES" | "NO",
      shares: p.shares / SCALE,
      avgPrice: p.avgBuyPrice / 100,
      currentValue: p.currentValue / SCALE,
      pnl: p.unrealizedPnl / SCALE,
    }));
    return {
      positions,
      openOrders: data.openOrders ?? [],
      totalCost: data.totalCost ?? 0,
      totalPnl: data.totalPnl ?? 0,
    };
  },

  async getTradeHistory(address: string | null): Promise<
    Array<{
      tradeId: number;
      marketId: number;
      question: string;
      token: string;
      shares: number;
      cost: number;
      priceCents: number;
      timestamp: string | null;
      txHash: string;
    }>
  > {
    if (!address) return [];
    const res = await fetch(
      `${API_BASE}/api/portfolio/${encodeURIComponent(address)}/history`
    );
    if (!res.ok) return [];
    const data = (await res.json()) as { trades: Array<{
      tradeId: number;
      marketId: number;
      question: string;
      token: string;
      shares: number;
      cost: number;
      priceCents: number;
      timestamp: string | null;
      txHash: string;
    }> };
    return data.trades ?? [];
  },

  async getLeaderboard(params?: {
    sort?: "volume" | "profit" | "trades";
    limit?: number;
  }): Promise<{
    leaderboard: Array<{
      rank: number;
      address: string;
      tradeCount: number;
      totalVolume: number;
      marketsTraded: number;
      totalProfit: number;
    }>;
    sortedBy: string;
  }> {
    try {
      const searchParams = new URLSearchParams();
      if (params?.sort) searchParams.set("sort", params.sort);
      if (params?.limit) searchParams.set("limit", String(params.limit));
      const qs = searchParams.toString();
      const res = await fetch(`${API_BASE}/api/leaderboard${qs ? `?${qs}` : ""}`);
      if (!res.ok) return { leaderboard: [], sortedBy: "volume" };
      return res.json();
    } catch {
      return { leaderboard: [], sortedBy: "volume" };
    }
  },

  async getRedemptionStatus(address: string | null): Promise<{
    redeemableMarkets: Array<{
      marketId: number;
      question: string;
      winningOutcome: string;
      winningToken: string;
      redeemableShares: number;
      redemptionValue: number;
      originalCost: number;
      profit: number;
      contractAddress: string;
    }>;
    totalRedeemable: number;
  }> {
    if (!address) return { redeemableMarkets: [], totalRedeemable: 0 };
    const res = await fetch(
      `${API_BASE}/api/portfolio/${encodeURIComponent(address)}/redemption-status`
    );
    if (!res.ok) return { redeemableMarkets: [], totalRedeemable: 0 };
    const data = (await res.json()) as {
      redeemableMarkets: Array<{
        marketId: number;
        question: string;
        winningOutcome: string;
        winningToken: string;
        redeemableShares: number;
        redemptionValue: number;
        originalCost: number;
        profit: number;
        contractAddress: string;
      }>;
      totalRedeemable: number;
    };
    return {
      redeemableMarkets: data.redeemableMarkets ?? [],
      totalRedeemable: data.totalRedeemable ?? 0,
    };
  },

  /**
   * Claim rewards for a resolved market. Calls backend to validate and get claim data,
   * then the frontend executes redeemWinning on-chain with the returned args.
   */
  async claimRewards(
    address: string,
    marketId: number
  ): Promise<{
    marketId: number;
    redeemableShares: number;
    contractAddress: string;
    profit: number;
  }> {
    const res = await fetch(
      `${API_BASE}/api/portfolio/${encodeURIComponent(address)}/claim`,
      {
        method: "POST",
        headers: { "Content-Type": "application/json" },
        body: JSON.stringify({ market_id: marketId }),
      }
    );
    if (!res.ok) {
      const err = (await res.json().catch(() => ({}))) as { error?: string; detail?: string; message?: string };
      throw new Error(err?.error ?? err?.detail ?? err?.message ?? "Claim failed");
    }
    const data = (await res.json()) as {
      marketId: number;
      redeemableShares: number;
      contractAddress: string;
      profit: number;
    };
    return {
      marketId: data.marketId,
      redeemableShares: data.redeemableShares,
      contractAddress: data.contractAddress,
      profit: data.profit,
    };
  },
};
