import assert from "node:assert/strict";
import { readFileSync } from "node:fs";
import { join } from "node:path";
import { decodeBatch, pageIdForPath } from "../reference/sync/format";
import { MemoryReplicaStore, Replica } from "../reference/sync/replica";
import { Network, random, type Engine } from "./harness";
import { NativeProbe, nativeEngine } from "./native-peer";

export async function focusedSchedules(probe: NativeProbe) {
  let time = 1000;
  const clock = () => ++time;
  const setup = (left: boolean, right: boolean) => {
    probe.request({ op: "reset" });
    const network = new Network(random(17));
    for (const name of ["office", "home"]) network.view(name);
    const make = (name: string, native: boolean) => {
      if (native) return {
        engine: nativeEngine(probe, name)(network.syncFolder(name), clock),
        attach: async (data: Uint8Array) => probe.request<string>({ op: "attach", peer: name, data: [...data], original: "image.png" }),
      };
      const store = new MemoryReplicaStore();
      const replica = new Replica({ notebook: "nb-test", device: name, store, folder: network.folder(name), now: clock });
      const engine: Engine = { store, replica, rebuild() { throw new Error("Focused TypeScript fixture does not rebuild"); } };
      return { engine, attach: (data: Uint8Array) => replica.attach(data, "image.png") };
    };
    return { network, office: make("office", left), home: make("home", right) };
  };
  const page = pageIdForPath("Plan.md");
  for (const [left, right] of [[true, true], [true, false], [false, true]]) {
    const { network, office, home } = setup(left, right);
    const bytes = Buffer.from("صورة میٹنگ 🙂");
    const reference = await office.attach(bytes);
    const saved = office.engine.replica.save({ page, base: [], path: "Plan.md", text: `![image](${reference})\n` });
    await office.engine.replica.publish();
    const folder = network.syncFolder("office");
    const name = folder.list("devices/office/log")[0];
    const log = `devices/office/log/${name}`;
    network.view("home").set(log, folder.read(log)!);
    const before = await home.engine.replica.import();
    assert.equal(home.engine.replica.head(page)?.id, saved.revision.id);
    assert.deepEqual(before.missingAttachments, [reference.slice("attachments/".length)]);
    network.deliverAll();
    const after = await home.engine.replica.import();
    assert.deepEqual(after.missingAttachments, []);
    const delivered = right
      ? readFileSync(join(probe.directory, "peers/home", reference))
      : Buffer.from(network.syncFolder("home").read(reference)!);
    assert.deepEqual(delivered, bytes);
  }
  {
    const { network, office, home } = setup(true, false);
    const first = office.engine.replica.save({ page, base: [], path: "Plan.md", text: "first" });
    const { pending: selected } = probe.request<{ pending: { name: string; data: number[] } }>({ op: "select", peer: "office", now: clock() });
    const second = office.engine.replica.save({ page, base: [first.revision.id], path: "Plan.md", text: "saved during publication" });
    assert.equal(second.replaced, undefined);
    const decoded = decodeBatch(selected.name, Uint8Array.from(selected.data), { notebook: "nb-test", device: "office" });
    assert(decoded.ok);
    assert.deepEqual(decoded.value.revisions.map((revision) => revision.id), [first.revision.id]);
    network.syncFolder("office").write(`devices/office/log/${selected.name}`, Uint8Array.from(selected.data));
    probe.request({ op: "uncertain", peer: "office" });
    probe.request({ op: "reopen", peer: "office" });
    assert.deepEqual(office.engine.store.outbox(), [first.revision.id, second.revision.id]);
    network.deliverAll();
    await home.engine.replica.import();
    assert.equal(home.engine.replica.head(page)?.id, first.revision.id);
    await office.engine.replica.publish();
    assert.deepEqual(office.engine.store.outbox(), []);
    network.deliverAll();
    await home.engine.replica.import();
    assert.equal(home.engine.replica.head(page)?.id, second.revision.id);
    assert.equal(network.syncFolder("office").list("devices/office/log").length, 2);
  }
  for (const [left, right] of [[true, false], [false, true]]) {
    const { network, office, home } = setup(left, right);
    const first = office.engine.replica.save({ page, base: [], path: "Plan.md", text: "original" });
    await office.engine.replica.publish();
    network.deliverAll();
    await home.engine.replica.import();
    const deleted = office.engine.replica.save({ page, base: [first.revision.id], path: "Plan.md", text: "", deleted: true });
    await office.engine.replica.publish();
    network.deliverAll();
    await home.engine.replica.import();
    const stale = home.engine.replica.save({ page, base: [first.revision.id], path: "Plan.md", text: "stale window edit" });
    // The stale save merges at once, and the edit outlives the deletion.
    assert.equal(stale.conflict, false);
    const merged = stale.merged!;
    assert.deepEqual(new Set(merged.parents), new Set([deleted.revision.id, stale.revision.id]));
    assert.equal(merged.deleted, false);
    assert.equal(merged.text, "stale window edit");
    assert.deepEqual(home.engine.replica.heads(page).map((head) => head.id), [merged.id]);
    await home.engine.replica.publish();
    network.deliverAll();
    await office.engine.replica.import();
    assert.equal(office.engine.replica.head(page)?.id, merged.id);
    assert.deepEqual(office.engine.replica.conflicts(), []);
  }
  {
    probe.request({ op: "reset" });
    const network = new Network(random(5));
    for (const name of ["office", "copy"]) network.view(name);
    const office = nativeEngine(probe, "office")(network.syncFolder("office"), clock);
    const copy = nativeEngine(probe, "office", "nb-test", "office-copy")(network.syncFolder("copy"), clock);
    office.replica.save({ page, base: [], path: "Plan.md", text: "original writer" });
    const pending = copy.replica.save({ page, base: [], path: "Plan.md", text: "copied writer" });
    await office.replica.publish();
    network.deliverAll();
    await assert.rejects(copy.replica.publish(), /conflict|distinct writer|unrecorded/i);
    assert.deepEqual(copy.store.outbox(), [pending.revision.id]);
    assert.equal(copy.store.get(pending.revision.id)?.text, "copied writer");
    assert.equal(network.syncFolder("office").list("devices/office/log").length, 1);
  }
  // rt 71b62cba83e3: each runtime merges the same heads on import, independently, to one revision ID.
  for (const [left, right] of [[true, false], [false, true]]) {
    const { network, office, home } = setup(left, right);
    const start = office.engine.replica.save({ page, base: [], path: "Plan.md", text: "- start\n" });
    await office.engine.replica.publish();
    network.deliverAll();
    await home.engine.replica.import();
    home.engine.replica.save({ page, base: [start.revision.id], path: "Plan.md", text: "- start\n- home\n" });
    office.engine.replica.save({ page, base: [start.revision.id], path: "Plan.md", text: "- [x] start\n" });
    await home.engine.replica.publish();
    await office.engine.replica.publish();
    network.deliverAll();
    await home.engine.replica.import();
    await office.engine.replica.import();
    const merged = home.engine.replica.head(page)!;
    assert.equal(merged.text, "- [x] start\n- home\n");
    assert.equal(office.engine.replica.head(page)?.id, merged.id);
    assert.deepEqual(office.engine.store.outbox(), [merged.id]);
    assert.deepEqual(home.engine.store.outbox(), [merged.id]);
  }
  // rt c3a036587871: a save made while a batch is being published keeps the version in that batch.
  for (const [left, right] of [[true, false], [true, true]]) {
    const { network, office, home } = setup(left, right);
    const one = office.engine.replica.save({ page, base: [], path: "Plan.md", text: "- one\n" });
    await office.engine.replica.publish();
    const two = office.engine.replica.save({ page, base: [one.revision.id], path: "Plan.md", text: "- two\n" });
    probe.request({ op: "select", peer: "office", now: clock() });
    const three = office.engine.replica.save({ page, base: [two.revision.id], path: "Plan.md", text: "- three\n" });
    assert.equal(three.replaced, undefined);
    await office.engine.replica.publish();
    await office.engine.replica.publish();
    assert(office.engine.store.get(two.revision.id));
    network.deliverAll();
    await home.engine.replica.import();
    const head = home.engine.replica.head(page)!;
    assert.equal(head.id, three.revision.id);
    home.engine.replica.save({ page, base: [head.id], path: "Plan.md", text: "- three\n- from home\n" });
    await home.engine.replica.publish();
    network.deliverAll();
    const report = await office.engine.replica.import();
    assert.deepEqual(report.waiting, []);
    assert.equal(office.engine.replica.head(page)?.text, "- three\n- from home\n");
  }
  // rt c3a036587871: a TypeScript computer reads back a revision it published but forgot.
  // Native stores never drop a selected revision, so only the TypeScript side forgets here.
  {
    const { network, office, home } = setup(false, true);
    const one = office.engine.replica.save({ page, base: [], path: "Plan.md", text: "- one\n" });
    await office.engine.replica.publish();
    (office.engine.store as MemoryReplicaStore).discard(one.revision.id);
    network.deliverAll();
    await home.engine.replica.import();
    home.engine.replica.save({ page, base: [one.revision.id], path: "Plan.md", text: "- one\n- from home\n" });
    await home.engine.replica.publish();
    network.deliverAll();
    const report = await office.engine.replica.import();
    assert.deepEqual(report.waiting, []);
    assert(office.engine.store.get(one.revision.id));
    assert.equal(office.engine.replica.head(page)?.text, "- one\n- from home\n");
  }
  return 12;
}
