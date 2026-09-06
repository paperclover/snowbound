#!/usr/bin/env node

import { mkdir, readFile, readdir, writeFile } from "node:fs/promises";
import { homedir } from "node:os";
import { dirname, join, resolve } from "node:path";

const rates = { input: 10, cached: 1, output: 50 };

function usageCost(usage) {
  return ((usage.input_tokens - usage.cached_input_tokens) * rates.input
    + usage.cached_input_tokens * rates.cached
    + usage.output_tokens * rates.output) / 1_000_000;
}

function duration(ms) {
  const seconds = Math.round(ms / 1000);
  const hours = Math.floor(seconds / 3600);
  const minutes = Math.floor((seconds % 3600) / 60);
  const remainder = seconds % 60;
  return hours ? `${hours}h ${minutes}m` : minutes ? `${minutes}m ${remainder}s` : `${remainder}s`;
}

function unionDuration(intervals) {
  const merged = [];
  for (const interval of intervals.sort((a, b) => a[0] - b[0])) {
    const previous = merged.at(-1);
    if (previous && interval[0] <= previous[1]) previous[1] = Math.max(previous[1], interval[1]);
    else merged.push([...interval]);
  }
  return merged.reduce((total, [start, end]) => total + end - start, 0);
}

async function filesBelow(root) {
  const entries = await readdir(root, { withFileTypes: true });
  const nested = await Promise.all(entries.map((entry) => {
    const path = join(root, entry.name);
    return entry.isDirectory() ? filesBelow(path) : entry.name.endsWith(".jsonl") ? [path] : [];
  }));
  return nested.flat();
}

async function loadEvents(paths) {
  const events = [];
  for (const path of paths) {
    for (const line of (await readFile(path, "utf8")).trim().split("\n")) {
      if (!line) continue;
      try { events.push(JSON.parse(line)); } catch { /* Ignore a partially-written final line. */ }
    }
  }
  return events;
}

function snapshotBefore(records, time) {
  return records.filter((record) => Date.parse(record.timestamp) <= time).at(-1)?.payload.thread_token_usage;
}

function taskRows(events, start, end) {
  const tasks = new Map();
  for (const event of events) {
    if (event.type !== "event_msg" || !["task_started", "task_complete"].includes(event.payload.type)) continue;
    const prior = tasks.get(event.payload.turn_id) ?? {};
    tasks.set(event.payload.turn_id, { ...prior, ...event.payload });
  }
  return [...tasks.values()].filter((task) => task.started_at >= start && task.started_at < end && task.duration_ms);
}

function toolIntervals(events, start, end) {
  return events.flatMap((event) => {
    const item = event.type === "event_msg" && event.payload.type === "item_completed" ? event.payload.item : undefined;
    if (!item || !["CommandExecution", "McpToolCall"].includes(item.type)) return [];
    const milliseconds = (item.duration?.secs ?? 0) * 1000 + (item.duration?.nanos ?? 0) / 1_000_000;
    const finished = Date.parse(event.timestamp);
    return milliseconds && finished >= start && finished < end ? [[finished - milliseconds, finished]] : [];
  });
}

function segment(events, records, boundary, previous, finalTime) {
  const start = boundary.start ? Date.parse(boundary.start) : -Infinity;
  const end = boundary.end ? Date.parse(boundary.end) : finalTime;
  const tasks = taskRows(events, start, end);
  const tools = toolIntervals(events, start, end);
  const finish = snapshotBefore(records, end);
  const begin = previous ?? snapshotBefore(records, start);
  return {
    name: boundary.name,
    kind: boundary.kind ?? "segment",
    cost: finish ? usageCost(finish) - (begin ? usageCost(begin) : 0) : null,
    durationMs: tasks.reduce((total, task) => total + task.duration_ms, 0),
    apiTtftMs: tasks.reduce((total, task) => total + (task.time_to_first_token_ms ?? 0), 0),
    toolWallMs: unionDuration(tools),
    toolCoreMs: tools.reduce((total, [startTime, endTime]) => total + endTime - startTime, 0),
  };
}

function chart(segments) {
  const maximum = Math.max(...segments.map((segment) => segment.cost ?? 0), 1);
  return segments.map((segment) => `<div class="row"><span>${escapeHtml(segment.name)}</span><div class="bar"><i style="width:${(segment.cost ?? 0) / maximum * 100}%"></i></div><b>${segment.cost === null ? "—" : `$${segment.cost.toFixed(2)}`}</b></div>`).join("\n");
}

function escapeHtml(value) {
  return value.replaceAll("&", "&amp;").replaceAll("<", "&lt;").replaceAll(">", "&gt;");
}

function html(report) {
  const rows = report.segments.map((segment) => `<tr><td>${escapeHtml(segment.name)}</td><td>${segment.kind}</td><td>${segment.cost === null ? "—" : `$${segment.cost.toFixed(2)}`}</td><td>${duration(segment.durationMs)}</td><td>${duration(segment.toolWallMs)}</td><td>${duration(segment.toolCoreMs)}</td><td>≥${duration(segment.apiTtftMs)}</td></tr>`).join("\n");
  return `<!doctype html><meta charset="utf-8"><title>Codex usage report</title><style>body{font:14px system-ui;margin:40px;max-width:900px;color:#202124}table{border-collapse:collapse;width:100%}th,td{text-align:left;padding:8px;border-bottom:1px solid #ddd}.row{display:grid;grid-template-columns:220px 1fr 90px;gap:12px;align-items:center;margin:8px 0}.bar{background:#edf0f2;height:14px;border-radius:8px;overflow:hidden}.bar i{display:block;height:100%;background:#3874cb}</style><h1>Codex usage report</h1><p>API-equivalent rate: Astra standard ($10/M uncached input, $1/M cached input, $50/M output). Tool core time counts parallel calls separately; API TTFT is a lower bound.</p><h2>Cost</h2>${chart(report.segments)}<h2>Segments</h2><table><thead><tr><th>Segment</th><th>Type</th><th>Cost</th><th>Duration</th><th>Tool wall</th><th>Tool core</th><th>Recorded API</th></tr></thead><tbody>${rows}</tbody></table>`;
}

function usage() {
  console.error("Usage: node tools/codex_usage_report.mjs report.json [output.html]");
  process.exit(1);
}

const [configPath, outputPath] = process.argv.slice(2);
if (!configPath) usage();
const config = JSON.parse(await readFile(resolve(configPath), "utf8"));
const codexHome = config.codexHome ?? join(homedir(), ".codex");
const paths = config.sources ?? (await Promise.all(["sessions", "archived_sessions"].map((name) => filesBelow(join(codexHome, name))))).flat();
const events = await loadEvents(paths);
const selected = events.filter((event) => event.type !== "token_usage_record" || config.threadIds.includes(event.payload.thread_id));
const records = selected.filter((event) => event.type === "token_usage_record").sort((a, b) => Date.parse(a.timestamp) - Date.parse(b.timestamp));
if (!records.length) throw new Error("No token records matched config.threadIds.");
const finalTime = Date.parse(records.at(-1).timestamp);
const segments = config.boundaries.map((boundary, index) => segment(selected, records, boundary, index ? snapshotBefore(records, Date.parse(config.boundaries[index - 1].end ?? new Date(finalTime).toISOString())) : undefined, finalTime));
const report = { threadIds: config.threadIds, total: usageCost(records.at(-1).payload.thread_token_usage), segments };
if (outputPath) {
  await mkdir(dirname(resolve(outputPath)), { recursive: true });
  await writeFile(resolve(outputPath), html(report));
} else console.log(JSON.stringify(report, null, 2));
