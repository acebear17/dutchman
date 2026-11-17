//! Selections the source does not record, inferred by measuring candidates in SolidCraft.
//!
//! Two plausible readings of an unresolved sketch pick come first, each admitted by the
//! source body check as soon as one passes. When neither does, every region of the sketch
//! the pick can name is rebuilt in SolidCraft and measured against the source; the one
//! candidate that matches is kept, and a tie or no match keeps the refusal.

use std::time::{Duration, Instant};

use super::{describe, map_feature, profile, stored_body_centers, Context, History, Mapped};
use dutchman_core::{Inferred, InferredAgainst, InferredSelection, Report, Severity};
use serde_json::{json, Value};

/// What measuring one candidate's replay against the source found.
#[derive(Clone, Copy, Debug, PartialEq)]
pub(crate) enum Fit {
    /// It matches within the check's tolerances.
    Matches(Measure),
    /// It replays, and does not match.
    Differs,
    /// It could not be measured (no SolidCraft, a timeout, nothing stored to compare with).
    Unknown,
}

/// How a matching candidate was measured.
#[derive(Clone, Copy, Debug, PartialEq)]
pub(crate) struct Measure {
    pub(crate) against: InferredAgainst,
    /// The candidate's solid and the source's compared directly, when asked for and possible.
    pub(crate) deviation: Option<crate::verify::Deviation>,
}

/// The measurement the writer supplies: the existing body comparison (body count, box,
/// volume and area against the source's final shape, or the centres of the bodies the source
/// stores after the feature). `final_state` means no later active feature builds geometry, so
/// the final source is also this feature's state; `compare` also loads the source's solid
/// beside the replay, when it has one body, for the distance between their surfaces.
pub(crate) trait Host {
    fn fit(
        &mut self,
        prefix: &History,
        stored: Option<&[crate::verify::StoredBody]>,
        final_state: bool,
        compare: bool,
    ) -> Fit;

    /// The regions SolidCraft finds in the sketch feature `sketch` of `prefix`: each one's
    /// area and the point it names it by. `None` when SolidCraft cannot say.
    fn regions(&mut self, _prefix: &History, _sketch: u64) -> Option<Vec<SketchRegion>> {
        None
    }
}

/// A measurement alone, with no sketch regions: planning and tests.
impl<F> Host for F
where
    F: FnMut(&History, Option<&[crate::verify::StoredBody]>, bool, bool) -> Fit,
{
    fn fit(
        &mut self,
        prefix: &History,
        stored: Option<&[crate::verify::StoredBody]>,
        final_state: bool,
        compare: bool,
    ) -> Fit {
        self(prefix, stored, final_state, compare)
    }
}

/// The measuring host, as the mapper takes it.
pub(crate) type Check<'a> = dyn Host + 'a;

/// One region of a sketch as SolidCraft finds it: its area (mm²) and a point (in the sketch's
/// own coordinates) that names it, the centre of its area.
#[derive(Clone, Copy, Debug, PartialEq)]
pub(crate) struct SketchRegion {
    pub(crate) area: f64,
    pub(crate) point: [f64; 2],
}

const UNCONFIRMED: &str =
    "its inferred profile region could not be confirmed by the source body check";

/// Regions measured for one feature, at most. Each costs one SolidCraft replay of the history
/// up to the feature; the pull request that set this gives the measurements.
pub(crate) const MAX_CANDIDATES: usize = 24;

/// The time measuring the candidates of one feature may take, beyond which the feature keeps
/// its refusal.
pub(crate) const MEASURE_BUDGET: Duration = Duration::from_secs(120);

/// None means the ordinary mapping applies. Every trial has its own mapping state and
/// report, so a refused candidate cannot leave names, parameters, or helper sketches behind.
pub(super) fn map(
    feature: &Value,
    ctx: &mut Context,
    report: &mut Report,
    history: &mut History,
    params: &[Value],
    final_state: bool,
    check: &mut Check<'_>,
) -> Option<Result<Mapped, String>> {
    let first = candidates(feature, ctx);
    if first.is_empty() {
        return None;
    }
    let stored = stored_body_centers(feature);
    let mut mapping_failure = None;
    let mut tried = 0usize;
    for candidate in &first {
        let Some(trial) = Trial::map(candidate, ctx, history, params, &mut mapping_failure) else {
            continue;
        };
        tried += 1;
        let Fit::Matches(measure) = check.fit(&trial.prefix, stored.as_deref(), final_state, false)
        else {
            continue;
        };
        let mapped = trial.admit(ctx, history, report, measure, tried, 1);
        report.push_coded(
            Severity::Degraded,
            describe(feature),
            "history/profile-region-inferred",
            "Dutchman inferred the profile region from the sketch selection; the source body check confirmed it",
        );
        return Some(Ok(mapped));
    }
    if let Some(found) = measured(feature, ctx, report, history, params, final_state, check) {
        return Some(found);
    }
    Some(Err(if tried > 0 {
        UNCONFIRMED.to_owned()
    } else {
        mapping_failure.unwrap_or_else(|| UNCONFIRMED.to_owned())
    }))
}

