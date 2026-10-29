use gpui::{App, AppContext, Entity};
use language::Buffer;
use multi_buffer::MultiBuffer;
use std::ops::Range;
use text::{Bias, ToPoint};

pub fn buffer(
    source: Entity<Buffer>,
    range: Range<usize>,
    cx: &mut App,
) -> Result<Entity<MultiBuffer>, String> {
    let snapshot = source.read(cx).snapshot();
    if range.start > range.end
        || snapshot.clip_offset(range.start, Bias::Left) != range.start
        || snapshot.clip_offset(range.end, Bias::Left) != range.end
    {
        return Err("Cannot open this excerpt: its source range is out of bounds, reversed, or splits a UTF-8 character. Resolve the block again and retry.".into());
    }
    let range = range.start.to_point(&snapshot)..range.end.to_point(&snapshot);
    Ok(cx.new(|cx| MultiBuffer::excerpt(source, range, cx)))
}
