// Fuzzy matching for short names: the query's characters must appear in
// order. Matches at the start, at word starts and in runs score higher;
// gaps cost a little. positions index the name's characters ([...name]),
// for highlighting.
export type FuzzyMatch = { score: number; positions: number[] };

const boundary = /[\s\-_/.:([]/;

function bonus(chars: string[], index: number) {
  if (index === 0) return 10;
  const before = chars[index - 1],
    here = chars[index];
  if (boundary.test(before)) return 8;
  if (before === before.toLowerCase() && here !== here.toLowerCase()) return 6;
  return 0;
}

export function fuzzyMatch(
  query: string,
  text: string,
): FuzzyMatch | undefined {
  const raw = [...query].filter((c) => c.trim()),
    q = raw.map((c) => c.toLocaleLowerCase()),
    chars = [...text],
    lower = chars.map((c) => c.toLocaleLowerCase()),
    n = chars.length;
  if (!q.length) return { score: 0, positions: [] };
  if (q.length > n) return undefined;
  // best[i][j]: best score with q[0..i] matched and q[i] at chars[j].
  const best = q.map(() => Array.from({ length: n }, () => -Infinity)),
    from = q.map(() => Array.from({ length: n }, () => -1));
  for (let i = 0; i < q.length; i++)
    for (let j = i; j < n; j++) {
      if (lower[j] !== q[i]) continue;
      const own = 1 + bonus(chars, j) + (chars[j] === raw[i] ? 1 : 0);
      if (i === 0) {
        best[i][j] = own - Math.min(j, 5);
        continue;
      }
      for (let k = i - 1; k < j; k++) {
        if (best[i - 1][k] === -Infinity) continue;
        const run = k === j - 1 ? 5 : -Math.min(j - k - 1, 3),
          value = best[i - 1][k] + own + run;
        if (value > best[i][j]) {
          best[i][j] = value;
          from[i][j] = k;
        }
      }
    }
  const last = q.length - 1;
  let end = -1;
  for (let j = 0; j < n; j++)
    if (
      best[last][j] > -Infinity &&
      (end < 0 || best[last][j] > best[last][end])
    )
      end = j;
  if (end < 0) return undefined;
  const positions: number[] = [];
  for (let i = last, j = end; i >= 0; j = from[i][j], i--) positions.unshift(j);
  // Shorter names win ties: "Ops" over "Ops review" for "ops".
  return { score: best[last][end] - (n - positions.length) * 0.05, positions };
}

// Items whose name matches, best first; ties keep the given order.
export function fuzzyFilter<T>(
  items: T[],
  query: string,
  name: (item: T) => string,
) {
  return items
    .map((item, order) => ({
      item,
      order,
      match: fuzzyMatch(query, name(item)),
    }))
    .filter(
      (value): value is { item: T; order: number; match: FuzzyMatch } =>
        !!value.match,
    )
    .sort((a, b) => b.match.score - a.match.score || a.order - b.order);
}
