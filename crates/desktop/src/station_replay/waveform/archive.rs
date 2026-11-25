use super::{Block, StationId, source};

pub(super) fn load(
    station: &StationId,
    from: i64,
    to: i64,
    send: impl FnMut(Block) -> bool,
) -> Result<bool, String> {
    let utc = |time| {
        chrono::DateTime::from_timestamp_millis(time)
            .unwrap()
            .to_rfc3339_opts(chrono::SecondsFormat::Millis, true)
    };
    let request = source::client()?
        .get("https://service.earthscope.org/fdsnws/dataselect/1/query")
        .query(&[
            ("net", station.network.as_str()),
            ("sta", station.code.as_str()),
            ("loc", "*"),
            ("cha", "?HZ,?NZ"),
            ("start", &utc(from)),
            ("end", &utc(to)),
            ("format", "geocsv.tspair"),
            ("nodata", "204"),
        ]);
    let body = source::read(request, "EarthScope waveforms")?;
    Ok(parse(&body, station, send))
}

fn parse(text: &str, station: &StationId, mut send: impl FnMut(Block) -> bool) -> bool {
    let mut block: Option<Block> = None;
    let mut found = false;
    for line in text.lines().chain(std::iter::once("# dataset:")) {
        if line.starts_with("# dataset:") {
            if let Some(block) = block
                .take()
                .filter(|b| b.rate > 0.0 && !b.samples.is_empty())
            {
                found = true;
                if !send(block) {
                    break;
                }
            }
        } else if let Some(sid) = line.strip_prefix("# SID:") {
            let mut parts = sid.trim().split('_');
            if let (Some(net), Some(sta), Some(loc), Some(chan), None) = (
                parts.next(),
                parts.next(),
                parts.next(),
                parts.next(),
                parts.next(),
            ) && net == station.network
                && sta == station.code
                && matches!(chan.as_bytes(), [_, b'H' | b'N', b'Z'])
            {
                block = Some(Block {
                    channel: format!("{loc}.{chan}"),
                    rate: 0.,
                    samples: Vec::new(),
                });
            }
        } else if let Some(b) = &mut block {
            if let Some(rate) = line.strip_prefix("# sample_rate_hz:") {
                b.rate = rate.trim().parse().unwrap_or(0.);
            } else if let Some((time, value)) = line.split_once(',')
                && let Some(t) = source::parse_utc(time.trim())
                && let Ok(v) = value.trim().parse::<f64>()
                && v.is_finite()
            {
                b.samples.push((t.timestamp_millis(), v));
            }
        }
    }
    found
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn parses_time_stamped_samples_without_joining_channels() {
        let station = super::super::station("iu.sspa").unwrap();
        let raw = "# dataset: GeoCSV 2.0\n# SID: IU_SSPA_00_BHZ\n# sample_rate_hz: 20\nTime, Sample\n2024-04-05T14:23:40.050Z, -12\n# dataset: GeoCSV 2.0\n# SID: IU_OTHER_00_BHZ\n# sample_rate_hz: 20\n2024-04-05T14:23:40Z, 17";
        let mut blocks = Vec::new();
        assert!(parse(raw, &station, |b| {
            blocks.push(b);
            true
        }));
        assert_eq!(blocks.len(), 1);
        assert_eq!(blocks[0].channel, "00.BHZ");
        assert_eq!(blocks[0].samples, vec![(1712327020050, -12.)]);
    }

    #[test]
    #[ignore = "downloads the archived April 2024 New Jersey quake"]
    fn fetches_real_quake_trace() {
        let sta = super::super::station("IU.SSPA").unwrap();
        let mut count = 0;
        let mut has_deflection = false;
        assert!(
            load(&sta, 1712327010000, 1712327130000, |b| {
                count += b.samples.len();
                has_deflection |= b.samples.windows(2).any(|x| x[0].1 != x[1].1);
                true
            })
            .unwrap()
        );
        eprintln!("{count} archived samples from IU.SSPA");
        assert!(count > 100 && has_deflection);
    }
}
