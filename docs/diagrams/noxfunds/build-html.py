#!/usr/bin/env python3
"""Build flowcharts.html, every diagram on one page, from the Markdown files beside it.

    python3 docs/diagrams/noxfunds/build-html.py

The .md files are the source; the page embeds them verbatim and renders them in the browser
(marked for the text, Mermaid for the diagrams, both pinned, from jsDelivr). Re-run after
editing any .md, so the page never disagrees with the files GitHub shows.
"""

import json
from pathlib import Path

HERE = Path(__file__).resolve().parent
OUT = HERE / "flowcharts.html"
FILES = ["README.md"] + sorted(p.name for p in HERE.glob("[0-9][0-9]-*.md"))

docs = [{"id": Path(f).stem, "md": (HERE / f).read_text(encoding="utf-8")} for f in FILES]
# `</` would end the <script> element early; `<\/` is the same string to JavaScript.
payload = json.dumps(docs, ensure_ascii=False).replace("</", "<\\/")

PAGE = """<!doctype html>
<html lang="en">
<head>
<meta charset="utf-8">
<meta name="viewport" content="width=device-width, initial-scale=1">
<title>NOXFUNDS Flowcharts</title>
<style>
  :root { --bg: #ffffff; --fg: #1d2330; --muted: #5b6475; --line: #dde1e8; --soft: #f4f6f9; --accent: #3b5bdb; }
  @media (prefers-color-scheme: dark) {
    :root:not([data-theme="light"]) { --bg: #12151b; --fg: #e4e7ee; --muted: #9aa3b5; --line: #2b313c; --soft: #1a1f27; --accent: #8ea4ff; }
  }
  :root[data-theme="dark"] { --bg: #12151b; --fg: #e4e7ee; --muted: #9aa3b5; --line: #2b313c; --soft: #1a1f27; --accent: #8ea4ff; }
  * { box-sizing: border-box; }
  body { margin: 0; background: var(--bg); color: var(--fg); font: 16px/1.6 system-ui, -apple-system, "Segoe UI", sans-serif; }
  nav { position: sticky; top: 0; background: var(--bg); border-bottom: 1px solid var(--line); padding: 10px 16px; display: flex; gap: 14px; flex-wrap: wrap; font-size: 14px; z-index: 1; }
  nav a { color: var(--muted); text-decoration: none; }
  nav a:hover { color: var(--accent); }
  main { max-width: 1100px; margin: 0 auto; padding: 8px 16px 64px; }
  section { border-bottom: 1px solid var(--line); padding: 24px 0; }
  a { color: var(--accent); }
  h1 { font-size: 1.6rem; } h2 { font-size: 1.25rem; margin-top: 2rem; }
  table { border-collapse: collapse; display: block; overflow-x: auto; font-size: 14px; margin: 12px 0; }
  th, td { border: 1px solid var(--line); padding: 6px 10px; text-align: left; vertical-align: top; }
  th { background: var(--soft); }
  code { background: var(--soft); padding: 1px 5px; border-radius: 4px; font-size: 0.9em; }
  pre:not(.mermaid) { background: var(--soft); padding: 12px; border-radius: 6px; overflow-x: auto; font-size: 13px; }
  pre:not(.mermaid) code { background: none; padding: 0; }
  pre.mermaid { background: var(--soft); border: 1px solid var(--line); border-radius: 8px; padding: 12px; overflow-x: auto; text-align: center; }
</style>
</head>
<body>
<nav id="nav"></nav>
<main id="main"><p>Loading the diagrams (needs an internet connection the first time)...</p></main>
<script id="docs" type="application/json">__PAYLOAD__</script>
<script src="https://cdn.jsdelivr.net/npm/marked@15.0.12/marked.min.js"></script>
<script type="module">
  import mermaid from "https://cdn.jsdelivr.net/npm/mermaid@11.17.2/dist/mermaid.esm.min.mjs";
  const docs = JSON.parse(document.getElementById("docs").textContent);
  const dark = matchMedia("(prefers-color-scheme: dark)").matches;
  const esc = (s) => s.replace(/&/g, "&amp;").replace(/</g, "&lt;").replace(/>/g, "&gt;");
  marked.use({
    renderer: {
      code({ text, lang }) {
        return lang === "mermaid"
          ? `<pre class="mermaid">${esc(text)}</pre>`
          : `<pre><code>${esc(text)}</code></pre>`;
      },
      link({ href, tokens }) {
        const inner = this.parser.parseInline(tokens);
        const local = /^(README|\\d\\d-[\\w-]+)\\.md$/.exec(href);
        return `<a href="${local ? "#" + local[1] : href}">${inner}</a>`;
      },
    },
  });
  document.getElementById("main").innerHTML = docs
    .map((d) => `<section id="${d.id}">${marked.parse(d.md)}</section>`)
    .join("");
  document.getElementById("nav").innerHTML = docs
    .map((d) => `<a href="#${d.id}">${d.id === "README" ? "Overview" : d.id}</a>`)
    .join("");
  mermaid.initialize({ startOnLoad: false, theme: dark ? "dark" : "default", securityLevel: "strict" });
  await mermaid.run({ querySelector: "pre.mermaid" });
</script>
</body>
</html>
"""

OUT.write_text(PAGE.replace("__PAYLOAD__", payload), encoding="utf-8")
print(f"wrote {OUT.relative_to(HERE.parents[2])}: {len(docs)} files")
