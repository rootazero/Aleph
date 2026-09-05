// Just enough TOML to read an `agent-detect` manifest, for `derive_chrome.mjs`.
//
// Node has no TOML in its standard library and this fixture tree has no
// package.json, so the choice was: a dependency the fixture cannot install
// offline, or a reader for the subset the manifests actually use. The subset
// is small and CLOSED — `crates/agent-detect/src/manifest.rs` defines the
// schema, and every manifest in `crates/agent-detect/src/manifests/` is
// generated against it:
//
//   key = "string" | 'literal string' | 123 | true | ["a", "b"]
//   [[rules]]                                  array of tables
//   any = [ { line_regex = ['…'] }, … ]        inline tables inside an array
//   arrays that span lines, `#` comments
//
// ⚠️ This parser is NOT a second copy of the schema and must never grow into
// one (判据 §1). It answers "what does this file say", not "is this a valid
// manifest" — the validation lives in `agent_detect::manifest::parse_manifest`
// and the fixture reaches it at runtime through `terminal{explain}`.
//
// A shape it cannot read is a THROW, never a silent `undefined`: every caller
// in `derive_chrome.mjs` is building a screen out of what it finds here, and a
// screen built from a missing literal paints chrome no rule matches — which on
// the wire is indistinguishable from detection being broken, i.e. exactly the
// failure this fixture exists to catch.

const isSpace = (ch) => ch === " " || ch === "\t" || ch === "\r" || ch === "\n";

class Reader {
  constructor(src) {
    this.s = src;
    this.i = 0;
  }

  fail(what) {
    const upto = this.s.slice(0, this.i);
    const line = upto.split("\n").length;
    throw new Error(`toml_min: ${what} at line ${line}: ${JSON.stringify(this.s.slice(this.i, this.i + 60))}`);
  }

  // Whitespace and comments. `inLine` keeps a newline significant, which is
  // what separates `key = value` pairs at the top level.
  skip(inLine = false) {
    for (;;) {
      const ch = this.s[this.i];
      if (ch === undefined) return;
      if (ch === "#") {
        while (this.i < this.s.length && this.s[this.i] !== "\n") this.i += 1;
        continue;
      }
      if (inLine ? ch === " " || ch === "\t" || ch === "\r" : isSpace(ch)) {
        this.i += 1;
        continue;
      }
      return;
    }
  }

  string() {
    const quote = this.s[this.i];
    this.i += 1;
    // Literal strings (single quotes) take no escapes at all — which is why
    // the manifests write their regexes that way, and why unescaping one here
    // would corrupt every `\s` and `\x{2800}` in the file.
    if (quote === "'") {
      const end = this.s.indexOf("'", this.i);
      if (end === -1) this.fail("unterminated literal string");
      const out = this.s.slice(this.i, end);
      this.i = end + 1;
      return out;
    }
    let out = "";
    while (this.i < this.s.length) {
      const ch = this.s[this.i];
      if (ch === '"') {
        this.i += 1;
        return out;
      }
      if (ch === "\\") {
        const esc = this.s[this.i + 1];
        this.i += 2;
        if (esc === "n") out += "\n";
        else if (esc === "t") out += "\t";
        else if (esc === "r") out += "\r";
        else if (esc === "u") {
          out += String.fromCodePoint(parseInt(this.s.slice(this.i, this.i + 4), 16));
          this.i += 4;
        } else out += esc;
        continue;
      }
      out += ch;
      this.i += 1;
    }
    this.fail("unterminated basic string");
    return "";
  }

  value() {
    this.skip();
    const ch = this.s[this.i];
    if (ch === '"' || ch === "'") return this.string();
    if (ch === "[") return this.array();
    if (ch === "{") return this.inlineTable();
    const word = /^[^,\]}\n#]+/.exec(this.s.slice(this.i));
    if (!word) this.fail("expected a value");
    const raw = word[0].trim();
    this.i += word[0].length;
    if (raw === "true") return true;
    if (raw === "false") return false;
    if (/^[+-]?\d+$/.test(raw)) return Number(raw);
    return raw;
  }

  array() {
    this.i += 1; // [
    const out = [];
    for (;;) {
      this.skip();
      if (this.s[this.i] === "]") {
        this.i += 1;
        return out;
      }
      if (this.s[this.i] === undefined) this.fail("unterminated array");
      out.push(this.value());
      this.skip();
      if (this.s[this.i] === ",") this.i += 1;
    }
  }

  inlineTable() {
    this.i += 1; // {
    const out = {};
    for (;;) {
      this.skip();
      if (this.s[this.i] === "}") {
        this.i += 1;
        return out;
      }
      if (this.s[this.i] === undefined) this.fail("unterminated inline table");
      const key = this.key();
      this.skip();
      if (this.s[this.i] !== "=") this.fail("expected `=` in inline table");
      this.i += 1;
      out[key] = this.value();
      this.skip();
      if (this.s[this.i] === ",") this.i += 1;
    }
  }

  key() {
    this.skip();
    if (this.s[this.i] === '"' || this.s[this.i] === "'") return this.string();
    const m = /^[A-Za-z0-9_.-]+/.exec(this.s.slice(this.i));
    if (!m) this.fail("expected a key");
    this.i += m[0].length;
    return m[0];
  }
}

/**
 * Parse one manifest. Top-level pairs land on the root object; `[[name]]`
 * blocks accumulate into `root[name]` as an array; `[name]` sets the current
 * table. Nested dotted table names are NOT supported — no manifest uses one,
 * and guessing at a shape this fixture has never seen would be worse than
 * throwing on it.
 */
export function parseToml(src) {
  const r = new Reader(src);
  const root = {};
  let table = root;
  for (;;) {
    r.skip();
    if (r.i >= r.s.length) return root;
    if (r.s[r.i] === "[") {
      const isArray = r.s[r.i + 1] === "[";
      r.i += isArray ? 2 : 1;
      const name = r.key();
      r.skip(true);
      const close = isArray ? "]]" : "]";
      if (r.s.slice(r.i, r.i + close.length) !== close) r.fail("unterminated table header");
      r.i += close.length;
      if (name.includes(".")) r.fail(`dotted table name ${name} is outside this reader's subset`);
      if (isArray) {
        root[name] = root[name] ?? [];
        table = {};
        root[name].push(table);
      } else {
        root[name] = root[name] ?? {};
        table = root[name];
      }
      continue;
    }
    const key = r.key();
    r.skip(true);
    if (r.s[r.i] !== "=") r.fail("expected `=` after key");
    r.i += 1;
    table[key] = r.value();
  }
}
