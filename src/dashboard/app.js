const DATA = window.GRAINE_DATA;

const $ = (id) => document.getElementById(id);

const esc = (value) =>
  String(value ?? '').replace(/[&<>"']/g, (character) => ({
    '&': '&amp;',
    '<': '&lt;',
    '>': '&gt;',
    '"': '&quot;',
    "'": '&#39;',
  })[character]);

const fmt = (number) => {
  if (number === null || number === undefined) {
    return '—';
  }

  const value = Number(number);
  const absolute = Math.abs(value);
  const sign = value < 0 ? '−' : '';

  if (absolute >= 1024 ** 4) {
    return sign + (absolute / 1024 ** 4).toFixed(2) + ' TiB';
  }
  if (absolute >= 1024 ** 3) {
    return sign + (absolute / 1024 ** 3).toFixed(2) + ' GiB';
  }
  if (absolute >= 1024 ** 2) {
    return sign + (absolute / 1024 ** 2).toFixed(1) + ' MiB';
  }
  if (absolute >= 1024) {
    return sign + (absolute / 1024).toFixed(1) + ' KiB';
  }

  return sign + absolute.toFixed(0) + ' B';
};

const net = (torrent) =>
  (torrent.uploaded_bytes || 0) - (torrent.downloaded_bytes || 0);

const fmtNet = (bytes) =>
  `<span class="${bytes >= 0 ? 'positive' : 'negative'}">${bytes >= 0 ? '+' : '−'}${fmt(Math.abs(bytes))}</span>`;

const date = (timestamp) =>
  timestamp ? new Date(timestamp * 1000).toLocaleString() : '—';

const shortDate = (timestamp) =>
  timestamp ? new Date(timestamp * 1000).toLocaleDateString() : '—';

const pct = (number) =>
  `${Number(number || 0).toFixed(1)}%`;

const period = (name) =>
  DATA.periods[name] || { torrents: [], tracker_history: [] };

const diskCategories = [
  { key: 'radarr', label: 'Radarr', color: 'var(--disk-radarr)' },
  { key: 'sonarr', label: 'Sonarr', color: 'var(--disk-sonarr)' },
  { key: 'autobrr', label: 'Autobrr', color: 'var(--disk-autobrr)' },
  { key: 'other', label: 'Other', color: 'var(--disk-other)' },
];

const trackerColors = Array.from({ length: 8 }, (_, index) =>
  `var(--tracker-color-${index + 1})`
);


// --------------------------------------------------
// Header
// --------------------------------------------------

function renderHeader() {
  const run = DATA.run;
  $('last-run').textContent =
    `Last execution ${date(run.timestamp)} · ${run.managed_torrents} managed torrents · ${run.clusters} completed clusters`;

  if (run.mode === 'dry') {
    $('notice').innerHTML = `
      <div class="notice">
        Dry run — no torrents were removed.
      </div>
    `;
  } else if (run.result === 'over_limit') {
    $('notice').innerHTML = `
      <div class="notice">
        Disk remains above the high watermark: Graine could not reclaim
        enough space while keeping torrents within their grace period.
      </div>
    `;
  } else {
    $('notice').innerHTML = '';
  }
}


// --------------------------------------------------
// Overview metrics
// --------------------------------------------------

function renderMetrics() {
  const run = DATA.run;
  const torrentData =
    Number(DATA.reclaimable_bytes || 0) + Number(DATA.protected_bytes || 0);

  const cards = [
    [
      'Disk used',
      pct(run.disk_used_percent),
      `${fmt(run.disk_used_bytes)} of ${fmt(run.disk_total_bytes)}`,
    ],
    [
      'Managed torrent data',
      fmt(torrentData),
      'Unique data represented by retained completed torrent clusters',
    ],
  ];

  $('metrics').innerHTML = cards
    .map((card) => `
      <div class="card metric">
        <div class="label">${card[0]}</div>
        <div class="value">${card[1]}</div>
        <div class="hint">${card[2]}</div>
      </div>
    `)
    .join('');
}


