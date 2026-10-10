"use client";

import { Block, SearchResult, favicon, host } from "@/lib/api";
import Answer from "./Answer";

const WMO: Record<number, string> = {
  0: "☀️", 1: "🌤️", 2: "⛅", 3: "☁️", 45: "🌫️", 48: "🌫️", 51: "🌦️", 53: "🌦️", 55: "🌧️",
  61: "🌧️", 63: "🌧️", 65: "⛈️", 71: "🌨️", 73: "🌨️", 75: "❄️", 80: "🌦️", 81: "🌧️", 82: "⛈️", 95: "⛈️",
};

function Card({ children }: { children: React.ReactNode }) {
  return <div className="border border-edge bg-panel rounded-lg p-4 my-3 max-w-md">{children}</div>;
}

function Weather({ data }: { data: any }) {
  const c = data.current ?? {};
  const unit = data.unit === "fahrenheit" ? "°F" : "°C";
  const days: string[] = data.forecast?.time ?? [];
  return (
    <Card>
      <div className="flex items-baseline gap-3">
        <span className="text-3xl">{WMO[c.code] ?? "🌡️"}</span>
        <span className="text-2xl font-semibold">{Math.round(c.temperature)}{unit}</span>
        <span className="text-mute text-sm">{data.location}</span>
      </div>
      <div className="text-xs text-mute mt-2 grid grid-cols-3 gap-2">
        <span>feels {Math.round(c.feels_like)}{unit}</span>
        <span>humidity {c.humidity}%</span>
        <span>wind {Math.round(c.wind_speed)} km/h</span>
      </div>
      {days.length > 1 && (
        <div className="mt-3 grid grid-cols-5 gap-1 text-center text-xs">
          {days.slice(1, 6).map((d, i) => (
            <div key={d} className="border border-edge rounded p-1">
              <div className="text-mute">{d.slice(5)}</div>
              <div>{WMO[data.forecast.code?.[i + 1]] ?? "•"}</div>
              <div>{Math.round(data.forecast.max?.[i + 1])}{unit}</div>
            </div>
          ))}
        </div>
      )}
    </Card>
  );
}

function Stocks({ data }: { data: any }) {
  const change = data.open != null && data.close != null ? data.close - data.open : null;
  return (
    <Card>
      <div className="flex items-baseline justify-between">
        <span className="font-semibold">{String(data.symbol).toUpperCase()}</span>
        <span className="text-xl">{data.close}</span>
      </div>
      <div className="text-xs text-mute mt-1">
        {change != null && (
          <span className={change >= 0 ? "text-emerald-400" : "text-red-400"}>
            {change >= 0 ? "+" : ""}
            {change.toFixed(2)} today ·{" "}
          </span>
        )}
        O {data.open} H {data.high} L {data.low} · {data.date}
      </div>
    </Card>
  );
}

function Calculation({ data }: { data: any }) {
  return (
    <Card>
      <div className="text-xs text-mute">Calculator</div>
      <div className="font-mono text-sm mt-1">{data.expression}</div>
      <div className="text-xl mt-1 font-semibold">{data.result}</div>
    </Card>
  );
}

function Time({ data }: { data: any }) {
  return (
    <Card>
      <div className="text-xs text-mute">Local time{data.timezone ? ` · ${data.timezone}` : ""}</div>
      <div className="text-xl mt-1 font-semibold">{data.time}</div>
    </Card>
  );
}

function Sources({ results }: { results: SearchResult[] }) {
  return (
    <ol className="my-3 space-y-2">
      {results.map((r, i) => (
        <li key={r.url + i} id={`src-${i + 1}`} className="flex gap-2 text-sm scroll-mt-4">
          <span className="text-mute w-5 text-right shrink-0">{i + 1}</span>
          <img src={favicon(r.url)} alt="" className="w-4 h-4 mt-0.5 shrink-0" />
          <div className="min-w-0">
            <a href={r.url} target="_blank" rel="noreferrer" className="hover:text-accent break-words">
              {r.title || host(r.url)}
            </a>
            <div className="text-xs text-mute">
              {host(r.url)}
              {r.published_at ? ` · ${r.published_at}` : ""}
            </div>
            {r.snippet && <p className="text-xs text-mute mt-0.5 line-clamp-2">{r.snippet}</p>}
          </div>
        </li>
      ))}
    </ol>
  );
}

function MediaGrid({ results, videos }: { results: SearchResult[]; videos?: boolean }) {
  return (
    <div className="grid grid-cols-2 sm:grid-cols-3 gap-2 my-3">
      {results.slice(0, 12).map((r, i) => (
        <a
          key={r.url + i}
          href={r.url}
          target="_blank"
          rel="noreferrer"
          className="border border-edge rounded overflow-hidden hover:border-accent"
        >
          {r.thumbnail || r.image ? (
            // eslint-disable-next-line @next/next/no-img-element
            <img src={r.thumbnail || r.image!} alt={r.title} loading="lazy" className="w-full h-28 object-cover" />
          ) : (
            <div className="w-full h-28 bg-edge flex items-center justify-center text-2xl">{videos ? "▶" : "🖼"}</div>
          )}
          <div className="p-2 text-xs truncate">{r.title}</div>
        </a>
      ))}
    </div>
  );
}

export default function BlockView({
  block,
  onFollowUp,
}: {
  block: Block;
  onFollowUp?: (q: string) => void;
}) {
  const d = block.data;
  switch (block.type) {
    case "message":
      return <Answer text={block.message ?? ""} />;
    case "references":
      return d?.web?.length ? (
        <details open className="my-3">
          <summary className="text-xs uppercase tracking-wide text-mute cursor-pointer">
            {d.web.length} sources
          </summary>
          <Sources results={d.web} />
        </details>
      ) : null;
    case "related":
      return d?.questions?.length ? (
        <div className="flex flex-wrap gap-2 my-3">
          {d.questions.map((q: string) => (
            <button
              key={q}
              onClick={() => onFollowUp?.(q)}
              className="border border-edge rounded-full px-3 py-1 text-sm text-mute hover:text-ink hover:border-accent"
            >
              {q}
            </button>
          ))}
        </div>
      ) : null;
    case "images":
      return d?.results?.length ? <MediaGrid results={d.results} /> : null;
    case "videos":
      return d?.results?.length ? <MediaGrid results={d.results} videos /> : null;
    case "widget":
      if (d?.name === "calculator") return <Calculation data={d} />;
      if (d?.name === "time") return <Time data={d} />;
      return block.message ? <p className="text-sm my-2">{block.message}</p> : null;
    case "weather":
      return <Weather data={d ?? {}} />;
    case "stocks":
      return <Stocks data={d ?? {}} />;
    case "calculation":
      return <Calculation data={d ?? {}} />;
    case "time":
      return <Time data={d ?? {}} />;
    case "answered":
      return block.message ? (
        <p className="text-xs text-mute mt-3">{block.message}</p>
      ) : null;
    case "document":
      return <p className="text-sm text-mute my-2">{block.message}</p>;
    default:
      return block.message ? <p className="text-sm my-2">{block.message}</p> : null;
  }
}
