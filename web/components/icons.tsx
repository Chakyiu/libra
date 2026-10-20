import type { ReactNode } from "react";

type IconProps = { size?: number; stroke?: number; className?: string };

function icon(paths: ReactNode, defaults: { size?: number; stroke?: number } = {}) {
  return function Icon({ size = defaults.size ?? 15, stroke = defaults.stroke ?? 1.6, className }: IconProps) {
    return (
      <svg
        width={size}
        height={size}
        viewBox="0 0 24 24"
        fill="none"
        stroke="currentColor"
        strokeWidth={stroke}
        strokeLinecap="round"
        strokeLinejoin="round"
        aria-hidden="true"
        className={className}
      >
        {paths}
      </svg>
    );
  };
}

/** The Libra scales. */
export function Mark({ size = 18, stroke = 1.7, dot = false }: { size?: number; stroke?: number; dot?: boolean }) {
  return (
    <svg width={size} height={size} viewBox="0 0 24 24" fill="none" stroke="currentColor" strokeWidth={stroke} strokeLinecap="round" strokeLinejoin="round" aria-hidden="true">
      <path d="M12 3.5v16.5M7.5 20h9M4 7.5h16" />
      <path d="M4 7.5 1.4 13.5h5.2L4 7.5Z" />
      <path d="M20 7.5l-2.6 6h5.2L20 7.5Z" />
      {dot && <circle cx="12" cy="4.6" r="1.1" fill="currentColor" stroke="none" />}
    </svg>
  );
}

export const MenuIcon = icon(<path d="M4 5h16M4 12h10M4 19h16" />, { stroke: 1.7 });
export const PlusIcon = icon(<path d="M12 5v14M5 12h14" />, { size: 14, stroke: 1.8 });
/** A four-point sparkle: the helper that turns a sentence into a job. */
export const SparkleIcon = icon(
  <path d="M12 4.5 13.6 9.4 18.5 11 13.6 12.6 12 17.5 10.4 12.6 5.5 11 10.4 9.4Z" />,
  { size: 14, stroke: 1.7 },
);
/** A toothed cog — a hub with rays reads as the theme's sun. */
export const GearIcon = icon(
  <>
    <path d="M10.31 5.21L10.43 2.63L13.57 2.63L13.69 5.21A7 7 0 0 1 15.61 6L17.52 4.27L19.73 6.48L18 8.39A7 7 0 0 1 18.79 10.31L21.37 10.43L21.37 13.57L18.79 13.69A7 7 0 0 1 18 15.61L19.73 17.52L17.52 19.73L15.61 18A7 7 0 0 1 13.69 18.79L13.57 21.37L10.43 21.37L10.31 18.79A7 7 0 0 1 8.39 18L6.48 19.73L4.27 17.52L6 15.61A7 7 0 0 1 5.21 13.69L2.63 13.57L2.63 10.43L5.21 10.31A7 7 0 0 1 6 8.39L4.27 6.48L6.48 4.27L8.39 6A7 7 0 0 1 10.31 5.21Z" />
    <circle cx="12" cy="12" r="3" />
  </>,
);
export const SunIcon = icon(
  <>
    <circle cx="12" cy="12" r="4.2" />
    <path d="M12 2.6v2.4M12 19v2.4M21.4 12H19M5 12H2.6M18.6 5.4 16.9 7.1M7.1 16.9 5.4 18.6M18.6 18.6 16.9 16.9M7.1 7.1 5.4 5.4" />
  </>,
  { size: 14 },
);
export const MoonIcon = icon(<path d="M20 14.2A8.4 8.4 0 0 1 9.8 4 8.4 8.4 0 1 0 20 14.2Z" />, { size: 14 });
export const ArrowIcon = icon(<path d="M5 12h13M12 5.5 18.5 12 12 18.5" />, { size: 13, stroke: 2 });
export const CopyIcon = icon(
  <>
    <rect x="9" y="9" width="11" height="11" rx="1.5" />
    <path d="M5.5 15H5a1 1 0 0 1-1-1V5a1 1 0 0 1 1-1h9a1 1 0 0 1 1 1v.5" />
  </>,
  { size: 14 },
);
export const DownloadIcon = icon(<path d="M12 4v11M7 10.5l5 5 5-5M5 20h14" />, { size: 14 });
export const RerunIcon = icon(<path d="M20 12a8 8 0 1 1-2.3-5.6M20 4.5v4.6h-4.6" />, { size: 14 });
export const ChevronUpIcon = icon(<path d="m6 14 6-6 6 6" />, { size: 12, stroke: 2.2 });
export const ChevronDownIcon = icon(<path d="m6 9.5 6 6 6-6" />, { size: 10, stroke: 2.2 });
export const CloseIcon = icon(<path d="M6.5 6.5l11 11M17.5 6.5l-11 11" />, { size: 13, stroke: 1.8 });
export const ClipIcon = icon(
  <path d="M19.5 11.5 12 19a4.6 4.6 0 0 1-6.5-6.5l7.7-7.7a3.1 3.1 0 0 1 4.4 4.4l-7.6 7.6a1.5 1.5 0 0 1-2.2-2.2l7-7" />,
  { size: 12, stroke: 1.8 },
);
export const RailIcon = icon(
  <>
    <rect x="3.5" y="4.5" width="17" height="15" rx="1" />
    <path d="M14.5 4.5v15" />
  </>,
  { size: 14 },
);
export const CompassIcon = icon(
  <>
    <circle cx="12" cy="12" r="8.5" />
    <path d="m15.4 8.6-2.1 4.7-4.7 2.1 2.1-4.7 4.7-2.1Z" />
  </>,
  { size: 14 },
);
export const GlobeIcon = icon(
  <>
    <circle cx="12" cy="12" r="9" />
    <path d="M3.2 9h17.6M3.2 15h17.6M12 3a15 15 0 0 1 0 18M12 3a15 15 0 0 0 0 18" />
  </>,
  { size: 17 },
);
export const SearchIcon = icon(
  <>
    <circle cx="11" cy="11" r="6.5" />
    <path d="m20 20-4.4-4.4" />
  </>,
  { size: 17 },
);
export const ShieldIcon = icon(<path d="M12 3.5 19 6v5.5c0 4.2-3 7.4-7 9-4-1.6-7-4.8-7-9V6l7-2.5Z" />, { size: 17 });
export const FlameIcon = icon(
  <path d="M12 20.5a6 6 0 0 0 6-6c0-3.5-2.5-5.5-3.5-8.5-1.5 2-2 3.5-2 5-1-1-1.6-2.4-1.6-3.6C8.6 9 6 11.2 6 14.5a6 6 0 0 0 6 6Z" />,
  { size: 17 },
);
export const CodeIcon = icon(<path d="m9 8-5 4 5 4M15 8l5 4-5 4" />, { size: 17, stroke: 1.7 });
/** A stopwatch: the Automator's schedules. Same 14 px box as the compass beside it. */
export const TimerIcon = icon(
  <>
    <circle cx="12" cy="13.5" r="7.5" />
    <path d="M12 10v3.5l2.4 1.7M9.5 3.5h5M12 3.5V6" />
  </>,
  { size: 14 },
);
