import assert from "node:assert/strict";
import { pageIdForPath, type Revision } from "../reference/sync/format";
import { Computer, Network, random, type EngineFactory } from "./harness";

// rt's sync-go-harness hand-over, with a native computer at home: a Go office saves text with
// an emoji, home continues it, and neither side keeps competing versions. rt's lone surrogate
// is left out because native pages hold UTF-8 text.
export async function goHandover(home: EngineFactory, office: EngineFactory) {
  const network = new Network(random(5));
  let time = 0;
  const clock = () => ++time;
  for (const name of ["office", "home"]) network.view(name);
  const goOffice = new Computer("office", network, "nb-test", clock, "office", undefined, office);
  const nativeHome = new Computer("home", network, "nb-test", clock, "home", undefined, home);
  const page = pageIdForPath("Plan.md");
  const saved = new Map<string, Revision>();
  goOffice.type(page, "Plan.md", "- from the office 😀\n");
  goOffice.save(page, saved);
  await goOffice.publish();
  network.deliverAll();
  await nativeHome.sync();
  assert.equal(nativeHome.replica.head(page)?.text, "- from the office 😀\n");
  nativeHome.type(page, "Plan.md", "- continued at home\n");
  nativeHome.save(page, saved);
  await nativeHome.publish();
  network.deliverAll();
  await goOffice.sync();
  assert.equal(goOffice.replica.head(page)?.text, "- continued at home\n");
  assert.equal(goOffice.replica.head(page)?.id, nativeHome.replica.head(page)?.id);
  assert.deepEqual(goOffice.replica.conflicts(), []);
}
