# media-office

Aleph plugin for extracting text, tables, and metadata from Microsoft Office documents.

## Supported Formats

- **DOCX** — Word documents (paragraphs, tables, metadata)
- **XLSX** — Excel spreadsheets (sheets, cells, shared strings)
- **PPTX** — PowerPoint presentations (slides, text frames)

## How It Works

Office Open XML files (DOCX/XLSX/PPTX) are ZIP archives containing XML. This plugin uses `adm-zip` to read the archive and regex-based parsing to extract content from the XML — no heavy Office dependencies required.

## Tools

| Tool | Description |
|------|-------------|
| `office_extract_text` | Extract all text from a document |
| `office_extract_tables` | Extract tables (XLSX/DOCX only) |
| `office_metadata` | Get document metadata |
| `office_convert` | Format conversion (stub) |

## Dependencies

- `adm-zip` — ZIP archive reading
