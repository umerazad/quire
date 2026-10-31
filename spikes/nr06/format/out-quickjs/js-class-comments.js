// Keeps a bounded cache
class Lru {
  constructor(limit = 10) {
    this.limit = limit;
    this.map = new Map();
  } // ctor
  get(k) {
    if (!this.map.has(k)) return undefined;
    const v = this.map.get(k);
    this.map.delete(k);
    this.map.set(k, v);
    return v;
  }
  /* evict oldest */ set(k, v) {
    this.map.delete(k);
    this.map.set(k, v);
    if (this.map.size > this.limit) {
      this.map.delete(this.map.keys().next().value);
    }
  }
}
export default Lru;
