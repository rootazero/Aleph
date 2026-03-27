// DOCX parser — extracts text and metadata from .docx files.
//
// DOCX is a ZIP archive containing XML files:
//   - word/document.xml  — main document body with <w:t> text elements
//   - docProps/core.xml  — metadata (title, author, created, modified)

const AdmZip = require("adm-zip");

/**
 * Extract all <w:t> text runs from a DOCX document.xml string.
 * Groups text by paragraph (<w:p>) boundaries.
 */
function extractParagraphs(xml) {
  const paragraphs = [];
  // Split on paragraph tags
  const pParts = xml.split(/<w:p[\s>]/);

  for (let i = 1; i < pParts.length; i++) {
    const pBlock = pParts[i].split("</w:p>")[0] || "";
    const texts = [];
    const textMatches = pBlock.matchAll(/<w:t(?:\s[^>]*)?>([^<]*)<\/w:t>/g);
    for (const m of textMatches) {
      texts.push(m[1]);
    }
    const line = texts.join("");
    if (line.length > 0) {
      paragraphs.push(line);
    }
  }

  return paragraphs;
}

/**
 * Extract tables from document.xml.
 * Returns an array of tables, each table is an array of rows,
 * each row is an array of cell strings.
 */
function extractTables(xml) {
  const tables = [];
  const tblParts = xml.split(/<w:tbl[\s>]/);

  for (let i = 1; i < tblParts.length; i++) {
    const tblBlock = tblParts[i].split("</w:tbl>")[0] || "";
    const rows = [];
    const trParts = tblBlock.split(/<w:tr[\s>]/);

    for (let j = 1; j < trParts.length; j++) {
      const trBlock = trParts[j].split("</w:tr>")[0] || "";
      const cells = [];
      const tcParts = trBlock.split(/<w:tc[\s>]/);

      for (let k = 1; k < tcParts.length; k++) {
        const tcBlock = tcParts[k].split("</w:tc>")[0] || "";
        const texts = [];
        const textMatches = tcBlock.matchAll(/<w:t(?:\s[^>]*)?>([^<]*)<\/w:t>/g);
        for (const m of textMatches) {
          texts.push(m[1]);
        }
        cells.push(texts.join(""));
      }

      if (cells.length > 0) {
        rows.push(cells);
      }
    }

    if (rows.length > 0) {
      tables.push(rows);
    }
  }

  return tables;
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
    description: get("description"),
    revision: get("revision"),
  };
}

/**
 * Parse a DOCX file and return extracted content.
 *
 * @param {string} filePath - Absolute path to the .docx file
 * @returns {{ paragraphs: string[], tables: string[][][], metadata: object }}
 */
function parse(filePath) {
  const zip = new AdmZip(filePath);

  // Extract main document text
  const docEntry = zip.getEntry("word/document.xml");
  if (!docEntry) {
    throw new Error("Invalid DOCX: missing word/document.xml");
  }
  const docXml = docEntry.getData().toString("utf-8");
  const paragraphs = extractParagraphs(docXml);
  const tables = extractTables(docXml);

  // Extract metadata
  const coreEntry = zip.getEntry("docProps/core.xml");
  const coreXml = coreEntry ? coreEntry.getData().toString("utf-8") : null;
  const metadata = parseMetadata(coreXml);

  return { paragraphs, tables, metadata };
}

module.exports = { parse };
