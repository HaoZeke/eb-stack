//! EasyBuild candidate graph as a resolvo DependencyProvider (CDCL SAT).
//!
//! Feasibility is decided by resolvo. Multi-root *optimization* (priority-lex
//! newest jointly consistent stack) lives in [`solve_with_resolvo`], which
//! constrains and re-solves rather than returning the first SAT assignment.

use crate::domain::{Candidate, Pin, Policy, StackLock};
use crate::version::{cmp_version, matches_req};
use resolvo::utils::Pool;
use resolvo::{
    Candidates, Condition, ConditionId, Dependencies, DependencyProvider, HintDependenciesAvailable,
    Interner, KnownDependencies, NameId, SolvableId, SolverCache, StringId, VersionSetId,
    VersionSetUnionId,
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
    interned: Mutex<HashMap<(NameId, u32), SolvableId>>,
}

impl EbProvider {
    pub fn from_universe(
        candidates_in: &[Candidate],
        policy: &Policy,
        baseline: Option<&StackLock>,
    ) -> Result<Self, String> {
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
        for idxs in by_name.values_mut() {
            idxs.sort_by(|&a, &b| cmp_version(&candidates[a].version, &candidates[b].version));
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
        if let Some(ru) = &policy.require_upgrade {
            if ru.relative_to_baseline {
                let base_ver = baseline
                    .and_then(|b| b.package(&ru.name))
                    .map(|p| p.version.clone())
                    .ok_or_else(|| {
                        format!(
                            "require_upgrade {} needs baseline package version",
                            ru.name
                        )
                    })?;
                let Some(ranked) = ranks.get(&ru.name) else {
                    return Err(format!("require_upgrade unknown package {}", ru.name));
                };
                let mut max_non_upgrade: Option<u32> = None;
                for (rank, idx) in ranked {
                    if cmp_version(&candidates[*idx].version, &base_ver)
                        != std::cmp::Ordering::Greater
                    {
                        max_non_upgrade = Some(*rank);
                    }
                }
                if let Some(m) = max_non_upgrade {
                    min_rank_exclusive.insert(ru.name.clone(), m);
                }
                let any_upgrade = ranked.iter().any(|(_, idx)| {
                    cmp_version(&candidates[*idx].version, &base_ver)
                        == std::cmp::Ordering::Greater
                });
                if !any_upgrade {
                    return Err(format!(
                        "no candidate for {} newer than baseline {}",
                        ru.name, base_ver
                    ));
                }
            }
        }

        for root in &policy.roots {
            if !ranks.contains_key(root) {
                return Err(format!("no candidates for root package {root}"));
            }
        }

        Ok(Self {
            pool,
            candidates,
            name_ids,
            ranks,
            pin_ranks,
            min_rank_exclusive,
            interned: Mutex::new(HashMap::new()),
        })
    }

    fn intern_solvable(&self, name_id: NameId, rank: u32) -> SolvableId {
        let mut g = self.interned.lock().unwrap();
        *g.entry((name_id, rank))
            .or_insert_with(|| self.pool.intern_solvable(name_id, rank))
    }

    fn range_matching(&self, pkg: &str, version_req: &str) -> Ranges<u32> {
        let Some(ranked) = self.ranks.get(pkg) else {
            return Ranges::empty();
        };
        let mut range = Ranges::empty();
        for (rank, idx) in ranked {
            if matches_req(&self.candidates[*idx].version, version_req) {
                range = range.union(&Ranges::singleton(*rank));
            }
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
        // Version only: resolvo already prefixes display_name (package).
        let c = self.candidate_for_solvable(solvable);
        c.version.clone()
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
            candidates
                .candidates
                .push(self.intern_solvable(name, *rank));
        }
        if candidates.candidates.is_empty() {
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
            let range = self.range_matching(&d.name, &d.version_req);
            if range == Ranges::empty() {
                let reason = self.pool.intern_string(format!(
                    "unsatisfiable dep {} {} from {}={}",
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
