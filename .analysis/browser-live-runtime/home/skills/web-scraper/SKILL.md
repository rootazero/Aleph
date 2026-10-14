---
name: web-scraper
description: "Web data extraction — structured scraping, search, PDF download via curl and Python. Use when extracting structured data from web pages, downloading files, scraping HTML, parsing tables/links, performing web searches without API keys, or extracting text from PDFs. For JS-rendered pages, use playwright instead."
---

# Web Data Extraction

## Prerequisites

```bash
pip install requests beautifulsoup4    # core
pip install pdfplumber                 # optional: PDF extraction
```

## Decision Guide

| Need | Tool |
|------|------|
| Quick text from a page | `WebFetch` (built-in) |
| JS-rendered content | `playwright` (A2) |
| Structured data extraction | **This skill** |
| Bulk downloads | **This skill** |
| Search without API key | **This skill** |

## Scraping Recipes

For complete examples (HTML extraction, curl-only scraping, search, file downloads, PDF extraction), see [references/scraping-recipes.md](references/scraping-recipes.md).

## Best Practices

- **Respect robots.txt**: Check `curl -s https://example.com/robots.txt` before scraping
- **Rate limiting**: Add `time.sleep(1)` between requests
- **User-Agent**: Always set a realistic User-Agent header
- **Output as JSON**: Default to JSON for downstream processing with `data-pipeline` (A5)

## Gotchas

- **JS-rendered content**: curl/requests only get raw HTML. Use `playwright` (A2) for SPAs
- **Anti-bot**: Rotate User-Agents, add delays
- **Encoding**: Check `resp.encoding` and force UTF-8 if needed
- **Relative URLs**: Use `urllib.parse.urljoin(base_url, relative_url)` to resolve

## Aleph Integration

- Synergy with `data-pipeline` (A5): scrape -> JSON -> jq/python processing
- Synergy with `search` (F7): search finds information, web-scraper extracts data
- Synergy with `playwright` (A2): curl for static, Playwright for JS-rendered
