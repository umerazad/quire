import { expect, it } from "vitest";
import zlib from "node:zlib";
import {
  attachmentFileName,
  attachmentHash,
  canonical,
  decodeBatch,
  decodeCheckpoint,
  encodeBatch,
  encodeCheckpoint,
  pageIdForPath,
  parseName,
  revisionId,
  sha256,
  type Revision,
} from "./sync/format";
import {
  MemoryRevisions,
  addRevision,
  chainStep,
  createRevision,
  isAncestor,
} from "./sync/graph";

const notebook = "nb-1",
  office = "office",
  home = "home",
  laptop = "laptop";
const page = pageIdForPath("2026-10-29.md");
const edit = (
  store: MemoryRevisions,
  parents: string[],
  text: string,
  device = office,
) =>
  createRevision(store, {
    page,
    parents,
    path: "2026-10-29.md",
    text,
    created: 1,
    device,
  });
const batch = (
  revisions: Revision[],
  seq = 1,
  previous: string | null = null,
) =>
  encodeBatch({
    notebook,
    device: office,
    seq,
    previous,
    created: 1_000,
    revisions,
    attachments: [],
  });

it("freezes the revision id and batch encoding of format 1", () => {
  const fields = {
    page: "p-1",
    parents: [],
    generation: 0,
    path: "A.md",
    deleted: false,
    text: "- hello\r\n- مرحبا\n",
  };
  // Changing these values breaks every existing notebook. Add a format.
  expect(revisionId(fields)).toBe(
    sha256(
      '{"deleted":false,"format":1,"generation":0,"page":"p-1","parents":[],"path":"A.md","text":"- hello\\r\\n- مرحبا\\n"}',
    ),
  );
  expect(
    canonical({ b: [2, { d: 1, c: null }], a: "x", skip: undefined }),
  ).toBe('{"a":"x","b":[2,{"c":null,"d":1}]}');
  const store = new MemoryRevisions();
  const first = edit(store, [], "- one\n");
  expect(batch([first]).name).toBe(batch([first]).name);
  expect(batch([first]).name).toMatch(/^000000001-[0-9a-f]{64}\.qlog\.zst$/);
});
it("round-trips batches and checkpoints and keeps fields it does not know", () => {
  const store = new MemoryRevisions();
  const first = { ...edit(store, [], "- one\n"), future: { colour: "red" } };
  const encoded = encodeBatch({
    notebook,
    device: office,
    seq: 1,
    previous: null,
    created: 5,
    revisions: [first],
    attachments: [attachmentFileName(Buffer.from("png"), "Photo.PNG")],
    later: [1, 2],
  });
  const decoded = decodeBatch(encoded.name, encoded.data, {
    notebook,
    device: office,
  });
  expect(decoded).toMatchObject({ ok: true, hash: encoded.hash });
  if (!decoded.ok) return;
  expect(decoded.value.revisions[0]).toEqual(first);
  expect(decoded.value.later).toEqual([1, 2]);
  expect(attachmentHash(decoded.value.attachments[0])).toBe(sha256("png"));
  expect(decoded.value.attachments[0]).toMatch(/\.png$/);

  const checkpoint = encodeCheckpoint({
    notebook,
    device: home,
    seq: 3,
    created: 9,
    cursors: { [office]: { seq: 1, hash: encoded.hash } },
    heads: [first],
  });
  const read = decodeCheckpoint(checkpoint.name, checkpoint.data, {
    notebook,
    device: home,
  });
  expect(read.ok && read.value.heads[0].id).toBe(first.id);
});
it("rejects damaged, truncated, misplaced and newer files without throwing", () => {
  const store = new MemoryRevisions();
  const good = batch([edit(store, [], "- one\n")]);
  const expected = { notebook, device: office };
  const reason = (name: string, data: Uint8Array, at = expected) => {
    const result = decodeBatch(name, data, at);
    return result.ok ? "ok" : result.reason;
  };
  expect(
    reason(good.name, good.data.subarray(0, good.data.length - 4)),
  ).toMatch(/incomplete/);
  const flipped = Buffer.from(good.data);
  flipped[flipped.length - 6] ^= 0xff;
  expect(reason(good.name, flipped)).not.toBe("ok");
  expect(
    reason(good.name, good.data, { notebook: "other", device: office }),
  ).toMatch(/another notebook/);
  expect(reason(good.name, good.data, { notebook, device: home })).toMatch(
    /another computer/,
  );
  expect(
    reason(good.name.replace("000000001", "000000002"), good.data),
  ).toMatch(/number does not match/);
  expect(reason(good.name.replace(".qlog", " (1).qlog"), good.data)).toMatch(
    /not a batch/,
  );
  const tampered = edit(store, [], "- one\n");
  tampered.text = "- changed\n";
  expect(reason(batch([tampered]).name, batch([tampered]).data)).toMatch(
    /id does not match/,
  );
  const text = canonical({
    format: 2,
    notebook,
    device: office,
    seq: 1,
    previous: null,
    created: 1,
    revisions: [],
    attachments: [],
  });
  const future = decodeBatch(
    `000000001-${sha256(text)}.qlog.zst`,
    zlib.zstdCompressSync(Buffer.from(text)),
    expected,
  );
  expect(future).toMatchObject({ ok: false, newer: 2 });
});
it("names files strictly so partial writes and conflict copies are skipped", () => {
  const hash = "a".repeat(64);
  expect(parseName(`000000012-${hash}.qlog.zst`, "qlog")).toEqual({
    seq: 12,
    hash,
  });
  for (const name of [
    `.000000012-${hash}.qlog.zst.partial`,
    `000000012-${hash} (1).qlog.zst`,
    `000000012-${hash} (Home's conflicted copy).qlog.zst`,
    `000000012-${hash}-DESKTOP-1.qlog.zst`,
    `12-${hash}.qlog.zst`,
  ])
    expect(parseName(name, "qlog")).toBeUndefined();
  expect(parseName(`000000012-${hash}.qsnap.zst`, "qsnap")?.seq).toBe(12);
});
it("fast-forwards, detects forks as conflicts and merges them across three computers", () => {
  const store = new MemoryRevisions();
  const base = edit(store, [], "- start\n");
  expect(addRevision(store, base)).toMatchObject({
    kind: "added",
    conflict: false,
  });
  expect(addRevision(store, base)).toEqual({ kind: "duplicate" });
  const fromOffice = edit(store, [base.id], "- office\n");
  const fromHome = edit(store, [base.id], "- home\n", home);
  addRevision(store, fromOffice);
  const forked = addRevision(store, fromHome);
  expect(forked).toMatchObject({ kind: "added", conflict: true });
  expect(store.heads(page)).toHaveLength(2);
  // A third computer that saw only the office edit fast-forwards from it,
  // and the conflict with home remains.
  const fromLaptop = edit(store, [fromOffice.id], "- laptop\n", laptop);
  expect(addRevision(store, fromLaptop)).toMatchObject({ conflict: true });
  expect(new Set(store.heads(page))).toEqual(
    new Set([fromLaptop.id, fromHome.id]),
  );
  const merged = edit(store, store.heads(page), "- laptop\n- home\n", home);
  expect(addRevision(store, merged)).toMatchObject({
    kind: "added",
    heads: [merged.id],
    conflict: false,
  });
  for (const id of [base.id, fromOffice.id, fromHome.id, fromLaptop.id])
    expect(isAncestor(store, id, merged.id)).toBe(true);
  expect(isAncestor(store, fromHome.id, fromLaptop.id)).toBe(false);
  expect(isAncestor(store, merged.id, base.id)).toBe(false);
});
it("converges on identical edits and waits for missing parents", () => {
  const officeStore = new MemoryRevisions(),
    homeStore = new MemoryRevisions();
  // Both computers create today's daily note with the same starting text.
  const a = edit(officeStore, [], "- \n"),
    b = edit(homeStore, [], "- \n", home);
  expect(a.id).toBe(b.id);
  addRevision(officeStore, a);
  addRevision(officeStore, { ...b, created: 99 });
  expect(officeStore.heads(page)).toEqual([a.id]);
  const child = edit(officeStore, [a.id], "- typed\n");
  expect(addRevision(homeStore, child)).toEqual({
    kind: "missing",
    ids: [a.id],
  });
  expect(addRevision(officeStore, { ...child, generation: 5 })).toMatchObject({
    kind: "invalid",
  });
});
it("orders each computer's batches and reports gaps and copied device ids", () => {
  const h1 = "1".repeat(64),
    h2 = "2".repeat(64),
    h3 = "3".repeat(64);
  expect(chainStep(undefined, { seq: 1, previous: null }, h1)).toEqual({
    kind: "apply",
  });
  expect(chainStep(undefined, { seq: 2, previous: h1 }, h2)).toEqual({
    kind: "gap",
    expected: 1,
  });
  const at = { seq: 1, hash: h1 };
  expect(chainStep(at, { seq: 2, previous: h1 }, h2)).toEqual({
    kind: "apply",
  });
  expect(chainStep(at, { seq: 1, previous: null }, h1)).toEqual({
    kind: "applied",
  });
  expect(chainStep(at, { seq: 1, previous: null }, h3).kind).toBe("fork");
  expect(chainStep(at, { seq: 2, previous: h3 }, h2).kind).toBe("fork");
});
it("returns heads newest generation first, counting the revision being added", () => {
  const store = new MemoryRevisions();
  const base = edit(store, [], "- start\n");
  addRevision(store, base);
  const deep = edit(store, [base.id], "- one\n");
  addRevision(store, deep);
  const deeper = edit(store, [deep.id], "- two\n");
  addRevision(store, deeper);
  // A late edit from the old base: the newer head must still come first.
  const late = edit(store, [base.id], "- late\n", home);
  const added = addRevision(store, late);
  expect(added).toMatchObject({ kind: "added", heads: [deeper.id, late.id] });
  const fresh = edit(store, [deeper.id], "- three\n");
  expect(addRevision(store, fresh)).toMatchObject({
    heads: [fresh.id, late.id],
  });
});
