import assert from "node:assert/strict";
import { pageIdForPath, type Revision } from "../reference/sync/format";
import { Computer, Network, random, type EngineFactory } from "./harness";

// Two editors on one page at once: each keeps typing into its own buffer while the other's
// versions arrive, saves from whatever version it last loaded, and continues from the merge,
// as the harness editor model and rt's editor do. `office` and `home` choose each engine;
// undefined is rt's frozen TypeScript replica.
export async function editOnePageAtOnce(seed: number, office?: EngineFactory, home?: EngineFactory, rounds = 24) {
  const rng = random(seed);
  const network = new Network(rng);
  let time = 5_000;
  const clock = () => ++time;
  for (const name of ["office", "home"]) network.view(name);
  const computers = [
    new Computer("office", network, "nb-test", clock, "office", undefined, office),
    new Computer("home", network, "nb-test", clock, "home", undefined, home),
  ];
  const [left, right] = computers;
  const path = "Plan.md", page = pageIdForPath(path), saved = new Map<string, Revision>();
  left.type(page, path, "- top\n\n- middle\n\n- bottom\n");
  left.save(page, saved);
  await left.publish();
  network.deliverAll();
  await right.sync();
  const typed: string[] = [];
  for (let round = 0; round < rounds; round++) {
    // The office writes under the top line and home under the bottom one, both on the version
    // each editor shows, so every save after the first races the other editor's.
    for (const [computer, anchor] of [[left, "- top\n"], [right, "- bottom\n"]] as const) {
      const buffer = computer.open(page, path);
      const line = `- ${computer.name} ${round}\n`;
      typed.push(line);
      computer.type(page, path, buffer.text.replace(anchor, anchor + line));
    }
    for (const computer of rng() < 0.5 ? computers : [right, left]) {
      if (rng() < 0.7) computer.save(page, saved);
      if (rng() < 0.5) await computer.publish();
    }
    for (let i = 0; i < rng() * 6; i++) network.deliverOne();
    for (const computer of computers) if (rng() < 0.6) await computer.sync();
  }
  for (const computer of computers) computer.save(page, saved);
  for (let round = 0; round < 12; round++) {
    for (const computer of computers) await computer.publish();
    network.deliverAll();
    for (const computer of computers) await computer.sync();
    if (round >= 3 && computers.every((computer) => !computer.store.outbox().length)) break;
  }
  for (const computer of computers) assert.deepEqual(computer.replica.conflicts(), [], `seed ${seed}: ${computer.name} kept competing versions`);
  const [first, second] = computers.map((computer) => computer.replica.head(page)!);
  assert.equal(first.id, second.id, `seed ${seed}: both computers end on one version`);
  for (const line of typed) assert(first.text.includes(line), `seed ${seed}: the merged page lost ${JSON.stringify(line)}:\n${first.text}`);
  for (const line of ["- top\n", "- middle\n", "- bottom\n"]) assert(first.text.includes(line), `seed ${seed}: lost ${line}`);
  return typed.length;
}
