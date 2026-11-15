import { useCallback, useEffect, useMemo, useState, type ReactNode } from "react";
import {
  Activity,
  AlertTriangle,
  ArrowDownToLine,
  Database,
  HardDrive,
  Info,
  Leaf,
  Moon,
  RefreshCw,
  Search,
  Sun,
  Upload,
} from "lucide-react";
import {
  Area,
  AreaChart,
  CartesianGrid,
  Line,
  LineChart,
  ReferenceLine,
  ResponsiveContainer,
  Tooltip,
  XAxis,
  YAxis,
} from "recharts";
import { Button } from "@/components/ui/button";
import {
  Card,
  CardContent,
  CardHeader,
  CardTitle,
} from "@/components/ui/card";
import {
  Select,
  SelectContent,
  SelectItem,
  SelectTrigger,
  SelectValue,
} from "@/components/ui/select";
import {
  Table,
  TableBody,
  TableCell,
  TableHead,
  TableHeader,
  TableRow,
} from "@/components/ui/table";
import { Tabs, TabsContent, TabsList, TabsTrigger } from "@/components/ui/tabs";
import { formatBytes, formatDate, formatNetBytes, formatPercent, formatShortDate, netUpload } from "@/lib/format";
import { LanguageContext, initialLanguage, translate, useLanguage, type Language } from "@/lib/i18n";
import type {
  DashboardCluster,
  DashboardData,
  DeletionBatch,
  PeriodKey,
  TrackerMetric,
} from "@/types";

type Page = "overview" | "storage" | "leaderboard" | "evictions";

const PERIOD_LABELS: Record<PeriodKey, string> = {
  all: "All time", year: "This year", month: "This month", week: "This week",
};
const TRACKER_METRICS: { value: TrackerMetric; label: string }[] = [
  { value: "total_net_bytes", label: "Total net" },
  { value: "cross_seed_net_bytes", label: "Cross-seed net" },
  { value: "regular_net_bytes", label: "Regular net" },
];

const SERIES_COLORS = [
  "var(--series-1)",
  "var(--series-2)",
  "var(--series-3)",
  "var(--series-4)",
  "var(--series-5)",
  "var(--series-6)",
  "var(--series-7)",
  "var(--series-8)",
];

const EMPTY_PERIOD = { torrents: [], tracker_history: [] };

function useSystemTheme() {
  const [dark, setDark] = useState(() =>
    typeof window !== "undefined" && window.matchMedia("(prefers-color-scheme: dark)").matches,
  );

  useEffect(() => {
    const media = window.matchMedia("(prefers-color-scheme: dark)");
    const update = (event: MediaQueryListEvent) => setDark(event.matches);
    media.addEventListener("change", update);
    return () => media.removeEventListener("change", update);
  }, []);

  useEffect(() => {
    document.documentElement.classList.toggle("dark", dark);
    document.documentElement.style.colorScheme = dark ? "dark" : "light";
  }, [dark]);

  return { dark, setDark };
}

