"use client";

import { useState } from "react";
import type { Block, SearchResult } from "@/lib/api";
import { StockQuote } from "@/components/StockQuote";

/** WMO weather codes as Open-Meteo reports them. */
const WMO: Record<number, string> = {
  0: "Clear", 1: "Mostly clear", 2: "Partly cloudy", 3: "Overcast", 45: "Fog", 48: "Freezing fog",
  51: "Light drizzle", 53: "Drizzle", 55: "Heavy drizzle", 56: "Freezing drizzle", 57: "Freezing drizzle",
  61: "Light rain", 63: "Rain", 65: "Heavy rain", 66: "Freezing rain", 67: "Freezing rain",
  71: "Light snow", 73: "Snow", 75: "Heavy snow", 77: "Snow grains", 80: "Showers", 81: "Showers",
  82: "Violent showers", 85: "Snow showers", 86: "Snow showers", 95: "Thunderstorm", 96: "Thunderstorm, hail",
  99: "Thunderstorm, hail",
};

const num = (v: unknown) => (typeof v === "number" && Number.isFinite(v) ? v : null);

function weekday(iso: string) {
  const d = new Date(`${iso}T12:00:00`);
  return Number.isNaN(d.getTime()) ? iso : d.toLocaleDateString([], { weekday: "short" });
}

function Weather({ data }: { data: any }) {
  const c = data.current ?? {};
  const unit = data.unit === "fahrenheit" ? "°F" : "°C";
  const temp = num(c.temperature);
  const feels = num(c.feels_like);
  const humidity = num(c.humidity);
  const wind = num(c.wind_speed);
  const f = data.forecast ?? {};
  const days: string[] = Array.isArray(f.time) ? f.time.slice(0, 5) : [];

  return (
    <aside className="datum" aria-label={`Weather in ${data.location}`}>
      <p className="kicker">Weather · {data.location}</p>
      <p className="datum__value">
        {temp != null ? `${Math.round(temp)}${unit}` : "—"}
        {WMO[c.code] && <small>{WMO[c.code]}</small>}
      </p>
      <div className="datum__row">
        {feels != null && <span className="meta">Feels {Math.round(feels)}°</span>}
        {humidity != null && <span className="meta">Humidity {Math.round(humidity)}%</span>}
        {wind != null && <span className="meta">Wind {Math.round(wind)} km/h</span>}
      </div>
      {days.length > 1 && (
        <div className="datum__days">
          {days.map((d, i) => {
            const hi = num(f.max?.[i]);
            const lo = num(f.min?.[i]);
            return (
              <div key={d}>
                <p className="kicker">{i === 0 ? "Today" : weekday(d)}</p>
                <b>
                  {hi != null ? `${Math.round(hi)}°` : "—"}
                  {lo != null && <small> / {Math.round(lo)}°</small>}
                </b>
                <span>{WMO[f.code?.[i]] ?? ""}</span>
              </div>
            );
          })}
        </div>
      )}
    </aside>
  );
}

function Calculation({ data }: { data: any }) {
  return (
    <aside className="datum">
      <p className="kicker">Calculator</p>
      <p className="datum__expr">{data.expression}</p>
      <p className="datum__value">= {data.result}</p>
    </aside>
  );
}

function Time({ data }: { data: any }) {
  // "2026-10-12 11:57:03 +09:00"
  const [date, clock, offset] = String(data.time ?? "").split(" ");
  const where = timePlace(data);
  const abbr = String(data.abbr || "").trim();
  return (
    <aside className="datum">
      <p className="kicker">{where ? `Time in ${where}` : "Local time"}</p>
      <p className="datum__value">{clock ? clock.slice(0, 5) : data.time}</p>
      {date && (
        <p className="meta" style={{ margin: 0 }}>
          {date}
          {abbr ? ` · ${abbr}` : ""}
          {offset ? ` · UTC${offset}` : ""}
        </p>
      )}
    </aside>
  );
}

function timePlace(data: any) {
  const place = String(data.place || "").trim();
  if (place && place !== "local") return place;
  const tz = String(data.timezone || "").trim();
  if (!tz || tz === "local") return "";
  const city = tz.includes("/") ? tz.split("/").pop() || tz : tz;
  return city.replaceAll("_", " ");
}

export function Widget({ block }: { block: Block }) {
  const d = block.data ?? {};
  switch (block.type) {
    case "weather":
      return <Weather data={d} />;
    case "stocks":
      return <StockQuote data={d} />;
    case "calculation":
      return <Calculation data={d} />;
    case "time":
      return <Time data={d} />;
    case "widget":
      if (d.name === "calculator") return <Calculation data={d} />;
      if (d.name === "time") return <Time data={d} />;
      return block.message ? <p className="callout">{block.message}</p> : null;
    default:
      return null;
  }
}

/** Images or videos the engine found alongside the answer. */
export function MediaStrip({ items, kind }: { items: SearchResult[]; kind: "images" | "videos" }) {
  const [all, setAll] = useState(false);
  const [broken, setBroken] = useState<ReadonlySet<string>>(new Set());
  const usable = items.filter((r) => (r.thumbnail || r.image) && !broken.has(r.url));
  if (!usable.length) return null;
  const shown = all ? usable : usable.slice(0, 4);

  return (
    <figure className="media">
      <div className="media__grid">
        {shown.map((r, i) => (
          <a key={`${r.url}${i}`} className="media__item" href={r.url} target="_blank" rel="noreferrer noopener" title={r.title}>
            {/* eslint-disable-next-line @next/next/no-img-element */}
            <img
              src={(r.thumbnail || r.image)!}
              alt={r.title}
              loading="lazy"
              referrerPolicy="no-referrer"
              onError={() => setBroken((b) => new Set(b).add(r.url))}
            />
            {kind === "videos" && <span className="media__play">Play</span>}
          </a>
        ))}
      </div>
      <figcaption>
        <span className="meta">
          {kind === "videos" ? "Videos" : "Images"} · {usable.length} found
        </span>
        {usable.length > 4 && (
          <button type="button" className="linkish meta" onClick={() => setAll((a) => !a)}>
            {all ? "Show fewer" : `Show all ${usable.length}`}
          </button>
        )}
      </figcaption>
    </figure>
  );
}
