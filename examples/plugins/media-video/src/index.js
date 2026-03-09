// media-video — Aleph Example Plugin (Node.js)
//
// Demonstrates:
//   1. JSON-RPC 2.0 stdio communication with the Aleph host
//   2. Tool handler (video_extract_frames)
//   3. Hook handler (PreToolUse interceptor for media_understand)
//
// This is a stub implementation. A real version would shell out to ffmpeg.

const readline = require("readline");

// ---------------------------------------------------------------------------
// Tool handlers
// ---------------------------------------------------------------------------

/**
 * video_extract_frames — extract keyframes from a video file.
 *
 * Stub: returns a placeholder result explaining that ffmpeg is required
 * for real extraction.
 */
function videoExtractFrames(params) {
  const filePath = params.file_path;
  const count = params.count ?? 10;
  const format = params.format ?? "png";

  if (!filePath) {
    return {
      error: "file_path is required",
    };
  }

  // In a real implementation this would:
  //   1. Verify the file exists and is a supported video format
  //   2. Run: ffmpeg -i <file_path> -vf "select=eq(pict_type\\,I)" -vsync vfr -frames:v <count> out_%03d.<format>
  //   3. Return the list of extracted frame paths

  return {
    status: "stub",
    message: `Would extract ${count} keyframe(s) from "${filePath}" as ${format} using ffmpeg`,
    frames: Array.from({ length: count }, (_, i) => ({
      index: i,
      path: `/tmp/media-video/frame_${String(i).padStart(3, "0")}.${format}`,
      timestamp_sec: null,
    })),
    note: "This is a stub. Install ffmpeg and replace this handler with a real implementation.",
  };
}

// ---------------------------------------------------------------------------
// Hook handlers
// ---------------------------------------------------------------------------

/**
 * PreToolUse hook for "media_understand" tool.
 *
 * When the AI is about to call media_understand, this hook enriches the
 * tool input with video metadata (codec, duration, resolution) so the
 * media_understand tool has richer context.
 */
function onPreToolUse(params) {
  const toolName = params.tool_name || "";
  const toolInput = params.tool_input || {};

  // Only act on media_understand calls that reference a video file
  const filePath = toolInput.file_path || toolInput.path || "";
  const videoExtensions = [".mp4", ".mov", ".avi", ".mkv", ".webm", ".flv"];
  const isVideo = videoExtensions.some((ext) =>
    filePath.toLowerCase().endsWith(ext),
  );

  if (!isVideo) {
    // Not a video file — pass through without modification
    return { action: "continue" };
  }

  // In a real implementation this would run:
  //   ffprobe -v quiet -print_format json -show_format -show_streams <file_path>
  // and parse the output to get codec, duration, resolution, etc.

  return {
    action: "continue",
    modified_input: {
      ...toolInput,
      _video_metadata: {
        source: "media-video-plugin",
        note: "Stub metadata — install ffprobe for real values",
        codec: "unknown",
        duration_sec: null,
        resolution: null,
        fps: null,
      },
    },
  };
}

// ---------------------------------------------------------------------------
// JSON-RPC 2.0 stdio listener
// ---------------------------------------------------------------------------

const HANDLER_MAP = {
  videoExtractFrames,
  onPreToolUse,
};

const rl = readline.createInterface({
  input: process.stdin,
  output: process.stdout,
  terminal: false,
});

rl.on("line", (line) => {
  let request;
  try {
    request = JSON.parse(line);
  } catch {
    writeResponse({
      jsonrpc: "2.0",
      id: null,
      error: { code: -32700, message: "Parse error" },
    });
    return;
  }

  const { id, method, params } = request;

  // Route to the correct handler
  if (method === "plugin.call") {
    const handlerName = params && params.handler;
    const handler = HANDLER_MAP[handlerName];

    if (!handler) {
      writeResponse({
        jsonrpc: "2.0",
        id,
        error: {
          code: -32601,
          message: `Unknown handler: ${handlerName}`,
        },
      });
      return;
    }

    try {
      const result = handler(params.arguments || {});
      writeResponse({ jsonrpc: "2.0", id, result });
    } catch (err) {
      writeResponse({
        jsonrpc: "2.0",
        id,
        error: {
          code: -32000,
          message: err.message || "Internal handler error",
        },
      });
    }
  } else if (method === "ping") {
    writeResponse({ jsonrpc: "2.0", id, result: "pong" });
  } else {
    writeResponse({
      jsonrpc: "2.0",
      id,
      error: { code: -32601, message: `Method not found: ${method}` },
    });
  }
});

function writeResponse(response) {
  process.stdout.write(JSON.stringify(response) + "\n");
}

// Signal readiness
writeResponse({
  jsonrpc: "2.0",
  id: "init",
  result: { status: "ready", plugin_id: "media-video" },
});