export default function App() {
  const [language] = useState<Language>(initialLanguage);
  const t = (key: string) => translate(language, key);
  useEffect(() => {
    document.documentElement.lang = language;
  }, [language]);
  const [dashboard, setDashboard] = useState<DashboardData | null>(null);
  const [loading, setLoading] = useState(true);
  const [refreshing, setRefreshing] = useState(false);
  const [error, setError] = useState<string | null>(null);
  const [page, setPage] = useState<Page>("overview");
  const [trackerPeriod, setTrackerPeriod] = useState<PeriodKey>("all");
  const [trackerMetric, setTrackerMetric] = useState<TrackerMetric>("total_net_bytes");
  const [leaderPeriod, setLeaderPeriod] = useState<PeriodKey>("all");
  const [rankingSearch, setRankingSearch] = useState("");
  const { dark, setDark } = useSystemTheme();

  const loadDashboard = useCallback(async (manual = false) => {
    if (manual) setRefreshing(true);
    try {
      const response = await fetch("/api/dashboard", { headers: { Accept: "application/json" } });
      if (!response.ok) {
        const body = await response.json().catch(() => null) as { error?: string } | null;
        throw new Error(body?.error || `Dashboard request failed (${response.status})`);
      }
      const data = await response.json() as DashboardData;
      setDashboard(data);
      setError(null);
    } catch (cause) {
      const message = cause instanceof Error ? cause.message : "Unknown dashboard error";
      setError(message);
    } finally {
      setLoading(false);
      setRefreshing(false);
    }
  }, []);

  useEffect(() => {
    void loadDashboard();
    const timer = window.setInterval(() => void loadDashboard(), 60_000);
    return () => window.clearInterval(timer);
  }, [loadDashboard]);

  if (loading) {
    return <PageState title={t("Loading dashboard")} detail={t("Connecting to the Graine API…")} />;
  }

  if (!dashboard) {
    return (
      <PageState
        title={t("Dashboard unavailable")}
        detail={error || t("No completed management run has been stored yet.")}
        action={<Button onClick={() => void loadDashboard(true)} disabled={refreshing}><RefreshCw className="h-4 w-4" /> {t("Retry")}</Button>}
      />
    );
  }

  const run = dashboard.run;
  const notice = run.mode === "dry"
    ? { variant: "info" as const, text: t("Dry run — no torrents were removed.") }
    : run.result === "over_limit"
      ? { variant: "warning" as const, text: t("Disk remains above the high watermark: Graine could not reclaim enough space while keeping torrents within their grace period.") }
      : null;
  const lastRun = `${t("Last execution")} ${formatDate(run.timestamp, language)}`;

  return (
    <LanguageContext.Provider value={language}>
    <main className="min-h-screen bg-background text-foreground">
      <div className="mx-auto max-w-[1280px] px-5 pb-10 sm:px-8 lg:px-10">
        <header className="flex min-h-20 flex-wrap items-center justify-between gap-4 border-b border-border py-4">
          <div className="flex items-center gap-3">
            <div className="grid h-10 w-10 place-items-center rounded-lg border bg-muted/40 text-primary">
              <Leaf className="h-5 w-5" />
            </div>
            <div>
              <h1 className="text-[26px] font-bold leading-tight tracking-tight">graine</h1>
              <p className="text-xs text-muted-foreground">{t("Torrent storage and transfer statistics")}</p>
            </div>
          </div>

          <div className="flex items-center gap-2">
            <span className="max-w-[160px] truncate text-[11px] text-muted-foreground sm:max-w-none sm:text-xs" title={lastRun}>{lastRun}</span>
            <Button variant="outline" size="icon" onClick={() => void loadDashboard(true)} disabled={refreshing} aria-label={t("Refresh dashboard")}>
              <RefreshCw className={`h-4 w-4 ${refreshing ? "animate-spin" : ""}`} />
            </Button>
            <Button variant="outline" size="icon" onClick={() => setDark(!dark)} aria-label={t(dark ? "Switch to light theme" : "Switch to dark theme")}>
              {dark ? <Sun className="h-4 w-4" /> : <Moon className="h-4 w-4" />}
            </Button>
          </div>
        </header>

        <Tabs value={page} onValueChange={(value) => setPage(value as Page)} className="mt-5">
          <TabsList className="h-auto min-h-10 w-full flex-wrap justify-start gap-1 overflow-visible rounded-lg border border-border bg-transparent p-1 sm:w-auto [&_[data-state=active]]:bg-muted/70 [&_[data-state=active]]:shadow-none">
            <TabsTrigger value="overview" className="gap-2"><Activity className="h-4 w-4" /> {t("Overview")} </TabsTrigger>
            <TabsTrigger value="storage" className="gap-2"><HardDrive className="h-4 w-4" /> {t("Storage")} </TabsTrigger>
            <TabsTrigger value="leaderboard" className="gap-2"><Upload className="h-4 w-4" /> {t("Leaderboard")} </TabsTrigger>
            <TabsTrigger value="evictions" className="gap-2"><ArrowDownToLine className="h-4 w-4" /> {t("Evictions")} </TabsTrigger>
          </TabsList>

          {notice && (
            <div className={`mt-5 flex items-start gap-2 rounded-lg border px-3 py-2.5 text-sm ${notice.variant === "warning" ? "border-amber-500/30 bg-amber-500/5 text-amber-800 dark:text-amber-300" : "border-primary/30 bg-primary/5 text-primary"}`}>
              {notice.variant === "warning" ? <AlertTriangle className="mt-0.5 h-4 w-4 shrink-0" /> : <Info className="mt-0.5 h-4 w-4 shrink-0" />}
              <span>{notice.text}</span>
            </div>
          )}
          {error && (
            <div className="mt-4 flex items-center justify-between gap-3 rounded-lg border border-destructive/30 bg-destructive/5 px-3 py-2 text-sm text-destructive">
              <span>{t("Refresh failed")}: {error}. {t("Showing the last successfully loaded data.")}</span>
              <Button size="sm" variant="outline" onClick={() => void loadDashboard(true)} disabled={refreshing}>Retry</Button>
            </div>
          )}

          <TabsContent value="overview">
            <Overview
              dashboard={dashboard}
              trackerPeriod={trackerPeriod}
              setTrackerPeriod={setTrackerPeriod}
              trackerMetric={trackerMetric}
              setTrackerMetric={setTrackerMetric}
              rankingSearch={rankingSearch}
              setRankingSearch={setRankingSearch}
            />
          </TabsContent>
          <TabsContent value="storage">
            <StoragePage dashboard={dashboard} />
          </TabsContent>
          <TabsContent value="leaderboard">
            <LeaderboardPage
              dashboard={dashboard}
              leaderPeriod={leaderPeriod}
              setLeaderPeriod={setLeaderPeriod}
            />
          </TabsContent>
          <TabsContent value="evictions">
            <EvictionsPage batches={dashboard.deletions} />
          </TabsContent>
        </Tabs>

      </div>
    </main>
    </LanguageContext.Provider>
  );
}

