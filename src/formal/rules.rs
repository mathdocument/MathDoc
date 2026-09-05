//! Pure attestation dependency propagation; no files or database access.

use crate::core::FormalCodeStatus;
use anyhow::{bail, Result};
use std::collections::{BTreeMap, HashMap};

pub(super) struct Candidate {
    pub(super) token: String,
    pub(super) artifact_sha256: String,
    pub(super) dependencies: BTreeMap<String, String>,
}

pub(super) struct LanguageState {
    pub(super) status: FormalCodeStatus,
    pub(super) candidate: Option<Candidate>,
}

pub(super) struct EvaluatedNode {
    pub(super) lean: LanguageState,
    pub(super) rocq: LanguageState,
}

pub(super) fn propagate_verified(
    states: &mut [EvaluatedNode],
    index_by_fnode: &HashMap<String, usize>,
    language: &str,
) -> Result<()> {
    let mut remaining = vec![None; states.len()];
    let mut referrers = vec![Vec::new(); states.len()];
    for (index, state) in states.iter().enumerate() {
        let Some(candidate) = language_state(state, language)?.candidate.as_ref() else {
            continue;
        };
        let mut dependencies = Vec::with_capacity(candidate.dependencies.len());
        let valid = candidate.dependencies.iter().all(|(fnode, token)| {
            let Some(dependency_index) = index_by_fnode.get(fnode).copied() else {
                return false;
            };
            let token_matches = language_state(&states[dependency_index], language)
                .ok()
                .and_then(|state| state.candidate.as_ref())
                .is_some_and(|dependency| dependency.token == *token);
            if token_matches {
                dependencies.push(dependency_index);
            }
            token_matches
        });
        if valid {
            remaining[index] = Some(dependencies.len());
            for dependency in dependencies {
                referrers[dependency].push(index);
            }
        }
    }

    let mut queue = std::collections::VecDeque::new();
    for (index, count) in remaining.iter().enumerate() {
        if *count == Some(0) {
            queue.push_back(index);
        }
    }
    while let Some(index) = queue.pop_front() {
        language_state_mut(&mut states[index], language)?.status = FormalCodeStatus::Verified;
        for &referrer in &referrers[index] {
            let Some(count) = &mut remaining[referrer] else {
                continue;
            };
            *count -= 1;
            if *count == 0 {
                queue.push_back(referrer);
            }
        }
    }
    Ok(())
}

pub(super) fn language_state<'a>(
    state: &'a EvaluatedNode,
    language: &str,
) -> Result<&'a LanguageState> {
    match language {
        "lean" => Ok(&state.lean),
        "rocq" => Ok(&state.rocq),
        _ => bail!("unsupported formal language: {language}"),
    }
}

pub(super) fn language_state_mut<'a>(
    state: &'a mut EvaluatedNode,
    language: &str,
) -> Result<&'a mut LanguageState> {
    match language {
        "lean" => Ok(&mut state.lean),
        "rocq" => Ok(&mut state.rocq),
        _ => bail!("unsupported formal language: {language}"),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn candidate(token: &str, dependencies: &[(&str, &str)]) -> EvaluatedNode {
        EvaluatedNode {
            lean: LanguageState {
                status: FormalCodeStatus::Unverified,
                candidate: Some(Candidate {
                    token: token.to_string(),
                    artifact_sha256: String::new(),
                    dependencies: dependencies
                        .iter()
                        .map(|(node, token)| (node.to_string(), token.to_string()))
                        .collect(),
                }),
            },
            rocq: LanguageState {
                status: FormalCodeStatus::NoCode,
                candidate: None,
            },
        }
    }

    #[test]
    fn verification_requires_matching_tokens_along_the_entire_chain() {
        let mut states = vec![
            candidate("a1", &[]),
            candidate("b1", &[("a", "a1")]),
            candidate("c1", &[("b", "old-b")]),
            candidate("d1", &[("c", "c1")]),
        ];
        let index = ["a", "b", "c", "d"]
            .into_iter()
            .enumerate()
            .map(|(i, node)| (node.to_string(), i))
            .collect();
        propagate_verified(&mut states, &index, "lean").unwrap();
        assert_eq!(
            states.iter().map(|s| s.lean.status).collect::<Vec<_>>(),
            [
                FormalCodeStatus::Verified,
                FormalCodeStatus::Verified,
                FormalCodeStatus::Unverified,
                FormalCodeStatus::Unverified
            ]
        );
        assert!(states
            .iter()
            .all(|s| s.rocq.status == FormalCodeStatus::NoCode));
    }

    #[test]
    fn cycles_and_missing_dependencies_never_become_verified() {
        let mut states = vec![
            candidate("a1", &[("b", "b1")]),
            candidate("b1", &[("a", "a1")]),
            candidate("c1", &[("missing", "x")]),
        ];
        let index = [
            ("a".to_string(), 0),
            ("b".to_string(), 1),
            ("c".to_string(), 2),
        ]
        .into();
        propagate_verified(&mut states, &index, "lean").unwrap();
        assert!(states
            .iter()
            .all(|s| s.lean.status == FormalCodeStatus::Unverified));
    }
}
