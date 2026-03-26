# Typst Templates

## Resume
```typst
#set page(paper: "a4", margin: (x: 1.5cm, y: 2cm))
#set text(font: "New Computer Modern", size: 10pt)

#align(center)[
  #text(size: 20pt, weight: "bold")[Your Name]
  #linebreak()
  email\@example.com | github.com/you | +1-234-567-8900
]

#line(length: 100%)

== Experience

*Senior Engineer* | Company Name #h(1fr) 2024 -- Present
- Led team of 5 engineers on core infrastructure
- Reduced build times by 60% through caching strategy

== Education

*M.S. Computer Science* | University Name #h(1fr) 2020 -- 2022
```

## Paper
```typst
#set page(paper: "a4", margin: 2.5cm)
#set text(font: "New Computer Modern", size: 11pt)
#set par(justify: true)
#set heading(numbering: "1.1")

#align(center)[
  #text(size: 16pt, weight: "bold")[Paper Title]
  #linebreak()
  #text(size: 12pt)[Author Name]
  #linebreak()
  #text(size: 10pt, style: "italic")[Institution]
]

#set text(size: 10pt)
*Abstract.* #lorem(80)

= Introduction
#lorem(120)

= Related Work
#lorem(100)

= Methodology
#lorem(150)

= Results
#lorem(100)

= Conclusion
#lorem(60)
```

## LaTeX Quick Reference (for existing projects)

```bash
# Compile LaTeX locally (requires texlive)
# brew install --cask mactex  # WARNING: ~4GB download
pdflatex document.tex
bibtex document
pdflatex document.tex
pdflatex document.tex  # yes, run twice for references
```

### LaTeX Common Gotchas

| Issue | Fix |
|-------|-----|
| Special chars: `# $ % & _ { } ~ ^` | Escape with backslash: `\#`, `\$`, etc. |
| Quotes: "wrong" | Use `` `single' `` or ` ``double'' ` |
| Missing package | `\usepackage{packagename}` in preamble |
| Float placement | Use `[htbp]` or `\usepackage{float}` with `[H]` |
| Table too wide | Use `tabularx` with `X` column type |
| Encoding error | Add `\usepackage[utf8]{inputenc}` |
