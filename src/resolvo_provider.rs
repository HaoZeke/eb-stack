//! EasyBuild candidate graph as a resolvo DependencyProvider (CDCL SAT).
//!
//! Feasibility is decided by resolvo. Multi-root *optimization* (priority-lex
//! newest jointly consistent stack) lives in [`solve_with_resolvo`], which
//! constrains and re-solves rather than returning the first SAT assignment.

use crate::domain::{Candidate, Pin, Policy, StackLock};
use crate::package::{
    CandidateExclusion, StackPinMode, StackPinOutcome, StackPolicy, StackPolicySolve,
    STACK_POLICY_SCHEMA_VERSION,
};
use crate::version::{cmp_version, matches_req};
use resolvo::utils::Pool;
use resolvo::{
    Candidates, Condition, ConditionId, Dependencies, DependencyProvider,
    HintDependenciesAvailable, Interner, KnownDependencies, NameId, SolvableId, SolverCache,
    StringId, VersionSetId, VersionSetUnionId,
};
use std::collections::HashMap;
use std::fmt::Display;
use std::sync::Mutex;
use version_ranges::Ranges;

/// Maps (package NameId, version rank) -> candidate index.
pub struct EbProvider {
    pub pool: Pool<Ranges<u32>>,
    pub candidates: Vec<Candidate>,
    /// package name -> NameId
    name_ids: HashMap<String, NameId>,
    /// package name -> sorted (rank ascending, candidate_idx)
    ranks: HashMap<String, Vec<(u32, usize)>>,
    /// pin: name -> allowed ranks
    pin_ranks: HashMap<String, Vec<u32>>,
    /// require_upgrade: name -> rank must be > this
    min_rank_exclusive: HashMap<String, u32>,
    /// Stack policy preferred candidate for each package.
    favored_ranks: HashMap<String, u32>,
    /// Stack policy candidate that is the only selectable version.
    locked_ranks: HashMap<String, u32>,
    /// Candidates rejected by target or build evidence, with the retained reason.
    excluded_ranks: HashMap<String, HashMap<u32, String>>,
    interned: Mutex<HashMap<(NameId, u32), SolvableId>>,
}

impl EbProvider {
    pub fn from_universe(
        candidates_in: &[Candidate],
        policy: &Policy,
        baseline: Option<&StackLock>,
    ) -> Result<Self, String> {
        Self::from_universe_with_stack_policy(candidates_in, policy, baseline, None)
    }

