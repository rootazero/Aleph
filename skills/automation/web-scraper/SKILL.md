---
name: web-scraper
description: Web data extraction — structured scraping, search, PDF download via curl and Python
emoji: "🕷️"
category: automation
allowed-tools:
  - Bash
requirements:
  binaries:
    - curl
    - python3
  platforms:
    - macos
    - linux
  install:
    - manager: pip
      package: beautifulsoup4
    - manager: pip
      package: requests
triggers:
  - scrape
  - extract data
  - crawl
  - parse HTML
  - download page
---

# Web Data Extraction

## When to Use

Invoke this skill when you need to extract structured data from web pages, download files, or perform web searches without API keys. Use this for data extraction tasks; for testing and interaction, use `playwright` (A2) instead.

## Decision Guide

| Need | Tool |
|------|------|
| Quick text from a page | `WebFetch` (Aleph built-in) |
| JS-rendered content | `playwright` (A2) |
| Structured data extraction | **This skill** |
| Bulk downloads | **This skill** |
| Search without API key | **This skill** |

## Prerequisites

```bash
# Core (optional — curl alone works for basic scraping)
pip install requests beautifulsoup4

# PDF extraction (optional)
pip install pdfplumber

# Verify
python3 -c "from bs4 import BeautifulSoup; print('BS4 OK')"
```

## Core Operations

### Extract Structured Data from HTML

```python
#!/usr/bin/env python3
"""Extract structured data from a web page."""
import requests
from bs4 import BeautifulSoup
import json, sys

url = sys.argv[1] if len(sys.argv) > 1 else "https://example.com"
headers = {"User-Agent": "Mozilla/5.0 (Macintosh; Intel Mac OS X 10_15_7) AppleWebKit/537.36"}
resp = requests.get(url, headers=headers, timeout=30)
soup = BeautifulSoup(resp.text, "html.parser")

# Extract all links
links = [{"text": a.get_text(strip=True), "href": a.get("href")}
         for a in soup.select("a[href]") if a.get_text(strip=True)]

# Extract tables
tables = []
for table in soup.select("table"):
    rows = []
    for tr in table.select("tr"):
        cells = [td.get_text(strip=True) for td in tr.select("td,th")]
        if cells:
            rows.append(cells)
    if rows:
        tables.append(rows)

print(json.dumps({"links": links[:20], "tables": tables}, indent=2, ensure_ascii=False))
```

Save as `tmp/scrape.py` and run: `python3 tmp/scrape.py "https://target-url"`

### Curl-Only Scraping (No Python)

```bash
# Download page and extract with grep/sed
curl -s "https://example.com" | grep -oP 'href="\K[^"]+' | head -20

# Extract title
curl -s "https://example.com" | grep -oP '<title>\K[^<]+'

# Download and convert to text (via lynx or w3m if available)
curl -s "https://example.com" | python3 -c "
import sys
from html.parser import HTMLParser
class T(HTMLParser):
    def __init__(self): super().__init__(); self.text = []
    def handle_data(self, d): self.text.append(d.strip())
t = T(); t.feed(sys.stdin.read()); print(' '.join(filter(None, t.text)))
"
```

### Search Without API Keys

```bash
# DuckDuckGo HTML search
curl -s "https://html.duckduckgo.com/html/?q=python+web+scraping" \
  -H "User-Agent: Mozilla/5.0" | python3 -c "
from bs4 import BeautifulSoup
import sys, json
soup = BeautifulSoup(sys.stdin.read(), 'html.parser')
results = [{'title': r.get_text(strip=True), 'url': r.get('href', '')}
           for r in soup.select('.result__a')]
print(json.dumps(results[:10], indent=2))
"
```

### Download Files

```bash
# Download with auto-filename
curl -sLOJ "https://example.com/report.pdf"

# Download to specific path
curl -sL "https://example.com/data.csv" -o downloads/data.csv

# Download multiple URLs
cat urls.txt | xargs -I{} curl -sLOJ "{}"
```

### PDF Text Extraction

```python
#!/usr/bin/env python3
"""Download a PDF and extract text."""
import requests, sys
try:
    import pdfplumber
except ImportError:
    print("Install: pip install pdfplumber"); sys.exit(1)

url = sys.argv[1]
resp = requests.get(url, timeout=30)
with open("/tmp/doc.pdf", "wb") as f:
    f.write(resp.content)

with pdfplumber.open("/tmp/doc.pdf") as pdf:
    for i, page in enumerate(pdf.pages):
        text = page.extract_text()
        if text:
            print(f"--- Page {i+1} ---")
            print(text)
```

## Best Practices

- **Respect robots.txt**: Check `curl -s https://example.com/robots.txt` before scraping
- **Rate limiting**: Add `time.sleep(1)` between requests to avoid being blocked
- **User-Agent**: Always set a realistic User-Agent header
- **Error handling**: Check HTTP status codes, handle timeouts and connection errors
- **Output as JSON**: Default to JSON output for downstream processing with `data-pipeline` (A5)

## Gotchas

- **JS-rendered content**: curl/requests only get raw HTML. Use `playwright` (A2) for SPAs
- **Anti-bot**: Some sites block automated requests. Rotate User-Agents, add delays
- **Encoding**: Check `resp.encoding` and force UTF-8 if needed: `resp.encoding = 'utf-8'`
- **Relative URLs**: Use `urllib.parse.urljoin(base_url, relative_url)` to resolve

## Aleph Integration

- Synergy with `data-pipeline` (A5): scrape → JSON → jq/python processing
- Synergy with `search` (F7): `search` finds information, `web-scraper` extracts data
- Synergy with `playwright` (A2): curl for static, Playwright for JS-rendered
