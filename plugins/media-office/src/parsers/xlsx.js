// XLSX parser — extracts sheet data and metadata from .xlsx files.
//
// XLSX is a ZIP archive containing XML files:
//   - xl/sharedStrings.xml   — string lookup table
//   - xl/workbook.xml        — sheet names
//   - xl/worksheets/sheet*.xml — cell data
//   - docProps/core.xml      — metadata

const AdmZip = require("adm-zip");

/**
 * Parse the shared strings table from xl/sharedStrings.xml.
 * Returns an array of strings indexed by their position.
 */
function parseSharedStrings(xml) {
  if (!xml) return [];

  const strings = [];
  const siParts = xml.split(/<si[\s>]/);

  for (let i = 1; i < siParts.length; i++) {
    const siBlock = siParts[i].split("</si>")[0] || "";
    const texts = [];
    const tMatches = siBlock.matchAll(/<t(?:\s[^>]*)?>([^<]*)<\/t>/g);
    for (const m of tMatches) {
      texts.push(m[1]);
    }
    strings.push(texts.join(""));
  }

  return strings;
}

/**
 * Parse sheet names from xl/workbook.xml.
 * Returns an array of { name, sheetId } objects.
 */
function parseSheetNames(xml) {
  if (!xml) return [];

  const sheets = [];
  const sheetMatches = xml.matchAll(/<sheet\s+[^>]*name="([^"]*)"[^>]*sheetId="(\d+)"[^>]*\/?>/g);
  for (const m of sheetMatches) {
    sheets.push({ name: m[1], sheetId: parseInt(m[2], 10) });
  }

  return sheets;
}

/**
 * Convert a column letter (A, B, ..., Z, AA, AB, ...) to a 0-based index.
 */
function colLetterToIndex(letters) {
  let idx = 0;
  for (let i = 0; i < letters.length; i++) {
    idx = idx * 26 + (letters.charCodeAt(i) - 64);
  }
  return idx - 1;
}

/**
 * Parse a worksheet XML into rows of cell values.
 *
 * @param {string} xml - The worksheet XML content
 * @param {string[]} sharedStrings - The shared strings table
 * @returns {string[][]} Array of rows, each row an array of cell values
 */
function parseSheet(xml, sharedStrings) {
  const rows = [];
  const rowParts = xml.split(/<row[\s>]/);

  for (let i = 1; i < rowParts.length; i++) {
    const rowBlock = rowParts[i].split("</row>")[0] || "";
    const cells = new Map(); // colIndex -> value

    const cellMatches = rowBlock.matchAll(
      /<c\s+r="([A-Z]+)\d+"(?:\s+[^>]*)?(?:\s+t="([^"]*)")?[^>]*>(?:[\s\S]*?<v>([^<]*)<\/v>)?[\s\S]*?<\/c>|<c\s+r="([A-Z]+)\d+"(?:\s+[^>]*)?(?:\s+t="([^"]*)")?[^>]*\/>/g
    );

    for (const m of cellMatches) {
      const colStr = m[1] || m[4];
      const cellType = m[2] || m[5];
      const rawValue = m[3];

      if (!colStr) continue;

      const colIdx = colLetterToIndex(colStr);
      let value = "";

      if (rawValue !== undefined) {
        if (cellType === "s") {
          // Shared string reference
          const ssIdx = parseInt(rawValue, 10);
          value = sharedStrings[ssIdx] || "";
        } else {
          value = rawValue;
        }
      }

      cells.set(colIdx, value);
    }

    if (cells.size > 0) {
      const maxCol = Math.max(...cells.keys());
      const row = [];
      for (let c = 0; c <= maxCol; c++) {
        row.push(cells.get(c) || "");
      }
      rows.push(row);
    }
  }

  return rows;
}

/**
 * Parse metadata from docProps/core.xml.
 */
function parseMetadata(xml) {
  if (!xml) return {};

  const get = (tag) => {
    const m = xml.match(new RegExp(`<(?:dc:|cp:|dcterms:)?${tag}[^>]*>([^<]*)<`));
    return m ? m[1].trim() : null;
  };

  return {
    title: get("title"),
    subject: get("subject"),
    creator: get("creator"),
    last_modified_by: get("lastModifiedBy"),
    created: get("created"),
    modified: get("modified"),
  };
}

/**
 * Parse an XLSX file and return extracted content.
 *
 * @param {string} filePath - Absolute path to the .xlsx file
 * @param {string} [targetSheet] - Optional sheet name to extract (defaults to all)
 * @returns {{ sheets: Array<{name: string, rows: string[][]}>, metadata: object }}
 */
function parse(filePath, targetSheet) {
  const zip = new AdmZip(filePath);

  // Shared strings
  const ssEntry = zip.getEntry("xl/sharedStrings.xml");
  const sharedStrings = ssEntry
    ? parseSharedStrings(ssEntry.getData().toString("utf-8"))
    : [];

  // Sheet names
  const wbEntry = zip.getEntry("xl/workbook.xml");
  const wbXml = wbEntry ? wbEntry.getData().toString("utf-8") : "";
  const sheetDefs = parseSheetNames(wbXml);

  // Parse worksheets
  const sheets = [];
  for (let idx = 0; idx < sheetDefs.length; idx++) {
    const def = sheetDefs[idx];

    if (targetSheet && def.name !== targetSheet) {
      continue;
    }

    const sheetPath = `xl/worksheets/sheet${idx + 1}.xml`;
    const sheetEntry = zip.getEntry(sheetPath);
    if (!sheetEntry) continue;

    const sheetXml = sheetEntry.getData().toString("utf-8");
    const rows = parseSheet(sheetXml, sharedStrings);
    sheets.push({ name: def.name, rows });
  }

  // Metadata
  const coreEntry = zip.getEntry("docProps/core.xml");
  const coreXml = coreEntry ? coreEntry.getData().toString("utf-8") : null;
  const metadata = parseMetadata(coreXml);

  return { sheets, metadata };
}

module.exports = { parse };