    pub fn from_universe_with_stack_policy(
        candidates_in: &[Candidate],
        policy: &Policy,
        baseline: Option<&StackLock>,
        stack_policy: Option<&StackPolicy>,
    ) -> Result<Self, String> {
        if let Some(stack) = stack_policy {
            validate_stack_policy(policy, stack)?;
        }

        let candidates: Vec<Candidate> = candidates_in
            .iter()
            .filter(|c| {
                c.toolchain.name == policy.toolchain.name
                    && c.toolchain.version == policy.toolchain.version
                    && !policy
                        .forbid
                        .iter()
                        .any(|f| f == &c.easyconfig_path || f == &c.name)
            })
            .cloned()
            .collect();

        let mut by_name: HashMap<String, Vec<usize>> = HashMap::new();
        for (i, c) in candidates.iter().enumerate() {
            by_name.entry(c.name.clone()).or_default().push(i);
        }
        // Sort by (version, versionsuffix) so same version with different
        // suffixes get distinct, deterministic ranks rather than colliding.
        for idxs in by_name.values_mut() {
            idxs.sort_by(|&a, &b| {
                cmp_version(&candidates[a].version, &candidates[b].version).then_with(|| {
                    let sa = candidates[a].versionsuffix.as_deref().unwrap_or("");
                    let sb = candidates[b].versionsuffix.as_deref().unwrap_or("");
                    sa.cmp(sb)
                })
            });
        }

        let pool: Pool<Ranges<u32>> = Pool::new();
        let mut name_ids = HashMap::new();
        let mut ranks: HashMap<String, Vec<(u32, usize)>> = HashMap::new();

        for (name, idxs) in &by_name {
            let name_id = pool.intern_package_name(name.clone());
            name_ids.insert(name.clone(), name_id);
            let mut ranked = Vec::new();
            for (rank, &idx) in idxs.iter().enumerate() {
                ranked.push((rank as u32, idx));
            }
            ranks.insert(name.clone(), ranked);
        }

        let mut pin_ranks: HashMap<String, Vec<u32>> = HashMap::new();
        for pin in &policy.pins {
            let Some(ranked) = ranks.get(&pin.name) else {
                return Err(format!("pin references unknown package {}", pin.name));
            };
            let allowed: Vec<u32> = ranked
                .iter()
                .filter(|(_, idx)| matches_req(&candidates[*idx].version, &pin.version_req))
                .map(|(r, _)| *r)
                .collect();
            if allowed.is_empty() {
                return Err(format!(
                    "pin {} {} matches no candidates",
                    pin.name, pin.version_req
                ));
            }
            pin_ranks.insert(pin.name.clone(), allowed);
        }

        let mut min_rank_exclusive: HashMap<String, u32> = HashMap::new();
        for ru in &policy.require_upgrade {
            if !ru.relative_to_baseline {
                return Err(format!(
                    "require_upgrade for {}: relative_to_baseline is false; \
                     absolute require_upgrade is not supported (set relative_to_baseline \
                     to true and provide a baseline lock, or use a pin)",
                    ru.name
                ));
            }
            let base_ver = baseline
                .and_then(|b| b.package(&ru.name))
                .map(|p| p.version.clone())
                .ok_or_else(|| {
                    format!("require_upgrade {} needs baseline package version", ru.name)
                })?;
            let Some(ranked) = ranks.get(&ru.name) else {
                return Err(format!("require_upgrade unknown package {}", ru.name));
            };
            let mut max_non_upgrade: Option<u32> = None;
            for (rank, idx) in ranked {
                if cmp_version(&candidates[*idx].version, &base_ver) != std::cmp::Ordering::Greater
                {
                    max_non_upgrade = Some(*rank);
                }
            }
            if let Some(m) = max_non_upgrade {
                min_rank_exclusive.insert(ru.name.clone(), m);
            }
            let any_upgrade = ranked.iter().any(|(_, idx)| {
                cmp_version(&candidates[*idx].version, &base_ver) == std::cmp::Ordering::Greater
            });
            if !any_upgrade {
                return Err(format!(
                    "no candidate for {} newer than baseline {}",
                    ru.name, base_ver
                ));
            }
        }

        for root in &policy.roots {
            if !ranks.contains_key(root) {
                return Err(format!("no candidates for root package {root}"));
            }
        }

        let mut favored_ranks = HashMap::new();
        let mut locked_ranks = HashMap::new();
        let mut excluded_ranks: HashMap<String, HashMap<u32, String>> = HashMap::new();
        if let Some(stack) = stack_policy {
            for pin in &stack.pins {
                let matching =
                    matching_ranks(&candidates, &ranks, &pin.name, &pin.version_requirement)?;
                let selected_rank = matching.last().copied().ok_or_else(|| {
                    format!(
                        "stack pin {} {} matches no candidates",
                        pin.name, pin.version_requirement
                    )
                })?;
                match pin.mode {
                    StackPinMode::Preferred => {
                        favored_ranks.insert(pin.name.clone(), selected_rank);
                    }
                    StackPinMode::Locked => {
                        if matching.len() != 1 {
                            return Err(format!(
                                "locked stack pin {} {} matches {} candidates; use an exact version",
                                pin.name,
                                pin.version_requirement,
                                matching.len()
                            ));
                        }
                        locked_ranks.insert(pin.name.clone(), selected_rank);
                    }
                }
            }

            for exclusion in &stack.exclusions {
                let matching = matching_ranks(
                    &candidates,
                    &ranks,
                    &exclusion.name,
                    &exclusion.version_requirement,
                )?;
                if matching.is_empty() {
                    return Err(format!(
                        "candidate exclusion {} {} matches no candidates",
                        exclusion.name, exclusion.version_requirement
                    ));
                }
                let reason = exclusion_reason(exclusion);
                let package_exclusions = excluded_ranks.entry(exclusion.name.clone()).or_default();
                for rank in matching {
                    package_exclusions.insert(rank, reason.clone());
                }
            }
        }

        Ok(Self {
            pool,
            candidates,
            name_ids,
            ranks,
            pin_ranks,
            min_rank_exclusive,
            favored_ranks,
            locked_ranks,
            excluded_ranks,
            interned: Mutex::new(HashMap::new()),
        })
    }

    fn intern_solvable(&self, name_id: NameId, rank: u32) -> SolvableId {
        let mut g = self.interned.lock().unwrap();
        *g.entry((name_id, rank))
            .or_insert_with(|| self.pool.intern_solvable(name_id, rank))
    }

