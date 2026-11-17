export type RunSummary = {
  timestamp: number;
  mode: "live" | "dry" | "fixture" | "historical";
  result: "ok" | "over_limit" | "imported";
  disk_used_bytes: number;
  disk_total_bytes: number;
  disk_used_percent: number;
  projected_used_bytes: number;
  projected_used_percent: number;
  high_watermark_percent: number;
  low_watermark_percent: number;
  estimated_reclaimed_bytes: number;
  actual_reclaimed_bytes: number | null;
  managed_torrents: number;
  complete_torrents: number;
  clusters: number;
};

export type HistorySnapshot = {
  timestamp: number;
  disk_used_bytes: number;
  disk_total_bytes: number;
  disk_used_percent: number;
  projected_used_bytes: number;
  projected_used_percent: number;
  high_watermark_percent: number;
  low_watermark_percent: number;
  reclaimable_bytes: number;
  protected_bytes: number;
  estimated_reclaimed_bytes: number;
  actual_reclaimed_bytes: number | null;
  category_usage: Record<string, number>;
};

export type DashboardMember = {
  name: string;
  tracker: string;
  category: string;
  is_cross_seed: boolean;
  uploaded_bytes: number;
  seeding_days: number;
  inactive_days: number;
  adjusted_upload_per_day?: number;
};

export type DashboardCluster = {
  rank: number;
  name: string;
  torrent_names: string[];
  tracker_hosts: string[];
  category: string;
  status: string;
  reason: string | null;
  priority: number;
  score: number | null;
  cost_bytes: number;
  data_bytes: number;
  uploaded_bytes: number;
  seeding_days: number;
  torrent_count: number;
  cross_seed_count: number;
  grace_remaining_seconds?: number;
  members?: DashboardMember[];
};

export type LeaderboardEntry = {
  hash: string;
  name: string;
  tracker: string;
  category: string;
  is_cross_seed: boolean;
  uploaded_bytes: number;
  downloaded_bytes: number;
  timestamp: number;
};

export type TrackerHistoryTotals = {
  total_net_bytes: number;
  regular_net_bytes: number;
  cross_seed_net_bytes: number;
};

export type TrackerHistoryPoint = {
  timestamp: number;
  trackers: Record<string, TrackerHistoryTotals>;
};

export type AnalyticsPeriod = {
  torrents: LeaderboardEntry[];
  tracker_history: TrackerHistoryPoint[];
};

export type DeletionEvent = {
  name: string;
  cross_seed_count?: number;
  torrent_names: string[];
  tracker_hosts: string[];
  category: string;
  reason: string;
  priority: number;
  score: number | null;
  estimated_reclaimed_bytes: number;
  uploaded_bytes: number;
  seeding_days: number;
};

export type DeletionBatch = {
  batch_id: string;
  timestamp: number;
  estimated_reclaimed_bytes: number;
  observed_reclaimed_bytes: number | null;
  clusters: DeletionEvent[];
};

export type DashboardData = {
  generated_at: number;
  run: RunSummary;
  history: HistorySnapshot[];
  reclaimable_bytes: number;
  protected_bytes: number;
  ranking: DashboardCluster[];
  periods: Record<PeriodKey, AnalyticsPeriod>;
  rank_changes?: Partial<Record<PeriodKey, Record<string, number>>>;
  rank_baselines_at?: Partial<Record<PeriodKey, number>>;
  deletions: DeletionBatch[];
};

export type TrackerMetric = "total_net_bytes" | "regular_net_bytes" | "cross_seed_net_bytes";
export type PeriodKey = "all" | "year" | "month" | "week";

export type ManagementRun = {
  id: number;
  started_at: number;
  finished_at: number | null;
  duration_ms: number | null;
  trigger: "scheduled" | "manual" | "once" | "archive";
  mode: "live" | "dry" | "fixture" | "historical";
  status: "running" | "ok" | "over_limit" | "error" | "interrupted" | "historical";
  summary: (RunSummary & {
    planned_torrents?: number;
    planned_clusters?: number;
    removed_torrents?: number;
    removed_clusters?: number;
    deletion_batch_id?: string | null;
  }) | null;
  snapshot: HistorySnapshot | null;
  deletion_batch: DeletionBatch | null;
  error: string | null;
};
