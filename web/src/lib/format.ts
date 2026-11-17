import type { Language } from "./i18n";

export function formatBytes(input: number | null | undefined): string {
  if (input === null || input === undefined || !Number.isFinite(input)) return "—";
  const value = Number(input);
  const absolute = Math.abs(value);
  const sign = value < 0 ? "−" : "";
  if (absolute >= 1024 ** 4) return `${sign}${(absolute / 1024 ** 4).toFixed(2)} TiB`;
  if (absolute >= 1024 ** 3) return `${sign}${(absolute / 1024 ** 3).toFixed(2)} GiB`;
  if (absolute >= 1024 ** 2) return `${sign}${(absolute / 1024 ** 2).toFixed(1)} MiB`;
  if (absolute >= 1024) return `${sign}${(absolute / 1024).toFixed(1)} KiB`;
  return `${sign}${absolute.toFixed(0)} B`;
}

export function formatNetBytes(value: number | null | undefined): string {
  if (value === null || value === undefined || !Number.isFinite(value)) return "—";
  const sign = value > 0 ? "+" : value < 0 ? "−" : "";
  return `${sign}${formatBytes(Math.abs(value))}`;
}

// Use European day/month/year ordering in both UI languages.
// Set the locale explicitly: falling back to the browser default can produce MM/DD/YYYY.
function dateLocale(language: Language): string {
  return language === "fr" ? "fr-FR" : "en-GB";
}

export function formatDate(timestamp: number | null | undefined, language: Language = "en"): string {
  if (!timestamp) return "—";
  const date = new Date(timestamp * 1000);
  const parts = new Intl.DateTimeFormat(dateLocale(language), {
    day: "2-digit",
    month: "2-digit",
    year: "numeric",
    hour: "2-digit",
    minute: "2-digit",
    hour12: false,
    hourCycle: "h23",
  }).formatToParts(date);
  const value = (type: string) => parts.find((part) => part.type === type)?.value ?? "";
  return `${value("day")}/${value("month")}/${value("year")} ${value("hour")}:${value("minute")}`;
}

export function formatShortDate(timestamp: number | null | undefined, language: Language = "en"): string {
  if (!timestamp) return "—";
  const parts = new Intl.DateTimeFormat(dateLocale(language), {
    day: "2-digit",
    month: "2-digit",
    year: "numeric",
  }).formatToParts(new Date(timestamp * 1000));
  const value = (type: string) => parts.find((part) => part.type === type)?.value ?? "";
  return `${value("day")}/${value("month")}/${value("year")}`;
}

export function formatPercent(value: number | null | undefined): string {
  return `${Number(value || 0).toFixed(1)}%`;
}

export function netUpload(uploaded: number, downloaded: number): number {
  return uploaded - downloaded;
}