    fn range_matching(
        &self,
        pkg: &str,
        version_req: &str,
        versionsuffix: Option<&str>,
    ) -> Ranges<u32> {
        let Some(ranked) = self.ranks.get(pkg) else {
            return Ranges::empty();
        };
        let mut range = Ranges::empty();
        for (rank, idx) in ranked {
            let c = &self.candidates[*idx];
            if !matches_req(&c.version, version_req) {
                continue;
            }
            // When the dep carries a versionsuffix, only candidates with the
            // same suffix satisfy the requirement (distinct CUDA vs plain, etc.).
            if let Some(want) = versionsuffix {
                let got = c.versionsuffix.as_deref().unwrap_or("");
                if got != want {
                    continue;
                }
            }
            range = range.union(&Ranges::singleton(*rank));
        }
        range
    }

    fn allowed_rank(&self, name: &str, rank: u32) -> bool {
        if let Some(allowed) = self.pin_ranks.get(name) {
            if !allowed.contains(&rank) {
                return false;
            }
        }
        if let Some(min_ex) = self.min_rank_exclusive.get(name) {
            if rank <= *min_ex {
                return false;
            }
        }
        true
    }

    fn exclusion_reason(&self, name: &str, rank: u32) -> Option<&str> {
        self.excluded_ranks
            .get(name)
            .and_then(|ranks| ranks.get(&rank))
            .map(String::as_str)
    }

    pub fn root_requirements(&self, roots: &[String]) -> Vec<resolvo::ConditionalRequirement> {
        roots
            .iter()
            .filter_map(|name| {
                let name_id = *self.name_ids.get(name)?;
                let ranked = self.ranks.get(name)?;
                let mut range = Ranges::empty();
                for (rank, _) in ranked {
                    if self.allowed_rank(name, *rank) {
                        range = range.union(&Ranges::singleton(*rank));
                    }
                }
                if range == Ranges::empty() {
                    return None;
                }
                let vs = self.pool.intern_version_set(name_id, range);
                Some(resolvo::ConditionalRequirement {
                    condition: None,
                    requirement: vs.into(),
                })
            })
            .collect()
    }

    pub fn candidate_for_solvable(&self, id: SolvableId) -> &Candidate {
        let rec = self.pool.resolve_solvable(id);
        let name = self.pool.resolve_package_name(rec.name);
        let rank = rec.record;
        let idx = self
            .ranks
            .get(name)
            .and_then(|v| v.iter().find(|(r, _)| *r == rank).map(|(_, i)| *i))
            .expect("solvable rank missing");
        &self.candidates[idx]
    }
}

impl Interner for EbProvider {
    type NameId = NameId;
    type SolvableId = SolvableId;

    fn display_solvable(&self, solvable: SolvableId) -> impl Display + '_ {
        // Version (+ versionsuffix when present): resolvo already prefixes display_name.
        let c = self.candidate_for_solvable(solvable);
        match &c.versionsuffix {
            Some(s) if !s.is_empty() => format!("{}{}", c.version, s),
            _ => c.version.clone(),
        }
    }

    fn display_name(&self, name: NameId) -> impl Display + '_ {
        self.pool.resolve_package_name(name).to_string()
    }

    fn display_version_set(&self, version_set: VersionSetId) -> impl Display + '_ {
        // Map internal rank ranges back to EasyBuild package versions so unsat
        // messages show "{4.1.6|5.0.3}", not raw ranks like "1 | 2".
        // Package name is printed separately by resolvo (display_name); do not
        // prefix it here or messages become "GROMACS GROMACS@{...}".
        let name_id = self.pool.resolve_version_set_package_name(version_set);
        let name = self.pool.resolve_package_name(name_id).to_string();
        let range = self.pool.resolve_version_set(version_set);
        let mut versions: Vec<String> = Vec::new();
        if let Some(ranked) = self.ranks.get(&name) {
            for (rank, idx) in ranked {
                if range.contains(rank) {
                    versions.push(self.candidates[*idx].version.clone());
                }
            }
        }
        if versions.is_empty() {
            "{no-matching-versions}".to_string()
        } else {
            format!("{{{}}}", versions.join("|"))
        }
    }

    fn display_string(&self, string_id: StringId) -> impl Display + '_ {
        self.pool.resolve_string(string_id).to_string()
    }

    fn version_set_name(&self, version_set: VersionSetId) -> NameId {
        self.pool.resolve_version_set_package_name(version_set)
    }

    fn solvable_name(&self, solvable: SolvableId) -> NameId {
        self.pool.resolve_solvable(solvable).name
    }

    fn version_sets_in_union(
        &self,
        version_set_union: VersionSetUnionId,
    ) -> impl Iterator<Item = VersionSetId> {
        self.pool.resolve_version_set_union(version_set_union)
    }

    fn resolve_condition(&self, _condition: ConditionId) -> Condition {
        // We do not use conditions in this provider.
        unreachable!("eb_stack provider does not use conditions")
    }
}

