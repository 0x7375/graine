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

export function formatDate(timestamp: number | null | undefined, language: Language = "en"): string {
  if (!timestamp) return "—";
  return new Date(timestamp * 1000).toLocaleString(language === "fr" ? "fr-FR" : undefined);
}

export function formatShortDate(timestamp: number | null | undefined, language: Language = "en"): string {
  if (!timestamp) return "—";
  return new Date(timestamp * 1000).toLocaleDateString(language === "fr" ? "fr-FR" : undefined);
}

export function formatPercent(value: number | null | undefined): string {
  return `${Number(value || 0).toFixed(1)}%`;
}

export function netUpload(uploaded: number, downloaded: number): number {
  return uploaded - downloaded;
}
