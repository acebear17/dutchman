//! Bounded sketch-pick and extrude-scope candidates, admitted only by the source body check.

use super::{describe, map_feature, profile, stored_body_centers, Context, History, Mapped};
use dutchman_core::{Report, Severity};
use serde_json::{json, Value};

/// The existing body comparison supplied by the writer. `final_state` means no later
/// active feature builds geometry, so the final source is also this feature's state.
pub(crate) type Check<'a> =
    dyn FnMut(&History, Option<&[crate::verify::StoredBody]>, bool) -> bool + 'a;

const UNCONFIRMED: &str =
    "its inferred profile region could not be confirmed by the source body check";

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
    let scope = body_scope(feature, ctx);
    let mut candidates = candidates(feature, ctx);
    let inferred_profile = !candidates.is_empty();
    if !inferred_profile && scope.is_none() {
        return None;
    }
    let stored = stored_body_centers(feature);
    // Intermediate scope candidates need source measurements. Without them the writer's
    // check cannot confirm a scope; avoid constructing and replaying a futile prefix.
    let scope = scope.filter(|_| final_state || stored.is_some());
    if !inferred_profile && scope.is_some() {
        candidates.push(feature.clone());
    }
    if candidates.is_empty() {
        return None;
    }
    let mut mapping_failure = None;
    let mut checked = false;
    let trials: Vec<_> = scope
        .iter()
        .flat_map(|targets| {
            candidates
                .iter()
                .map(move |candidate| (candidate, Some(targets)))
        })
        .chain(
            candidates
                .iter()
                .filter(|_| inferred_profile)
                .map(|candidate| (candidate, None)),
        )
        .collect();
    for (candidate, targets) in trials {
        let mut trial = ctx.clone();
        let mut notes = Report::default();
        let mut preceding = history.features.clone();
        let mut mapped = match map_feature(candidate, &mut trial, &mut notes, &mut preceding) {
            Ok(Mapped::Feature(mapped)) => mapped,
            Err(why) => {
                mapping_failure.get_or_insert(why);
                continue;
            }
            Ok(_) => continue,
        };
        if let (Some(targets), Some(fields)) = (targets, mapped.as_object_mut()) {
            fields.insert("targets".to_owned(), json!(targets));
        }
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
        checked = true;
        if !check(&prefix, stored.as_deref(), final_state) {
            continue;
        }
        *ctx = trial;
        history.features = preceding;
        report.append(notes);
        if inferred_profile {
            report.push_coded(
                Severity::Degraded, describe(feature), "history/profile-region-inferred",
                "Dutchman inferred the profile region from the sketch selection; the source body check confirmed it",
            );
        }
        if targets.is_some() {
            report.push_coded(
                Severity::Info,
                describe(feature),
                "history/body-scope-confirmed",
                "The source body check confirmed the extrude's selected target bodies",
            );
        }
        return Some(Ok(Mapped::Feature(mapped)));
    }
    // Scope candidates never replace an existing mapping unless the source confirms them.
    // Keep profile inference's original failure behaviour, including on older targets.
    if !inferred_profile {
        return None;
    }
    Some(Err(if checked {
        UNCONFIRMED.to_owned()
    } else {
        mapping_failure.unwrap_or_else(|| UNCONFIRMED.to_owned())
    }))
}

/// CADIR extrudes carry modified body identities in their outputs, but no participant
/// operand. Try those identities as targets only when every one is already named. A join's
/// outputs need not name every consumed input, so even resolved identities require replay
/// against the source state before this candidate can be used. Never interpret native ids.
fn body_scope(feature: &Value, ctx: &Context) -> Option<Vec<String>> {
    if !crate::VersionGate::ScopedBodies.supported(ctx.target_version)
        || super::operation_kind(feature) != Some("extrude")
        || !matches!(
            super::operation(feature).get("op").and_then(Value::as_str),
            Some("join" | "cut" | "intersect")
        )
        || ctx.named_bodies.len() < 2
    {
        return None;
    }
    let outputs = feature.get("outputs")?.as_array()?;
    if outputs.is_empty() {
        return None;
    }
    // A multi-target join may consume names. CADIR outputs do not establish which name
    // survives each union, so leave those to the ordinary checked mapping.
    if outputs.len() > 1
        && super::operation(feature).get("op").and_then(Value::as_str) == Some("join")
    {
        return None;
    }
    let selection = json!({"kind": "bodies", "value": outputs});
    super::modify::body_names(&selection, ctx).ok()
}

/// Try the selected neutral curve identities first, then the whole carrier sketch. Native
/// strings are opaque: only exact matches to mapped entity ids can supply curve identities.
/// There are at most two trials, including when a loft has several unresolved sections.
fn candidates(feature: &Value, ctx: &Context) -> Vec<Value> {
    fn visit(
        value: &Value,
        path: String,
        depth: usize,
        ctx: &Context,
        refs: &mut Vec<(String, Value, Value)>,
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
            let mapped = ctx.sketches.get(sketch)?;
            let ids = value.pointer("/value/selections")?.as_array()?;
            let whole = json!({"kind": "sketch", "value": sketch});
            if ids.len() > 10_000 {
                return None;
            }
            let selected_ids: Vec<&Value> = ids
                .iter()
                .filter(|id| id.as_str().is_some_and(|id| mapped.curve(id).is_some()))
                .collect();
            let selected = if !selected_ids.is_empty() {
                json!({"kind": "sketch_entities", "value": {"sketch": sketch, "entities": selected_ids}})
            } else {
                whole.clone()
            };
            refs.push((path, selected, whole));
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
    if visit(
        feature.get("definition").unwrap_or(&Value::Null),
        "/definition".to_owned(),
        0,
        ctx,
        &mut refs,
    )
    .is_none()
        || refs.is_empty()
    {
        return Vec::new();
    }
    let mut out = Vec::new();
    for whole in [false, true] {
        let mut candidate = feature.clone();
        for (path, selected, all) in &refs {
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