// --------------------------------------------------
// Stacked disk usage chart
// --------------------------------------------------

function renderDiskChart() {
  const history = DATA.history || [];

  if (!history.length) {
    $('disk-chart').innerHTML = '<div class="empty">No history yet.</div>';
    return;
  }

  const width = 900;
  const height = 280;
  const left = 62;
  const right = 20;
  const top = 16;
  const bottom = 34;
  const innerWidth = width - left - right;
  const innerHeight = height - top - bottom;

  const points = history.map((entry, index) => {
    const usage = entry.category_usage || {};
    const categoryBytes = (key) => Object.entries(usage)
      .filter(([category]) => category.toLowerCase() === key)
      .reduce((sum, [, bytes]) => sum + Number(bytes || 0), 0);

    const total = Number(entry.disk_used_bytes || 0);
    let radarr = categoryBytes('radarr');
    let sonarr = categoryBytes('sonarr');
    let autobrr = categoryBytes('autobrr');
    const diskTotal = Number(entry.disk_total_bytes || 0);
    const attributed = radarr + sonarr + autobrr;

    // Filesystem accounting (for example, reflinks or compression) can make st_blocks
    // estimates differ slightly from df. Keep the stacked total bounded by actual usage.
    if (attributed > total && attributed > 0) {
      const scale = total / attributed;
      radarr *= scale;
      sonarr *= scale;
      autobrr *= scale;
    }
    const other = Math.max(0, total - radarr - sonarr - autobrr);

    return {
      index,
      timestamp: Number(entry.timestamp || 0),
      total: diskTotal ? total / diskTotal * 100 : 0,
      values: [radarr, sonarr, autobrr, other].map((bytes) =>
        diskTotal ? bytes / diskTotal * 100 : 0
      ),
      high: Number(entry.high_watermark_percent || 85),
      low: Number(entry.low_watermark_percent || 80),
    };
  });

  const x = (index) =>
    left + (points.length === 1 ? innerWidth / 2 : index / (points.length - 1) * innerWidth);
  const y = (value) =>
    top + (100 - value) / 100 * innerHeight;

  let svg = `
    <svg class="chart" viewBox="0 0 ${width} ${height}" role="img"
         aria-label="Stacked disk usage history by torrent category">
  `;

  // Percentage grid and labels.
  for (let value = 0; value <= 100; value += 20) {
    svg += `
      <line class="gridline" x1="${left}" x2="${width - right}"
            y1="${y(value)}" y2="${y(value)}" />
      <text x="${left - 10}" y="${y(value) + 4}" text-anchor="end">${value}%</text>
    `;
  }

  // Build each category as a band between the previous cumulative total and this one.
  const cumulative = new Array(points.length).fill(0);
  diskCategories.forEach((category, categoryIndex) => {
    const lower = cumulative.slice();
    points.forEach((point, pointIndex) => {
      cumulative[pointIndex] += point.values[categoryIndex];
    });

    const upperPath = points.map((point, pointIndex) =>
      `${x(pointIndex)},${y(cumulative[pointIndex])}`
    );
    const lowerPath = points.map((point, pointIndex) =>
      `${x(pointIndex)},${y(lower[pointIndex])}`
    ).reverse();

    if (points.some((point) => point.values[categoryIndex] > 0)) {
      svg += `
        <path d="M${upperPath.join(' L')} L${lowerPath.join(' L')} Z"
              fill="${category.color}" opacity="0.78">
          <title>${category.label}</title>
        </path>
      `;
    }
  });

  const last = history[history.length - 1];
  const high = Number(last.high_watermark_percent || 85);
  const low = Number(last.low_watermark_percent || 80);

  // Total usage outline and watermark references.
  const totalLine = points
    .map((point, index) => `${x(index)},${y(point.total)}`)
    .join(' ');

  svg += `
    <polyline class="disk-total" points="${totalLine}" />
    <line class="watermark-high" x1="${left}" x2="${width - right}"
          y1="${y(high)}" y2="${y(high)}" />
    <line class="watermark-low" x1="${left}" x2="${width - right}"
          y1="${y(low)}" y2="${y(low)}" />
    <text x="${left}" y="${height - 8}">${esc(shortDate(points[0].timestamp))}</text>
    <text x="${width - right}" y="${height - 8}" text-anchor="end">${esc(shortDate(points[points.length - 1].timestamp))}</text>
  `;

  svg += '</svg>';
  $('disk-chart').innerHTML = svg;
}


