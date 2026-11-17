//! What Dutchman can map of each source feature, found without SolidCraft and without writing:
//! the history mapping alone, with every step that only SolidCraft can confirm named as such
//! instead of being run.

use std::collections::BTreeMap;

use dutchman_core::{FeatureOutcome, FeatureStatus, Model, Report};
use serde_json::Value;

use crate::{history, repeats, SolidCraftWriter};

/// Why a feature that maps is kept only when SolidCraft confirms it while converting.
const INFERRED: &str = "its profile region is inferred from the sketch selection; SolidCraft's \
                        body check confirms it while converting";
const WHOLE_SKETCH: &str = "it takes the whole of a sketch with several profiles; SolidCraft's \
                            check confirms the result while converting";
const BLEND: &str = "its input edges are found on SolidCraft's replay of the history before it, \
                     while converting";
const PROFILE: &str = "its profile region is not checked against the source; SolidCraft's check \
                       of the converted history compares the result";

/// What mapping says of one source feature.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Verdict {
    /// It maps to SolidCraft features as it is.
    Mapped,
    /// It maps, but is kept only when SolidCraft confirms it while converting; the text says
    /// what SolidCraft checks.
    NeedsCheck(&'static str),
    /// SolidCraft needs no feature for it (a principal datum plane, a component insert).
    NotNeeded,
    /// It is left out: suppressed in the source, or it changes no geometry there.
    LeftOut,
    /// Dutchman cannot map it yet; the outcome's reason says why.
    NotSupported,
    /// It is not looked at because an earlier feature blocks the history.
    AfterBlocker,
}

/// One source feature: what mapping made of it and the verdict.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct FeatureVerdict {
    pub outcome: FeatureOutcome,
    pub verdict: Verdict,
}

/// The verdicts of one part's features.
#[derive(Debug)]
pub struct PartVerdicts<'a> {
    /// The part's file (or name) inside an assembly; `None` for a single part.
    pub part: Option<String>,
    /// The part's model, for listing its bodies and parameters.
    pub model: &'a Model,
    /// Its source features, in timeline order.
    pub features: Vec<FeatureVerdict>,
    /// Why none of its history maps, when none does and the features don't say.
    pub history: Option<String>,
}

impl SolidCraftWriter {
    /// What mapping says of each source feature of `model`, part by part, without running
    /// SolidCraft or writing anything. Parts an assembly repeats are listed once, as
    /// conversion writes them once.
    pub fn verdicts<'a>(&self, model: &'a Model) -> Vec<PartVerdicts<'a>> {
        let Some(assembly) = &model.assembly else {
            return vec![self.part_verdicts(model, model, None)];
        };
        if let Some(flat) = crate::flattened_assembly(model) {
            return vec![self.part_verdicts(&flat, model, None)];
        }
        let repeated = repeats::find(assembly);
        let mut parts = Vec::new();
        repeats::leaves(assembly, &repeated, &mut Vec::new(), &mut parts);
        parts
            .into_iter()
            .map(|part| {
                let file = part
                    .source_file
                    .clone()
                    .unwrap_or_else(|| part.name.clone());
                self.part_verdicts(part, part, Some(file))
            })
            .collect()
    }

    fn part_verdicts<'a>(
        &self,
        mapped: &Model,
        shown: &'a Model,
        part: Option<String>,
    ) -> PartVerdicts<'a> {
        // Each inferred profile candidate is accepted as SolidCraft's check would accept it,
        // so later features are mapped as converting maps them; the ids say which ones.
        let mut inferred = Vec::new();
        let mapping = history::map_with_profile_check(
            mapped,
            &mut Report::default(),
            self.resolved_version(),
            self.sketches,
            &mut |prefix, _, _| {
                if let Some(id) = prefix
                    .features
                    .last()
                    .and_then(|feature| feature.get("id"))
                    .and_then(Value::as_u64)
                {
                    inferred.push(id);
                }
                true
            },
        );
        let mut checks: BTreeMap<usize, &'static str> = BTreeMap::new();
        if let Ok(history) = &mapping.result {
            for pending in &history.blends {
                checks.insert(pending.outcome, BLEND);
            }
            for id in &history.unconfirmed {
                if let Some(&row) = history.sources.get(id) {
                    checks.insert(row, WHOLE_SKETCH);
                }
            }
            for id in &inferred {
                if let Some(&row) = history.sources.get(id) {
                    checks.insert(row, INFERRED);
                }
            }
        }
        let features: Vec<FeatureVerdict> = mapping
            .features
            .into_iter()
            .enumerate()
            .map(|(row, outcome)| {
                let verdict = match outcome.status {
                    FeatureStatus::Mapped => match checks.get(&row) {
                        Some(&why) => Verdict::NeedsCheck(why),
                        None if outcome.profile_unverified => Verdict::NeedsCheck(PROFILE),
                        None => Verdict::Mapped,
                    },
                    FeatureStatus::NotNeeded => Verdict::NotNeeded,
                    FeatureStatus::LeftOut => Verdict::LeftOut,
                    FeatureStatus::Blocked
                        if outcome.reason.as_deref() == Some(history::AFTER_BLOCKER) =>
                    {
                        Verdict::AfterBlocker
                    }
                    FeatureStatus::Blocked => Verdict::NotSupported,
                };
                FeatureVerdict { outcome, verdict }
            })
            .collect();
        PartVerdicts {
            part,
            model: shown,
            history: mapping.result.err(),
            features,
        }
    }
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use super::*;
    use crate::testing::{cube, feature, with};

    #[test]
    fn a_design_without_history_has_one_part_and_says_why_nothing_maps() {
        let model = cube();
        let parts = SolidCraftWriter::new().verdicts(&model);
        assert_eq!(parts.len(), 1);
        assert_eq!(parts[0].part, None);
        assert!(parts[0].features.is_empty());
        assert_eq!(
            parts[0].history.as_deref(),
            Some("the design carries no feature history")
        );
    }

    #[test]
    fn each_feature_gets_the_mappers_verdict() {
        let native = json!({"kind": "native", "value": {"kind": "X", "parameters": {}}});
        let mut suppressed = feature(2, 1, "Hidden", native.clone());
        suppressed["suppressed"] = json!(true);
        let model = with(
            cube(),
            json!([
                feature(
                    1,
                    0,
                    "XY",
                    json!({"kind": "datum_principal_plane", "value": {"plane": "top"}})
                ),
                suppressed,
                feature(3, 2, "Odd", native.clone()),
                feature(4, 3, "Later", native),
            ]),
            json!([]),
        );
        let parts = SolidCraftWriter::new().verdicts(&model);
        let verdicts: Vec<(Option<&str>, Verdict)> = parts[0]
            .features
            .iter()
            .map(|f| (f.outcome.name.as_deref(), f.verdict))
            .collect();
        assert_eq!(
            verdicts,
            [
                (Some("XY"), Verdict::NotNeeded),
                (Some("Hidden"), Verdict::LeftOut),
                (Some("Odd"), Verdict::NotSupported),
                (Some("Later"), Verdict::AfterBlocker),
            ]
        );
        assert!(parts[0].features[2].outcome.reason.is_some());
    }
}
