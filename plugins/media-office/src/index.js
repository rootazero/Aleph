// media-office — Aleph Plugin (Node.js)
//
// Extracts text, tables, and metadata from Office documents (DOCX, XLSX, PPTX).
// Uses adm-zip to read the ZIP archives and regex to parse the internal XML.

const readline = require("readline");
const path = require("path");
const fs = require("fs");

const docxParser = require("./parsers/docx");
const xlsxParser = require("./parsers/xlsx");
const pptxParser = require("./parsers/pptx");

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

function detectType(filePath) {
  const ext = path.extname(filePath).toLowerCase();
  switch (ext) {
    case ".docx":
      return "docx";
    case ".xlsx":
      return "xlsx";
    case ".pptx":
      return "pptx";
    default:
      return null;
  }
}

function requireFile(filePath) {
  if (!filePath) {
    throw new Error("file_path is required");
  }
  if (!fs.existsSync(filePath)) {
    throw new Error(`File not found: ${filePath}`);
  }
}

// ---------------------------------------------------------------------------
// Formatters
// ---------------------------------------------------------------------------

/**
 * Format table rows as a Markdown table.
 */
function tablesToMarkdown(rows) {
  if (rows.length === 0) return "";

  const header = rows[0];
  const divider = header.map(() => "---");
  const lines = [
    "| " + header.join(" | ") + " |",
    "| " + divider.join(" | ") + " |",
  ];
  for (let i = 1; i < rows.length; i++) {
    // Pad row to match header length
    const row = rows[i];
    while (row.length < header.length) row.push("");
    lines.push("| " + row.join(" | ") + " |");
  }
  return lines.join("\n");
}

/**
 * Format table rows as CSV.
 */