// --------------------------------------------------
// Tracker performance chart
// --------------------------------------------------

function renderTrackerChart() {
  const selectedPeriod = period($('tracker-period').value);
  const metric = $('tracker-metric').value;
  const history = selectedPeriod.tracker_history || [];
  const trackerNames = [...new Set(
    history.flatMap((point) => Object.keys(point.trackers || {}))
  )].sort();

  const series = trackerNames
    .map((name) => ({
      name,
      values: history.map((point) =>
        Number(point.trackers?.[name]?.[metric] || 0)
      ),
    }))
    .filter((tracker) => tracker.values.some((value) => value !== 0));

  if (!history.length || !series.length) {
    $('tracker-chart').innerHTML = `
      <div class="empty">No configured tracker activity recorded for this period yet.</div>
    `;
    $('tracker-legend').innerHTML = '';
    return;
  }

  const allValues = series.flatMap((tracker) => tracker.values);
  let min = Math.min(0, ...allValues);
  let max = Math.max(0, ...allValues);
  if (min === max) {
    min -= 1;
    max += 1;
  }
  const padding = (max - min) * 0.08;
  min -= padding;
  max += padding;

  const width = 900;
  const height = 290;
  const left = 84;
  const right = 20;
  const top = 18;
  const bottom = 36;
  const innerWidth = width - left - right;
  const innerHeight = height - top - bottom;
  const firstTimestamp = Number(history[0].timestamp || 0);
  const lastTimestamp = Number(history[history.length - 1].timestamp || firstTimestamp);

  const x = (timestamp, index) =>
    lastTimestamp === firstTimestamp
      ? left + innerWidth / 2
      : left + (timestamp - firstTimestamp) / (lastTimestamp - firstTimestamp) * innerWidth;
  const y = (value) =>
    top + (max - value) / (max - min) * innerHeight;
  const formatAxis = (value) => `${value < 0 ? '−' : ''}${fmt(Math.abs(value))}`;

  let svg = `
    <svg class="chart" viewBox="0 0 ${width} ${height}" role="img"
         aria-label="Cumulative net upload by tracker over time">
  `;

  for (let index = 0; index <= 4; index++) {
    const value = max - (max - min) * index / 4;
    svg += `
      <line class="gridline" x1="${left}" x2="${width - right}"
            y1="${y(value)}" y2="${y(value)}" />
      <text x="${left - 10}" y="${y(value) + 4}" text-anchor="end">${esc(formatAxis(value))}</text>
    `;
  }

  if (min < 0 && max > 0) {
    svg += `
      <line class="gridline" x1="${left}" x2="${width - right}"
            y1="${y(0)}" y2="${y(0)}" stroke-dasharray="2 4" />
    `;
  }

  series.forEach((tracker, trackerIndex) => {
    const color = trackerColors[trackerIndex % trackerColors.length];
    const polyline = tracker.values
      .map((value, index) => `${x(Number(history[index].timestamp || 0), index)},${y(value)}`)
      .join(' ');
    const finalValue = tracker.values[tracker.values.length - 1];
    const finalX = x(lastTimestamp, tracker.values.length - 1);
    const finalY = y(finalValue);

    svg += `
      <polyline class="tracker-line" stroke="${color}" points="${polyline}">
        <title>${esc(tracker.name)}: ${esc(fmtNet(finalValue).replace(/<[^>]*>/g, ''))}</title>
      </polyline>
      <circle class="tracker-point" cx="${finalX}" cy="${finalY}" r="3.5" fill="${color}" />
    `;
  });

  svg += `
    <text x="${left}" y="${height - 8}">${esc(shortDate(firstTimestamp))}</text>
    <text x="${width - right}" y="${height - 8}" text-anchor="end">${esc(shortDate(lastTimestamp))}</text>
  `;
  svg += '</svg>';
  $('tracker-chart').innerHTML = svg;

  $('tracker-legend').innerHTML = series
    .map((tracker, index) => {
      const value = tracker.values[tracker.values.length - 1];
      return `
        <span class="tracker-legend-item" style="--series-color: ${trackerColors[index % trackerColors.length]}">
          <i class="tracker-legend-swatch"></i>
          <span>${esc(tracker.name)}</span>
          <strong class="tracker-legend-value">${fmtNet(value)}</strong>
        </span>
      `;
    })
    .join('');
}