/// One candidate mapped on a copy of the mapping state, with the history it would end.
struct Trial<'a> {
    ctx: Context<'a>,
    notes: Report,
    preceding: Vec<Value>,
    mapped: Value,
    prefix: History,
}

impl<'a> Trial<'a> {
    fn map(
        candidate: &Value,
        ctx: &Context<'a>,
        history: &History,
        params: &[Value],
        mapping_failure: &mut Option<String>,
    ) -> Option<Self> {
        let mut trial = ctx.clone();
        let mut notes = Report::default();
        let mut preceding = history.features.clone();
        let mut mapped = match map_feature(candidate, &mut trial, &mut notes, &mut preceding) {
            Ok(Mapped::Feature(mapped)) => mapped,
            Err(why) => {
                mapping_failure.get_or_insert(why);
                return None;
            }
            Ok(_) => return None,
        };
        let name = trial.unique_name(&mapped, preceding.iter());
        if let Some(fields) = mapped.as_object_mut() {
            fields.insert("name".to_owned(), json!(name));
        }
        let mut prefix = history.clone();
        prefix.features = preceding.clone();
        prefix.features.extend(trial.extra.iter().cloned());
        prefix.features.push(mapped.clone());
        prefix.params = params.to_vec();
        prefix.next_id = trial.next_id;
        prefix.looks = trial.looks.clone();
        prefix.default_bodies = trial.default_bodies.clone();
        super::params::unsigned_dimensions(&mut prefix, &trial);
        prefix.sheet_rules = trial
            .sheet_rules
            .iter()
            .map(|(thickness, name)| super::sheet::rule(name, *thickness))
            .collect();
        Some(Trial {
            ctx: trial,
            notes,
            preceding,
            mapped,
            prefix,
        })
    }

    /// Keep this candidate: its mapping state becomes the mapping's, and the feature is
    /// reported as inferred.
    fn admit(
        self,
        ctx: &mut Context<'a>,
        history: &mut History,
        report: &mut Report,
        measure: Measure,
        candidates: usize,
        matches: usize,
    ) -> Mapped {
        *ctx = self.ctx;
        ctx.inferred = Some(Inferred {
            selection: InferredSelection::ProfileRegion,
            candidates,
            matches,
            against: measure.against,
            deviation_nm: measure.deviation.and_then(|d| nanometres(d.max)),
        });
        history.features = self.preceding;
        report.append(self.notes);
        Mapped::Feature(self.mapped)
    }
}

/// A distance in whole nanometres, so the report compares equal.
pub(crate) fn nanometres(mm: f64) -> Option<u64> {
    let nm = (mm * 1e6).round();
    // Bounded first, so the conversion cannot saturate.
    (nm.is_finite() && (0.0..1e15).contains(&nm)).then_some(nm as u64)
}