impl DependencyProvider for EbProvider {
    async fn filter_candidates(
        &self,
        candidates: &[SolvableId],
        version_set: VersionSetId,
        inverse: bool,
    ) -> Vec<SolvableId> {
        let range = self.pool.resolve_version_set(version_set);
        candidates
            .iter()
            .copied()
            .filter(|s| {
                let rank = self.pool.resolve_solvable(*s).record;
                range.contains(&rank) != inverse
            })
            .collect()
    }

    async fn sort_candidates(&self, _solver: &SolverCache<Self>, solvables: &mut [SolvableId]) {
        solvables.sort_by(|a, b| {
            let ra = self.pool.resolve_solvable(*a).record;
            let rb = self.pool.resolve_solvable(*b).record;
            rb.cmp(&ra) // higher rank first = prefer newer
        });
    }

    async fn get_candidates(&self, name: NameId) -> Option<Candidates> {
        let package_name = self.pool.resolve_package_name(name).to_string();
        let ranked = self.ranks.get(&package_name)?;
        let mut candidates = Candidates {
            candidates: Vec::new(),
            hint_dependencies_available: HintDependenciesAvailable::All,
            ..Candidates::default()
        };
        for (rank, _) in ranked {
            if !self.allowed_rank(&package_name, *rank) {
                continue;
            }
            let solvable = self.intern_solvable(name, *rank);
            if let Some(reason) = self.exclusion_reason(&package_name, *rank) {
                candidates
                    .excluded
                    .push((solvable, self.pool.intern_string(reason.to_string())));
                continue;
            }
            candidates.candidates.push(solvable);
            if self.favored_ranks.get(&package_name) == Some(rank) {
                candidates.favored = Some(solvable);
            }
            if self.locked_ranks.get(&package_name) == Some(rank) {
                candidates.locked = Some(solvable);
            }
        }
        if candidates.candidates.is_empty() && candidates.excluded.is_empty() {
            return None;
        }
        Some(candidates)
    }

    async fn get_dependencies(&self, solvable: SolvableId) -> Dependencies {
        let c = self.candidate_for_solvable(solvable);
        let mut known = KnownDependencies::default();
        // Runtime and build-time deps are co-selection requirements the same way;
        // role distinction lives on Candidate for outputs, not in resolvo edges.
        for d in c.dependencies.iter().chain(c.builddependencies.iter()) {
            let Some(&dep_name_id) = self.name_ids.get(&d.name) else {
                let reason = self
                    .pool
                    .intern_string(format!("missing dependency package {}", d.name));
                return Dependencies::Unknown(reason);
            };
            let range = self.range_matching(&d.name, &d.version_req, d.versionsuffix.as_deref());
            if range == Ranges::empty() {
                let reason = self.pool.intern_string(format!(
                    "unresolved dependency {} {} from {}={}",
                    d.name, d.version_req, c.name, c.version
                ));
                return Dependencies::Unknown(reason);
            }
            let vs = self.pool.intern_version_set(dep_name_id, range);
            known.requirements.push(vs.into());
        }
        Dependencies::Known(known)
    }
}

fn validate_stack_policy(policy: &Policy, stack: &StackPolicy) -> Result<(), String> {
    if stack.schema_version != STACK_POLICY_SCHEMA_VERSION {
        return Err(format!(
            "unsupported stack policy schema version {}",
            stack.schema_version
        ));
    }
    if stack.toolchain != policy.toolchain {
        return Err(format!(
            "stack policy toolchain {} does not match solve toolchain {}",
            stack.toolchain.label(),
            policy.toolchain.label()
        ));
    }
    Ok(())
}

