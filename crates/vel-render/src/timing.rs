//! Deferred GPU timestamp readback, adapted from the MIT/Apache P0 prototype.
use std::{sync::mpsc, time::Duration};
pub(crate) const MAX_MEASURED_FRAMES: u32 = 600;
#[derive(Debug)]
pub(crate) struct Queries {
    pub(crate) set: wgpu::QuerySet,
    pub(crate) resolve: wgpu::Buffer,
    pub(crate) readback: wgpu::Buffer,
    period: f32,
    pub(crate) recorded: u32,
}
impl Queries {
    pub(crate) fn new(device: &wgpu::Device, queue: &wgpu::Queue) -> Self {
        Self {
            set: device.create_query_set(&wgpu::QuerySetDescriptor {
                label: Some("600 render-pass timestamp pairs"),
                ty: wgpu::QueryType::Timestamp,
                count: MAX_MEASURED_FRAMES * 2,
            }),
            resolve: device.create_buffer(&wgpu::BufferDescriptor {
                label: Some("aligned query resolve"),
                size: MAX_MEASURED_FRAMES as u64 * 256,
                usage: wgpu::BufferUsages::QUERY_RESOLVE | wgpu::BufferUsages::COPY_SRC,
                mapped_at_creation: false,
            }),
            readback: device.create_buffer(&wgpu::BufferDescriptor {
                label: Some("deferred packed timestamp readback"),
                size: MAX_MEASURED_FRAMES as u64 * 16,
                usage: wgpu::BufferUsages::COPY_DST | wgpu::BufferUsages::MAP_READ,
                mapped_at_creation: false,
            }),
            period: queue.get_timestamp_period(),
            recorded: 0,
        }
    }
    pub(crate) fn read(&self, device: &wgpu::Device) -> Result<Vec<f64>, String> {
        let (tx, rx) = mpsc::channel();
        self.readback
            .slice(..)
            .map_async(wgpu::MapMode::Read, move |r| {
                let _ = tx.send(r);
            });
        device
            .poll(wgpu::PollType::Wait {
                submission_index: None,
                timeout: Some(Duration::from_secs(15)),
            })
            .map_err(|e| e.to_string())?;
        rx.recv_timeout(Duration::from_secs(1))
            .map_err(|e| e.to_string())?
            .map_err(|e| e.to_string())?;
        let bytes = self
            .readback
            .slice(..)
            .get_mapped_range()
            .map_err(|e| e.to_string())?;
        let times = decode_timestamps(&bytes, self.recorded as usize, self.period)?;
        drop(bytes);
        self.readback.unmap();
        Ok(times)
    }
}
/// Convert native GPU ticks to milliseconds; zero/reversed timestamps are explicit errors.
pub fn decode_timestamps(bytes: &[u8], count: usize, period: f32) -> Result<Vec<f64>, String> {
    if !period.is_finite() || period <= 0. || bytes.len() < count * 16 {
        return Err("invalid timestamp buffer/period".into());
    }
    bytes
        .as_chunks::<16>()
        .0
        .iter()
        .take(count)
        .map(|b| {
            let a = u64::from_ne_bytes(b[..8].try_into().unwrap());
            let z = u64::from_ne_bytes(b[8..16].try_into().unwrap());
            if z < a || (a == 0 && z == 0) {
                Err(format!(
                    "unavailable/nonmonotonic GPU timestamp pair: {a}, {z}"
                ))
            } else {
                Ok((z - a) as f64 * period as f64 / 1_000_000.)
            }
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn timestamp_readback_rejects_missing_data_and_preserves_device_period() {
        let bytes: Vec<u8> = [100u64, 2100, 5000, 9000]
            .into_iter()
            .flat_map(u64::to_ne_bytes)
            .collect();
        assert_eq!(decode_timestamps(&bytes, 2, 2.).unwrap(), [0.004, 0.008]);
        assert!(decode_timestamps(&bytes, 3, 2.).is_err());
        assert!(decode_timestamps(&[0; 16], 1, 1.).is_err());
        let reversed: Vec<u8> = [100u64, 99]
            .into_iter()
            .flat_map(u64::to_ne_bytes)
            .collect();
        assert!(decode_timestamps(&reversed, 1, 1.).is_err());
        assert!(decode_timestamps(&bytes, 2, f32::NAN).is_err());
    }
}