/// Measure every region the pick can name, and keep the only one that matches: within the
/// check's tolerances and, where the solids were compared, the surface distance allowed. Of
/// several, only one with as many faces as the source's solid; otherwise it is a tie.
/// `None` when the feature has no such regions, so the earlier refusal stands.
fn measured(
    feature: &Value,
    ctx: &mut Context,
    report: &mut Report,
    history: &mut History,
    params: &[Value],
    final_state: bool,
    check: &mut Check<'_>,
) -> Option<Result<Mapped, String>> {
    let stored = stored_body_centers(feature);
    // Short of the final state, only the bodies the source stores after the feature can be
    // compared with: without them nothing could match.
    if !final_state && stored.is_none() {
        return None;
    }
    let features = match regions(feature, ctx, history, params, check)? {
        Ok(features) => features,
        Err(why) => return Some(Err(why)),
    };
    let total = features.len();
    let started = Instant::now();
    let mut found: Vec<(Trial, Measure)> = Vec::new();
    let mut measured = 0usize;
    let mut unknown = false;
    let mut mapping_failure = None;
    for candidate in &features {
        if started.elapsed() >= MEASURE_BUDGET {
            return Some(Err(format!(
                "its profile region could not be inferred: measuring {measured} of {total} \
                 candidate regions took the {} s allowed",
                MEASURE_BUDGET.as_secs()
            )));
        }
        let Some(trial) = Trial::map(candidate, ctx, history, params, &mut mapping_failure) else {
            continue;
        };
        measured += 1;
        match check.fit(&trial.prefix, stored.as_deref(), final_state, true) {
            Fit::Matches(measure) if measure.deviation.is_none_or(|d| d.within()) => {
                found.push((trial, measure));
            }
            Fit::Matches(_) | Fit::Differs => {}
            // Nothing measured at all (planning, or no SolidCraft): the earlier refusal stands.
            Fit::Unknown if measured == 1 => return None,
            Fit::Unknown => unknown = true,
        }
    }
    if unknown {
        return Some(Err(
            "its profile region could not be inferred: some candidate regions could not be \
             measured in SolidCraft, so no match is shown to be the only one"
                .to_owned(),
        ));
    }
    let matches = found.len();
    if matches > 1 {
        // Several candidates with the same body measures: only a face count that alone
        // agrees with the source's solid tells them apart.
        found.retain(|(_, measure)| {
            measure
                .deviation
                .and_then(|d| d.faces)
                .is_some_and(|(replayed, source)| replayed == source)
        });
        if found.len() != 1 {
            return Some(Err(format!(
                "its profile region could not be inferred: {matches} of {total} candidate \
                 regions match the source equally"
            )));
        }
    }
    let Some((trial, measure)) = found.pop() else {
        return Some(Err(if measured == 0 {
            mapping_failure.unwrap_or_else(|| UNCONFIRMED.to_owned())
        } else {
            format!(
                "its profile region could not be inferred: none of {measured} candidate regions \
                 matches the source"
            )
        }));
    };
    let against = match measure.against {
        InferredAgainst::OriginalShape => "the source's final shape",
        InferredAgainst::StoredBodies => "the bodies the source stores after it",
    };
    let mapped = trial.admit(ctx, history, report, measure, measured, matches);
    ctx.measured = true;
    report.push_coded(
        Severity::Degraded,
        describe(feature),
        "history/profile-region-measured",
        format!(
            "Dutchman inferred the profile region by rebuilding {measured} candidate regions in \
             SolidCraft; it is the only one that matches {against}"
        ),
    );
    Some(Ok(mapped))
}

/// The regions an unresolved native pick can name, as features selecting them: every choice of
/// as many loop regions as the pick has selections (one or two), the whole sketch aside, which
/// the first two readings already tried. `None` when the feature has no single unresolved
/// sketch pick whose sketch's loops alone bound its regions; the reason when there are more
/// regions than are measured.
fn regions(
    feature: &Value,
    ctx: &Context,
    history: &History,
    params: &[Value],
    check: &mut Check<'_>,
) -> Option<Result<Vec<Value>, String>> {
    let refs = unresolved(feature, ctx)?;
    let [(path, selection)] = refs.as_slice() else {
        return None;
    };
    let sketch = selection.pointer("/value/sketch")?.as_str()?;
    let picks = selection.pointer("/value/selections")?.as_array()?.len();
    if !(1..=2).contains(&picks) {
        return None;
    }
    let row = ctx.row("sketches", sketch)?;
    let entities = profile::sketch_entities(ctx, sketch);
    let Some(loops) = super::crossing::loop_regions(row, &entities) else {
        // Other curves divide the loops cadmpeg lists: SolidCraft's own regions of the
        // written sketch, each named by a point, are the candidates.
        return solidcraft_regions(feature, path, sketch, picks, ctx, history, params, check);
    };
    let count = loops.len();
    if count <= picks {
        return None;
    }
    let choices = match choices(count, picks) {
        Ok(choices) => choices,
        Err(why) => return Some(Err(why)),
    };
    let mut features = Vec::new();
    let mut seen: Vec<Vec<usize>> = Vec::new();
    for choice in choices {
        // Under even-odd nesting a union of regions is the loops in an odd number of them.
        let mut taken: Vec<usize> = Vec::new();
        for region in choice.iter().filter_map(|&index| loops.get(index)) {
            for &member in region {
                if let Some(at) = taken.iter().position(|&loop_index| loop_index == member) {
                    taken.remove(at);
                } else {
                    taken.push(member);
                }
            }
        }
        taken.sort_unstable();
        if taken.is_empty() || taken.len() == count || seen.contains(&taken) {
            continue;
        }
        let mut candidate = feature.clone();
        let slot = candidate.pointer_mut(path)?;
        *slot = json!({"kind": "sketch_profiles", "value": {"sketch": sketch, "profiles": taken}});
        seen.push(taken);
        features.push(candidate);
    }
    (!features.is_empty()).then_some(Ok(features))
}