function PageState({ title, detail, action }: { title: string; detail: string; action?: ReactNode }) {
  return (
    <main className="grid min-h-screen place-items-center bg-background px-4 text-foreground">
      <div className="max-w-md text-center">
        <div className="mx-auto mb-4 grid h-11 w-11 place-items-center rounded-xl border bg-muted/50 text-primary"><Leaf className="h-5 w-5" /></div>
        <h1 className="text-2xl font-bold">{title}</h1>
        <p className="mt-2 text-sm text-muted-foreground">{detail}</p>
        {action && <div className="mt-5 flex justify-center">{action}</div>}
      </div>
    </main>
  );
}

function Overview({
  dashboard,
  trackerPeriod,
  setTrackerPeriod,
  trackerMetric,
  setTrackerMetric,
  rankingSearch,
  setRankingSearch,
}: {
  dashboard: DashboardData;
  trackerPeriod: PeriodKey;
  setTrackerPeriod: (value: PeriodKey) => void;
  trackerMetric: TrackerMetric;
  setTrackerMetric: (value: TrackerMetric) => void;
  rankingSearch: string;
  setRankingSearch: (value: string) => void;
}) {
  return (
    <div className="space-y-6 pt-5">
      <TrackerPerformance
        periods={dashboard.periods}
        trackerPeriod={trackerPeriod}
        setTrackerPeriod={setTrackerPeriod}
        trackerMetric={trackerMetric}
        setTrackerMetric={setTrackerMetric}
      />
      <RankingPage rows={dashboard.ranking} search={rankingSearch} onSearch={setRankingSearch} />
    </div>
  );
}

function StoragePage({ dashboard }: { dashboard: DashboardData }) {
  const language = useLanguage();
  const t = (key: string) => translate(language, key);
  const { run } = dashboard;
  const torrentData = Number(dashboard.reclaimable_bytes || 0) + Number(dashboard.protected_bytes || 0);

  return (
    <div className="space-y-6 pt-5">
      <section className="space-y-3">
        <h2 className="text-2xl font-bold tracking-tight">{t("Disk usage")}</h2>
        <div className="grid gap-3 sm:grid-cols-2">
          <MetricCard
            icon={<HardDrive className="h-4 w-4" />}
            label={t("Disk used")}
            value={formatPercent(run.disk_used_percent)}
            detail={`${formatBytes(run.disk_used_bytes)} ${language === "fr" ? "sur" : "of"} ${formatBytes(run.disk_total_bytes)}`}
          />
          <MetricCard
            icon={<Database className="h-4 w-4" />}
            label={t("Managed torrent data")}
            value={formatBytes(torrentData)}
            detail={t("Unique data represented by retained completed torrent clusters")}
          />
        </div>
      </section>

      <section>
        <Card>
          <CardHeader className="pb-2">
            <CardTitle>{t("Disk usage history")}</CardTitle>
          </CardHeader>
          <CardContent>
            <DiskHistoryChart history={dashboard.history} />
            <div className="mt-3 flex flex-wrap gap-x-4 gap-y-2 text-xs text-muted-foreground">
              <LegendDot color="var(--chart-radarr)" label="Radarr" />
              <LegendDot color="var(--chart-sonarr)" label="Sonarr" />
              <LegendDot color="var(--chart-autobrr)" label="Autobrr" />
              <LegendDot color="var(--chart-other)" label={t("Other / unclassified")} />
              <LegendLine color="var(--destructive)" label={t("High watermark")} dashed />
              <LegendLine color="var(--chart-low)" label={t("Low watermark")} dashed />
            </div>
          </CardContent>
        </Card>
      </section>
    </div>
  );
}

