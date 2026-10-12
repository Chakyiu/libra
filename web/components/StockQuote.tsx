"use client";

import { useEffect, useId, useRef, useState } from "react";

type Bar = { t: number; p: number; v?: number | null };

const RANGES = [
  ["1d", "1D"],
  ["5d", "5D"],
  ["1m", "1M"],
  ["6m", "6M"],
  ["ytd", "YTD"],
  ["1y", "1Y"],
  ["5y", "5Y"],
] as const;

const num = (v: unknown) => (typeof v === "number" && Number.isFinite(v) ? v : null);

function barsOf(raw: unknown): Bar[] {
  if (!Array.isArray(raw)) return [];
  const points = raw.filter(
    (p): p is Bar =>
      !!p && typeof p.t === "number" && typeof p.p === "number" && Number.isFinite(p.p) && Number.isFinite(p.t),
  );
  if (points.length < 3) return points;
  const drop = points.map(() => false);
  for (let i = 1; i < points.length - 1; i++) {
    const a = points[i - 1].p;
    const b = points[i].p;
    const c = points[i + 1].p;
    if (a <= 0 || b <= 0 || c <= 0) continue;
    const neighbors = Math.abs(a - c) / Math.max(a, c);
    if (neighbors < 0.005 && Math.abs(b - a) / a > 0.006 && Math.abs(b - c) / c > 0.006) drop[i] = true;
  }
  return points.filter((_, i) => !drop[i]);
}

function usd(n: number, currency: string) {
  const abs = Math.abs(n);
  const digits = abs !== 0 && abs < 0.01 ? 4 : 2;
  return n.toLocaleString("en-US", {
    style: "currency",
    currency: currency || "USD",
    minimumFractionDigits: digits,
    maximumFractionDigits: digits,
  });
}

function signedUsd(n: number, currency: string) {
  const sign = n > 0 ? "+" : n < 0 ? "−" : "";
  return `${sign}${usd(Math.abs(n), currency)}`;
}

function compact(n: number, dollar: boolean) {
  const abs = Math.abs(n);
  const units: [number, string][] = [
    [1e12, "T"],
    [1e9, "B"],
    [1e6, "M"],
    [1e3, "K"],
  ];
  for (const [div, suffix] of units) {
    if (abs >= div) return `${dollar ? "$" : ""}${(abs / div).toFixed(2)}${suffix}`;
  }
  return `${dollar ? "$" : ""}${Math.round(abs).toLocaleString("en-US")}`;
}

function pair(a: unknown, b: unknown, currency: string) {
  const lo = num(a);
  const hi = num(b);
  if (lo == null || hi == null) return "—";
  return `${usd(lo, currency)}–${usd(hi, currency)}`;
}

function Caret({ up }: { up: boolean }) {
  return (
    <svg className="quote__caret" width="9" height="9" viewBox="0 0 10 10" aria-hidden="true">
      {up ? <path d="M5 1.2 9.2 8.4H.8Z" /> : <path d="M5 8.8.8 1.6h8.4Z" />}
    </svg>
  );
}

function Delta({ change, pct, currency }: { change: number | null; pct: number | null; currency: string }) {
  if (change == null) return null;
  const up = change >= 0;
  return (
    <span className={up ? "quote__delta is-up" : "quote__delta is-down"}>
      {signedUsd(change, currency)}
      <Caret up={up} />
      {pct != null && <span>{Math.abs(pct).toFixed(2)}%</span>}
    </span>
  );
}

function Logo({ src, symbol }: { src?: string; symbol: string }) {
  const [ok, setOk] = useState(Boolean(src));
  useEffect(() => setOk(Boolean(src)), [src]);
  return (
    <span className="quote__logo" aria-hidden="true">
      {ok && src ? (
        // eslint-disable-next-line @next/next/no-img-element
        <img src={src} alt="" referrerPolicy="no-referrer" onError={() => setOk(false)} />
      ) : (
        <b>{symbol.slice(0, 1)}</b>
      )}
    </span>
  );
}