/// Every choice of `picks` (one or two) of `count` regions, or the reason when there are more
/// than are measured.
fn choices(count: usize, picks: usize) -> Result<Vec<Vec<usize>>, String> {
    let mut choices: Vec<Vec<usize>> = Vec::new();
    if picks == 1 {
        choices.extend((0..count).map(|a| vec![a]));
    } else {
        for a in 0..count {
            for b in a + 1..count {
                choices.push(vec![a, b]);
            }
        }
    }
    if choices.len() > MAX_CANDIDATES {
        return Err(format!(
            "its profile region could not be inferred: its sketch has {} candidate regions, \
             more than the {MAX_CANDIDATES} measured",
            choices.len()
        ));
    }
    Ok(choices)
}

/// Candidates from the regions SolidCraft finds in the written sketch, each named by the
/// centre of its area. A centre outside its own region names another region or none: that
/// candidate then rebuilds another region's shape, or fails, and cannot add a wrong match.
#[allow(clippy::too_many_arguments)]
fn solidcraft_regions(
    feature: &Value,
    path: &str,
    sketch: &str,
    picks: usize,
    ctx: &Context,
    history: &History,
    params: &[Value],
    check: &mut Check<'_>,
) -> Option<Result<Vec<Value>, String>> {
    let id = ctx.sketches.get(sketch)?.feature;
    let mut prefix = history.clone();
    prefix.params = params.to_vec();
    prefix.next_id = ctx.next_id;
    prefix.looks = ctx.looks.clone();
    prefix.default_bodies = ctx.default_bodies.clone();
    super::params::unsigned_dimensions(&mut prefix, ctx);
    prefix.sheet_rules = ctx
        .sheet_rules
        .iter()
        .map(|(thickness, name)| super::sheet::rule(name, *thickness))
        .collect();
    let found = check.regions(&prefix, id)?;
    let count = found.len();
    if count <= picks {
        return None;
    }
    let choices = match choices(count, picks) {
        Ok(choices) => choices,
        Err(why) => return Some(Err(why)),
    };
    let mut features = Vec::new();
    let mut seen: Vec<Vec<[f64; 2]>> = Vec::new();
    for choice in choices {
        let points: Vec<[f64; 2]> = choice
            .iter()
            .filter_map(|&index| found.get(index))
            .map(|region| region.point)
            .collect();
        if points.len() != picks || seen.contains(&points) {
            continue;
        }
        let mut candidate = feature.clone();
        let slot = candidate.pointer_mut(path)?;
        *slot = json!({"kind": MEASURED_POINTS, "value": {"sketch": sketch, "points": points}});
        seen.push(points);
        features.push(candidate);
    }
    (!features.is_empty()).then_some(Ok(features))
}

/// The profile form of a measured candidate named by points in the written sketch: Dutchman's
/// own, never read from a source file.
pub(super) const MEASURED_POINTS: &str = "dutchman_measured_points";

/// The feature's unresolved sketch picks: each one's JSON pointer and its selection. `None`
/// when it has none, or a pick names no earlier mapped sketch.
fn unresolved(feature: &Value, ctx: &Context) -> Option<Vec<(String, Value)>> {
    fn visit(
        value: &Value,
        path: String,
        depth: usize,
        ctx: &Context,
        refs: &mut Vec<(String, Value)>,
    ) -> Option<()> {
        // The supported loft has at most fifty sections. Do not expand unbounded selections.
        if depth > 32 || refs.len() > 50 {
            return None;
        }
        if value.get("kind").and_then(Value::as_str) == Some("sketch_selection") {
            if profile::profile(Some(value), ctx).is_ok() {
                return Some(());
            }
            let sketch = value.pointer("/value/sketch")?.as_str()?;
            ctx.sketches.get(sketch)?;
            if value.pointer("/value/selections")?.as_array()?.len() > 10_000 {
                return None;
            }
            refs.push((path, value.clone()));
            return Some(());
        }
        match value {
            Value::Object(fields) => {
                for (key, child) in fields {
                    visit(
                        child,
                        format!("{path}/{}", key.replace('~', "~0").replace('/', "~1")),
                        depth + 1,
                        ctx,
                        refs,
                    )?;
                }
            }
            Value::Array(values) => {
                for (index, child) in values.iter().enumerate() {
                    visit(child, format!("{path}/{index}"), depth + 1, ctx, refs)?;
                }
            }
            _ => {}
        }
        Some(())
    }
    let mut refs = Vec::new();
    visit(
        feature.get("definition").unwrap_or(&Value::Null),
        "/definition".to_owned(),
        0,
        ctx,
        &mut refs,
    )?;
    (!refs.is_empty()).then_some(refs)
}