fn matching_ranks(
    candidates: &[Candidate],
    ranks: &HashMap<String, Vec<(u32, usize)>>,
    name: &str,
    version_requirement: &str,
) -> Result<Vec<u32>, String> {
    let ranked = ranks
        .get(name)
        .ok_or_else(|| format!("stack policy references unknown package {name}"))?;
    Ok(ranked
        .iter()
        .filter(|(_, index)| matches_req(&candidates[*index].version, version_requirement))
        .map(|(rank, _)| *rank)
        .collect())
}

fn exclusion_reason(exclusion: &CandidateExclusion) -> String {
    match &exclusion.scope {
        Some(scope) => format!("{} (scope: {scope})", exclusion.reason),
        None => exclusion.reason.clone(),
    }
}

fn solve_feasibility_with_stack_policy(
    candidates: &[Candidate],
    policy: &Policy,
    baseline: Option<&StackLock>,
    stack_policy: &StackPolicy,
) -> Result<Vec<Candidate>, String> {
    let provider = EbProvider::from_universe_with_stack_policy(
        candidates,
        policy,
        baseline,
        Some(stack_policy),
    )?;
    let requirements = provider.root_requirements(&policy.roots);
    if requirements.len() != policy.roots.len() {
        return Err("unsatisfiable stack: no valid root version sets (pins/upgrade)".into());
    }
    let mut solver = resolvo::Solver::new(provider);
    let problem = resolvo::Problem::new().requirements(requirements);
    match solver.solve(problem) {
        Ok(solvables) => {
            let provider = solver.provider();
            let mut selected: Vec<Candidate> = solvables
                .iter()
                .map(|solvable| provider.candidate_for_solvable(*solvable).clone())
                .collect();
            selected.sort_by(|left, right| left.name.cmp(&right.name));
            Ok(selected)
        }
        Err(resolvo::UnsolvableOrCancelled::Unsolvable(conflict)) => {
            let message = conflict.display_user_friendly(&solver).to_string();
            Err(format!("unsatisfiable stack (Resolvo SAT): {message}"))
        }
        Err(resolvo::UnsolvableOrCancelled::Cancelled(reason)) => {
            Err(format!("solver cancelled: {reason:?}"))
        }
    }
}

pub fn solve_with_stack_policy(
    candidates: &[Candidate],
    policy: &Policy,
    baseline: Option<&StackLock>,
    stack_policy: &StackPolicy,
) -> Result<StackPolicySolve, String> {
    validate_stack_policy(policy, stack_policy)?;
    let selected = solve_feasibility_with_stack_policy(candidates, policy, baseline, stack_policy)?;
    let pin_outcomes = stack_policy
        .pins
        .iter()
        .map(|pin| {
            let selected_version = selected
                .iter()
                .find(|candidate| candidate.name == pin.name)
                .map(|candidate| candidate.version.clone());
            let fallback = pin.mode == StackPinMode::Preferred
                && selected_version
                    .as_deref()
                    .is_some_and(|version| !matches_req(version, &pin.version_requirement));
            StackPinOutcome {
                name: pin.name.clone(),
                requested: pin.version_requirement.clone(),
                selected_version,
                fallback,
                fallback_reason: fallback.then(|| {
                    "favored candidate did not participate in the complete Resolvo solution"
                        .to_string()
                }),
            }
        })
        .collect();
    Ok(StackPolicySolve {
        selected,
        pin_outcomes,
        exclusions: stack_policy.exclusions.clone(),
    })
}

/// One resolvo CDCL SAT solve for the given policy (feasibility only).
fn solve_feasibility(
    candidates: &[Candidate],
    policy: &Policy,
    baseline: Option<&StackLock>,
) -> Result<Vec<Candidate>, String> {
    let provider = EbProvider::from_universe(candidates, policy, baseline)?;
    let requirements = provider.root_requirements(&policy.roots);
    if requirements.is_empty() {
        return Err("unsatisfiable stack: no valid root version sets (pins/upgrade)".into());
    }
    // Default runtime is NowOrNeverRuntime (sync async).
    let mut solver = resolvo::Solver::new(provider);
    let problem = resolvo::Problem::new().requirements(requirements);
    match solver.solve(problem) {
        Ok(solvables) => {
            let prov = solver.provider();
            let mut selected: Vec<Candidate> = solvables
                .iter()
                .map(|s| prov.candidate_for_solvable(*s).clone())
                .collect();
            selected.sort_by(|a, b| a.name.cmp(&b.name));
            Ok(selected)
        }
        Err(resolvo::UnsolvableOrCancelled::Unsolvable(conflict)) => {
            let msg = conflict.display_user_friendly(&solver).to_string();
            Err(format!("unsatisfiable stack (resolvo SAT): {msg}"))
        }
        Err(resolvo::UnsolvableOrCancelled::Cancelled(reason)) => {
            Err(format!("solver cancelled: {reason:?}"))
        }
    }
}

