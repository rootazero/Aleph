// PPTX parser — extracts slide text and metadata from .pptx files.
//
// PPTX is a ZIP archive containing XML files:
//   - ppt/slides/slide*.xml  — slide content with <a:t> text elements
//   - docProps/core.xml      — metadata

const AdmZip = require("adm-zip");

/**
 * Extract text from all <a:t> tags in a slide XML string.
 * Groups text by text frame (<a:txBody>) boundaries.
 */
function extractSlideText(xml) {
  const blocks = [];
  const bodyParts = xml.split(/<a:txBody[\s>]/);

  for (let i = 1; i < bodyParts.length; i++) {
    const bodyBlock = bodyParts[i].split("</a:txBody>")[0] || "";
    const paragraphs = [];
    const pParts = bodyBlock.split(/<a:p[\s>]/);

    for (let j = 1; j < pParts.length; j++) {
      const pBlock = pParts[j].split("</a:p>")[0] || "";
      const texts = [];
      const tMatches = pBlock.matchAll(/<a:t>([^<]*)<\/a:t>/g);
      for (const m of tMatches) {
        texts.push(m[1]);
      }
      const line = texts.join("");
      if (line.length > 0) {
        paragraphs.push(line);
      }
    }

    if (paragraphs.length > 0) {
      blocks.push(paragraphs.join("\n"));
    }
  }

  return blocks.join("\n\n");
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
 * Parse a PPTX file and return extracted content.
 *
 * @param {string} filePath - Absolute path to the .pptx file
 * @returns {{ slides: Array<{number: number, text: string}>, metadata: object }}
 */
function parse(filePath) {
  const zip = new AdmZip(filePath);
  const entries = zip.getEntries();

  // Collect slide entries and sort by slide number
  const slideEntries = entries
    .filter((e) => /^ppt\/slides\/slide\d+\.xml$/.test(e.entryName))
    .sort((a, b) => {
      const numA = parseInt(a.entryName.match(/slide(\d+)/)[1], 10);
      const numB = parseInt(b.entryName.match(/slide(\d+)/)[1], 10);
      return numA - numB;
    });

  const slides = [];
  for (const entry of slideEntries) {
    const num = parseInt(entry.entryName.match(/slide(\d+)/)[1], 10);
    const xml = entry.getData().toString("utf-8");
    const text = extractSlideText(xml);
    slides.push({ number: num, text });
  }

  // Metadata
  const coreEntry = zip.getEntry("docProps/core.xml");
  const coreXml = coreEntry ? coreEntry.getData().toString("utf-8") : null;
  const metadata = parseMetadata(coreXml);

  return { slides, metadata };
}

module.exports = { parse };
