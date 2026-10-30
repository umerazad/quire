// Three-way merge of page text, shared by the editor, the desktop engine
// and (as tui/internal/merge) the terminal app. Two computers merging the
// same versions must produce the same text, so every step here is
// deterministic and mirrored exactly in Go; tests/fixtures/merges.json
// holds the cases both run.
//
// Lines changed on only one side apply. Where both sides changed the same
// lines, the change is merged word by word; if that also overlaps, both
// versions are kept, the first side's lines before the second's, and a
// block id the second copy would repeat is dropped from it. Only that
// last case counts as an overlap: lines both sides added at one place are
// all kept and nothing is repeated.

export type MergeResult = { text: string; overlaps: number };

type Region = { aStart: number; aEnd: number; bStart: number; bEnd: number };
type Chunk =
  | { kind: "same"; items: string[] }
  | { kind: "both"; base: string[]; first: string[]; second: string[] };

// Beyond this many edits the middle is treated as replaced outright.
const maxEdits = 2000;

// Matched index pairs of a shortest edit script from a to b (Myers).
function matches(a: string[], b: string[]): [number, number][] {
  const n = a.length,
    m = b.length,
    max = n + m;
  if (max === 0) return [];
  const offset = max + 1;
  const v = new Int32Array(2 * max + 3);
  const trace: Int32Array[] = [];
  let found = -1;
  for (let d = 0; d <= Math.min(max, maxEdits); d++) {
    for (let k = -d; k <= d; k += 2) {
      let x =
        k === -d || (k !== d && v[offset + k - 1] < v[offset + k + 1])
          ? v[offset + k + 1]
          : v[offset + k - 1] + 1;
      let y = x - k;
      while (x < n && y < m && a[x] === b[y]) {
        x++;
        y++;
      }
      v[offset + k] = x;
      if (x >= n && y >= m) {
        found = d;
        break;
      }
    }
    trace.push(v.slice(offset - d, offset + d + 1));
    if (found >= 0) break;
  }
  if (found < 0) return [];
  const pairs: [number, number][] = [];
  let x = n,
    y = m;
  for (let d = found; d > 0; d--) {
    const previous = trace[d - 1];
    const at = (k: number) => previous[k + d - 1];
    const k = x - y;
    const down = k === -d || (k !== d && at(k - 1) < at(k + 1));
    const previousK = down ? k + 1 : k - 1;
    const previousX = at(previousK),
      previousY = previousX - previousK;
    while (x > previousX && y > previousY) pairs.push([--x, --y]);
    x = previousX;
    y = previousY;
  }
  while (x > 0 && y > 0) pairs.push([--x, --y]);
  return pairs.reverse();
}

// The runs where a and b differ, in order.
export function diffRegions(a: string[], b: string[]): Region[] {
  let start = 0;
  while (start < a.length && start < b.length && a[start] === b[start]) start++;
  let endA = a.length,
    endB = b.length;
  while (endA > start && endB > start && a[endA - 1] === b[endB - 1]) {
    endA--;
    endB--;
  }
  const pairs = matches(a.slice(start, endA), b.slice(start, endB)).map(
    ([i, j]) => [i + start, j + start] as const,
  );
  pairs.push([endA, endB]);
  const regions: Region[] = [];
  let i = start,
    j = start;
  for (const [pi, pj] of pairs) {
    if (pi > i || pj > j)
      regions.push({ aStart: i, aEnd: pi, bStart: j, bEnd: pj });
    i = pi + 1;
    j = pj + 1;
  }
  return regions;
}

const inserts = (r: Region) => r.aStart === r.aEnd;