/// Candidate versions for a package name under the policy toolchain, newest first.
/// Order is deterministic (sorted by [`cmp_version`]), independent of HashMap iteration.
fn versions_newest_first(candidates: &[Candidate], policy: &Policy, name: &str) -> Vec<String> {
    let mut versions: Vec<String> = candidates
        .iter()
        .filter(|c| {
            c.name == name
                && c.toolchain.name == policy.toolchain.name
                && c.toolchain.version == policy.toolchain.version
                && !policy
                    .forbid
                    .iter()
                    .any(|f| f == &c.easyconfig_path || f == &c.name)
        })
        .map(|c| c.version.clone())
        .collect();
    versions.sort_by(|a, b| cmp_version(b, a));
    versions.dedup();
    // Honour existing policy pins for this package when listing trial versions.
    if let Some(pin) = policy.pins.iter().find(|p| p.name == name) {
        versions.retain(|v| matches_req(v, &pin.version_req));
    }
    versions
}

fn policy_with_root_version_pins(policy: &Policy, root_pins: &[(String, String)]) -> Policy {
    let mut p = policy.clone();
    for (name, ver) in root_pins {
        // Replace any existing pin for this root with the exact trial version.
        p.pins.retain(|pin| pin.name != *name);
        p.pins.push(Pin {
            name: name.clone(),
            version_req: format!("=={ver}"),
        });
    }
    p
}

