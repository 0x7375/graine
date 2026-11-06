
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

const fmtNet = (bytes) => `
    <span class="${bytes >= 0 ? 'positive' : 'negative'}">
        ${bytes >= 0 ? '+' : '−'}${fmt(Math.abs(bytes))}
    </span>
`;

const date = (timestamp) =>
  timestamp ? new Date(timestamp * 1000).toLocaleString() : '—';

const pct = (number) =>
  `${Number(number || 0).toFixed(1)}%`;

const period = (name) =>
  DATA.periods[name] || { trackers: [], torrents: [] };


// --------------------------------------------------
// Header
// --------------------------------------------------

function renderHeader() {
  const run = DATA.run;
  $('last-run').textContent = `
        Last execution ${date(run.timestamp)}
        · ${run.managed_torrents} managed torrents
        · ${run.clusters} completed clusters
    `.trim();

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
  const actual = run.actual_reclaimed_bytes;

  const cards = [
    [
      'Disk used',
      pct(run.disk_used_percent),
      `${fmt(run.disk_used_bytes)} of ${fmt(run.disk_total_bytes)}`,
    ],
    [
      'Torrent data',
      fmt(Number(DATA.reclaimable_bytes || 0) + Number(DATA.protected_bytes || 0)),
      'Unique space used by retained torrent data',
    ],
    [
      run.mode === 'dry' ? 'Would reclaim' : 'Reclaimed this run',
      actual === null
        ? fmt(run.estimated_reclaimed_bytes)
        : fmt(actual),
      actual === null
        ? 'Estimated from this run’s candidates'
        : 'Observed df usage change',
    ],
  ];

  $('metrics').innerHTML = cards
    .map((card, index) => `
            <div class="card metric">
                <div class="label">${card[0]}</div>
                <div class="value ${index === 2 && Number(actual || 0) > 0 ? 'good' : ''
                }">${card[1]}</div>
                <div class="hint">${card[2]}</div>
            </div>
        `)
    .join('');
}


// --------------------------------------------------
// Disk usage chart
// --------------------------------------------------

function renderChart() {
  const history = DATA.history || [];

  if (!history.length) {
    $('disk-chart').innerHTML = `
            <div class="empty">No history yet.</div>
        `;
    return;
  }

  const width = 900;
  const height = 230;

  const left = 48;
  const right = 18;
  const top = 14;
  const bottom = 30;

  const innerWidth = width - left - right;
  const innerHeight = height - top - bottom;

  const values = history.map((entry) =>
    Number(entry.disk_used_percent || 0)
  );

  const max = Math.max(
    100,
    ...values,
    Number(history[history.length - 1].high_watermark_percent || 85)
  );

  const min = 0;

  const x = (index) =>
    left + (
      history.length === 1
        ? innerWidth / 2
        : index / (history.length - 1) * innerWidth
    );

  const y = (value) =>
    top + (max - value) / (max - min) * innerHeight;

  let svg = `
        <svg
            class="chart"
            viewBox="0 0 ${width} ${height}"
            role="img"
            aria-label="Disk usage history"
        >
    `;

  // Horizontal grid lines and labels.
  for (let value = 0; value <= 100; value += 20) {
    svg += `
            <line
                class="gridline"
                x1="${left}"
                x2="${width - right}"
                y1="${y(value)}"
                y2="${y(value)}"
            />
            <text
                x="${left - 9}"
                y="${y(value) + 4}"
                text-anchor="end"
            >${value}%</text>
        `;
  }

  const lastEntry = history[history.length - 1];

  const high = Number(lastEntry.high_watermark_percent || 85);
  const low = Number(lastEntry.low_watermark_percent || 80);

  // Watermark reference lines.
  svg += `
        <line
            class="high"
            x1="${left}"
            x2="${width - right}"
            y1="${y(high)}"
            y2="${y(high)}"
        />
        <line
            class="low"
            x1="${left}"
            x2="${width - right}"
            y1="${y(low)}"
            y2="${y(low)}"
        />
    `;

  // Usage history line.
  const points = history
    .map((entry, index) =>
      `${x(index)},${y(entry.disk_used_percent)}`
    )
    .join(' ');

  svg += `
        <polyline class="usage" points="${points}" />
    `;

  // Highlight the latest measurement.
  svg += `
        <circle
            cx="${x(history.length - 1)}"
            cy="${y(lastEntry.disk_used_percent)}"
            r="4"
            fill="var(--accent)"
        />

        <text x="${left}" y="${height - 7}">
            ${esc(new Date(history[0].timestamp * 1000).toLocaleDateString())}
        </text>

        <text
            x="${width - right}"
            y="${height - 7}"
            text-anchor="end"
        >
            ${esc(new Date(lastEntry.timestamp * 1000).toLocaleDateString())}
        </text>
    `;

  svg += '</svg>';

  $('disk-chart').innerHTML = svg;
}


// --------------------------------------------------
// --------------------------------------------------
// Pool usage
// --------------------------------------------------