function clock(sec: number, tz: string) {
  const parts = new Intl.DateTimeFormat("en-US", {
    timeZone: tz,
    hour: "numeric",
    minute: "numeric",
    hourCycle: "h23",
  }).formatToParts(new Date(sec * 1000));
  return {
    h: Number(parts.find((p) => p.type === "hour")?.value),
    m: Number(parts.find((p) => p.type === "minute")?.value),
  };
}

function tipWhen(sec: number, tz: string, granularity: string) {
  const opts: Intl.DateTimeFormatOptions =
    granularity === "minute"
      ? { timeZone: tz, hour: "numeric", minute: "2-digit" }
      : { timeZone: tz, month: "short", day: "numeric", year: "numeric" };
  return new Intl.DateTimeFormat("en-US", opts).format(new Date(sec * 1000));
}

function Chart({
  points,
  prev,
  granularity,
  tz,
  currency,
  up,
  loading,
}: {
  points: Bar[];
  prev: number | null;
  granularity: string;
  tz: string;
  currency: string;
  up: boolean;
  loading: boolean;
}) {
  const box = useRef<HTMLDivElement>(null);
  const [width, setWidth] = useState(0);
  const [hover, setHover] = useState<number | null>(null);
  const uid = useId().replace(/:/g, "");

  useEffect(() => {
    const el = box.current;
    if (!el) return;
    const apply = () => setWidth(el.clientWidth);
    apply();
    const ro = new ResizeObserver(apply);
    ro.observe(el);
    return () => ro.disconnect();
  }, []);

  useEffect(() => setHover(null), [points]);

  const H = 248;
  const plotL = 48;
  const plotR = 12;
  const plotT = 18;
  const vols = points.map((p) => (typeof p.v === "number" && p.v > 0 ? p.v : 0));
  const hasVol = vols.some((v) => v > 0);
  const plotB = hasVol ? 74 : 30;
  const plotW = Math.max(1, width - plotL - plotR);
  const plotH = H - plotT - plotB;

  const prices = points.map((p) => p.p);
  let lo = Math.min(...prices);
  let hi = Math.max(...prices);
  const span0 = hi - lo || Math.abs(hi) * 0.02 || 1;
  const showPrev = prev != null && granularity === "minute" && prev > lo - span0 && prev < hi + span0;
  if (showPrev && prev != null) {
    lo = Math.min(lo, prev);
    hi = Math.max(hi, prev);
  }
  if (hi - lo < 1e-9) {
    lo -= Math.abs(lo) * 0.01 || 1;
    hi += Math.abs(hi) * 0.01 || 1;
  }
  const pad = (hi - lo) * 0.14;
  lo -= pad;
  hi += pad;

  const t0 = points[0]?.t ?? 0;
  const t1 = points[points.length - 1]?.t ?? t0;
  const xOf = (t: number) => plotL + (t1 === t0 ? plotW / 2 : ((t - t0) / (t1 - t0)) * plotW);
  const labelX = (x: number) => Math.min(width - 26, Math.max(plotL + 16, x));
  const yOf = (p: number) => plotT + (1 - (p - lo) / (hi - lo)) * plotH;

  const line = points
    .map((p, i) => `${i ? "L" : "M"}${xOf(p.t).toFixed(1)} ${yOf(p.p).toFixed(1)}`)
    .join("");
  const base = plotT + plotH;
  const area =
    points.length > 1
      ? `${line} L${xOf(t1).toFixed(1)} ${base.toFixed(1)} L${xOf(t0).toFixed(1)} ${base.toFixed(1)} Z`
      : "";

  const yTicks = [0.16, 0.5, 0.84].map((f) => lo + (hi - lo) * (1 - f));
  const span = hi - lo;
  const xLabels =
    granularity === "minute"
      ? [6, 9, 12, 15, 18].flatMap((hour) => {
          let best = -1;
          let bestDist = 20;
          points.forEach((p, i) => {
            const c = clock(p.t, tz);
            const dist = Math.abs(c.h * 60 + c.m - hour * 60);
            if (dist < bestDist) {
              bestDist = dist;
              best = i;
            }
          });
          if (best < 0) return [];
          const h12 = hour % 12 || 12;
          return [{ x: labelX(xOf(points[best].t)), text: `${h12} ${hour < 12 ? "AM" : "PM"}` }];
        })
      : Array.from({ length: Math.min(5, points.length) }, (_, i) => {
          const count = Math.min(5, points.length);
          const idx = Math.round((i * (points.length - 1)) / Math.max(1, count - 1));
          const sec = points[idx].t;
          const spanDays = (t1 - t0) / 86400;
          const text = new Intl.DateTimeFormat("en-US", {
            timeZone: tz,
            month: "short",
            ...(spanDays > 500 ? { year: "2-digit" } : { day: "numeric" }),
          }).format(new Date(sec * 1000));
          return { x: labelX(xOf(sec)), text };
        });

  const maxV = Math.max(...vols, 0);
  const slot = plotW / Math.max(1, points.length);
  const onMove = (e: React.PointerEvent<SVGSVGElement>) => {
    const rect = e.currentTarget.getBoundingClientRect();
    const x = ((e.clientX - rect.left) / rect.width) * width;
    let loI = 0;
    let hiI = points.length - 1;
    while (loI < hiI) {
      const mid = (loI + hiI) >> 1;
      if (xOf(points[mid].t) < x) loI = mid + 1;
      else hiI = mid;
    }
    const prevI = Math.max(0, loI - 1);
    const i = Math.abs(xOf(points[prevI].t) - x) <= Math.abs(xOf(points[loI].t) - x) ? prevI : loI;
    setHover(i);
  };
  const hot = hover != null ? points[hover] : null;

  return (
    <div className={up ? "quote__plot is-up" : "quote__plot is-down"} ref={box} data-loading={loading || undefined}>
      {width > 40 && (
        <svg
          width={width}
          height={H}
          viewBox={`0 0 ${width} ${H}`}
          role="img"
          aria-label="Price chart"
          onPointerMove={onMove}
          onPointerLeave={() => setHover(null)}
        >
          <defs>
            <pattern id={`${uid}-dots`} width="13" height="13" patternUnits="userSpaceOnUse">
              <circle cx="1" cy="1" r="0.7" fill="var(--fg)" opacity="0.16" />
            </pattern>
            <linearGradient id={`${uid}-fill`} x1="0" y1="0" x2="0" y2="1">
              <stop offset="0%" stopColor="var(--line)" stopOpacity="0.34" />
              <stop offset="100%" stopColor="var(--line)" stopOpacity="0" />
            </linearGradient>
          </defs>
          <rect x={plotL} y={plotT} width={plotW} height={plotH + (hasVol ? 46 : 0)} fill={`url(#${uid}-dots)`} />
          {yTicks.map((p) => (
            <text key={p} className="quote__tick" x={plotL - 8} y={yOf(p) + 3} textAnchor="end">
              {span < 8 || p < 100 ? p.toFixed(2) : p.toFixed(0)}
            </text>
          ))}
          {showPrev && prev != null && (
            <line
              x1={plotL}
              x2={plotL + plotW}
              y1={yOf(prev)}
              y2={yOf(prev)}
              stroke="var(--muted)"
              strokeDasharray="3 4"
              strokeWidth="1"
            />
          )}
          {hasVol &&
            points.map((p, i) => {
              const v = vols[i];
              if (!v) return null;
              const bh = Math.max(1, (v / maxV) * 34);
              const bw = Math.max(1, slot * 0.7);
              const prior = i > 0 ? points[i - 1].p : p.p;
              return (
                <rect
                  key={p.t}
                  x={xOf(p.t) - bw / 2}
                  y={base + 8 + (34 - bh)}
                  width={bw}
                  height={bh}
                  fill={p.p >= prior ? "var(--success)" : "var(--accent)"}
                  opacity="0.55"
                />
              );
            })}
          {area && <path d={area} fill={`url(#${uid}-fill)`} />}
          {points.length > 1 ? (
            <path d={line} fill="none" stroke="currentColor" strokeWidth="1.75" strokeLinejoin="round" strokeLinecap="round" />
          ) : (
            <circle cx={xOf(t0)} cy={yOf(points[0].p)} r="3" fill="currentColor" />
          )}
          {xLabels.map((l) => (
            <text key={`${l.text}${l.x}`} className="quote__tick" x={l.x} y={H - 8} textAnchor="middle">
              {l.text}
            </text>
          ))}
          {hot && (
            <g>
              <line x1={xOf(hot.t)} x2={xOf(hot.t)} y1={plotT} y2={base} stroke="var(--muted)" strokeWidth="1" />
              <circle cx={xOf(hot.t)} cy={yOf(hot.p)} r="3.5" fill="var(--surface)" stroke="currentColor" strokeWidth="2" />
            </g>
          )}
        </svg>
      )}
      {showPrev && prev != null && width > 40 && (
        <span className="quote__prev" style={{ top: yOf(prev) }}>
          Prev close {usd(prev, currency)}
        </span>
      )}
      {hot && width > 40 && (
        <span
          className="quote__tip"
          style={{
            left: Math.min(width - 8, Math.max(8, xOf(hot.t))),
            top: yOf(hot.p) < 56 ? yOf(hot.p) + 14 : yOf(hot.p) - 10,
            transform: yOf(hot.p) < 56 ? "translate(-50%, 0)" : "translate(-50%, -100%)",
          }}
        >
          <b>{usd(hot.p, currency)}</b>
          <span>{tipWhen(hot.t, tz, granularity)}</span>
        </span>
      )}
    </div>
  );
}