function MetricCard({ icon, label, value, detail }: { icon: ReactNode; label: string; value: string; detail: string }) {
  return (
    <Card>
      <CardContent className="p-4">
        <div className="flex items-center justify-between gap-2 text-muted-foreground">
          <span className="text-sm font-medium uppercase tracking-wide">{label}</span>
          {icon}
        </div>
        <div className="mt-3 font-mono text-2xl font-semibold tracking-tight tabular-nums">{value}</div>
        <p className="mt-1 min-h-8 text-xs leading-relaxed text-muted-foreground">{detail}</p>
      </CardContent>
    </Card>
  );
}

function DiskHistoryChart({ history }: { history: DashboardData["history"] }) {
  const language = useLanguage();
  const t = (key: string) => translate(language, key);
  const data = useMemo(() => history.map((entry) => {
    const categories = entry.category_usage || {};
    const categoryBytes = (name: string) => Object.entries(categories)
      .filter(([category]) => category.toLowerCase() === name)
      .reduce((sum, [, bytes]) => sum + Number(bytes || 0), 0);
    const radarr = categoryBytes("radarr");
    const sonarr = categoryBytes("sonarr");
    const autobrr = categoryBytes("autobrr");
    const used = Number(entry.disk_used_bytes || 0);
    const diskTotal = Number(entry.disk_total_bytes || 0);
    const attributed = radarr + sonarr + autobrr;
    const scale = attributed > used && attributed > 0 ? used / attributed : 1;
    const values = {
      Radarr: radarr * scale,
      Sonarr: sonarr * scale,
      Autobrr: autobrr * scale,
    };
    const other = Math.max(0, used - values.Radarr - values.Sonarr - values.Autobrr);
    const ratio = diskTotal > 0 ? 100 / diskTotal : 0;
    return {
      timestamp: entry.timestamp,
      date: formatShortDate(entry.timestamp, language),
      Radarr: values.Radarr * ratio,
      Sonarr: values.Sonarr * ratio,
      Autobrr: values.Autobrr * ratio,
      Other: other * ratio,
      Total: diskTotal > 0 ? used / diskTotal * 100 : 0,
      High: entry.high_watermark_percent || 85,
      Low: entry.low_watermark_percent || 80,
    };
  }), [history, language]);

  if (data.length === 0) return <EmptyState message={t("No history yet.")} />;

  return (
    <div className="h-[310px] w-full pt-2">
      <ResponsiveContainer width="100%" height="100%">
        <AreaChart data={data} margin={{ top: 10, right: 12, left: 2, bottom: 2 }}>
          <CartesianGrid stroke="var(--border)" strokeDasharray="3 5" vertical={false} />
          <XAxis dataKey="date" tickLine={false} axisLine={false} minTickGap={35} tick={{ fill: "var(--muted-foreground)", fontSize: 11 }} />
          <YAxis domain={[0, 100]} ticks={[0, 20, 40, 60, 80, 100]} tickFormatter={(value: number) => `${value}%`} tickLine={false} axisLine={false} width={43} tick={{ fill: "var(--muted-foreground)", fontSize: 11 }} />
          <Tooltip content={<DiskTooltip />} />
          <Area type="monotone" dataKey="Radarr" stackId="disk" stroke="var(--chart-radarr)" fill="var(--chart-radarr)" fillOpacity={0.82} strokeWidth={1} />
          <Area type="monotone" dataKey="Sonarr" stackId="disk" stroke="var(--chart-sonarr)" fill="var(--chart-sonarr)" fillOpacity={0.82} strokeWidth={1} />
          <Area type="monotone" dataKey="Autobrr" stackId="disk" stroke="var(--chart-autobrr)" fill="var(--chart-autobrr)" fillOpacity={0.82} strokeWidth={1} />
          <Area type="monotone" dataKey="Other" stackId="disk" stroke="var(--chart-other)" fill="var(--chart-other)" fillOpacity={0.82} strokeWidth={1} />
          <Line type="monotone" dataKey="Total" stroke="var(--foreground)" strokeWidth={1.7} dot={false} activeDot={{ r: 3 }} />
          <ReferenceLine y={data[data.length - 1]?.High} stroke="var(--destructive)" strokeDasharray="5 4" />
          <ReferenceLine y={data[data.length - 1]?.Low} stroke="var(--chart-low)" strokeDasharray="5 4" />
        </AreaChart>
      </ResponsiveContainer>
    </div>
  );
}

