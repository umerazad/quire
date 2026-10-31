type Tagged = { tag: string; value: unknown };
const discarded = Symbol("discarded");

/** A data-only EDN reader. Reader evaluation and application tag handlers are never run. */
export function readEDN(text: string): unknown {
  let cursor = 0;
  const fail = (): never => {
    throw new Error(
      `Invalid or unsupported EDN near character ${cursor}. Export the graph again as JSON or EDN and retry.`,
    );
  };
  const whitespace = () => {
    while (cursor < text.length) {
      if (/[\s,]/u.test(text[cursor])) cursor++;
      else if (text[cursor] === ";") {
        while (cursor < text.length && text[cursor] !== "\n") cursor++;
      } else break;
    }
  };
  const token = () => {
    const start = cursor;
    while (cursor < text.length && !/[\s,[\](){}";]/u.test(text[cursor]))
      cursor++;
    if (start === cursor) fail();
    return text.slice(start, cursor);
  };
  const read = (depth: number): unknown => {
    if (depth > 256) fail();
    whitespace();
    if (cursor >= text.length) fail();
    const character = text[cursor++];
    if (character === '"') {
      const pieces: string[] = [];
      let start = cursor;
      while (cursor < text.length) {
        const current = text[cursor++];
        if (current === '"') {
          pieces.push(text.slice(start, cursor - 1));
          return pieces.join("");
        }
        if (current !== "\\") continue;
        pieces.push(text.slice(start, cursor - 1));
        const escaped = text[cursor++];
        const escapes: Record<string, string> = {
          n: "\n",
          r: "\r",
          t: "\t",
          b: "\b",
          f: "\f",
          '"': '"',
          "\\": "\\",
        };
        if (escaped === "u") {
          const hex = text.slice(cursor, cursor + 4);
          if (!/^[a-f\d]{4}$/i.test(hex)) fail();
          pieces.push(String.fromCharCode(Number.parseInt(hex, 16)));
          cursor += 4;
        } else {
          if (!Object.hasOwn(escapes, escaped)) fail();
          pieces.push(escapes[escaped]);
        }
        start = cursor;
      }
      return fail();
    }
    if (character === "[" || character === "(" || character === "{") {
      const close = character === "[" ? "]" : character === "(" ? ")" : "}";
      const values: unknown[] = [];
      while (true) {
        whitespace();
        if (text[cursor] === close) {
          cursor++;
          break;
        }
        const value = read(depth + 1);
        if (value !== discarded) values.push(value);
      }
      if (character !== "{") return values;
      if (values.length % 2) fail();
      const map = new Map<unknown, unknown>();
      for (let i = 0; i < values.length; i += 2) {
        if (map.has(values[i])) fail();
        map.set(values[i], values[i + 1]);
      }
      return map;
    }
    if (character === "#") {
      if (text[cursor] === "#") {
        const numeric = token();
        if (!["#NaN", "#Inf", "#-Inf"].includes(numeric)) fail();
        return { edn: `#${numeric}` };
      }
      if (text[cursor] === "_") {
        cursor++;
        read(depth + 1);
        return discarded;
      }
      if (text[cursor] === "{") {
        cursor++;
        const values: unknown[] = [];
        while (true) {
          whitespace();
          if (text[cursor] === "}") {
            cursor++;
            break;
          }
          const entry = read(depth + 1);
          if (entry !== discarded) values.push(entry);
        }
        return values;
      }
      const tag = token();
      if (!/^[a-z][\w./-]*$/i.test(tag)) fail();
      const value = read(depth + 1);
      if (value === discarded) fail();
      return { tag, value } satisfies Tagged;
    }
    if (character === "\\") {
      const first = text[cursor++];
      if (!first) fail();
      if (!/[a-z]/i.test(first)) return first;
      cursor--;
      const name = token();
      const chars: Record<string, string> = {
        newline: "\n",
        return: "\r",
        space: " ",
        tab: "\t",
        backspace: "\b",
        formfeed: "\f",
      };
      if (Object.hasOwn(chars, name)) return chars[name];
      if (/^u[a-f\d]{4}$/i.test(name))
        return String.fromCharCode(Number.parseInt(name.slice(1), 16));
      if (name.length === 1) return name;
      return fail();
    }
    cursor--;
    const atom = token();
    if (atom === "nil") return null;
    if (atom === "true") return true;
    if (atom === "false") return false;
    if (/^[+-]?\d+(?:\.\d+)?(?:e[+-]?\d+)?[MN]?$/i.test(atom)) {
      const number = Number(atom.replace(/[MN]$/, ""));
      return Number.isFinite(number) &&
        (!Number.isInteger(number) || Number.isSafeInteger(number))
        ? number
        : { edn: atom };
    }
    if (atom.startsWith(":")) return atom;
    if (/^[\d#]/.test(atom)) return fail();
    return { symbol: atom };
  };
  let value = read(0);
  while (value === discarded) value = read(0);
  whitespace();
  if (cursor !== text.length) fail();
  return value;
}

export function ednJSON(value: unknown): unknown {
  if (value instanceof Map) {
    if ([...value.keys()].every((key) => typeof key === "string"))
      return Object.fromEntries(
        [...value].map(([key, entry]) => [key, ednJSON(entry)]),
      );
    return {
      map: [...value].map(([key, entry]) => [ednJSON(key), ednJSON(entry)]),
    };
  }
  if (Array.isArray(value)) return value.map(ednJSON);
  if (value && typeof value === "object")
    return Object.fromEntries(
      Object.entries(value).map(([key, entry]) => [key, ednJSON(entry)]),
    );
  return value;
}
