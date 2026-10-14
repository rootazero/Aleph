// system.js — Collect OS-level health metrics
//
// Gathers CPU load averages, memory usage, disk space, and system uptime.

const os = require("os");
const { execSync } = require("child_process");

/**
 * Collect system health metrics.
 *
 * @returns {{ cpu: object, memory: object, disk: object, uptime: object }}
 */
function collectSystemHealth() {
  const loadAvg = os.loadavg();
  const totalMem = os.totalmem();
  const freeMem = os.freemem();
  const usedMem = totalMem - freeMem;
  const uptimeSec = os.uptime();

  const cpu = {
    load_avg_1m: loadAvg[0],
    load_avg_5m: loadAvg[1],
    load_avg_15m: loadAvg[2],
    cores: os.cpus().length,
  };

  const memory = {
    total_bytes: totalMem,
    used_bytes: usedMem,
    free_bytes: freeMem,
    usage_percent: Number(((usedMem / totalMem) * 100).toFixed(1)),
  };

  const disk = collectDiskUsage();

  const uptime = {
    seconds: uptimeSec,
    human: formatUptime(uptimeSec),
  };

  return { cpu, memory, disk, uptime };
}

/**
 * Parse `df -h /` output to get root disk usage.
 */
function collectDiskUsage() {
  try {
    const output = execSync("df -h /", { encoding: "utf-8", timeout: 5000 });
    const lines = output.trim().split("\n");
    if (lines.length < 2) {
      return { error: "Unexpected df output" };
    }
    const parts = lines[1].split(/\s+/);
    return {
      filesystem: parts[0],
      size: parts[1],
      used: parts[2],
      available: parts[3],
      usage_percent: parts[4],
      mount: parts[5],
    };
  } catch (err) {
    return { error: err.message };
  }
}

/**
 * Format seconds into a human-readable uptime string.
 */
function formatUptime(seconds) {
  const days = Math.floor(seconds / 86400);
  const hours = Math.floor((seconds % 86400) / 3600);
  const minutes = Math.floor((seconds % 3600) / 60);
  const parts = [];
  if (days > 0) parts.push(`${days}d`);
  if (hours > 0) parts.push(`${hours}h`);
  parts.push(`${minutes}m`);
  return parts.join(" ");
}

module.exports = { collectSystemHealth };
