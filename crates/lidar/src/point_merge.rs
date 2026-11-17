//! Coverage-first union of original survey observations, with shared limits.
use super::*;

pub enum MergeEvent {
    Sources(Vec<Source>),
    Reading { source: Source, refining: bool },
    Data(Event),
}
#[derive(Clone, Debug, serde::Serialize)]
pub struct Receipt {
    pub source: Source,
    pub header: Option<Header>,
    pub points: usize,
    pub complete: bool,
    pub error: Option<String>,
}
#[derive(Clone, Debug, serde::Serialize)]
pub struct Report {
    pub total: Summary,
    pub surveys: Vec<Receipt>,
    pub unverified_heights: bool,
    pub survey_limit_reached: bool,
}
impl Report {
    pub fn contributors(&self) -> usize {
        self.surveys.iter().filter(|s| s.points > 0).count()
    }
}
/// A declared EPSG vertical reference includes its defined units. Missing,
/// custom or invalid declarations are unverified, never inferred from XY.
pub fn vertical_reference(header: &Header) -> Option<String> {
    let value = &header.crs["vertical"];
    let code = value
        .as_u64()
        .and_then(|v| u32::try_from(v).ok())
        .or_else(|| {
            value
                .as_str()
                .and_then(|s| s.trim().trim_start_matches("EPSG:").parse::<u32>().ok())
        })?;
    (code != 0 && code != 32767).then(|| format!("EPSG:{code}"))
}

pub fn load(
    catalog: &Path,
    region: Region,
    chosen: Option<Source>,
    active: Option<&Path>,
    max_points: usize,
    mut project: impl FnMut(&str, &mut [[f64; 2]]) -> Result<()>,
    mut emit: impl FnMut(MergeEvent) -> Result<()>,
) -> Result<Report> {
    let (mut candidates, limited) = super::candidates(catalog, region)?;
    if let Some(source) = &chosen
        && !candidates.iter().any(|s| s.id == source.id)
    {
        candidates.push(source.clone());
    }
    emit(MergeEvent::Sources(candidates.clone()))?;
    let merging = chosen.is_none();
    let selected = chosen.map_or(candidates, |s| vec![s]);
    let resources = StreamBudget::default();
    let mut report = run(
        selected,
        active,
        max_points,
        &resources,
        &mut |source, quota, cursor, limits, sink| {
            super::stream_resume(
                catalog,
                region,
                source,
                active,
                quota,
                limits,
                cursor,
                |crs, xy| project(crs, xy),
                sink,
            )
        },
        &mut emit,
    )?;
    if limited && merging {
        report.survey_limit_reached = true;
        report.total.complete = false;
        if !report.total.limit.is_empty() {
            report.total.limit.push_str(" · ");
        }
        report
            .total
            .limit
            .push_str("64 survey candidate limit reached; additional surveys may cover gaps");
    }
    Ok(report)
}

