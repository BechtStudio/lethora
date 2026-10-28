// Word count over a CodeMirror document, chunk by chunk (never joined into one string).

import type { Text } from "@codemirror/state";

function isSpace(c: number): boolean {
  return (
    c <= 32 ||
    c === 0xa0 ||
    c === 0x1680 ||
    (c >= 0x2000 && c <= 0x200a) ||
    c === 0x2028 ||
    c === 0x2029 ||
    c === 0x202f ||
    c === 0x205f ||
    c === 0x3000
  );
}

/** Whitespace-delimited words. */
export function countWords(doc: Text): number {
  let words = 0;
  let inWord = false;
  for (const iter = doc.iter(); !iter.next().done; ) {
    const s = iter.value;
    for (let i = 0; i < s.length; i++) {
      if (isSpace(s.charCodeAt(i))) inWord = false;
      else if (!inWord) {
        inWord = true;
        words++;
      }
    }
  }
  return words;
}
