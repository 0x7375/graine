import { Fragment, useCallback, useEffect, useMemo, useRef, useState, type ReactNode } from "react";
import {
  Activity,
  AlertTriangle,
  ChevronDown,
  Clock3,
  Database,
  HardDrive,
  History,
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
import { LanguageContext, initialLanguage, translate, countLabel, useLanguage, type Language } from "@/lib/i18n";
import type {
  DashboardCluster,
  DashboardData,
  ManagementRun,
  PeriodKey,
  TrackerMetric,
} from "@/types";

type Page = "overview" | "storage" | "leaderboard" | "runs";

type RunStatus = {
  running: boolean;
  started_at: number | null;
  finished_at: number | null;
  completed_cycles: number;
  last_error: string | null;
};

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

// Category names already display cross-seed status separately.
const displayCategory = (category: string) => category.replace(/\.cross$/i, "");

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
  const [startingRun, setStartingRun] = useState(false);
  const [runStatus, setRunStatus] = useState<RunStatus | null>(null);
  const [runError, setRunError] = useState<string | null>(null);
  const lastCompletedRef = useRef<number | null>(null);
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

  useEffect(() => {
    let active = true;
    let pending = false;
    const pollStatus = async () => {
      if (pending) return;
      pending = true;
      try {
        const response = await fetch("/api/run/status", { cache: "no-store" });
        if (!response.ok) throw new Error(`Status request failed (${response.status})`);
        const state = await response.json() as RunStatus;
        if (!active) return;
        setRunStatus(state);
        if (lastCompletedRef.current !== null && state.completed_cycles !== lastCompletedRef.current) {
          void loadDashboard();
          setRunError(state.last_error ? translate(language, state.last_error) : null);
        }
        lastCompletedRef.current = state.completed_cycles;
      } catch {
        // Keep the dashboard available if the status endpoint is temporarily unavailable.
      } finally {
        pending = false;
      }
    };
    void pollStatus();
    const timer = window.setInterval(() => void pollStatus(), 1_500);
    return () => { active = false; window.clearInterval(timer); };
  }, [loadDashboard, language]);

  const triggerRun = useCallback(async () => {
    if (runStatus?.running || startingRun) return;
    setRunError(null);
    setStartingRun(true);
    try {
      const response = await fetch("/api/run", { method: "POST" });
      if (response.status === 409) {
        setRunStatus((previous) => previous ? { ...previous, running: true } : previous);
        return;
      }
      if (!response.ok) {
        const body = await response.json().catch(() => null) as { error?: string } | null;
        throw new Error(body?.error || `HTTP ${response.status}`);
      }
      setRunStatus((previous) => previous ? { ...previous, running: true } : previous);
    } catch (reason) {
      setRunError(reason instanceof Error ? reason.message : t("Manual run failed"));
    } finally {
      setStartingRun(false);
    }
  }, [runStatus, startingRun, language]);

  const isRunning = startingRun || Boolean(runStatus?.running);

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

          <div className="flex shrink-0 items-center gap-2">
            <Button variant="outline" size="icon" onClick={() => setDark(!dark)} aria-label={t(dark ? "Switch to light theme" : "Switch to dark theme")}>
              {dark ? <Sun className="h-4 w-4" /> : <Moon className="h-4 w-4" />}
            </Button>
            <Button size="icon" onClick={() => void triggerRun()}
              disabled={isRunning}
              title={`${t(isRunning ? "Management running" : "Run management now")} · ${lastRun}`}
              aria-label={isRunning ? t("Management running") : t("Run management now")}>
              <RefreshCw className={`h-4 w-4 ${isRunning ? "animate-spin" : ""}`} />
            </Button>
          </div>
        </header>

        <Tabs value={page} onValueChange={(value) => setPage(value as Page)} className="mt-5">
          <TabsList className="h-auto min-h-10 w-full flex-wrap justify-start gap-1 overflow-visible rounded-lg border border-border bg-transparent p-1 sm:w-auto [&_[data-state=active]]:bg-muted/70 [&_[data-state=active]]:shadow-none">
            <TabsTrigger value="overview" className="gap-2"><Activity className="h-4 w-4" /> {t("Overview")} </TabsTrigger>
            <TabsTrigger value="storage" className="gap-2"><HardDrive className="h-4 w-4" /> {t("Storage")} </TabsTrigger>
            <TabsTrigger value="leaderboard" className="gap-2"><Upload className="h-4 w-4" /> {t("Leaderboard")} </TabsTrigger>
            <TabsTrigger value="runs" className="gap-2"><History className="h-4 w-4" /> {t("History")} </TabsTrigger>
          </TabsList>

          {notice && (
            <div className={`mt-5 flex items-start gap-2 rounded-lg border px-3 py-2.5 text-sm ${notice.variant === "warning" ? "border-amber-500/30 bg-amber-500/5 text-amber-800 dark:text-amber-300" : "border-primary/30 bg-primary/5 text-primary"}`}>
              {notice.variant === "warning" ? <AlertTriangle className="mt-0.5 h-4 w-4 shrink-0" /> : <Info className="mt-0.5 h-4 w-4 shrink-0" />}
              <span>{notice.text}</span>
            </div>
          )}
          {runError && (
            <div className="mt-4 rounded-lg border border-destructive/30 bg-destructive/5 px-3 py-2 text-sm text-destructive">
              {t("Manual run failed")}: {runError}
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
          <TabsContent value="runs">
            <ExecutionHistoryPage refreshKey={runStatus?.completed_cycles ?? 0} />
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
          <YAxis domain={[0, 100]} ticks={[0, 20, 40, 60, 80, 100]} tickFormatter={(value: number) => value === 0 ? "" : `${value}%`} tickLine={false} axisLine={false} width={43} tick={{ fill: "var(--muted-foreground)", fontSize: 11 }} />
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
  const [expandedCluster, setExpandedCluster] = useState<string | null>(null);
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

      <Card className="overflow-hidden">
        {filteredRows.length === 0 ? (
          <EmptyState message={t("No matching clusters.")} />
        ) : (
          <div>
            <Table containerClassName="rounded-lg" visibleRows={10.5}>
              <TableHeader className="sticky top-0 z-10 bg-card">
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
                {filteredRows.map((cluster, index) => {
                  const key = `${cluster.rank}-${cluster.name}`;
                  const expanded = expandedCluster === key;
                  const inGrace = cluster.status === "GRACE";
                  const graceSeconds = cluster.grace_remaining_seconds;
                  const graceDays = graceSeconds !== undefined && graceSeconds >= 86400;
                  const graceCount = graceSeconds === undefined ? null : Math.ceil(graceSeconds / (graceDays ? 86400 : 3600));
                  const graceTooltip = graceCount === null ? t("Protected by grace period") :
                    `${t("Protected by grace period")} · ${countLabel(language, graceCount, graceDays ? "day" : "hour", graceDays ? "days" : "hours")} ${t(language === "fr" && graceCount === 1 ? "remaining singular" : "remaining")}`;
                  const expandable = cluster.torrent_count > 1;
                  return (
                    <Fragment key={key}>
                      <TableRow className={`h-[60px] ${expandable ? "cursor-pointer" : "hover:bg-transparent"}`} data-ranking-cluster onClick={expandable ? () => setExpandedCluster(expanded ? null : key) : undefined}>
                        <TableCell className="font-mono text-xs text-muted-foreground">{String(index + 1).padStart(2, "0")}</TableCell>
                        <TableCell>
                          {expandable ? (
                            <button type="button" className="flex max-w-[390px] min-w-0 items-center gap-2 text-left focus-visible:rounded-sm focus-visible:outline-none focus-visible:ring-2 focus-visible:ring-ring"
                              aria-expanded={expanded} aria-controls={`cluster-details-${cluster.rank}`} onClick={(event) => { event.stopPropagation(); setExpandedCluster(expanded ? null : key); }}>
                              <ChevronDown className={`h-4 w-4 shrink-0 text-muted-foreground transition-transform ${expanded ? "" : "-rotate-90"}`} />
                              <span className="min-w-0 truncate font-medium" title={cluster.torrent_names.join("\n")}>{cluster.name}</span>
                            </button>
                          ) : (
                            <div className="max-w-[390px] truncate font-medium" title={cluster.name}>{cluster.name}</div>
                          )}
                          <div className={`mt-1 flex max-w-[390px] items-center gap-2 text-xs text-muted-foreground ${expandable ? "pl-6" : ""}`}>
                            <span className="min-w-0 truncate">{countLabel(language, cluster.torrent_count, "torrent", "torrents")} · {cluster.tracker_hosts.join(", ")}</span>
                            {inGrace && <span className="inline-flex shrink-0 items-center text-primary" title={graceTooltip} aria-label={graceTooltip}>
                              <Clock3 className="h-3.5 w-3.5" />
                            </span>}
                          </div>
                        </TableCell>
                        <TableCell className="text-sm text-muted-foreground">{displayCategory(cluster.category)}</TableCell>
                        <TableCell className="whitespace-nowrap text-right font-mono font-medium tabular-nums text-foreground">
                          {cluster.score === null ? (Number(cluster.cost_bytes) === 0 ? "∞" : "—") : Number(cluster.score).toFixed(2)}
                        </TableCell>
                        <TableCell className="text-right font-mono tabular-nums text-muted-foreground">{formatBytes(cluster.cost_bytes)}</TableCell>
                        <TableCell className="text-right font-mono tabular-nums text-muted-foreground">{formatBytes(cluster.uploaded_bytes)}</TableCell>
                      </TableRow>
                      {expandable && expanded && <TableRow className="bg-muted/70 hover:bg-muted/70" data-ranking-details>
                        <TableCell colSpan={6} className="pl-8 pr-8 py-1" id={`cluster-details-${cluster.rank}`}>
                          <ClusterDetails cluster={cluster} />
                        </TableCell>
                      </TableRow>}
                    </Fragment>
                  );
                })}
              </TableBody>
            </Table>
          </div>
        )}
      </Card>
    </section>
  );
}

function ClusterDetails({ cluster }: { cluster: DashboardCluster }) {
  const language = useLanguage();
  const t = (key: string) => translate(language, key);
  // Keep one visible entry per logical tracker. Multiple torrents can legitimately
  // belong to the same tracker; their uploaded values are combined, not dropped.
  const byTracker = new Map<string, {
    tracker: string;
    name: string;
    count: number;
    crossSeeds: number;
    uploadedBytes: number;
  }>();
  for (const member of cluster.members ?? []) {
    const group = byTracker.get(member.tracker) ?? {
      tracker: member.tracker,
      name: member.name,
      count: 0,
      crossSeeds: 0,
      uploadedBytes: 0,
    };
    // Prefer the original torrent's filename when its tracker has cross-seeds.
    if (!member.is_cross_seed && group.crossSeeds === group.count) {
      group.name = member.name;
    }
    group.count += 1;
    if (member.is_cross_seed) group.crossSeeds += 1;
    group.uploadedBytes += member.uploaded_bytes;
    byTracker.set(member.tracker, group);
  }
  const groups = [...byTracker.values()].sort((a, b) => b.uploadedBytes - a.uploadedBytes);

  return (
    <div className="space-y-2">
      {groups.length ? (
        <div className="divide-y divide-border/60">
          {groups.map((group) => (
            <div key={group.tracker} className="flex min-w-0 items-center justify-between gap-4 py-2.5">
              <div className="min-w-0 flex-1">
                <div className="truncate text-sm font-medium" title={group.name}>{group.name}</div>
                <div className="mt-0.5 text-xs text-muted-foreground">
                  {group.tracker}
                  {group.count > 1 ? ` · ${countLabel(language, group.count, "torrent", "torrents")}` : ""}
                  {group.crossSeeds > 0 ? ` · ${t("Cross-seed")}` : ""}
                </div>
              </div>
              <span className="shrink-0 whitespace-nowrap font-mono text-sm tabular-nums text-foreground">
                {formatBytes(group.uploadedBytes)}
              </span>
            </div>
          ))}
        </div>
      ) : (
        // Older stored dashboards have names but no per-torrent tracker mapping.
        <div className="space-y-1 text-sm text-muted-foreground">
          <div>{cluster.tracker_hosts.join(", ")}</div>
          <div>{countLabel(language, cluster.torrent_count, "torrent", "torrents")}</div>
        </div>
      )}
    </div>
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
    .slice(0, 100);

  const changes = dashboard.rank_changes?.[leaderPeriod] || {};

  return (
    <section className="space-y-3 pt-5">
      <div className="flex flex-wrap items-end justify-between gap-3">
        <div>
          <h2 className="text-2xl font-bold tracking-tight">{t("Torrent leaderboard")}</h2>
          <p className="mt-1 text-sm text-muted-foreground">{t("Torrents ranked by net upload.")}</p>
        </div>
        <div className="flex items-center gap-2">
          <label className="text-xs text-muted-foreground" htmlFor="leader-period">{t("Period")}</label>
          <Select value={leaderPeriod} onValueChange={(value) => setLeaderPeriod(value as PeriodKey)}>
            <SelectTrigger id="leader-period"><SelectValue /></SelectTrigger>
            <SelectContent>{Object.entries(PERIOD_LABELS).map(([value, label]) => <SelectItem key={value} value={value}>{t(label)}</SelectItem>)}</SelectContent>
          </Select>
        </div>
      </div>
      <Card className="overflow-hidden">
        {leaders.length === 0 ? (
          <EmptyState message={t("No torrent activity recorded for this period yet.")} />
        ) : (
          <Table containerClassName="rounded-lg" visibleRows={10.5}>
            <TableHeader className="sticky top-0 z-10 bg-card">
              <TableRow>
                <TableHead className="w-[1%] whitespace-nowrap">#</TableHead>
                <TableHead className="min-w-[260px]">{t("Torrent")}</TableHead>
                <TableHead>{t("Category")}</TableHead>
                <TableHead>{t("Last activity")}</TableHead>
                <TableHead className="text-right">{t("Net upload")}</TableHead>
              </TableRow>
            </TableHeader>
            <TableBody>
              {leaders.map((torrent, index) => {
                const net = netUpload(torrent.uploaded_bytes, torrent.downloaded_bytes);
                const change = changes[torrent.hash];
                return (
                  <TableRow key={torrent.hash} className="h-[60px]">
                    <TableCell className="whitespace-nowrap font-mono text-xs">
                      <span className="text-muted-foreground">{index + 1}</span>
                      {change !== undefined && change !== 0 && (
                        <span className={`ml-2 font-semibold ${change > 0 ? "text-primary" : "text-destructive"}`} title={`${t(leaderPeriod === "all" ? "Since previous execution" : "Since first run in period")}${dashboard.rank_baselines_at?.[leaderPeriod] ? ` (${formatDate(dashboard.rank_baselines_at[leaderPeriod]!, language)})` : ""}: ${t(change > 0 ? "up" : "down")} ${countLabel(language, Math.abs(change), "position", "positions")}`}>
                          {change > 0 ? `+${change}` : change}
                        </span>
                      )}
                    </TableCell>
                    <TableCell>
                      <div className="max-w-[400px] truncate font-medium" title={torrent.name}>{torrent.name}</div>
                      <div className="mt-1 text-xs text-muted-foreground">{torrent.tracker}{torrent.is_cross_seed ? ` · ${t("Cross-seed")}` : ""}</div>
                    </TableCell>
                    <TableCell className="whitespace-nowrap text-muted-foreground">{displayCategory(torrent.category)}</TableCell>
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

const RUNS_PAGE_SIZE = 30;
type HistoryFilter = "all" | "deletions" | "errors";
const HISTORY_FILTERS: { value: HistoryFilter; label: string }[] = [
  { value: "all", label: "All" },
  { value: "deletions", label: "Deletions" },
  { value: "errors", label: "Errors" },
];

function formatDuration(milliseconds: number | null): string {
  if (milliseconds === null) return "—";
  if (milliseconds < 1000) return `${milliseconds} ms`;
  const seconds = Math.round(milliseconds / 100) / 10;
  if (seconds < 60) return `${seconds} s`;
  return `${Math.floor(seconds / 60)} min ${Math.round(seconds % 60)} s`;
}

function ExecutionHistoryPage({ refreshKey }: { refreshKey: number }) {
  const language = useLanguage();
  const t = (key: string) => translate(language, key);
  const [runs, setRuns] = useState<ManagementRun[]>([]);
  const [filter, setFilter] = useState<HistoryFilter>("all");
  const [loading, setLoading] = useState(false);
  const [loadingMore, setLoadingMore] = useState(false);
  const [hasMore, setHasMore] = useState(false);
  const [error, setError] = useState<string | null>(null);
  useEffect(() => {
    let active = true;
    setLoading(true);
    setError(null);
    const load = async () => {
      try {
        const response = await fetch(`/api/runs?limit=${RUNS_PAGE_SIZE}&filter=${filter}`, { cache: "no-store" });
        if (!response.ok) throw new Error(`HTTP ${response.status}`);
        const data = await response.json() as ManagementRun[];
        if (!active) return;
        setRuns(data);
        setHasMore(data.length === RUNS_PAGE_SIZE);
      } catch (reason) {
        if (active) setError(reason instanceof Error ? reason.message : String(reason));
      } finally {
        if (active) setLoading(false);
      }
    };
    void load();
    return () => { active = false; };
  }, [refreshKey, filter]);

  const loadMore = async () => {
    if (loadingMore || !hasMore || !runs.length) return;
    setLoadingMore(true);
    setError(null);
    try {
      const before = runs[runs.length - 1].id;
      const response = await fetch(`/api/runs?limit=${RUNS_PAGE_SIZE}&before=${before}&filter=${filter}`, { cache: "no-store" });
      if (!response.ok) throw new Error(`HTTP ${response.status}`);
      const data = await response.json() as ManagementRun[];
      setRuns((previous) => [...previous, ...data.filter((item) => !previous.some((old) => old.id === item.id))]);
      setHasMore(data.length === RUNS_PAGE_SIZE);
    } catch (reason) {
      setError(reason instanceof Error ? reason.message : String(reason));
    } finally {
      setLoadingMore(false);
    }
  };

  return (
    <section className="space-y-3 pt-5">
      <div className="flex flex-wrap items-center justify-between gap-3">
        <h2 className="text-2xl font-bold tracking-tight">{t("History")}</h2>
        <div className="inline-flex items-center gap-1 rounded-lg border border-border p-1" aria-label={t("Filter history")}>
          {HISTORY_FILTERS.map((item) => (
            <Button key={item.value} type="button" size="sm" variant="ghost" aria-pressed={filter === item.value}
              className={`h-8 px-3 ${filter === item.value ? "bg-muted/80 text-foreground" : "text-muted-foreground"}`}
              onClick={() => { if (filter !== item.value) { setRuns([]); setHasMore(false); setFilter(item.value); } }}>
              {t(item.label)}
            </Button>
          ))}
        </div>
      </div>
      {error && <p role="alert" className="text-sm text-destructive">{t("Could not load execution history.")} {error}</p>}
      {loading && runs.length === 0 ? (
        <EmptyState message={t("Loading executions…")} />
      ) : runs.length === 0 ? (
        <Card><EmptyState message={t(filter === "all" ? "No executions recorded yet." : "No matching executions.")} /></Card>
      ) : (
        <div>
          <div className="divide-y divide-border overflow-hidden rounded-lg border border-border bg-card">
            {runs.map((run) => <ExecutionItem key={run.id} run={run} />)}
          </div>
          {hasMore && (
            <div className="flex justify-center pt-3">
              <Button variant="outline" disabled={loadingMore} onClick={() => void loadMore()}>
                {t(loadingMore ? "Loading executions…" : "Show earlier executions")}
              </Button>
            </div>
          )}
        </div>
      )}
    </section>
  );
}

function ExecutionItem({ run }: { run: ManagementRun }) {
  const language = useLanguage();
  const t = (key: string) => translate(language, key);
  const isArchive = run.trigger === "archive" || run.status === "historical";
  const isPreview = run.mode === "dry" || run.mode === "fixture";
  const batch = run.deletion_batch;
  // One row per inode-linked cluster: a cross-seed never creates a duplicate row.
  const groups = batch?.clusters ?? [];
  const recordedTorrentCount = groups.reduce(
    (total, cluster) => total + (cluster.torrent_names.length || 1), 0,
  );
  const removed = run.summary?.removed_torrents ?? recordedTorrentCount;
  const planned = run.summary?.planned_torrents ?? 0;
  // Display logical data belonging to the deleted groups (unique unlinkable
  // inodes), not the unrelated global `df` delta recorded by older versions.
  const removedData = batch?.estimated_reclaimed_bytes ?? null;
  const statusLabels: Partial<Record<ManagementRun["status"], string>> = {
    running: "Running",
    ok: "Success",
    over_limit: "Storage limit reached",
    error: "Failed",
    interrupted: "Interrupted",
  };
  const triggerLabels: Record<Exclude<ManagementRun["trigger"], "archive">, string> = {
    scheduled: "Automatic", manual: "Manual", once: "Command line",
  };
  const statusLabel = statusLabels[run.status];
  const statusColor = run.status === "ok" ? "text-primary"
    : run.status === "error" || run.status === "interrupted" ? "text-destructive"
      : run.status === "over_limit" ? "text-amber-600 dark:text-amber-400"
        : "text-muted-foreground";
  const expandable = groups.length > 0 || Boolean(run.error);
  const count = isPreview ? planned : removed;
  const countLabel = isPreview
    ? t(count === 1 ? "torrent would be removed" : "torrents would be removed")
    : t(count === 1 ? "torrent removed" : "torrents removed");

  const header = (
    <div className="w-full min-w-0 px-4 py-3">
      <div className="flex min-w-0 flex-wrap items-center gap-x-2 gap-y-1 text-xs">
        <time className="shrink-0 whitespace-nowrap font-medium tabular-nums text-muted-foreground" dateTime={new Date(run.started_at * 1000).toISOString()}>
          {formatDate(run.started_at, language)}
        </time>
        {isArchive ? (
          <span className="text-muted-foreground">{t("Archived")}</span>
        ) : (
          <>
            {statusLabel && <span className={`font-semibold ${statusColor}`}>{t(statusLabel)}</span>}
            <span className="text-muted-foreground">{t(triggerLabels[run.trigger as keyof typeof triggerLabels])}</span>
            {isPreview && <span className="text-muted-foreground">· {t("Dry run")}</span>}
          </>
        )}
      </div>
      <div className="mt-1.5 flex min-w-0 items-center justify-between gap-3 text-sm">
        <span className="min-w-0 text-foreground">
          {isArchive && !groups.length
            ? <span className="text-muted-foreground">{t("No recorded deletions")}</span>
            : <><strong className="font-semibold">{count}</strong> <span>{countLabel}</span></>}
        </span>
        <div className="flex shrink-0 items-center gap-3 tabular-nums sm:gap-5">
          {!isPreview && removedData !== null && removed > 0 && (
            <span className="whitespace-nowrap text-muted-foreground">
              <strong className="font-semibold text-foreground">{formatBytes(removedData)}</strong> {t("data removed")}
            </span>
          )}
          {run.duration_ms !== null && (
            <span className="whitespace-nowrap text-xs text-muted-foreground">{formatDuration(run.duration_ms)}</span>
          )}
          {expandable && <ChevronDown aria-hidden="true" className="h-4 w-4 shrink-0 text-muted-foreground transition-transform group-open:rotate-180" />}
        </div>
      </div>
    </div>
  );

  return expandable ? (
    <details className="group min-w-0 bg-card">
      <summary className="cursor-pointer list-none hover:bg-muted/30 [&::-webkit-details-marker]:hidden">
        {header}
      </summary>
      <div className="border-t border-border bg-muted/70 px-8 py-1">
        {run.error && (
          <p className="py-2 text-sm text-destructive">{run.error}</p>
        )}
        {groups.length > 0 && (
          <div className="divide-y divide-border/60">
            {groups.map((cluster, index) => {
              const details = [
                displayCategory(cluster.category),
                cluster.tracker_hosts.join(", "),
              ].filter(Boolean).join(" · ");
              return (
                <div key={`${run.id}-${index}`} className="flex min-w-0 items-center justify-between gap-4 py-2">
                  <div className="min-w-0 flex-1">
                    <div className="truncate text-sm font-medium" title={cluster.torrent_names.join("\n")}>{cluster.name}</div>
                    <div className="mt-0.5 text-xs text-muted-foreground" title={details}>{details}</div>
                  </div>
                  <span
                    className="shrink-0 whitespace-nowrap font-mono text-sm tabular-nums text-foreground"
                  >
                    {formatBytes(cluster.estimated_reclaimed_bytes)}
                  </span>
                </div>
              );
            })}
          </div>
        )}
      </div>
    </details>
  ) : (
    <div className="min-w-0 bg-card">{header}</div>
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