type Sink<'a> = dyn FnMut(Event) -> Result<()> + 'a;
#[allow(clippy::too_many_arguments)]
fn run(
    sources: Vec<Source>,
    active: Option<&Path>,
    max_points: usize,
    resources: &StreamBudget,
    drive: &mut impl FnMut(
        &Source,
        usize,
        &mut StreamCursor,
        &StreamBudget,
        &mut Sink<'_>,
    ) -> Result<Summary>,
    emit: &mut impl FnMut(MergeEvent) -> Result<()>,
) -> Result<Report> {
    if !(1..=16_000_000).contains(&max_points) {
        return Err("Invalid combined point budget".into());
    }
    if sources.is_empty() {
        return Err("No supported original point source is indexed here. DEM availability is reported separately.".into());
    }
    let mut report = Report {
        total: Summary {
            source: if sources.len() > 1 {
                "merged".into()
            } else {
                sources[0].id.clone()
            },
            ..Default::default()
        },
        surveys: sources
            .into_iter()
            .map(|source| Receipt {
                source,
                header: None,
                points: 0,
                complete: false,
                error: None,
            })
            .collect(),
        unverified_heights: false,
        survey_limit_reached: false,
    };
    let count = report.surveys.len();
    let mut cursors = (0..count)
        .map(|_| StreamCursor::default())
        .collect::<Vec<_>>();
    let mut anchor: Option<String> = None;
    let coverage_share = if count == 1 {
        max_points
    } else {
        (max_points / (count * 2)).max(1)
    };
    // First give every candidate a chance to contribute measured coverage.
    // Then resume unfinished readers; record cursors prevent duplicate replay.
    for refining in [false, true] {
        for (i, cursor) in cursors.iter_mut().enumerate() {
            check(active)?;
            if report.total.points >= max_points || resources.limit().is_some() {
                break;
            }
            let receipt = &mut report.surveys[i];
            if receipt.complete || receipt.error.is_some() {
                continue;
            }
            let quota = if refining {
                max_points - report.total.points
            } else {
                coverage_share.min(max_points - report.total.points)
            };
            let source = receipt.source.clone();
            emit(MergeEvent::Reading {
                source: source.clone(),
                refining,
            })?;
            let mut callback_error = None;
            let mut emitted_this_pass = 0;
            let result = drive(&source, quota, cursor, resources, &mut |event| {
                match &event {
                    Event::Header(header) => {
                        if let (Some(existing), Some(next)) = (&anchor, vertical_reference(header))
                            && *existing != next
                        {
                            return Err(format!(
                                "Different declared height references ({existing} / {next}); inspect this survey separately"
                            ));
                        }
                        receipt.header = Some(header.clone());
                    }
                    Event::Batch { points, .. } => {
                        if points.len() > max_points - report.total.points
                            || points.len() > quota.saturating_sub(emitted_this_pass)
                        {
                            return Err("Source exceeded the combined point budget".into());
                        }
                        if let Some(header) = &receipt.header {
                            match vertical_reference(header) {
                                Some(key) => {
                                    if anchor.is_none() {
                                        anchor = Some(key);
                                    }
                                }
                                None => report.unverified_heights = true,
                            }
                        } else {
                            return Err("Source emitted points without provenance".into());
                        }
                        receipt.points += points.len();
                        report.total.points += points.len();
                        emitted_this_pass += points.len();
                    }
                }
                if let Err(e) = emit(MergeEvent::Data(event)) {
                    callback_error = Some(e.clone());
                    return Err(e);
                }
                Ok(())
            });
            if let Some(error) = callback_error {
                return Err(error);
            }
            match result {
                Ok(summary) => receipt.complete = summary.complete && cursor.complete,
                Err(error) => receipt.error = Some(error),
            }
            // Source failures retain all prior batches and do not stop neighbors.
        }
    }
    report.total.decoded = resources.decoded.get();
    report.total.nodes = resources.nodes.get();
    report.total.transferred_bytes = resources.transferred.get();
    report.total.complete = report
        .surveys
        .iter()
        .all(|s| s.complete && s.error.is_none());
    report.total.limit = if report.total.complete {
        String::new()
    } else if let Some(reason) = resources.limit() {
        reason.into()
    } else if report.total.points >= max_points {
        "Combined point budget reached; raise the budget and refine".into()
    } else {
        "Some survey sources are unavailable or incomplete; available measurements retained".into()
    };
    if report.total.points == 0 {
        return Err(report
            .surveys
            .iter()
            .filter_map(|s| s.error.as_ref())
            .next()
            .cloned()
            .unwrap_or_else(|| "No original source returns lie within this circle".into()));
    }
    Ok(report)
}

