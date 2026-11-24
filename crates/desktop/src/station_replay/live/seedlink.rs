use super::detect::{Detector, Motion};
use super::{FRESH_MS, Reading, StationId};
use chrono::NaiveDate;
use futures_util::{SinkExt, StreamExt};
use std::borrow::Cow;
use std::collections::HashMap;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::{Duration, Instant};
use tokio::time::timeout;
use tokio_tungstenite::WebSocketStream;
use tungstenite::Message;
use tungstenite::handshake::{client::generate_key, derive_accept_key};
use tungstenite::protocol::{Role, WebSocketConfig};

const URL: &str = "https://rtserve.earthscope.org/seedlink";
const MAX_FRAME: usize = 1 << 20;
type Socket = WebSocketStream<reqwest::Upgraded>;
type Error = Box<dyn std::error::Error + Send + Sync>;

async fn connect() -> Result<Socket, Error> {
    // Let reqwest negotiate TLS and honor the user's HTTPS proxy, as it does
    // for the FDSN inventory. Switch to WebSocket framing after the upgrade.
    let client = reqwest::Client::builder()
        .http1_only()
        .redirect(reqwest::redirect::Policy::none())
        .build()?;
    let key = generate_key();
    let response = client
        .get(URL)
        .header("Connection", "Upgrade")
        .header("Upgrade", "websocket")
        .header("Sec-WebSocket-Version", "13")
        .header("Sec-WebSocket-Key", &key)
        .send()
        .await?;
    if response.status() != reqwest::StatusCode::SWITCHING_PROTOCOLS {
        return Err(format!("EarthScope WebSocket returned {}", response.status()).into());
    }
    if response
        .headers()
        .get("Sec-WebSocket-Accept")
        .and_then(|v| v.to_str().ok())
        != Some(derive_accept_key(key.as_bytes()).as_str())
    {
        return Err("Invalid WebSocket upgrade response".into());
    }
    let config = WebSocketConfig {
        max_message_size: Some(MAX_FRAME),
        max_frame_size: Some(MAX_FRAME),
        ..Default::default()
    };
    let mut ws =
        WebSocketStream::from_raw_socket(response.upgrade().await?, Role::Client, Some(config))
            .await;
    // SLPROTO is valid as the first command. DATA without a sequence number
    // starts at the live edge for every public station matching the wildcard.
    for command in [
        "SLPROTO 4.0",
        "STATION *",
        "SELECT *_?_H_Z.?D",
        "SELECT *_?_N_Z.?D",
        "DATA",
    ] {
        ws.send(Message::Text(format!("{command}\r\n"))).await?;
        let reply = ws
            .next()
            .await
            .ok_or("SeedLink closed during negotiation")??;
        if reply.to_text()?.trim() != "OK" {
            return Err(format!("SeedLink rejected {command}: {}", reply.to_text()?.trim()).into());
        }
    }
    ws.send(Message::Text("END\r\n".into())).await?;
    Ok(ws)
}

// Validate miniSEED framing before handing a network record to the codec.
// For v2, both the data offset and record size come from inside the payload.
fn record_bytes(format: u8, data: &[u8]) -> Option<Cow<'_, [u8]>> {
    if format == b'3' {
        return (data.starts_with(b"MS\x03")
            && data.len() >= 40
            && u32::from_le_bytes(data[24..28].try_into().unwrap()) <= 200_000)
            .then_some(Cow::Borrowed(data));
    }
    if data.len() < 48 || data.starts_with(b"MS\x03") {
        return None;
    }
    // The v2 codec expects a big-endian header. Some stations use a
    // little-endian header and specify the sample byte order separately.
    let little = !(1900..=2500).contains(&u16::from_be_bytes([data[20], data[21]]));
    let word = |i: usize| {
        let pair = [data[i], data[i + 1]];
        usize::from(if little {
            u16::from_le_bytes(pair)
        } else {
            u16::from_be_bytes(pair)
        })
    };
    if !(1900..=2500).contains(&word(20)) || word(28) >= 10_000 {
        return None;
    }
    let mut header = Cow::Borrowed(data);
    if little {
        let bytes = header.to_mut();
        for i in [20, 22, 28, 30, 32, 34, 44, 46] {
            bytes.swap(i, i + 1);
        }
        bytes[40..44].reverse();
    }
    let samples_at = word(44);
    if !(48..=data.len()).contains(&samples_at) {
        return None;
    }
    let mut offset = word(46);
    let mut has_size = false;
    while offset >= 48 && offset + 4 <= samples_at {
        let kind = word(offset);
        let next = word(offset + 2);
        if little {
            let bytes = header.to_mut();
            bytes.swap(offset, offset + 1);
            bytes.swap(offset + 2, offset + 3);
        }
        if kind == 1000 {
            if offset + 8 > samples_at
                || 1usize.checked_shl(u32::from(data[offset + 6])) != Some(data.len())
            {
                return None;
            }
            has_size = true;
        }
        if next == 0 {
            return has_size.then_some(header);
        }
        if next < offset + 4 {
            break;
        }
        offset = next;
    }
    None
}