function DiskTooltip({ active, payload, label }: { active?: boolean; payload?: Array<{ name: string; value: number; color?: string }>; label?: string }) {
  const language = useLanguage();
  const t = (key: string) => translate(language, key);
  if (!active || !payload?.length) return null;
  return (
    <div className="rounded-md border bg-popover px-3 py-2 text-xs text-popover-foreground shadow-md">
      <p className="mb-1.5 font-medium">{label}</p>
      {payload.filter((item) => item.name !== "High" && item.name !== "Low").map((item) => (
        <div key={item.name} className="flex items-center justify-between gap-6 py-0.5">
          <span className="text-muted-foreground">{t(item.name)}</span>
          <span className="font-mono tabular-nums">{Number(item.value).toFixed(2)}%</span>
        </div>
      ))}
    </div>
  );
}

function RankingPage({
  rows,
  search,
  onSearch,
}: {
  rows: DashboardCluster[];
  search: string;
  onSearch: (value: string) => void;
}) {
  const language = useLanguage();
  const t = (key: string) => translate(language, key);
  const filteredRows = useMemo(() => {
    const query = search.trim().toLowerCase();
    return rows
      .filter((cluster) => [
        cluster.name,
        cluster.category,
        cluster.tracker_hosts.join(" "),
        cluster.torrent_names.join(" "),
      ].join(" ").toLowerCase().includes(query))
      .sort((a, b) => scoreForSort(b) - scoreForSort(a));
  }, [rows, search]);

  return (
    <section className="space-y-3">
      <div className="flex flex-wrap items-end justify-between gap-3">
        <div>
          <h2 className="text-2xl font-bold tracking-tight">{t("Ranking")}</h2>
          <p className="mt-1 text-sm text-muted-foreground">{t("Clusters ordered by score, highest first.")}</p>
        </div>
        <div className="relative">
          <Search className="pointer-events-none absolute left-2.5 top-2.5 h-4 w-4 text-muted-foreground" />
          <input
            value={search}
            onChange={(event) => onSearch(event.target.value)}
            placeholder={t("Search torrent, tracker or category…")}
            aria-label={t("Search ranking")}
            className="h-9 w-[min(320px,80vw)] rounded-md border border-input bg-background pl-9 pr-3 text-sm outline-none placeholder:text-muted-foreground focus-visible:ring-2 focus-visible:ring-ring"
          />
        </div>
      </div>

      <Card>
        {filteredRows.length === 0 ? (
          <EmptyState message={t("No matching clusters.")} />
        ) : (
          <div className="overflow-x-auto">
            <Table>
              <TableHeader>
                <TableRow>
                  <TableHead className="w-12">#</TableHead>
                  <TableHead className="min-w-[240px]">{t("Cluster / torrents")}</TableHead>
                  <TableHead>{t("Category")}</TableHead>
                  <TableHead className="text-right">{t("Score")}</TableHead>
                  <TableHead className="text-right">{t("Cost")}</TableHead>
                  <TableHead className="text-right">{t("Uploaded")}</TableHead>
                </TableRow>
              </TableHeader>
              <TableBody>
                {filteredRows.map((cluster, index) => (
                  <TableRow key={`${cluster.rank}-${cluster.name}`}>
                    <TableCell className="font-mono text-xs text-muted-foreground">{String(index + 1).padStart(2, "0")}</TableCell>
                    <TableCell>
                      <div className="max-w-[390px] truncate font-medium" title={cluster.torrent_names.join("\n")}>{cluster.name}</div>
                      <div className="mt-1 max-w-[390px] truncate text-xs text-muted-foreground">
                        {cluster.torrent_count} {t("torrent(s)")} · {cluster.tracker_hosts.join(", ")}
                      </div>
                    </TableCell>
                    <TableCell>{cluster.category}</TableCell>
                    <TableCell className="whitespace-nowrap text-right font-mono tabular-nums">
                      {cluster.score === null ? (Number(cluster.cost_bytes) === 0 ? "∞" : "—") : Number(cluster.score).toFixed(2)}
                    </TableCell>
                    <TableCell className="text-right font-mono tabular-nums">{formatBytes(cluster.cost_bytes)}</TableCell>
                    <TableCell className="text-right font-mono tabular-nums">{formatBytes(cluster.uploaded_bytes)}</TableCell>
                  </TableRow>
                ))}
              </TableBody>
            </Table>
          </div>
        )}
      </Card>
    </section>
  );
}

function scoreForSort(cluster: DashboardCluster): number {
  if (cluster.score !== null) return cluster.score;
  return cluster.cost_bytes === 0 ? Number.POSITIVE_INFINITY : Number.NEGATIVE_INFINITY;
}