// Splits base into stretches both sides kept and stretches either changed.
// Changes overlap when they touch the same base items, or when both insert
// at one place. An insertion just before the other side's change does not
// overlap it: it goes first, as one just after goes after.
function merge3(base: string[], first: string[], second: string[]): Chunk[] {
  const hunks = [
    ...diffRegions(base, first).map((r) => ({ side: 0, ...r })),
    ...diffRegions(base, second).map((r) => ({ side: 1, ...r })),
  ].sort(
    (p, q) =>
      p.aStart - q.aStart ||
      Number(inserts(q)) - Number(inserts(p)) ||
      p.side - q.side,
  );
  const chunks: Chunk[] = [];
  // Where each side's text stands relative to base, outside its hunks.
  const shift = [0, 0];
  let at = 0;
  const side = (s: number) => (s === 0 ? first : second);
  for (let i = 0; i < hunks.length;) {
    const groupStart = hunks[i].aStart;
    let groupEnd = hunks[i].aEnd;
    const group = [hunks[i++]];
    while (
      i < hunks.length &&
      (hunks[i].aStart < groupEnd ||
        (hunks[i].aStart === groupStart && inserts(hunks[i])))
    ) {
      groupEnd = Math.max(groupEnd, hunks[i].aEnd);
      group.push(hunks[i++]);
    }
    if (groupStart > at)
      chunks.push({ kind: "same", items: base.slice(at, groupStart) });
    // Each side's text for base[groupStart, groupEnd).
    const texts = [0, 1].map((s) => {
      const own = group.filter((h) => h.side === s);
      if (!own.length) {
        const from = groupStart + shift[s];
        return side(s).slice(from, from + groupEnd - groupStart);
      }
      const startShift = own[0].bStart - own[0].aStart;
      const last = own[own.length - 1];
      const endShift = last.bEnd - last.aEnd;
      shift[s] = endShift;
      return side(s).slice(groupStart + startShift, groupEnd + endShift);
    });
    const changed = [0, 1].map((s) => group.some((h) => h.side === s));
    if (!changed[1]) chunks.push({ kind: "same", items: texts[0] });
    else if (!changed[0]) chunks.push({ kind: "same", items: texts[1] });
    else if (equal(texts[0], texts[1]))
      chunks.push({ kind: "same", items: texts[0] });
    else
      chunks.push({
        kind: "both",
        base: base.slice(groupStart, groupEnd),
        first: texts[0],
        second: texts[1],
      });
    at = groupEnd;
  }
  if (at < base.length) chunks.push({ kind: "same", items: base.slice(at) });
  return chunks;
}
const equal = (a: string[], b: string[]) =>
  a.length === b.length && a.every((item, i) => item === b[i]);

// Words, runs of spaces and tabs, and line ends, so a merge inside a line
// works on whole words.
function tokens(lines: string[]) {
  const out: string[] = [];
  for (const line of lines) {
    out.push(...(line.match(/[ \t]+|[^ \t]+/g) ?? []));
    out.push("\n");
  }
  return out;
}

// Both sides changed the same lines: merge word by word, or keep both.
function resolve(
  base: string[],
  first: string[],
  second: string[],
): { lines: string[]; repeats: boolean[] } | undefined {
  const chunks = merge3(tokens(base), tokens(first), tokens(second));
  if (chunks.every((chunk) => chunk.kind === "same")) {
    const text = chunks.flatMap((chunk) => chunk.items).join("");
    if (text === "" || text.endsWith("\n")) {
      const lines = text.split("\n").slice(0, -1);
      return { lines, repeats: lines.map(() => false) };
    }
  }
  return undefined;
}
function keepBoth(first: string[], second: string[]) {
  const lines: string[] = [],
    repeats: boolean[] = [];
  let i = 0,
    j = 0;
  const push = (line: string, repeat: boolean) => {
    lines.push(line);
    repeats.push(repeat);
  };
  for (const region of [
    ...diffRegions(first, second),
    {
      aStart: first.length,
      aEnd: first.length,
      bStart: second.length,
      bEnd: second.length,
    },
  ]) {
    while (i < region.aStart) {
      push(first[i++], false);
      j++;
    }
    for (; i < region.aEnd; i++) push(first[i], false);
    for (; j < region.bEnd; j++) push(second[j], true);
  }
  return { lines, repeats };
}

const blockId = /(?:^|[ \t])\^([a-zA-Z0-9_-]+)[ \t]*$/;

// Merges the edits from base to first and from base to second.
export function mergeText(
  base: string,
  first: string,
  second: string,
): MergeResult {
  if (first === second) return { text: first, overlaps: 0 };
  if (base === first) return { text: second, overlaps: 0 };
  if (base === second) return { text: first, overlaps: 0 };
  const lines: string[] = [],
    repeats: boolean[] = [];
  let overlaps = 0;
  for (const chunk of merge3(
    base.split("\n"),
    first.split("\n"),
    second.split("\n"),
  )) {
    if (chunk.kind === "same") {
      lines.push(...chunk.items);
      repeats.push(...chunk.items.map(() => false));
      continue;
    }
    let resolved = resolve(chunk.base, chunk.first, chunk.second);
    if (!resolved) {
      if (chunk.base.length) overlaps++;
      resolved = keepBoth(chunk.first, chunk.second);
    }
    lines.push(...resolved.lines);
    repeats.push(...resolved.repeats);
  }
  // The second copy of a block gives up an id the first copy kept.
  const ids = new Set<string>();
  lines.forEach((line, i) => {
    const id = repeats[i] ? undefined : blockId.exec(line)?.[1];
    if (id) ids.add(id);
  });
  lines.forEach((line, i) => {
    if (!repeats[i]) return;
    const match = blockId.exec(line);
    if (!match) return;
    if (ids.has(match[1]))
      lines[i] = line.slice(0, match.index).replace(/[ \t]+$/, "");
    else ids.add(match[1]);
  });
  return { text: lines.join("\n"), overlaps };
}
