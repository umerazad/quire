// Ranges queried for overlap many times while more are added, such as the
// ranges already replaced by widgets while decorating a page.
export class RangeSet {
  private starts: number[] = [];
  private ends: number[] = [];
  // maxEnds[i] is the largest end among the first i + 1 ranges; it is
  // extended lazily and truncated where a range is inserted.
  private maxEnds: number[] = [];
  add(from: number, to: number) {
    let low = 0,
      high = this.starts.length;
    while (low < high) {
      const middle = (low + high) >>> 1;
      if (this.starts[middle] <= from) low = middle + 1;
      else high = middle;
    }
    this.starts.splice(low, 0, from);
    this.ends.splice(low, 0, to);
    this.maxEnds.length = Math.min(this.maxEnds.length, low);
  }
  overlaps(from: number, to: number) {
    for (let i = this.maxEnds.length; i < this.starts.length; i++)
      this.maxEnds.push(
        Math.max(this.maxEnds[i - 1] ?? -Infinity, this.ends[i]),
      );
    let low = 0,
      high = this.starts.length;
    while (low < high) {
      const middle = (low + high) >>> 1;
      if (this.starts[middle] < to) low = middle + 1;
      else high = middle;
    }
    return low > 0 && this.maxEnds[low - 1] > from;
  }
}