#[derive(Default)]
struct Processor {
    channels: HashMap<String, Detector>,
    batch: HashMap<StationId, Reading>,
}

impl Processor {
    fn packet(&mut self, frame: &[u8], now: i64) {
        if frame.len() < 17 || !matches!(&frame[..4], b"SE2D" | b"SE3D") {
            return;
        }
        let length = u32::from_le_bytes(frame[4..8].try_into().unwrap()) as usize;
        let offset = 17 + frame[16] as usize;
        let Some(data) = frame.get(offset..) else {
            return;
        };
        if data.len() != length {
            return;
        }
        let Some(data) = record_bytes(frame[2], data) else {
            return;
        };
        let Ok(mut record) = miniseed_rs::decode(&data) else {
            return;
        };
        if frame[2] == b'3' && record.sample_rate < 0.0 {
            // miniSEED 3 permits the sample interval in seconds as a negative number.
            record.sample_rate = -1.0 / record.sample_rate;
        }
        if record.activity_flags & 1 != 0
            || record.data_quality_flags & 0x80 != 0
            || record.flags & 3 != 0
        {
            return;
        }
        let Some(id) =
            super::super::source::station_id(Some(&record.network), Some(&record.station))
        else {
            return;
        };
        if frame.get(17..offset) != Some(format!("{}_{}", id.network, id.code).as_bytes()) {
            return;
        }
        let channel = record.channel.as_bytes();
        if !matches!(channel, [_, b'H' | b'N', b'Z'])
            || !(0.05..=5000.).contains(&record.sample_rate)
            || record.samples.is_empty()
        {
            return;
        }
        let t = record.start_time;
        let Some(start) = NaiveDate::from_yo_opt(i32::from(t.year), u32::from(t.day))
            .and_then(|d| {
                d.and_hms_nano_opt(
                    t.hour.into(),
                    t.minute.into(),
                    t.second.into(),
                    t.nanosecond,
                )
            })
            .map(|t| t.and_utc().timestamp_millis())
        else {
            return;
        };
        // In miniSEED 2 the fixed header correction is in tenths of a
        // millisecond. Apply it only if the data producer hasn't already.
        let start = if frame[2] == b'2' && record.activity_flags & 2 == 0 {
            start + i64::from(i32::from_be_bytes(data[40..44].try_into().unwrap())) / 10
        } else {
            start
        };
        let end =
            start + ((record.samples.len() - 1) as f64 * 1000. / record.sample_rate).round() as i64;
        if !(now - FRESH_MS..=now + 2_000).contains(&end) {
            return;
        }
        let detector = self
            .channels
            .entry(record.source_id.to_string())
            .or_default();
        let motion = detector
            .record(&record, start)
            .filter(|m| (now - FRESH_MS..=now + 2_000).contains(&m.time_ms));
        let reading = self.batch.entry(id).or_default();
        reading.sample_ms = reading.sample_ms.max(end);
        if let Some(m) = motion
            && reading
                .motion
                .is_none_or(|old: Motion| old.time_ms < m.time_ms)
        {
            reading.motion = Some(m);
        }
    }
}