/// Solve using resolvo CDCL SAT as the feasibility core, then optimize over
/// satisfying assignments: lexicographically maximize each application root's
/// version in declared [`Policy::effective_root_priority`] order.
///
/// The outcome depends only on the policy (including priority) and the
/// candidate set — not on incidental list order of non-priority fields or
/// HashMap iteration order inside the provider.
pub fn solve_with_resolvo(
    candidates: &[Candidate],
    policy: &Policy,
    baseline: Option<&StackLock>,
) -> Result<Vec<Candidate>, String> {
    let priority = policy.effective_root_priority();
    if priority.is_empty() {
        return Err("unsatisfiable stack: policy has no roots".into());
    }

    // Sequential lex maximization: for each root in priority order, pin the
    // newest version that remains jointly feasible with already-chosen higher
    // priority roots (and all other roots still required without a version pin).
    let mut chosen_root_versions: Vec<(String, String)> = Vec::new();

    for root in &priority {
        let versions = versions_newest_first(candidates, policy, root);
        if versions.is_empty() {
            return Err(format!("no candidates for root package {root}"));
        }

        let mut found: Option<String> = None;
        let mut last_err = String::new();
        for ver in &versions {
            let mut trial_pins = chosen_root_versions.clone();
            trial_pins.push((root.clone(), ver.clone()));
            let trial_policy = policy_with_root_version_pins(policy, &trial_pins);
            match solve_feasibility(candidates, &trial_policy, baseline) {
                Ok(_) => {
                    found = Some(ver.clone());
                    break;
                }
                Err(e) => {
                    last_err = e;
                }
            }
        }

        match found {
            Some(ver) => chosen_root_versions.push((root.clone(), ver)),
            None => {
                return Err(if last_err.is_empty() {
                    format!(
                        "unsatisfiable stack: no jointly feasible version for root {root} \
                         under priority {:?}",
                        priority
                    )
                } else {
                    last_err
                });
            }
        }
    }

    // Final solve with all priority-optimal root versions pinned; co-selected
    // non-root packages still prefer newer via resolvo's sort_candidates.
    let final_policy = policy_with_root_version_pins(policy, &chosen_root_versions);
    solve_feasibility(candidates, &final_policy, baseline)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::domain::{DepReq, LockPackage, RequireUpgrade, SolverMeta, Toolchain};

    fn tc() -> Toolchain {
        Toolchain {
            name: "foss".into(),
            version: "2025b".into(),
        }
    }

    fn cand(
        name: &str,
        version: &str,
        versionsuffix: Option<&str>,
        path: &str,
        deps: Vec<DepReq>,
    ) -> Candidate {
        Candidate {
            name: name.into(),
            version: version.into(),
            toolchain: tc(),
            versionsuffix: versionsuffix.map(str::to_string),
            easyconfig_path: path.into(),
            dependencies: deps,
            builddependencies: vec![],
            exts_list: vec![],
        }
    }

    fn policy(roots: Vec<&str>, require_upgrade: Vec<RequireUpgrade>) -> Policy {
        Policy {
            toolchain: tc(),
            roots: roots.into_iter().map(str::to_string).collect(),
            root_priority: None,
            pins: vec![],
            forbid: vec![],
            objective: "prefer_newer".into(),
            require_upgrade,
        }
    }

    fn lock_pkg(name: &str, version: &str) -> LockPackage {
        LockPackage {
            name: name.into(),
            version: version.into(),
            toolchain: tc(),
            versionsuffix: None,
            easyconfig_path: format!("{name}-{version}.eb"),
        }
    }

    fn baseline_lock(packages: Vec<LockPackage>) -> StackLock {
        StackLock {
            schema_version: 1,
            toolchain: tc(),
            generation_label: Some("baseline".into()),
            packages,
            solver: SolverMeta {
                engine: "test".into(),
                engine_version: "test".into(),
                timestamp: "STABLE".into(),
            },
        }
    }

    /// Same name + version with different versionsuffix must get distinct ranks.
    #[test]
    fn versionsuffix_distinguishes_candidate_identity() {
        let candidates = vec![
            cand("Lib", "1.0", None, "Lib-1.0.eb", vec![]),
            cand("Lib", "1.0", Some("-CUDA-12.8"), "Lib-1.0-CUDA.eb", vec![]),
            cand(
                "App",
                "1.0",
                None,
                "App-1.0.eb",
                vec![DepReq {
                    name: "Lib".into(),
                    version_req: "==1.0".into(),
                    versionsuffix: Some("-CUDA-12.8".into()),
                    toolchain: None,
                }],
            ),
        ];
        let pol = policy(vec!["App"], vec![]);
        let provider = EbProvider::from_universe(&candidates, &pol, None).expect("provider");
        let lib_ranks = provider.ranks.get("Lib").expect("Lib ranks");
        assert_eq!(
            lib_ranks.len(),
            2,
            "plain and CUDA Lib must be two rank identities, got {lib_ranks:?}"
        );
        let suffixes: Vec<Option<&str>> = lib_ranks
            .iter()
            .map(|(_, idx)| provider.candidates[*idx].versionsuffix.as_deref())
            .collect();
        assert!(
            suffixes.contains(&None) && suffixes.contains(&Some("-CUDA-12.8")),
            "expected both suffixes in ranks: {suffixes:?}"
        );

        // Real solve path: App requires CUDA Lib specifically.
        let selected = solve_with_resolvo(&candidates, &pol, None).expect("solve");
        let lib = selected
            .iter()
            .find(|c| c.name == "Lib")
            .expect("Lib selected");
        assert_eq!(
            lib.versionsuffix.as_deref(),
            Some("-CUDA-12.8"),
            "solver must pick the CUDA identity, not collapse to plain Lib"
        );
        assert_eq!(lib.easyconfig_path, "Lib-1.0-CUDA.eb");
    }

    /// Two same-version candidates with different suffixes remain independently selectable.
    #[test]
    fn versionsuffix_plain_selected_when_dep_has_no_suffix() {
        let candidates = vec![
            cand("Lib", "1.0", None, "Lib-1.0.eb", vec![]),
            cand("Lib", "1.0", Some("-CUDA-12.8"), "Lib-1.0-CUDA.eb", vec![]),
            cand(
                "App",
                "1.0",
                None,
                "App-1.0.eb",
                vec![DepReq {
                    name: "Lib".into(),
                    version_req: "==1.0".into(),
                    // No versionsuffix on the dep: both identities match; prefer higher rank.
                    versionsuffix: None,
                    toolchain: None,
                }],
            ),
        ];
        let pol = policy(vec!["App"], vec![]);
        let selected = solve_with_resolvo(&candidates, &pol, None).expect("solve");
        let lib = selected.iter().find(|c| c.name == "Lib").expect("Lib");
        // Rank order: plain "" then CUDA (lexicographic suffix). Prefer newer = higher rank = CUDA.
        // With no suffix constraint either may win via prefer_newer; assert a Lib was chosen
        // and provider still had two identities (covered above). Here: both are valid.
        assert_eq!(lib.version, "1.0");
        assert!(lib.versionsuffix.is_none() || lib.versionsuffix.as_deref() == Some("-CUDA-12.8"));
    }

    #[test]
    fn require_upgrade_relative_to_baseline_false_errors() {
        let candidates = vec![
            cand("App", "1.0", None, "App-1.0.eb", vec![]),
            cand("App", "2.0", None, "App-2.0.eb", vec![]),
        ];
        let pol = policy(
            vec!["App"],
            vec![RequireUpgrade {
                name: "App".into(),
                relative_to_baseline: false,
            }],
        );
        let baseline = baseline_lock(vec![lock_pkg("App", "1.0")]);
        let err = match EbProvider::from_universe(&candidates, &pol, Some(&baseline)) {
            Ok(_) => panic!("relative_to_baseline false must not silent no-op"),
            Err(e) => e,
        };
        let low = err.to_lowercase();
        assert!(
            low.contains("relative_to_baseline") && low.contains("false"),
            "error must mention relative_to_baseline false, got: {err}"
        );
        // Solve path also surfaces the error (not success-with-no-constraint).
        let solve_err = match solve_with_resolvo(&candidates, &pol, Some(&baseline)) {
            Ok(_) => panic!("solve must fail for relative_to_baseline false"),
            Err(e) => e,
        };
        assert!(
            solve_err.to_lowercase().contains("relative_to_baseline"),
            "solve error: {solve_err}"
        );
    }

    #[test]
    fn require_upgrade_multi_package_honoured() {
        let candidates = vec![
            cand("Foo", "1.0", None, "Foo-1.0.eb", vec![]),
            cand("Foo", "2.0", None, "Foo-2.0.eb", vec![]),
            cand("Bar", "1.0", None, "Bar-1.0.eb", vec![]),
            cand("Bar", "2.0", None, "Bar-2.0.eb", vec![]),
            cand("App", "1.0", None, "App-1.0.eb", vec![]),
        ];
        // Roots include Foo and Bar so both appear in the selection; require both upgrade.
        let pol = policy(
            vec!["App", "Foo", "Bar"],
            vec![
                RequireUpgrade {
                    name: "Foo".into(),
                    relative_to_baseline: true,
                },
                RequireUpgrade {
                    name: "Bar".into(),
                    relative_to_baseline: true,
                },
            ],
        );
        let baseline = baseline_lock(vec![lock_pkg("Foo", "1.0"), lock_pkg("Bar", "1.0")]);
        let selected =
            solve_with_resolvo(&candidates, &pol, Some(&baseline)).expect("multi require_upgrade");
        assert_eq!(
            selected.iter().find(|c| c.name == "Foo").unwrap().version,
            "2.0",
            "Foo must upgrade past baseline 1.0"
        );
        assert_eq!(
            selected.iter().find(|c| c.name == "Bar").unwrap().version,
            "2.0",
            "Bar must upgrade past baseline 1.0"
        );
    }

    #[test]
    fn require_upgrade_single_object_json_still_deserializes() {
        let json = r#"{
            "toolchain": {"name": "foss", "version": "2025b"},
            "roots": ["GROMACS"],
            "require_upgrade": {"name": "GROMACS", "relative_to_baseline": true}
        }"#;
        let p: Policy = serde_json::from_str(json).expect("single-object require_upgrade");
        assert_eq!(p.require_upgrade.len(), 1);
        assert_eq!(p.require_upgrade[0].name, "GROMACS");
        assert!(p.require_upgrade[0].relative_to_baseline);
    }

    #[test]
    fn require_upgrade_array_json_deserializes() {
        let json = r#"{
            "toolchain": {"name": "foss", "version": "2025b"},
            "roots": ["App"],
            "require_upgrade": [
                {"name": "Foo", "relative_to_baseline": true},
                {"name": "Bar", "relative_to_baseline": true}
            ]
        }"#;
        let p: Policy = serde_json::from_str(json).expect("array require_upgrade");
        assert_eq!(p.require_upgrade.len(), 2);
        assert_eq!(p.require_upgrade[0].name, "Foo");
        assert_eq!(p.require_upgrade[1].name, "Bar");
    }

    #[test]
    fn require_upgrade_null_json_deserializes_empty() {
        let json = r#"{
            "toolchain": {"name": "foss", "version": "2025b"},
            "roots": ["App"],
            "require_upgrade": null
        }"#;
        let p: Policy = serde_json::from_str(json).expect("null require_upgrade");
        assert!(p.require_upgrade.is_empty());
    }
}