// --------------------------------------------------
// Torrent leaderboard
// --------------------------------------------------

function renderLeaderboard() {
  const selectedPeriod = period($('leader-period').value);
  const rows = [...(selectedPeriod.torrents || [])]
    .sort((a, b) => net(b) - net(a))
    .slice(0, 10);

  if (!rows.length) {
    $('leader-table').innerHTML = `
      <div class="empty">No torrent activity recorded for this period yet.</div>
    `;
    return;
  }

  const body = rows
    .map((torrent, index) => `
      <tr>
        <td class="rank">${index + 1}</td>
        <td class="namecell" title="${esc(torrent.name)}">
          ${esc(torrent.name)}
          <small>
            ${esc(torrent.tracker)}${torrent.is_cross_seed ? ' · Cross-seed' : ''}
          </small>
        </td>
        <td>${date(torrent.timestamp)}</td>
        <td class="num">${fmtNet(net(torrent))}</td>
      </tr>
    `)
    .join('');

  $('leader-table').innerHTML = `
    <table>
      <thead>
        <tr>
          <th>#</th>
          <th>Torrent</th>
          <th>Last activity</th>
          <th class="num">Net upload</th>
        </tr>
      </thead>
      <tbody>${body}</tbody>
    </table>
  `;
}


// --------------------------------------------------
// Retention ranking
// --------------------------------------------------

function renderRanking() {
  const query = $('ranking-search').value.toLowerCase();
  const score = (cluster) => cluster.score === null
    ? (Number(cluster.cost_bytes) === 0 ? Infinity : -Infinity)
    : Number(cluster.score);

  const rows = (DATA.ranking || []).filter((cluster) => {
    const searchableText = [
      cluster.name,
      cluster.category,
      cluster.tracker_hosts.join(' '),
      cluster.torrent_names.join(' '),
    ]
      .join(' ')
      .toLowerCase();

    return searchableText.includes(query);
  }).sort((a, b) => score(b) - score(a));

  if (!rows.length) {
    $('ranking-table').innerHTML = '<div class="empty">No matching clusters.</div>';
    return;
  }

  const body = rows
    .map((cluster, index) => `
      <tr>
        <td class="rank">${String(index + 1).padStart(2, '0')}</td>
        <td class="namecell" title="${esc(cluster.torrent_names.join('\n'))}">
          ${esc(cluster.name)}
          <small>
            ${cluster.torrent_count} torrent(s) · ${esc(cluster.tracker_hosts.join(', '))}
          </small>
        </td>
        <td>${esc(cluster.category)}</td>
        <td class="num">
          ${cluster.score === null
            ? (Number(cluster.cost_bytes) === 0 ? '∞' : '—')
            : Number(cluster.score).toFixed(2)}
        </td>
        <td class="num">${fmt(cluster.cost_bytes)}</td>
        <td class="num">${fmt(cluster.uploaded_bytes)}</td>
      </tr>
    `)
    .join('');

  $('ranking-table').innerHTML = `
    <div class="ranking-scroll" tabindex="0" aria-label="Ranking, scroll for more clusters">
      <table>
        <thead>
          <tr>
            <th>#</th>
            <th>Cluster / torrents</th>
            <th>Category</th>
            <th class="num">Score</th>
            <th class="num">Cost</th>
            <th class="num">Uploaded</th>
          </tr>
        </thead>
        <tbody>${body}</tbody>
      </table>
    </div>
  `;
}