pub(super) async fn run(
    stop: &AtomicBool,
    mut publish: impl FnMut(HashMap<StationId, Reading>),
    connected: impl FnOnce(),
) -> Result<(), Error> {
    let handshake = timeout(Duration::from_secs(20), connect());
    tokio::pin!(handshake);
    let mut socket = loop {
        if stop.load(Ordering::Relaxed) {
            return Ok(());
        }
        if let Ok(result) = timeout(Duration::from_millis(250), &mut handshake).await {
            break result??;
        }
    };
    connected();
    let mut processor = Processor::default();
    let mut last_batch = Instant::now();
    let mut last_packet = Instant::now();
    let mut last_prune = Instant::now();
    while !stop.load(Ordering::Relaxed) {
        if let Ok(next) = timeout(Duration::from_secs(1), socket.next()).await {
            match next.ok_or("EarthScope closed the stream")?? {
                Message::Binary(bytes) => {
                    last_packet = Instant::now();
                    processor.packet(&bytes, chrono::Utc::now().timestamp_millis());
                }
                Message::Ping(_) => socket.flush().await?,
                Message::Close(_) => return Err("EarthScope closed the stream".into()),
                _ => {}
            }
        }
        if last_packet.elapsed() > Duration::from_secs(75) {
            return Err("No SeedLink packets for 75 seconds".into());
        }
        if last_batch.elapsed() >= Duration::from_secs(1) {
            if !processor.batch.is_empty() {
                publish(std::mem::take(&mut processor.batch));
            }
            last_batch = Instant::now();
        }
        if last_prune.elapsed() >= Duration::from_secs(60) {
            let cutoff = chrono::Utc::now().timestamp_millis() - FRESH_MS;
            processor.channels.retain(|_, d| d.last_ms >= cutoff);
            last_prune = Instant::now();
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    fn packet(payload: &[u8], station: &str, version: u8) -> Vec<u8> {
        let mut frame = b"SE2D".to_vec();
        frame[2] = version;
        frame.extend((payload.len() as u32).to_le_bytes());
        frame.extend([0; 8]);
        frame.push(station.len() as u8);
        frame.extend(station.as_bytes());
        frame.extend(payload);
        frame
    }

    #[test]
    fn maps_v2_and_v3_samples_by_station_and_rejects_stale_records() {
        use miniseed_rs::{MseedRecord, Samples, encode};
        let when = 10_000;
        for (version, record) in [(b'2', MseedRecord::new()), (b'3', MseedRecord::new_v3())] {
            let record = record
                .with_nslc("IU", "ANMO", "00", "BHZ")
                .with_sample_rate(20.)
                .with_samples(Samples::Int(vec![3; 40]));
            let data = encode(&record).unwrap();
            let frame = packet(&data, "IU_ANMO", version);
            let mut processor = Processor::default();
            processor.packet(&frame, when + FRESH_MS);
            assert!(processor.batch.is_empty());
            processor.packet(&packet(&data, "IU_OTHER", version), when);
            assert!(processor.batch.is_empty());
            processor.packet(&frame, when);
            assert_eq!(processor.batch.len(), 1);
            let reading = processor.batch.values().next().unwrap();
            assert_eq!(reading.sample_ms, 1950);
            assert!(reading.motion.is_none());
            if version == b'2' {
                let mut little = data.clone();
                let block = u16::from_be_bytes(data[46..48].try_into().unwrap()) as usize;
                for i in [20, 22, 28, 30, 32, 34, 44, 46, block, block + 2] {
                    little.swap(i, i + 1);
                }
                little[40..44].copy_from_slice(&500_i32.to_le_bytes());
                let mut alternate = Processor::default();
                alternate.packet(&packet(&little, "IU_ANMO", version), when);
                assert_eq!(alternate.batch.values().next().unwrap().sample_ms, 2000);
            }
        }
    }

    #[test]
    #[ignore = "downloads an archived FDSN waveform"]
    fn flags_recorded_motion_at_sspa_during_the_new_jersey_earthquake() {
        let uri = "https://service.earthscope.org/fdsnws/dataselect/1/query?net=IU&sta=SSPA&loc=00&cha=BHZ&starttime=2024-04-05T14:20:00&endtime=2024-04-05T14:27:00";
        let bytes = super::super::super::source::client()
            .unwrap()
            .get(uri)
            .send()
            .unwrap()
            .error_for_status()
            .unwrap()
            .bytes()
            .unwrap();
        let origin = 1_712_327_000_045_i64; // 2024-04-05 14:23:20.045 UTC
        let mut processor = Processor::default();
        let mut onsets = Vec::new();
        // This IU.SSPA recording consists of 512-byte miniSEED 2 records.
        for raw in bytes.chunks_exact(512) {
            let decoded = miniseed_rs::decode(raw).unwrap();
            let t = decoded.start_time;
            let now = NaiveDate::from_yo_opt(t.year.into(), t.day.into())
                .unwrap()
                .and_hms_nano_opt(
                    t.hour.into(),
                    t.minute.into(),
                    t.second.into(),
                    t.nanosecond,
                )
                .unwrap()
                .and_utc()
                .timestamp_millis()
                + 10_000;
            processor.packet(&packet(raw, "IU_SSPA", b'2'), now);
            for (_, r) in processor.batch.drain() {
                if let Some(hit) = r.motion {
                    onsets.push(hit.time_ms);
                }
            }
        }
        eprintln!("IU.SSPA observed onsets: {onsets:?}");
        assert!(
            onsets
                .iter()
                .any(|&t| (origin..origin + 150_000).contains(&t))
        );
    }

    #[test]
    #[ignore = "requires the public EarthScope SeedLink WebSocket"]
    fn earthscope_stream_delivers_fresh_waveforms_from_multiple_stations() {
        let rt = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .unwrap();
        let stop = AtomicBool::new(false);
        let mut ids = std::collections::HashSet::new();
        rt.block_on(async {
            timeout(
                Duration::from_secs(40),
                run(
                    &stop,
                    |batch| {
                        ids.extend(batch.into_keys());
                        if ids.len() >= 10 {
                            stop.store(true, Ordering::Relaxed);
                        }
                    },
                    || (),
                ),
            )
            .await
            .expect("waveform receive timeout")
            .unwrap();
        });
        assert!(ids.len() >= 10);
        eprintln!(
            "{} stations delivered fresh decoded vertical waveforms",
            ids.len()
        );
    }
}