// Recharts' default SVG <Text> wraps long tick values when its view box is narrow.
// A plain SVG text node keeps e.g. "745.06 GiB" on one line.
function TrackerYAxisTick({ x = 0, y = 0, payload }: {
  x?: number;
  y?: number;
  payload?: { value: number };
}) {
  const value = payload?.value;
  if (value === undefined || value === 0) return null;
  return (
    <text
      x={x - 6}
      y={y}
      dominantBaseline="middle"
      textAnchor="end"
      fill="var(--muted-foreground)"
      fontSize={11}
      fontFamily="ui-monospace, SFMono-Regular, Menlo, monospace"
    >
      {formatBytes(value)}
    </text>
  );
}

function TrackerPerformance({
  periods,
  trackerPeriod,
  setTrackerPeriod,
  trackerMetric,
  setTrackerMetric,
}: {
  periods: DashboardData["periods"];
  trackerPeriod: PeriodKey;
  setTrackerPeriod: (value: PeriodKey) => void;
  trackerMetric: TrackerMetric;
  setTrackerMetric: (value: TrackerMetric) => void;
}) {
  const language = useLanguage();
  const t = (key: string) => translate(language, key);
  const periodData = periods[trackerPeriod] || EMPTY_PERIOD;
  const trackerNames = useMemo(() => [...new Set(
    periodData.tracker_history.flatMap((point) => Object.keys(point.trackers || {})),
  )].sort(), [periodData.tracker_history]);

  const trackerData = useMemo(() => periodData.tracker_history.map((point) => {
    const row: Record<string, number | string> = {
      timestamp: point.timestamp,
      date: formatShortDate(point.timestamp, language),
    };
    for (const name of trackerNames) {
      row[name] = Number(point.trackers?.[name]?.[trackerMetric] || 0);
    }
    return row;
  }), [periodData.tracker_history, trackerNames, trackerMetric, language]);

  const visibleSeries = trackerNames.filter((name) => trackerData.some((point) => Number(point[name]) !== 0));

  return (
    <section className="space-y-3">
      <div className="flex flex-wrap items-end justify-between gap-3">
        <div>
          <h2 className="text-2xl font-bold tracking-tight">{t("Tracker performance")}</h2>
          <p className="mt-1 text-sm text-muted-foreground">{t("Cumulative net upload by configured tracker group.")}</p>
        </div>
        <div className="flex flex-wrap items-center gap-2">
          <label className="text-xs text-muted-foreground" htmlFor="tracker-period">{t("Period")}</label>
          <Select value={trackerPeriod} onValueChange={(value) => setTrackerPeriod(value as PeriodKey)}>
            <SelectTrigger id="tracker-period"><SelectValue /></SelectTrigger>
            <SelectContent>{Object.entries(PERIOD_LABELS).map(([value, label]) => <SelectItem key={value} value={value}>{t(label)}</SelectItem>)}</SelectContent>
          </Select>
          <label className="text-xs text-muted-foreground" htmlFor="tracker-metric">{t("Metric")}</label>
          <Select value={trackerMetric} onValueChange={(value) => setTrackerMetric(value as TrackerMetric)}>
            <SelectTrigger id="tracker-metric" className="w-[160px]"><SelectValue /></SelectTrigger>
            <SelectContent>{TRACKER_METRICS.map((metric) => <SelectItem key={metric.value} value={metric.value}>{t(metric.label)}</SelectItem>)}</SelectContent>
          </Select>
        </div>
      </div>
      <Card>
        <CardContent className="pt-5">
          {trackerData.length === 0 || visibleSeries.length === 0 ? (
            <EmptyState message={t("No configured tracker activity recorded for this period yet.")} />
          ) : (
            <>
              <div className="h-[280px] w-full">
                <ResponsiveContainer width="100%" height="100%">
                  <LineChart data={trackerData} margin={{ top: 22, right: 18, left: 0, bottom: 10 }}>
                    <CartesianGrid stroke="var(--border)" strokeDasharray="3 5" vertical={false} />
                    <XAxis dataKey="date" tickLine={false} axisLine={false} minTickGap={35} tickMargin={12} height={34} tick={{ fill: "var(--muted-foreground)", fontSize: 11 }} />
                    <YAxis tick={<TrackerYAxisTick />} tickLine={false} axisLine={false} width={84} />
                    <ReferenceLine y={0} stroke="var(--border)" />
                    <Tooltip content={<TrackerTooltip metric={trackerMetric} />} />
                    {visibleSeries.map((name, index) => (
                      <Line key={name} type="monotone" dataKey={name} stroke={SERIES_COLORS[index % SERIES_COLORS.length]} strokeWidth={2} dot={false} activeDot={{ r: 4 }} />
                    ))}
                  </LineChart>
                </ResponsiveContainer>
              </div>
              <div className="mt-3 flex flex-wrap gap-x-5 gap-y-2">
                {visibleSeries.map((name, index) => {
                  const last = trackerData[trackerData.length - 1];
                  return (
                    <div key={name} className="flex items-center gap-2 text-xs">
                      <span className="h-2 w-2 rounded-full" style={{ background: SERIES_COLORS[index % SERIES_COLORS.length] }} />
                      <span className="text-muted-foreground">{name}</span>
                      <span className={`font-mono tabular-nums ${Number(last[name]) >= 0 ? "text-primary" : "text-destructive"}`}>{formatNetBytes(Number(last[name]))}</span>
                    </div>
                  );
                })}
              </div>
            </>
          )}
          
        </CardContent>
      </Card>
    </section>
  );
}