#[cfg(test)]
mod tests {
    use super::*;
    fn source(id: &str) -> Source {
        Source {
            id: id.into(),
            name: id.into(),
            date: "2026".into(),
        }
    }
    fn header(s: &Source) -> Header {
        Header {
            source: s.clone(),
            publisher: format!("https://example.org/{}", s.id),
            crs: match s.id.as_str() {
                "right" => json!({"vertical":"5703"}),
                "feet" => json!({"vertical":6360}),
                _ => json!({"horizontal":"3857"}),
            },
        }
    }
    fn fake(
        s: &Source,
        quota: usize,
        cursor: &mut StreamCursor,
        _: &StreamBudget,
        sink: &mut Sink<'_>,
    ) -> Result<Summary> {
        if s.id == "failed" {
            return Err("Publisher unavailable".into());
        }
        sink(Event::Header(header(s)))?;
        let total = if s.id == "right" { 3 } else { 100 };
        let start = cursor.bulk as usize;
        let end = (start + quota).min(total);
        let points = (start..end)
            .map(|i| Point {
                source_xyz: [if s.id == "right" { 500. } else { -500. }, i as f64, 12.345],
                lon_lat: [0., 0.],
                classification: 2,
                intensity: 1234,
                return_number: 1,
                rgb: None,
            })
            .collect::<Vec<_>>();
        cursor.bulk = end as u64;
        cursor.complete = end == total;
        if !points.is_empty() {
            sink(Event::Batch {
                points,
                spacing_m: 1.,
            })?;
        }
        Ok(Summary {
            points: end - start,
            complete: cursor.complete,
            ..Default::default()
        })
    }
    #[test]
    fn coverage_pass_keeps_dense_first_survey_from_starving_neighbors_and_resumes_exactly() {
        let mut observations = Vec::new();
        let mut current = String::new();
        let report = run(
            vec![
                source("dense"),
                source("right"),
                source("failed"),
                source("feet"),
            ],
            None,
            24,
            &StreamBudget::default(),
            &mut fake,
            &mut |e| {
                match e {
                    MergeEvent::Data(Event::Header(h)) => current = h.source.id,
                    MergeEvent::Data(Event::Batch { points, .. }) => observations
                        .extend(points.into_iter().map(|p| (current.clone(), p.source_xyz))),
                    _ => {}
                }
                Ok(())
            },
        )
        .unwrap();
        assert_eq!(report.total.points, 24);
        assert_eq!(report.surveys[1].points, 3);
        assert_eq!(report.contributors(), 2);
        assert!(report.unverified_heights);
        assert!(
            report.surveys[2]
                .error
                .as_ref()
                .unwrap()
                .contains("unavailable")
        );
        assert!(
            report.surveys[3]
                .error
                .as_ref()
                .unwrap()
                .contains("Different declared")
        );
        assert!(observations.iter().all(|(_, p)| p[2] == 12.345));
        let unique = observations
            .iter()
            .map(|(id, p)| (id, p[0].to_bits(), p[1].to_bits(), p[2].to_bits()))
            .collect::<std::collections::HashSet<_>>();
        assert_eq!(unique.len(), observations.len());
        assert!(!report.total.complete);
        assert!(report.total.limit.contains("Combined point budget"));
    }
    #[test]
    fn shared_resource_limit_and_callback_cancellation_stop_the_whole_load() {
        let resources = StreamBudget {
            max_decoded: 5,
            ..Default::default()
        };
        let report = run(
            vec![source("dense"), source("right")],
            None,
            20,
            &resources,
            &mut |s, q, c, b, emit| {
                let result = fake(s, q, c, b, emit);
                b.decoded.set(5);
                result
            },
            &mut |_| Ok(()),
        )
        .unwrap();
        assert_eq!(report.total.points, 5);
        assert_eq!(report.surveys[1].points, 0);
        assert!(report.total.limit.contains("decoded-point"));
        let mut calls = 0;
        let err = run(
            vec![source("dense"), source("right")],
            None,
            20,
            &StreamBudget::default(),
            &mut |s, q, c, b, emit| {
                calls += 1;
                fake(s, q, c, b, emit)
            },
            &mut |e| {
                if matches!(e, MergeEvent::Data(_)) {
                    Err("Viewer closed".into())
                } else {
                    Ok(())
                }
            },
        )
        .unwrap_err();
        assert_eq!(err, "Viewer closed");
        assert_eq!(calls, 1);
    }
    #[test]
    fn declared_reference_keys_are_not_inferred_from_projected_xy_or_unknown_units() {
        let s = source("unknown");
        let mut h = header(&s);
        assert_eq!(vertical_reference(&h), None);
        h.crs["vertical"] = json!(5703);
        assert_eq!(vertical_reference(&h).as_deref(), Some("EPSG:5703"));
        h.crs["vertical"] = json!("EPSG:6360");
        assert_eq!(vertical_reference(&h).as_deref(), Some("EPSG:6360"));
        h.crs["vertical"] = json!(32767);
        assert_eq!(vertical_reference(&h), None);
    }
}
