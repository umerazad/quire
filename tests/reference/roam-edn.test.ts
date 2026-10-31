import { expect, it } from "vitest";
import { readEDN, ednJSON } from "./core/edn";
import { importRoamEDN } from "./core/roam-edn";
import { exportRoamJSON } from "./core/roam";

const database = (datoms: string) => `#datascript/DB {
  :schema {:block/children {:db/cardinality :db.cardinality/many}
           :block/refs {:db/cardinality :db.cardinality/many}}
  :datoms [${datoms}]}`;
const page = '[1 :node/title "Page" 100] [1 :block/uid "page" 100]';
const block = (id: number, uid: string, text: string) =>
  `[${id} :block/uid "${uid}" 100] [${id} :block/string ${JSON.stringify(text)} 100]`;

it("reads EDN strings, sets, comments, tags, non-finite numbers and composite map keys without evaluation", () => {
  const value = readEDN(String.raw`{:text "Line\n\u03bb\t\"\\"
    :set #{1 2 3} ; comment
    :list (true false nil) :discard [#_ 99 1]
    :tag #uuid "test" :infinite [##NaN ##Inf ##-Inf]
    :char [\newline \) \u03bb]
    :large 9007199254740993N :decimal 1.5M
    :complex {[1 2] :value}}`);
  expect(ednJSON(value)).toEqual({
    ":text": 'Line\nλ\t"\\',
    ":set": [1, 2, 3],
    ":list": [true, false, null],
    ":discard": [1],
    ":tag": { tag: "uuid", value: "test" },
    ":infinite": [{ edn: "##NaN" }, { edn: "##Inf" }, { edn: "##-Inf" }],
    ":char": ["\n", ")", "λ"],
    ":large": { edn: "9007199254740993N" },
    ":decimal": 1.5,
    ":complex": { map: [[[1, 2], ":value"]] },
  });
});

it.each([
  "{:a}",
  "[1)",
  '"unterminated',
  '"\\q"',
  "#=(do anything)",
  "{} {}",
  "#unknown",
  "[1",
  "{:a 1 :a 2}",
])("rejects malformed EDN without exposing note text: %s", (text) => {
  expect(() => readEDN(text)).toThrow("character");
});

it("reconstructs child order, hierarchy, references, users and source metadata", () => {
  const imported = importRoamEDN(
    database(`${page}
    [1 :block/children 2 100] [1 :block/children 3 100]
    ${block(2, "last", "((child))")} [2 :block/order 1 100]
    ${block(3, "first", "Parent")} [3 :block/order 0 100]
    [3 :block/children 4 100] [3 :block/open false 100] [3 :block/heading 2 100]
    ${block(4, "child", "Line\nλ")} [4 :block/order 0 100]
    [4 :create/time 123 100] [4 :create/user 5 100]
    [5 :user/uid "writer" 100] [5 :user/settings {:x #{1 2 3} :n ##NaN} 100]`),
  );
  const output = exportRoamJSON(imported.documents, imported.metadata).pages;
  expect(output).toMatchObject([
    {
      title: "Page",
      uid: "page",
      children: [
        {
          uid: "first",
          string: "Parent",
          open: false,
          heading: 2,
          children: [
            {
              uid: "child",
              string: "Line\nλ",
              "create-time": 123,
              ":create/user": { ":user/uid": "writer" },
            },
          ],
        },
        { uid: "last", string: "((child))" },
      ],
    },
  ]);
  expect(imported.report[1]).toContain("Recovered 0");
});

it("recovers orphan trees and blocks owned by non-outline database entities", () => {
  const imported = importRoamEDN(
    database(`${page}
    ${block(2, "orphan", "Orphan")} [2 :block/children 3 100]
    ${block(3, "descendant", "Descendant")}
    [10 :version/id "version" 100] [10 :block/children 4 100]
    ${block(4, "history", "Detached history")}`),
  );
  expect(imported.documents).toHaveLength(2);
  expect(imported.report[0]).toContain("3 blocks");
  expect(imported.report[1]).toContain("Recovered 3 detached");
  const recovered = Object.values(imported.metadata.pages).find(
    (p) => p.title === "Roam recovered blocks",
  )!;
  expect(recovered.children?.map((b) => b.uid)).toEqual(["orphan", "history"]);
  expect(recovered.children?.[0].children?.[0].uid).toBe("descendant");
});
it("preserves text from detached blocks whose database UID is empty", () => {
  const imported = importRoamEDN(
    database(`${page} ${block(2, "", "Recovered text")}`),
  );
  const recovered = Object.values(imported.metadata.pages).find(
    (p) => p.title === "Roam recovered blocks",
  )!;
  expect(recovered.children?.[0].string).toBe("Recovered text");
  expect(recovered.children?.[0].uid).toMatch(/^b-/);
  expect(imported.report.join("\n")).toContain("1 blocks with empty");
});

it("does not silently import cycles, multiple parents, missing children or malformed datoms", () => {
  for (const datoms of [
    `${page} [1 :block/children 2 100]`,
    `${page} ${block(2, "a", "A")} [2 :block/children 2 100]`,
    `${page} ${block(2, "a", "A")} [1 :block/children 2 100] [3 :block/children 2 100]`,
    `${page} [1 :node/title "Duplicate" 100]`,
    `${page} [2 :block/string "A" 100 false]`,
    '[1 :node/title "Page"]',
  ])
    expect(() => importRoamEDN(database(datoms))).toThrow("invalid");
  expect(() => importRoamEDN("[]")).toThrow("database");
});