function LeaderboardPage({
  dashboard,
  leaderPeriod,
  setLeaderPeriod,
}: {
  dashboard: DashboardData;
  leaderPeriod: PeriodKey;
  setLeaderPeriod: (value: PeriodKey) => void;
}) {
  const language = useLanguage();
  const t = (key: string) => translate(language, key);
  const selectedPeriod = dashboard.periods[leaderPeriod] || EMPTY_PERIOD;
  const leaders = [...selectedPeriod.torrents]
    .sort((a, b) => netUpload(b.uploaded_bytes, b.downloaded_bytes) - netUpload(a.uploaded_bytes, a.downloaded_bytes) || a.hash.localeCompare(b.hash))
    .slice(0, 10);

  const changes = dashboard.rank_changes?.[leaderPeriod] || {};

  return (
    <section className="space-y-3 pt-5">
      <div className="flex flex-wrap items-end justify-between gap-3">
        <div>
          <h2 className="text-2xl font-bold tracking-tight">{t("Torrent leaderboard")}</h2>
          <p className="mt-1 text-sm text-muted-foreground">{t("Top 10 torrents by net upload.")}</p>
        </div>
        <div className="flex items-center gap-2">
          <label className="text-xs text-muted-foreground" htmlFor="leader-period">{t("Period")}</label>
          <Select value={leaderPeriod} onValueChange={(value) => setLeaderPeriod(value as PeriodKey)}>
            <SelectTrigger id="leader-period"><SelectValue /></SelectTrigger>
            <SelectContent>{Object.entries(PERIOD_LABELS).map(([value, label]) => <SelectItem key={value} value={value}>{t(label)}</SelectItem>)}</SelectContent>
          </Select>
        </div>
      </div>
      <Card>
        {leaders.length === 0 ? (
          <EmptyState message={t("No torrent activity recorded for this period yet.")} />
        ) : (
          <Table>
            <TableHeader>
              <TableRow>
                <TableHead className="w-20">#</TableHead>
                <TableHead className="min-w-[260px]">{t("Torrent")}</TableHead>
                <TableHead>{t("Last activity")}</TableHead>
                <TableHead className="text-right">{t("Net upload")}</TableHead>
              </TableRow>
            </TableHeader>
            <TableBody>
              {leaders.map((torrent, index) => {
                const net = netUpload(torrent.uploaded_bytes, torrent.downloaded_bytes);
                const change = changes[torrent.hash];
                return (
                  <TableRow key={torrent.hash}>
                    <TableCell className="whitespace-nowrap font-mono text-xs">
                      <span className="text-muted-foreground">{index + 1}</span>
                      {change !== undefined && change !== 0 && (
                        <span className={`ml-2 font-semibold ${change > 0 ? "text-primary" : "text-destructive"}`} title={`${t("Since previous execution")}: ${t(change > 0 ? "up" : "down")} ${Math.abs(change)} ${t("position(s)")}`}>
                          {change > 0 ? `+${change}` : change}
                        </span>
                      )}
                    </TableCell>
                    <TableCell>
                      <div className="max-w-[400px] truncate font-medium" title={torrent.name}>{torrent.name}</div>
                      <div className="mt-1 text-xs text-muted-foreground">{torrent.tracker}{torrent.is_cross_seed ? ` · ${t("Cross-seed")}` : ""}</div>
                    </TableCell>
                    <TableCell className="whitespace-nowrap text-muted-foreground">{formatDate(torrent.timestamp, language)}</TableCell>
                    <TableCell className={`text-right font-mono tabular-nums ${net >= 0 ? "text-primary" : "text-destructive"}`}>{formatNetBytes(net)}</TableCell>
                  </TableRow>
                );
              })}
            </TableBody>
          </Table>
        )}
      </Card>
    </section>
  );
}

