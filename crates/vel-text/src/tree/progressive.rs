//! Prefix publication, isolated from the ordinary rope loader/edit paths.
use super::*;

// Snapshot variant of the same bounded pipeline. Keep the ordinary loader's hot
// path unchanged; this path packs completed leaves into shared subtree prefixes.
pub(crate) fn from_reader_snapshots(
    mut reader: impl io::Read,
    publish_bytes: usize,
    interval: std::time::Duration,
    mut publish: impl FnMut(Arc<Node>) -> io::Result<()>,
) -> io::Result<Arc<Node>> {
    const BLOCK: usize = 2 * 1024 * 1024;
    let workers = std::thread::available_parallelism()
        .map_or(1, usize::from)
        .min(4);
    std::thread::scope(|scope| {
        let mut inputs = Vec::new();
        let mut outputs = Vec::new();
        for _ in 0..workers {
            let (send, receive) = std::sync::mpsc::sync_channel::<(Vec<u8>, usize)>(1);
            let (result, output) = std::sync::mpsc::channel();
            scope.spawn(move || {
                while let Ok((buf, len)) = receive.recv() {
                    let leaves = simdutf8::basic::from_utf8(&buf[..len])
                        .map_err(|e| io::Error::new(io::ErrorKind::InvalidData, e))
                        .map(load_leaves);
                    if result.send((buf, leaves)).is_err() {
                        break;
                    }
                }
            });
            inputs.push(send);
            outputs.push(output);
        }
        let mut prefix = Node::leaf(String::new());
        let mut pending = Vec::new();
        let mut pending_bytes = 0;
        let mut first = true;
        let mut last_time = std::time::Instant::now();
        let mut collect = |nodes: Vec<Arc<Node>>, final_batch: bool| -> io::Result<()> {
            pending_bytes += nodes.iter().map(|n| n.summary.bytes).sum::<usize>();
            pending.extend(nodes);
            if final_batch
                || (pending_bytes != 0
                    && (first || pending_bytes >= publish_bytes || last_time.elapsed() >= interval))
            {
                repair(&mut pending);
                let next = root_from_nodes(std::mem::take(&mut pending));
                prefix = concat(&prefix, &next);
                publish(prefix.clone())?;
                pending_bytes = 0;
                first = false;
                last_time = std::time::Instant::now();
            }
            Ok(())
        };
        let mut carry = Vec::<u8>::new();
        let mut submitted = 0;
        let mut collected = 0;
        let mut serial_blocks = 0;
        let mut spare = None;
        loop {
            let slot = submitted % workers;
            let mut buf = if let Some(buf) = spare.take() {
                buf
            } else if submitted >= workers {
                let (buf, leaves) = outputs[slot].recv().map_err(io::Error::other)?;
                collect(leaves?.into_iter().map(loaded_node).collect(), false)?;
                collected += 1;
                buf
            } else {
                vec![0; BLOCK]
            };
            buf[..carry.len()].copy_from_slice(&carry);
            let mut len = carry.len();
            let mut eof = false;
            while len < BLOCK {
                match reader.read(&mut buf[len..]) {
                    Ok(0) => {
                        eof = true;
                        break;
                    }
                    Ok(n) => len += n,
                    Err(e) if e.kind() == io::ErrorKind::Interrupted => {}
                    Err(e) => return Err(e),
                }
            }
            let mut valid = len;
            if !eof && len > 0 {
                let mut lead = len - 1;
                while lead > 0 && buf[lead] & 0xc0 == 0x80 && len - lead <= 3 {
                    lead -= 1;
                }
                let width = match buf[lead] {
                    0xc2..=0xdf => 2,
                    0xe0..=0xef => 3,
                    0xf0..=0xf4 => 4,
                    _ => 1,
                };
                if width > len - lead {
                    valid = lead;
                }
            }
            carry.clear();
            carry.extend_from_slice(&buf[valid..len]);
            if valid > 0 {
                if serial_blocks < 2 {
                    // Keep the opening lines' indexes local to the editing thread.
                    let text = simdutf8::basic::from_utf8(&buf[..valid])
                        .map_err(|e| io::Error::new(io::ErrorKind::InvalidData, e))?;
                    let mut builder = Builder::default();
                    builder.push(text);
                    collect(builder.leaves(), false)?;
                    serial_blocks += 1;
                    spare = Some(buf);
                } else {
                    inputs[slot].send((buf, valid)).map_err(io::Error::other)?;
                    submitted += 1;
                }
            }
            if eof {
                break;
            }
        }
        drop(inputs);
        while collected < submitted {
            let (_, leaves) = outputs[collected % workers]
                .recv()
                .map_err(io::Error::other)?;
            collect(leaves?.into_iter().map(loaded_node).collect(), false)?;
            collected += 1;
        }
        collect(Vec::new(), true)?;
        Ok(prefix)
    })
}

// Join only the touching spines. Root occupancy is relaxed; repair before making
// either old root a non-root child, preserving the ordinary editing invariants.
pub(super) fn concat(left: &Arc<Node>, right: &Arc<Node>) -> Arc<Node> {
    fn join(left: &Arc<Node>, right: &Arc<Node>) -> Vec<Arc<Node>> {
        if left.height == right.height {
            let mut nodes = vec![left.clone(), right.clone()];
            repair(&mut nodes);
            return nodes;
        }
        let (tall, short, append) = if left.height > right.height {
            (left, right, true)
        } else {
            (right, left, false)
        };
        let Kind::Branch(children) = &tall.kind else {
            unreachable!("taller tree has children")
        };
        let edge = if append { children.len() - 1 } else { 0 };
        let joined = if append {
            join(&children[edge].node, short)
        } else {
            join(short, &children[edge].node)
        };
        let mut nodes: Vec<_> = children.iter().map(|c| c.node.clone()).collect();
        nodes.splice(edge..=edge, joined);
        repair(&mut nodes);
        groups(nodes, FANOUT)
            .into_iter()
            .map(Node::branch)
            .collect()
    }
    if left.summary.bytes == 0 {
        return right.clone();
    }
    if right.summary.bytes == 0 {
        return left.clone();
    }
    root_from_nodes(join(left, right))
}
