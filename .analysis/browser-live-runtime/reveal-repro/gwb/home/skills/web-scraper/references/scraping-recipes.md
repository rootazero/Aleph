# Scraping Recipes

## Extract Structured Data from HTML

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

## Curl-Only Scraping (No Python)

```bash
# Extract links
curl -s "https://example.com" | grep -oP 'href="\K[^"]+' | head -20

# Extract title
curl -s "https://example.com" | grep -oP '<title>\K[^<]+'

# HTML to text
curl -s "https://example.com" | python3 -c "
import sys
from html.parser import HTMLParser
class T(HTMLParser):
    def __init__(self): super().__init__(); self.text = []
    def handle_data(self, d): self.text.append(d.strip())
t = T(); t.feed(sys.stdin.read()); print(' '.join(filter(None, t.text)))
"
```

## Search Without API Keys

```bash
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

## Download Files

```bash
curl -sLOJ "https://example.com/report.pdf"              # auto-filename
curl -sL "https://example.com/data.csv" -o downloads/data.csv  # specific path
cat urls.txt | xargs -I{} curl -sLOJ "{}"                # multiple URLs
```

## PDF Text Extraction

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