// --------------------------------------------------
// Eviction history
// --------------------------------------------------

function renderDeletions() {
  const batches = [...(DATA.deletions || [])]
    .sort((a, b) => b.timestamp - a.timestamp);

  if (!batches.length) {
    $('deletion-table').innerHTML = `
      <div class="empty">No actual eviction has been recorded yet.</div>
    `;
    return;
  }

  const rows = [];
  for (const batch of batches) {
    for (let index = 0; index < batch.clusters.length; index++) {
      const cluster = batch.clusters[index];
      rows.push(`
        <tr>
          <td>${date(batch.timestamp)}</td>
          <td class="namecell" title="${esc(cluster.torrent_names.join('\n'))}">
            ${esc(cluster.name)}
            <small>${esc(cluster.tracker_hosts.join(', '))}</small>
          </td>
          <td>${esc(cluster.category)}</td>
          <td>${esc(cluster.reason)}</td>
          <td class="num">${cluster.score === null ? '—' : Number(cluster.score).toFixed(2)}</td>
          <td class="num">${fmt(cluster.estimated_reclaimed_bytes)}</td>
          <td class="num">
            ${index === 0 && batch.observed_reclaimed_bytes !== null
              ? fmt(batch.observed_reclaimed_bytes)
              : '—'}
          </td>
        </tr>
      `);
    }
  }

  $('deletion-table').innerHTML = `
    <table>
      <thead>
        <tr>
          <th>When</th>
          <th>Cluster</th>
          <th>Category</th>
          <th>Reason</th>
          <th class="num">Score</th>
          <th class="num">Estimated</th>
          <th class="num">Observed batch</th>
        </tr>
      </thead>
      <tbody>${rows.join('')}</tbody>
    </table>
  `;
}


// --------------------------------------------------
// Navigation and event listeners
// --------------------------------------------------

$('tracker-period').addEventListener('change', renderTrackerChart);
$('tracker-metric').addEventListener('change', renderTrackerChart);
$('leader-period').addEventListener('change', renderLeaderboard);
$('ranking-search').addEventListener('input', renderRanking);

const viewTabs = [...document.querySelectorAll('[role="tab"][data-view]')];

function activateView(tab, moveFocus = false) {
  viewTabs.forEach((item) => {
    const active = item === tab;
    item.classList.toggle('active', active);
    item.setAttribute('aria-selected', String(active));
    item.tabIndex = active ? 0 : -1;
    const panel = $(`view-${item.dataset.view}`);
    panel.hidden = !active;
    panel.classList.toggle('active', active);
  });

  if (moveFocus) {
    tab.focus();
  }
}

viewTabs.forEach((tab, index) => {
  tab.addEventListener('click', () => activateView(tab));
  tab.addEventListener('keydown', (event) => {
    if (!['ArrowLeft', 'ArrowRight', 'Home', 'End'].includes(event.key)) {
      return;
    }
    event.preventDefault();
    const next = event.key === 'Home' ? 0
      : event.key === 'End' ? viewTabs.length - 1
        : (index + (event.key === 'ArrowRight' ? 1 : -1) + viewTabs.length) % viewTabs.length;
    activateView(viewTabs[next], true);
  });
});
activateView(viewTabs[0]);


// --------------------------------------------------
// Initial render
// --------------------------------------------------

renderHeader();
renderMetrics();
renderDiskChart();
renderTrackerChart();
renderLeaderboard();
renderRanking();
renderDeletions();
