import { useEffect, useRef, useMemo } from "react";
import {
  createChart,
  AreaSeries,
  type IChartApi,
  type ISeriesApi,
  type UTCTimestamp,
  ColorType,
  CrosshairMode,
} from "lightweight-charts";
import { useChartStore } from "../../stores/chartStore";

interface ProbabilityChartProps {
  height?: number;
  className?: string;
  yesProbability?: number;
  noProbability?: number;
}

/** lightweight-charts requires strictly ascending time. Sort and dedupe. */
function normalizeChartData(
  data: { time: string; value: number }[]
): { time: number; value: number }[] {
  const valid = data.filter((d) => d.time != null && d.time !== "");
  if (valid.length === 0) return [];

  const toTimestamp = (t: string) => {
    const n = Number(t);
    if (!Number.isNaN(n) && String(n) === t) return n;
    return Math.floor(new Date(t).getTime() / 1000);
  };

  const sorted = [...valid].sort(
    (a, b) => toTimestamp(a.time) - toTimestamp(b.time)
  );

  const deduped: { time: number; value: number }[] = [];
  for (const d of sorted) {
    const ts = toTimestamp(d.time);
    const last = deduped[deduped.length - 1];
    if (last && last.time === ts) {
      last.value = d.value;
    } else {
      deduped.push({ time: ts, value: d.value });
    }
  }
  return deduped;
}

export function ProbabilityChart({
  height = 320,
  className = "",
  yesProbability,
}: ProbabilityChartProps) {
  const containerRef = useRef<HTMLDivElement>(null);
  const chartRef = useRef<IChartApi | null>(null);
  const seriesRef = useRef<ISeriesApi<"Area"> | null>(null);
  const { data } = useChartStore();

  const chartData = useMemo(
    () =>
      normalizeChartData(data).map((d) => ({
        time: d.time as UTCTimestamp,
        value: d.value * 100,
      })),
    [data]
  );

  // Compute change from first to last data point
  const { currentPct, changePct } = useMemo(() => {
    if (yesProbability != null) {
      const cur = Math.round(yesProbability * 100);
      if (chartData.length >= 2) {
        const first = chartData[0].value;
        return { currentPct: cur, changePct: +(cur - first).toFixed(1) };
      }
      return { currentPct: cur, changePct: 0 };
    }
    if (chartData.length === 0) return { currentPct: 50, changePct: 0 };
    const cur = Math.round(chartData[chartData.length - 1].value);
    if (chartData.length >= 2) {
      const first = chartData[0].value;
      return { currentPct: cur, changePct: +(cur - first).toFixed(1) };
    }
    return { currentPct: cur, changePct: 0 };
  }, [chartData, yesProbability]);

  useEffect(() => {
    if (!containerRef.current) return;

    // Cleanup previous chart
    if (chartRef.current) {
      chartRef.current.remove();
      chartRef.current = null;
      seriesRef.current = null;
    }

    if (chartData.length === 0) return;

    const chart = createChart(containerRef.current, {
      layout: {
        background: { type: ColorType.Solid, color: "transparent" },
        textColor: "#64748b",
        fontFamily:
          '"Inter", ui-sans-serif, system-ui, sans-serif',
        fontSize: 11,
      },
      grid: {
        vertLines: { visible: false },
        horzLines: { color: "rgba(148, 163, 184, 0.06)", style: 3 },
      },
      rightPriceScale: {
        borderVisible: false,
        scaleMargins: { top: 0.08, bottom: 0.08 },
      },
      timeScale: {
        borderVisible: false,
        timeVisible: true,
        secondsVisible: false,
        rightOffset: 2,
        fixLeftEdge: true,
        fixRightEdge: true,
      },
      crosshair: {
        mode: CrosshairMode.Magnet,
        vertLine: {
          color: "rgba(99, 102, 241, 0.3)",
          width: 1,
          style: 2,
          labelBackgroundColor: "#1e1b4b",
        },
        horzLine: {
          color: "rgba(99, 102, 241, 0.3)",
          width: 1,
          style: 2,
          labelBackgroundColor: "#1e1b4b",
        },
      },
      height,
    });

    const areaSeries = chart.addSeries(AreaSeries, {
      lineColor: "#3b82f6",
      lineWidth: 2,
      topColor: "rgba(59, 130, 246, 0.25)",
      bottomColor: "rgba(59, 130, 246, 0.0)",
      crosshairMarkerRadius: 5,
      crosshairMarkerBorderColor: "#3b82f6",
      crosshairMarkerBackgroundColor: "#1e293b",
      crosshairMarkerBorderWidth: 2,
      priceFormat: {
        type: "custom",
        formatter: (price: number) => `${Math.round(price)}%`,
        minMove: 0.01,
      },
      lastValueVisible: true,
      priceLineVisible: false,
    }) as ISeriesApi<"Area">;

    areaSeries.setData(chartData);
    chart.timeScale().fitContent();

    chartRef.current = chart;
    seriesRef.current = areaSeries;

    const handleResize = () => {
      if (containerRef.current && chartRef.current) {
        chartRef.current.applyOptions({
          width: containerRef.current.clientWidth,
        });
      }
    };
    window.addEventListener("resize", handleResize);

    return () => {
      window.removeEventListener("resize", handleResize);
      chart.remove();
      chartRef.current = null;
      seriesRef.current = null;
    };
  }, [height, chartData]);

  if (!data.length) {
    return (
      <div
        className={`flex items-center justify-center rounded-xl border border-white/5 bg-white/[0.02] text-sm text-muted-foreground ${className}`}
        style={{ height }}
      >
        No chart data yet
      </div>
    );
  }

  const isPositive = changePct >= 0;

  return (
    <div className={className}>
      {/* Polymarket-style header */}
      <div className="mb-4 flex items-baseline gap-3">
        <span className="text-xs font-semibold uppercase tracking-wider text-blue-400">
          Yes
        </span>
        <span className="text-3xl font-bold tracking-tight text-white">
          {currentPct}%{" "}
          <span className="text-lg font-normal text-slate-400">chance</span>
        </span>
        {changePct !== 0 && (
          <span
            className={`flex items-center gap-0.5 text-sm font-medium ${
              isPositive ? "text-emerald-400" : "text-red-400"
            }`}
          >
            {isPositive ? "\u25B2" : "\u25BC"} {Math.abs(changePct)}%
          </span>
        )}
      </div>

      {/* Chart */}
      <div
        ref={containerRef}
        style={{ height }}
        className="rounded-lg"
      />
    </div>
  );
}