function tablesToCsv(rows) {
  return rows
    .map((row) =>
      row
        .map((cell) => {
          const s = String(cell);
          if (s.includes(",") || s.includes('"') || s.includes("\n")) {
            return '"' + s.replace(/"/g, '""') + '"';
          }
          return s;
        })
        .join(",")
    )
    .join("\n");
}

// ---------------------------------------------------------------------------
// Tool handlers
// ---------------------------------------------------------------------------

function officeExtractText(params) {
  const filePath = params.file_path;
  const format = params.format || "markdown";
  requireFile(filePath);

  const type = detectType(filePath);
  if (!type) {
    throw new Error(
      `Unsupported file type: ${path.extname(filePath)}. Supported: .docx, .xlsx, .pptx`
    );
  }

  let text = "";

  if (type === "docx") {
    const result = docxParser.parse(filePath);
    if (format === "markdown") {
      text = result.paragraphs.join("\n\n");
    } else {
      text = result.paragraphs.join("\n");
    }
  } else if (type === "xlsx") {
    const result = xlsxParser.parse(filePath);
    const parts = [];
    for (const sheet of result.sheets) {
      if (format === "markdown") {
        parts.push(`## ${sheet.name}\n\n${tablesToMarkdown(sheet.rows)}`);
      } else {
        parts.push(
          `[${sheet.name}]\n${sheet.rows.map((r) => r.join("\t")).join("\n")}`
        );
      }
    }
    text = parts.join("\n\n");
  } else if (type === "pptx") {
    const result = pptxParser.parse(filePath);
    const parts = [];
    for (const slide of result.slides) {
      if (format === "markdown") {
        parts.push(`## Slide ${slide.number}\n\n${slide.text}`);
      } else {
        parts.push(`[Slide ${slide.number}]\n${slide.text}`);
      }
    }
    text = parts.join("\n\n");
  }

  return { text, format, file_type: type };
}

function officeExtractTables(params) {
  const filePath = params.file_path;
  const format = params.format || "markdown";
  const sheet = params.sheet;
  requireFile(filePath);

  const type = detectType(filePath);

  if (type === "xlsx") {
    const result = xlsxParser.parse(filePath, sheet);
    const tables = result.sheets.map((s) => {
      let formatted;
      if (format === "csv") {
        formatted = tablesToCsv(s.rows);
      } else if (format === "json") {
        // Use first row as headers
        if (s.rows.length < 2) {
          formatted = s.rows;
        } else {
          const headers = s.rows[0];
          formatted = s.rows.slice(1).map((row) => {
            const obj = {};
            headers.forEach((h, idx) => {
              obj[h || `col_${idx}`] = row[idx] || "";
            });
            return obj;
          });
        }
      } else {
        formatted = tablesToMarkdown(s.rows);
      }
      return { sheet_name: s.name, data: formatted };
    });
    return { tables, format };
  } else if (type === "docx") {
    const result = docxParser.parse(filePath);
    const tables = result.tables.map((rows, idx) => {
      let formatted;
      if (format === "csv") {
        formatted = tablesToCsv(rows);
      } else if (format === "json") {
        if (rows.length < 2) {
          formatted = rows;
        } else {
          const headers = rows[0];
          formatted = rows.slice(1).map((row) => {
            const obj = {};
            headers.forEach((h, i) => {
              obj[h || `col_${i}`] = row[i] || "";
            });
            return obj;
          });
        }
      } else {
        formatted = tablesToMarkdown(rows);
      }
      return { table_index: idx, data: formatted };
    });
    return { tables, format };
  } else {
    throw new Error(
      "office_extract_tables only supports .docx and .xlsx files"
    );
  }
}

function officeMetadata(params) {
  const filePath = params.file_path;
  requireFile(filePath);

  const type = detectType(filePath);
  if (!type) {
    throw new Error(
      `Unsupported file type: ${path.extname(filePath)}. Supported: .docx, .xlsx, .pptx`
    );
  }

  let metadata = {};
  let extra = {};

  if (type === "docx") {
    const result = docxParser.parse(filePath);
    metadata = result.metadata;
    extra = {
      paragraph_count: result.paragraphs.length,
      table_count: result.tables.length,
    };
  } else if (type === "xlsx") {
    const result = xlsxParser.parse(filePath);
    metadata = result.metadata;
    extra = {
      sheet_count: result.sheets.length,
      sheet_names: result.sheets.map((s) => s.name),
    };
  } else if (type === "pptx") {
    const result = pptxParser.parse(filePath);
    metadata = result.metadata;
    extra = { slide_count: result.slides.length };
  }

  return { file_type: type, metadata, ...extra };
}

function officeConvert(params) {
  const filePath = params.file_path;
  const outputFormat = params.output_format;
  requireFile(filePath);

  return {
    status: "stub",
    message: `Conversion from ${path.extname(filePath)} to .${outputFormat} is not yet implemented.`,
    note: "This tool is a placeholder. A future version may integrate LibreOffice or similar.",
  };
}

// ---------------------------------------------------------------------------
// Hook handler
// ---------------------------------------------------------------------------

/**
 * PostToolUse observer for "document_extract" tool.
 *
 * Enriches the tool result with office-specific metadata when the file
 * is an Office document.
 */
function onPostToolUse(params) {
  const toolOutput = params.tool_output || {};
  const toolInput = params.tool_input || {};

  const filePath = toolInput.file_path || toolInput.path || "";
  const type = detectType(filePath);

  if (!type || !fs.existsSync(filePath)) {
    return { action: "continue" };
  }

  // Attach office metadata to the result
  let officeMeta = {};
  try {
    if (type === "docx") {
      const r = docxParser.parse(filePath);
      officeMeta = {
        ...r.metadata,
        paragraph_count: r.paragraphs.length,
        table_count: r.tables.length,
      };
    } else if (type === "xlsx") {
      const r = xlsxParser.parse(filePath);
      officeMeta = {
        ...r.metadata,
        sheet_count: r.sheets.length,
        sheet_names: r.sheets.map((s) => s.name),
      };
    } else if (type === "pptx") {
      const r = pptxParser.parse(filePath);
      officeMeta = {
        ...r.metadata,
        slide_count: r.slides.length,
      };
    }
  } catch {
    // Parsing failed — don't break the pipeline
    return { action: "continue" };
  }

  return {
    action: "continue",
    annotations: {
      _office_metadata: {
        source: "media-office-plugin",
        file_type: type,
        ...officeMeta,
      },
    },
  };
}

// ---------------------------------------------------------------------------
// JSON-RPC 2.0 stdio listener
// ---------------------------------------------------------------------------

const HANDLER_MAP = {
  officeExtractText,
  officeExtractTables,
  officeMetadata,
  officeConvert,
  onPostToolUse,
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
  result: { status: "ready", plugin_id: "media-office" },
});