function TrackerTooltip({ active, payload, label, metric }: { active?: boolean; payload?: Array<{ name: string; value: number; color?: string }>; label?: string; metric: TrackerMetric }) {
  const language = useLanguage();
  const t = (key: string) => translate(language, key);
  if (!active || !payload?.length) return null;
  const metricLabel = TRACKER_METRICS.find((item) => item.value === metric)?.label || "Net";
  return (
    <div className="max-w-[280px] rounded-md border bg-popover px-3 py-2 text-xs text-popover-foreground shadow-md">
      <p className="mb-1.5 font-medium">{label} · {t(metricLabel)}</p>
      {payload.map((item) => (
        <div key={item.name} className="flex items-center justify-between gap-5 py-0.5">
          <span className="truncate text-muted-foreground">{item.name}</span>
          <span className={`font-mono tabular-nums ${Number(item.value) >= 0 ? "text-primary" : "text-destructive"}`}>{formatNetBytes(Number(item.value))}</span>
        </div>
      ))}
    </div>
  );
}

function EvictionsPage({ batches }: { batches: DeletionBatch[] }) {
  const language = useLanguage();
  const t = (key: string) => translate(language, key);
  const rows = [...batches]
    .sort((a, b) => b.timestamp - a.timestamp)
    .flatMap((batch) => batch.clusters.map((cluster, index) => ({
      batch,
      cluster,
      observed: index === 0 ? batch.observed_reclaimed_bytes : null,
    })));

  return (
    <section className="space-y-3 pt-5">
      <div>
        <h2 className="text-2xl font-bold tracking-tight">{t("Eviction history")}</h2>
        <p className="mt-1 text-sm text-muted-foreground">{t("Recent torrent removals.")}</p>
      </div>
      <Card>
        {rows.length === 0 ? (
          <EmptyState message={t("No actual eviction has been recorded yet.")} />
        ) : (
          <div className="overflow-x-auto">
            <Table>
              <TableHeader>
                <TableRow>
                  <TableHead>{t("When")}</TableHead>
                  <TableHead className="min-w-[240px]">{t("Cluster")}</TableHead>
                  <TableHead>{t("Category")}</TableHead>
                  <TableHead>{t("Reason")}</TableHead>
                  <TableHead className="text-right">{t("Score")}</TableHead>
                  <TableHead className="text-right">{t("Estimated")}</TableHead>
                  <TableHead className="text-right">{t("Observed batch")}</TableHead>
                </TableRow>
              </TableHeader>
              <TableBody>
                {rows.map(({ batch, cluster, observed }, index) => (
                  <TableRow key={`${batch.batch_id}-${index}`}>
                    <TableCell className="whitespace-nowrap text-xs text-muted-foreground">{formatDate(batch.timestamp, language)}</TableCell>
                    <TableCell>
                      <div className="max-w-[390px] truncate font-medium" title={cluster.torrent_names.join("\n")}>{cluster.name}</div>
                      <div className="mt-1 max-w-[390px] truncate text-xs text-muted-foreground">{cluster.tracker_hosts.join(", ")}</div>
                    </TableCell>
                    <TableCell>{cluster.category}</TableCell>
                    <TableCell>{t(cluster.reason)}</TableCell>
                    <TableCell className="whitespace-nowrap text-right font-mono tabular-nums">{cluster.score === null ? "—" : Number(cluster.score).toFixed(2)}</TableCell>
                    <TableCell className="text-right font-mono tabular-nums">{formatBytes(cluster.estimated_reclaimed_bytes)}</TableCell>
                    <TableCell className="text-right font-mono tabular-nums">{observed === null ? "—" : formatBytes(observed)}</TableCell>
                  </TableRow>
                ))}
              </TableBody>
            </Table>
          </div>
        )}
      </Card>
    </section>
  );
}

function LegendDot({ color, label }: { color: string; label: string }) {
  return <span className="inline-flex items-center gap-2"><span className="h-2 w-2 rounded-sm" style={{ background: color }} />{label}</span>;
}

function LegendLine({ color, label, dashed = false }: { color: string; label: string; dashed?: boolean }) {
  return <span className="inline-flex items-center gap-2"><span className={`w-4 border-t-2 ${dashed ? "border-dashed" : ""}`} style={{ borderColor: color }} />{label}</span>;
}

function EmptyState({ message }: { message: string }) {
  return <div className="grid min-h-32 place-items-center px-4 py-8 text-center text-sm text-muted-foreground">{message}</div>;
}
