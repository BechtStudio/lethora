// Sources library data (as `bib_entries` returns it), search, and the
// "Add source" form's BibLaTeX: key generation and value escaping.
// Pure functions, no DOM.

export interface BibEntry {
  key: string;
  entry_type: string;
  title?: string | null;
  /** "Family, Given". */
  authors: string[];
  year?: string | null;
}

export interface BibError {
  message: string;
  line: number;
  column: number;
}

export interface BibLibrary {
  entries: BibEntry[];
  errors: BibError[];
}

export interface ImportReport {
  added: string[];
  skipped_duplicates: string[];
}

export interface StyleInfo {
  id: string;
  name: string;
}

// ---- Display ----

const family = (name: string) => (name.includes(",") ? name.split(",")[0] : name.split(/\s+/).pop() ?? name).trim();

/** "Woolf", "Woolf & Bell", "Woolf et al." */
export function shortAuthors(authors: string[]): string {
  if (authors.length === 0) return "";
  if (authors.length === 1) return family(authors[0]);
  if (authors.length === 2) return `${family(authors[0])} & ${family(authors[1])}`;
  return `${family(authors[0])} et al.`;
}

/** "Woolf 1927" (either part may be missing). */
export function entryDetail(e: BibEntry): string {
  return [shortAuthors(e.authors), e.year ?? ""].filter(Boolean).join(" ");
}

// ---- Search ----

/** Lowercase text an entry is searched by (key, title, authors, year). */
export const searchText = (e: BibEntry) => `${e.key} ${e.title ?? ""} ${e.authors.join(" ")} ${e.year ?? ""}`.toLowerCase();

/**
 * Entries matching every whitespace-separated term of `query` (substring,
 * case-insensitive), in library order. `index` holds `searchText` per entry
 * (computed once per library, so searching 2000 entries is cheap).
 */
export function filterEntries(entries: BibEntry[], index: string[], query: string): BibEntry[] {
  const terms = query.toLowerCase().split(/\s+/).filter(Boolean);
  if (terms.length === 0) return entries;
  const out: BibEntry[] = [];
  for (let i = 0; i < entries.length; i++) {
    const text = index[i];
    if (terms.every((t) => text.includes(t))) out.push(entries[i]);
  }
  return out;
}

/** "2 added, 1 duplicate skipped (woolf1927)". */
export function importSummary(report: ImportReport): string {
  const added = report.added.length;
  const dup = report.skipped_duplicates;
  let s = added === 0 ? "Nothing new added" : `${added} added`;
  if (dup.length > 0) s += `, ${dup.length} duplicate${dup.length === 1 ? "" : "s"} skipped (${dup.join(", ")})`;
  return s;
}

// ---- Keys ----

/** Characters a citation key may use (so `@key` works in Typst). */
export const KEY_RE = /^[A-Za-z0-9_][A-Za-z0-9_\-:.]*$/;

/** Lowercase ASCII letters and digits only (accents dropped: "Brontë" → "bronte"). */
export function asciiWord(s: string): string {
  return s
    .normalize("NFKD")
    .replace(/[̀-ͯ]/g, "")
    .replace(/ß/g, "ss")
    .replace(/[æÆ]/g, "ae")
    .replace(/[øØ]/g, "o")
    .toLowerCase()
    .replace(/[^a-z0-9]/g, "");
}

const ARTICLES = new Set(["a", "an", "the", "der", "die", "das", "le", "la", "les", "el", "il"]);

/**
 * `lastname + year + first title word` (leading articles skipped), lowercase
 * ASCII, made unique against `existing` with a numeric suffix (2, 3, ...).
 */
export function makeKey(authors: string[], year: string, title: string, existing: Iterable<string>): string {
  const first = authors.find((a) => a.trim());
  const last = first ? asciiWord(family(first)) : "";
  const words = title.split(/\s+/).map(asciiWord).filter(Boolean);
  const word = words.find((w) => !ARTICLES.has(w)) ?? words[0] ?? "";
  const base = `${last}${asciiWord(year)}${word}` || "source";
  const taken = new Set([...existing].map((k) => k.toLowerCase()));
  if (!taken.has(base)) return base;
  for (let n = 2; ; n++) if (!taken.has(`${base}${n}`)) return `${base}${n}`;
}

// ---- BibLaTeX ----

const ESCAPES: Record<string, string> = {
  "\\": "\\textbackslash{}",
  "{": "\\{",
  "}": "\\}",
  "%": "\\%",
  "#": "\\#",
  "&": "\\&",
  $: "\\$",
  _: "\\_",
  "~": "\\textasciitilde{}",
  "^": "\\textasciicircum{}",
};

/** A literal text for a braced BibLaTeX field value (no unbalanced braces, no LaTeX surprises). */
export const escapeBib = (s: string) => s.replace(/\s+/g, " ").trim().replace(/[\\{}%#&$_~^]/g, (c) => ESCAPES[c]);

/** A verbatim field (url, doi): kept as written, braces percent-encoded, no whitespace. */
export const escapeVerbatim = (s: string) => s.trim().replace(/\s+/g, "").replace(/\{/g, "%7B").replace(/\}/g, "%7D");

/** A DOI without its resolver prefix. */
export const cleanDoi = (s: string) => escapeVerbatim(s.trim().replace(/^(?:https?:\/\/(?:dx\.)?doi\.org\/|doi:\s*)/i, ""));

export type SourceType = "article" | "book" | "inproceedings" | "online" | "misc";

/** The container field each type asks for (none for online/misc). */
export const CONTAINER: Record<SourceType, "journal" | "publisher" | "booktitle" | null> = {
  article: "journal",
  book: "publisher",
  inproceedings: "booktitle",
  online: null,
  misc: null,
};

export interface NewSource {
  type: SourceType;
  key: string;
  /** "Last, First", one per entry. */
  authors: string[];
  title: string;
  year: string;
  /** Journal, publisher or book title (per type). */
  container?: string;
  doi?: string;
  url?: string;
}

/** One author name, protected if it contains a stray " and " (the name separator). */
function authorValue(name: string): string {
  const v = escapeBib(name);
  return /\band\b/i.test(v) ? `{${v}}` : v;
}

/** The BibLaTeX entry for a source from the form. Throws on an invalid key. */
export function toBibLatex(s: NewSource): string {
  if (!KEY_RE.test(s.key)) throw new Error(`Invalid key “${s.key}”: use letters, digits, - _ : .`);
  const fields: Array<[string, string]> = [];
  const authors = s.authors.map((a) => a.trim()).filter(Boolean);
  if (authors.length) fields.push(["author", authors.map(authorValue).join(" and ")]);
  if (s.title.trim()) fields.push(["title", escapeBib(s.title)]);
  const container = CONTAINER[s.type];
  if (container && s.container?.trim()) fields.push([container, escapeBib(s.container)]);
  if (s.year.trim()) fields.push(["year", escapeBib(s.year)]);
  if (s.doi?.trim()) fields.push(["doi", cleanDoi(s.doi)]);
  if (s.url?.trim()) fields.push(["url", escapeVerbatim(s.url)]);
  const body = fields.map(([k, v]) => `  ${k} = {${v}},`).join("\n");
  return `@${s.type}{${s.key},\n${body}\n}\n`;
}