export function StockQuote({ data }: { data: any }) {
  const currency = String(data.currency || "USD");
  const symbol = String(data.symbol || "")
    .replace(/\.US$/i, "")
    .toUpperCase();
  const price = num(data.price) ?? num(data.close);
  const open = num(data.open);
  const change = num(data.change) ?? (price != null && open != null ? price - open : null);
  const pct =
    num(data.change_percent) ?? (change != null && open ? (change / open) * 100 : null);
  const home = String(data.range || "1d");
  const embedded = barsOf(data.points);
  const [range, setRange] = useState(home);
  const [series, setSeries] = useState<Bar[]>(embedded);
  const [granularity, setGranularity] = useState(String(data.granularity || (embedded.length ? "minute" : "day")));
  const [loading, setLoading] = useState(false);
  const shown = useRef(home);

  useEffect(() => {
    const next = barsOf(data.points);
    const homeRange = String(data.range || "1d");
    if (range === homeRange && next.length) {
      setSeries(next);
      setGranularity(String(data.granularity || "minute"));
      setLoading(false);
      shown.current = homeRange;
      return;
    }
    if (!/^[A-Z]{1,5}(\.[A-Z])?$/.test(symbol)) return;
    let gone = false;
    const ctrl = new AbortController();
    setLoading(true);
    fetch(`/api/stocks/${encodeURIComponent(symbol)}?range=${encodeURIComponent(range)}`, { signal: ctrl.signal })
      .then((r) => (r.ok ? r.json() : null))
      .then((body) => {
        if (gone) return;
        const points = barsOf(body?.points);
        if (!points.length) {
          setRange(shown.current);
          return;
        }
        setSeries(points);
        setGranularity(String(body.granularity || "day"));
        shown.current = range;
      })
      .catch(() => {
        if (!gone) setRange(shown.current);
      })
      .finally(() => {
        if (!gone) setLoading(false);
      });
    return () => {
      gone = true;
      ctrl.abort();
    };
  }, [range, data, symbol]);

  if (price == null || !symbol) return null;

  const name = String(data.name || symbol);
  const exchange = String(data.exchange || "");
  const tz = String(data.timezone || "America/New_York");
  const extended = data.extended && num(data.extended.price) != null ? data.extended : null;
  const up = (change ?? 0) >= 0;
  const when = data.as_of
    ? `${data.session_label || "At close"}: ${data.as_of}`
    : data.date
      ? `${data.date}${data.time ? ` ${data.time}` : ""}`
      : String(data.session_label || "");
  const canRange = /^[A-Z]{1,5}(\.[A-Z])?$/.test(symbol);
  const low = num(data.low);
  const high = num(data.high);
  const eps = num(data.eps);
  const volume = num(data.volume);
  const stats: [string, string][] = [
    ["Forward P/E", num(data.forward_pe) == null ? "—" : num(data.forward_pe)!.toFixed(2)],
    ["Market Cap", num(data.market_cap) == null ? "—" : compact(num(data.market_cap)!, true)],
    ["Open", open == null ? "—" : usd(open, currency)],
    ["Trailing P/E", num(data.trailing_pe) == null ? "—" : num(data.trailing_pe)!.toFixed(2)],
    ["Day Range", pair(low, high, currency)],
    ["Dividend Yield", num(data.dividend_yield) == null ? "—" : `${num(data.dividend_yield)!.toFixed(2)}%`],
    ["52W Range", pair(data.week_low, data.week_high, currency)],
    ["EPS", eps == null ? "—" : usd(eps, currency)],
    ["Volume", volume == null ? "—" : compact(volume, false)],
  ];

  return (
    <aside className="quote" aria-label={`${name} stock quote`}>
      <header className="quote__id">
        <Logo src={typeof data.logo === "string" ? data.logo : undefined} symbol={symbol} />
        <div>
          <p className="quote__name">{name}</p>
          <p className="meta quote__sym">
            {symbol}
            {exchange && ` · ${exchange}`}
            {data.country === "US" && (
              <>
                {" · "}
                <span className="quote__flag" aria-label="United States">
                  🇺🇸
                </span>
              </>
            )}
          </p>
        </div>
      </header>

      <div className="quote__prices" data-split={extended ? "true" : undefined}>
        <div>
          <p className="quote__px">
            {usd(price, currency)}
            <Delta change={change} pct={pct} currency={currency} />
          </p>
          {when && <p className="quote__when">{when}</p>}
        </div>
        {extended && (
          <div>
            <p className="quote__px">
              {usd(extended.price, currency)}
              <Delta change={num(extended.change)} pct={num(extended.change_percent)} currency={currency} />
            </p>
            <p className="quote__when">
              {extended.label || "After-hours"}
              {extended.as_of ? `: ${extended.as_of}` : ""}
            </p>
          </div>
        )}
      </div>

      {(series.length > 0 || canRange) && (
        <>
          {canRange && (
            <div className="quote__ranges" role="group" aria-label="Chart range">
              {RANGES.map(([id, label]) => (
                <button key={id} type="button" aria-pressed={range === id} onClick={() => setRange(id)}>
                  {label}
                </button>
              ))}
            </div>
          )}
          {series.length > 0 && (
            <Chart
              points={series}
              prev={num(data.previous_close)}
              granularity={granularity}
              tz={tz}
              currency={currency}
              up={up}
              loading={loading}
            />
          )}
        </>
      )}

      <dl className="quote__stats">
        {stats.map(([label, value]) => (
          <div key={label} className="quote__stat">
            <dt>{label}</dt>
            <dd>{value}</dd>
          </div>
        ))}
      </dl>
    </aside>
  );
}
