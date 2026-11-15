//! Release history capture/encoding timing. Each invocation is one independent run.
use std::{hint::black_box, time::Instant};
use vel_buffer::{Buffer, ops};
fn main() {
    for (case, payload) in [("one_byte", "x".to_owned()), ("100_byte", "A history edit preserves Unicode boundaries, selections, ownership and saved points in this buffer.\n".to_owned())] {
        let mut buffer = Buffer::new(""); let view = buffer.new_view();
        for _ in 0..10_000 { ops::insert(&mut buffer, view, &payload); }
        assert_eq!(buffer.history_entries(), 10_000);
        let start=Instant::now();let snapshot=black_box(buffer.export_history());let export_ns=start.elapsed().as_nanos();
        let start=Instant::now();let json=serde_json::to_vec(&snapshot).unwrap();let serialize_ns=start.elapsed().as_nanos();
        let raw_bytes=payload.len()*10_000;
        println!("{{\"case\":{case:?},\"export_ns\":{export_ns},\"serialize_ns\":{serialize_ns},\"serialized_bytes\":{},\"raw_edit_bytes\":{raw_bytes}}}",json.len());
    }
}