function renderPools() {
  const pools = DATA.pools || [];

  if (!pools.length) {
    $('pool-list').innerHTML = `
            <div class="empty">No pools.</div>
        `;
    return;
  }

  $('pool-list').innerHTML = [...pools].sort((a, b) =>
    (Number(b.reclaimable_bytes || 0) + Number(b.protected_bytes || 0)) -
    (Number(a.reclaimable_bytes || 0) + Number(a.protected_bytes || 0))
  )
    .map((pool) => {
      const used = Number(pool.reclaimable_bytes || 0) + Number(pool.protected_bytes || 0);

      const poolCap = pool.capacity_bytes === null
        ? null
        : Number(pool.capacity_bytes);
      const capacity = poolCap || Number(DATA.run.disk_total_bytes || 0);
      const ratio = capacity ? Math.min(100, used / capacity * 100) : 0;

      return `
                <div class="pool">
                    <div class="pooltop">
                        <strong>${esc(pool.name)}</strong>

                        <span>${fmt(used)} / ${fmt(capacity)}${poolCap ? ' pool cap' : ' filesystem'}</span>
                    </div>
                    <div class="bar">
                        <div class="used" style="width: ${ratio}%"></div>
                    </div>
                    <div class="splitlabels"><span>${ratio.toFixed(1)}% of ${poolCap ? 'pool cap' : 'filesystem'}</span></div>
                </div>
            `;
    })
    .join('');
}


// --------------------------------------------------
// Tracker performance
// --------------------------------------------------

function renderTrackers() {
  const selectedPeriod = period($('tracker-period').value);
  const rows = selectedPeriod.trackers || [];

  if (!rows.length) {
    $('tracker-table').innerHTML = `
            <div class="empty">
                No tracker activity recorded for this period yet.
            </div>
        `;
    return;
  }

  const sortedRows = [...rows].sort((a, b) => net(b.total) - net(a.total));
  $('tracker-table').innerHTML = `
        <div class="tracker-list" role="list">
          ${sortedRows.map((tracker, index) => `
            <div class="tracker-row" role="listitem">
              <span class="tracker-rank">${String(index + 1).padStart(2, '0')}</span>
              <span class="tracker-name" title="${esc(tracker.tracker)}">${esc(tracker.tracker)}</span>
              <span class="tracker-value">${fmtNet(net(tracker.total))}<br><small>${fmtNet(net(tracker.cross_seed))} cross-seed</small></span>
            </div>
          `).join('')}
        </div>
    `;
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
            <div class="empty">
                No torrent activity recorded for this period yet.
            </div>
        `;
    return;
  }

  const body = rows
    .map((torrent, index) => {
      return `
                <tr>
                    <td class="rank">${index + 1}</td>

                    <td
                        class="namecell"
                        title="${esc(torrent.name)}"
                    >
                        ${esc(torrent.name)}

                        <small>
                            ${esc(torrent.tracker)}
                            ${torrent.is_cross_seed ? ' · Cross-seed' : ''}
                        </small>
                    </td>

                    <td>${date(torrent.timestamp)}</td>
                    <td class="num">${fmtNet(net(torrent))}</td>
                </tr>
            `;
    })
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
// Ranking
// --------------------------------------------------

function renderRanking() {
  const query = $('ranking-search').value.toLowerCase();
  const score = (cluster) => cluster.score === null
    ? (Number(cluster.cost_bytes) === 0 ? Infinity : -Infinity)
    : Number(cluster.score);
  const rows = (DATA.ranking || []).filter((cluster) => {
    const searchableText = [
      cluster.name,
      cluster.pool,
      cluster.tracker_hosts.join(' '),
      cluster.torrent_names.join(' '),
    ]
      .join(' ')
      .toLowerCase();

    return searchableText.includes(query);
  }).sort((a, b) => score(b) - score(a));

  if (!rows.length) {
    $('ranking-table').innerHTML = `
            <div class="empty">No matching clusters.</div>
        `;
    return;
  }

  const body = rows
    .map((cluster, index) => `
            <tr>
                <td class="rank">${String(index + 1).padStart(2, '0')}</td>

                <td
                    class="namecell"
                    title="${esc(cluster.torrent_names.join('\n'))}"
                >
                    ${esc(cluster.name)}

                    <small>
                        ${cluster.torrent_count} torrent(s)
                        · ${esc(cluster.tracker_hosts.join(', '))}
                    </small>
                </td>

                <td>${esc(cluster.pool)}</td>

                <td class="num">
                    ${cluster.score === null
        ? (Number(cluster.cost_bytes) === 0 ? '∞' : '—')
        : Number(cluster.score).toFixed(2)
      }
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
                    <th>Pool</th>
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
            <div class="empty">
                No actual eviction has been recorded yet.
            </div>
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

                    <td
                        class="namecell"
                        title="${esc(cluster.torrent_names.join('\n'))}"
                    >
                        ${esc(cluster.name)}

                        <small>
                            ${esc(cluster.tracker_hosts.join(', '))}
                        </small>
                    </td>

                    <td>${esc(cluster.pool)}</td>
                    <td>${esc(cluster.reason)}</td>

                    <td class="num">
                        ${cluster.score === null
          ? '—'
          : Number(cluster.score).toFixed(2)
        }
                    </td>

                    <td class="num">
                        ${fmt(cluster.estimated_reclaimed_bytes)}
                    </td>

                    <td class="num">
                        ${index === 0 && batch.observed_reclaimed_bytes !== null
          ? fmt(batch.observed_reclaimed_bytes)
          : '—'
        }
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
                    <th>Pool</th>
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
// Event listeners
// --------------------------------------------------

$('tracker-period').addEventListener('change', renderTrackers);

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
  if (moveFocus) tab.focus();
}

viewTabs.forEach((tab, index) => {
  tab.addEventListener('click', () => activateView(tab));
  tab.addEventListener('keydown', (event) => {
    if (!['ArrowLeft', 'ArrowRight', 'Home', 'End'].includes(event.key)) return;
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
renderChart();
renderPools();
renderTrackers();
renderLeaderboard();
renderRanking();
renderDeletions();