/// Try the selected neutral curve identities first, then the whole carrier sketch. Native
/// strings are opaque: only exact matches to mapped entity ids can supply curve identities.
/// There are at most two trials, including when a loft has several unresolved sections.
fn candidates(feature: &Value, ctx: &Context) -> Vec<Value> {
    let Some(refs) = unresolved(feature, ctx) else {
        return Vec::new();
    };
    let mut readings = Vec::new();
    for (path, selection) in refs {
        let Some(sketch) = selection.pointer("/value/sketch").and_then(Value::as_str) else {
            return Vec::new();
        };
        let (Some(ids), Some(mapped)) = (
            selection
                .pointer("/value/selections")
                .and_then(Value::as_array),
            ctx.sketches.get(sketch),
        ) else {
            return Vec::new();
        };
        let whole = json!({"kind": "sketch", "value": sketch});
        let selected_ids: Vec<&Value> = ids
            .iter()
            .filter(|id| id.as_str().is_some_and(|id| mapped.curve(id).is_some()))
            .collect();
        let selected = if !selected_ids.is_empty() {
            json!({"kind": "sketch_entities", "value": {"sketch": sketch, "entities": selected_ids}})
        } else {
            whole.clone()
        };
        readings.push((path, selected, whole));
    }
    let mut out = Vec::new();
    for whole in [false, true] {
        let mut candidate = feature.clone();
        for (path, selected, all) in &readings {
            let Some(slot) = candidate.pointer_mut(path) else {
                return Vec::new();
            };
            *slot = if whole { all } else { selected }.clone();
        }
        if !out.contains(&candidate) {
            out.push(candidate);
        }
    }
    out
}

/// Blend edges whose proof failed, inferred when the blend is the last feature that builds
/// geometry: each combination of nearby rebuilt edges ([`super::blend::candidates`]) is
/// written and the complete replay measured against the source's final shape. The one that
/// matches is kept, as for profile regions. `None` keeps the proof's refusal: the blend is not
/// last, or no candidate edges are near enough.
pub(super) fn edges(
    history: &History,
    at: usize,
    source: usize,
    pending: &super::blend::Pending,
    bodies: &crate::verify::ReplayedBodies,
    measure: &mut dyn FnMut(&History) -> Fit,
) -> Option<Result<(Vec<super::blend::Selection>, Inferred), String>> {
    let last = history
        .features
        .iter()
        .skip(at.saturating_add(1))
        .all(|feature| feature.get("type").and_then(Value::as_str) == Some("sketch"));
    if !last {
        return None;
    }
    let candidates = super::blend::candidates(pending, bodies, MAX_CANDIDATES)?;
    let total = candidates.len();
    let started = Instant::now();
    let mut found = Vec::new();
    let mut unknown = false;
    for (index, selections) in candidates.into_iter().enumerate() {
        if started.elapsed() >= MEASURE_BUDGET {
            return Some(Err(format!(
                "its edges could not be inferred: measuring {index} of {total} candidate edge \
                 selections took the {} s allowed",
                MEASURE_BUDGET.as_secs()
            )));
        }
        let mut trial = history.clone();
        if super::split_blend(&mut trial, at, source, selections.clone()).is_err() {
            continue;
        }
        match measure(&trial) {
            Fit::Matches(measure) if measure.deviation.is_none_or(|d| d.within()) => {
                found.push((selections, measure));
            }
            Fit::Matches(_) | Fit::Differs => {}
            Fit::Unknown if index == 0 => return None,
            Fit::Unknown => unknown = true,
        }
    }
    if unknown {
        return Some(Err(
            "its edges could not be inferred: some candidate edge selections could not be \
             measured in SolidCraft, so no match is shown to be the only one"
                .to_owned(),
        ));
    }
    let matches = found.len();
    if matches > 1 {
        found.retain(|(_, measure)| {
            measure
                .deviation
                .and_then(|d| d.faces)
                .is_some_and(|(replayed, source)| replayed == source)
        });
        if found.len() != 1 {
            return Some(Err(format!(
                "its edges could not be inferred: {matches} of {total} candidate edge \
                 selections match the source equally"
            )));
        }
    }
    let Some((selections, measure)) = found.pop() else {
        return Some(Err(format!(
            "its edges could not be inferred: none of {total} candidate edge selections \
             matches the source"
        )));
    };
    Some(Ok((
        selections,
        Inferred {
            selection: InferredSelection::Edges,
            candidates: total,
            matches,
            against: measure.against,
            deviation_nm: measure.deviation.and_then(|d| nanometres(d.max)),
        },
    )))
}
