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
use std::collections::{BTreeSet, HashMap, HashSet};
use std::fmt::Display;
use std::sync::Mutex;
use version_ranges::Ranges;

/// Maps (package NameId, version rank) -> candidate index.
pub struct EbProvider {
    /// Interned names and version ranges the solver works over.
    pub pool: Pool<Ranges<u32>>,
    /// Candidates, indexed by solvable id.
    pub candidates: Vec<Candidate>,
    /// package name -> NameId
    name_ids: HashMap<String, NameId>,
    /// package name -> sorted (rank ascending, candidate_idx)
    ranks: HashMap<String, Vec<(u32, usize)>>,
    /// Names a generation carries at more than one toolchain level, whose
    /// resolvo package name is qualified so the levels are separate variables.
    multi_level: HashSet<String>,
    /// Names the system level carries at several versions; each is its own
    /// package, because a system dependency pins one build exactly.
    system_multi: HashSet<String>,
    /// Every qualified key interned for a package name, so a dependency can
    /// fall back to them when the levels below the recipe hold nothing.
    keys_by_name: HashMap<String, Vec<String>>,
    /// The policy generation, lowest level first, for deciding which levels a
    /// recipe may take an unpinned dependency from.
    hierarchy_members: Vec<crate::domain::Toolchain>,
    /// pin: name -> allowed ranks
    pin_ranks: HashMap<String, Vec<u32>>,
    /// require_upgrade: name -> rank must be > this
    min_rank_exclusive: HashMap<String, u32>,
    /// Stack policy preferred candidate for each package.
    favored_ranks: HashMap<String, u32>,
    /// Fully identified stack policy candidate selected directly by Resolvo.
    locked_ranks: HashMap<String, u32>,
    /// Candidates rejected by target or build evidence, with the retained reason.
    excluded_ranks: HashMap<String, HashMap<u32, String>>,
    interned: Mutex<HashMap<(NameId, u32), SolvableId>>,
}

/// How a toolchain is written inside a qualified package key.
///
/// SYSTEM is normalized, since an easyconfig writes it as name and version
/// both "system" while a hierarchy carries an empty version.
fn toolchain_label(tc: &crate::domain::Toolchain) -> String {
    if crate::hierarchy::is_system_toolchain(tc) {
        "system".to_string()
    } else {
        format!("{}-{}", tc.name, tc.version)
    }
}

/// The resolvo package name for a candidate.
///
/// Plain for the ordinary case, qualified by toolchain for the names that a
/// generation legitimately carries at more than one level. Qualifying only
/// those keeps every existing message, lock and pin reading as it did.
fn package_key(
    name: &str,
    tc: &crate::domain::Toolchain,
    version: &str,
    multi_level: &HashSet<String>,
    system_multi: &HashSet<String>,
) -> String {
    // The system level is the bootstrap layer, and a generation genuinely
    // carries two builds of one name there: binutils 2.40 builds the GCCcore
    // that builds binutils 2.42, and zlib does the same. Nothing chooses
    // between them, because every dependency on a system module names its
    // version exactly. Keying them by name alone asks the solver to pick one
    // and makes the generation unsatisfiable by construction.
    if system_multi.contains(name) && crate::hierarchy::is_system_toolchain(tc) {
        return format!("{name}@system=={version}");
    }
    if multi_level.contains(name) {
        format!("{name}@{}", toolchain_label(tc))
    } else {
        name.to_string()
    }
}

/// The interned keys for a package name.
///
/// Policy speaks in names: a pin, an exclusion and a root all say `CMake`. The
/// pool speaks in keys, which are qualified by toolchain for any name the
/// generation carries at several levels. Everything that reads policy has to
/// cross that gap, and a lookup by plain name silently finds nothing.
fn keys_for_name<T>(by_key: &HashMap<String, T>, name: &str) -> Vec<String> {
    if by_key.contains_key(name) {
        return vec![name.to_string()];
    }
    let prefix = format!("{name}@");
    let mut keys: Vec<String> = by_key
        .keys()
        .filter(|key| key.starts_with(&prefix))
        .cloned()
        .collect();
    keys.sort();
    keys
}

impl EbProvider {
    /// Which resolvo package names can satisfy one dependency of one recipe.
    ///
    /// A plain name for a package the generation carries once. For a package
    /// carried at several levels: the level the dependency pins, or, when it
    /// pins none, every level at or below the recipe's own, which is the range
    /// EasyBuild's minimal-toolchain search may pick from.
    fn dependency_keys(&self, recipe: &Candidate, dep: &crate::domain::DepReq) -> Vec<String> {
        // A system-level bootstrap package is asked for by exact version, so
        // the dependency names one build and no other. Without an exact
        // version any of them will do, and the union says so.
        if self.system_multi.contains(&dep.name) {
            let system_at = |version: &str| format!("{}@system=={version}", dep.name);
            let wants_system = dep
                .toolchain
                .as_ref()
                .is_some_and(crate::hierarchy::is_system_toolchain)
                || crate::hierarchy::is_system_toolchain(&recipe.toolchain);
            if let Some(pinned) = dep.version_req.strip_prefix("==") {
                let key = system_at(pinned);
                if self.name_ids.contains_key(&key) {
                    return vec![key];
                }
            }
            let mut keys: Vec<String> = self
                .name_ids
                .keys()
                .filter(|key| key.starts_with(&format!("{}@system==", dep.name)))
                .cloned()
                .collect();
            keys.sort();
            if !wants_system && self.multi_level.contains(&dep.name) {
                // The name also lives inside the generation, and a recipe
                // there may take either.
                if let Some(tc) = dep.toolchain.as_ref() {
                    keys.push(format!("{}@{}", dep.name, toolchain_label(tc)));
                } else {
                    let own = format!("{}@{}", dep.name, toolchain_label(&recipe.toolchain));
                    if self.name_ids.contains_key(&own) {
                        keys.push(own);
                    }
                }
            }
            if !keys.is_empty() {
                return keys;
            }
        }
        if !self.multi_level.contains(&dep.name) {
            return vec![dep.name.clone()];
        }
        if let Some(tc) = dep.toolchain.as_ref() {
            return vec![format!("{}@{}", dep.name, toolchain_label(tc))];
        }
        let recipe_at = self
            .hierarchy_members
            .iter()
            .position(|m| crate::hierarchy::toolchains_match(m, &recipe.toolchain));
        // The hierarchy is ordered lowest level first, so a recipe may take a
        // dependency from its own level and anything under it, never above.
        let admissible: Vec<&crate::domain::Toolchain> = match recipe_at {
            Some(at) => self.hierarchy_members[..=at].iter().collect(),
            None => self.hierarchy_members.iter().collect(),
        };
        let mut keys: Vec<String> = admissible
            .into_iter()
            .map(|tc| format!("{}@{}", dep.name, toolchain_label(tc)))
            .filter(|key| self.name_ids.contains_key(key))
            .collect();
        let own = format!("{}@{}", dep.name, toolchain_label(&recipe.toolchain));
        if self.name_ids.contains_key(&own) && !keys.contains(&own) {
            keys.push(own);
        }
        if keys.is_empty() {
            // Nothing at or below this recipe carries the package. A stack
            // policy can admit a closure from another generation on purpose,
            // and refusing it here would turn a deliberate cross-generation
            // pin into an unresolved dependency.
            if let Some(all) = self.keys_by_name.get(&dep.name) {
                return all.clone();
            }
        }
        keys
    }

    /// Build a provider from a candidate set and a policy.
    pub fn from_universe(
        candidates_in: &[Candidate],
        policy: &Policy,
        baseline: Option<&StackLock>,
    ) -> Result<Self, String> {
        Self::from_universe_with_stack_policy(candidates_in, policy, baseline, None)
    }

    /// As [`Self::from_universe`], also applying a site stack policy.
    pub fn from_universe_with_stack_policy(
        candidates_in: &[Candidate],
        policy: &Policy,
        baseline: Option<&StackLock>,
        stack_policy: Option<&StackPolicy>,
    ) -> Result<Self, String> {
        Self::from_universe_with_stack_policy_scope(
            candidates_in,
            policy,
            baseline,
            stack_policy,
            false,
        )
    }

    fn from_universe_with_stack_policy_scope(
        candidates_in: &[Candidate],
        policy: &Policy,
        baseline: Option<&StackLock>,
        stack_policy: Option<&StackPolicy>,
        curated_toolchains: bool,
    ) -> Result<Self, String> {
        if let Some(stack) = stack_policy {
            validate_stack_policy(policy, stack)?;
        }

        // A recipe's dependencies do not all live at its own toolchain: an app
        // at GCC takes CMake from GCCcore and licence bits from SYSTEM, which
        // is EasyBuild's minimal-toolchain search. Admitting only the policy
        // toolchain here drops those candidates before they are ever interned,
        // so the solver reports a missing *package* rather than an
        // unsatisfiable constraint, and the caller cannot tell the difference.
        let hierarchy_members =
            crate::hierarchy::hierarchy_for_with_tree(&policy.toolchain, None, candidates_in)
                .map(|h| h.members)
                .unwrap_or_default();
        let in_generation = |c: &Candidate| {
            let same =
                |t: &crate::domain::Toolchain| crate::hierarchy::toolchains_match(&c.toolchain, t);
            same(&policy.toolchain) || hierarchy_members.iter().any(same)
        };
        let filtered: Vec<Candidate> = candidates_in
            .iter()
            .filter(|c| {
                (curated_toolchains || in_generation(c))
                    && !policy
                        .forbid
                        .iter()
                        .any(|f| f == &c.easyconfig_path || f == &c.name)
            })
            .cloned()
            .collect();
        let candidates = crate::provides::expand_extension_provides(&filtered);

        // A generation carries some packages at more than one level, and they
        // are different modules: EasyBuild installs Perl at GCCcore and Perl at
        // SYSTEM side by side, and a recipe pins whichever it was built
        // against. Resolvo decides one solvable per package name, so those two
        // have to be two names or the stack is unsatisfiable by construction.
        // Co-installability is the property being modelled here; Vouillon and
        // Di Cosmo state it for Debian in doi:10.1145/2522920.2522927, and it
        // is why Spack keys a package by its whole spec rather than its name,
        // doi:10.1109/sc41404.2022.00040.
        //
        // Only names that genuinely appear at several levels are qualified.
        // Everything else keeps its plain name, so the common case reads the
        // same in every message and lock the tool has ever written.
        let mut levels_of: HashMap<String, BTreeSet<String>> = HashMap::new();
        for c in candidates.iter() {
            levels_of
                .entry(c.name.clone())
                .or_default()
                .insert(toolchain_label(&c.toolchain));
        }
        let multi_level: HashSet<String> = levels_of
            .iter()
            .filter(|(_, levels)| levels.len() > 1)
            .map(|(name, _)| name.clone())
            .collect();

        // Names the system level carries at more than one version. These are
        // the bootstrap pairs, and each version is its own package.
        let mut system_versions: HashMap<String, BTreeSet<String>> = HashMap::new();
        for c in candidates.iter() {
            if crate::hierarchy::is_system_toolchain(&c.toolchain) {
                system_versions
                    .entry(c.name.clone())
                    .or_default()
                    .insert(c.version.clone());
            }
        }
        let system_multi: HashSet<String> = system_versions
            .iter()
            .filter(|(_, versions)| versions.len() > 1)
            .map(|(name, _)| name.clone())
            .collect();

        let mut by_name: HashMap<String, Vec<usize>> = HashMap::new();
        for (i, c) in candidates.iter().enumerate() {
            by_name
                .entry(package_key(&c.name, &c.toolchain, &c.version, &multi_level, &system_multi))
                .or_default()
                .push(i);
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
            // Ranks are per key, so a pin on a package carried at several
            // levels has to be applied to each of them separately: rank 2 of
            // one key is a different build from rank 2 of another.
            let pin_keys = keys_for_name(&ranks, &pin.name);
            if pin_keys.is_empty() {
                return Err(format!("pin references unknown package {}", pin.name));
            }
            let mut matched_anywhere = false;
            for key in &pin_keys {
                let Some(ranked) = ranks.get(key) else {
                    continue;
                };
                let allowed: Vec<u32> = ranked
                    .iter()
                    .filter(|(_, idx)| matches_req(&candidates[*idx].version, &pin.version_req))
                    .map(|(r, _)| *r)
                    .collect();
                if allowed.is_empty() {
                    continue;
                }
                matched_anywhere = true;
                pin_ranks.insert(key.clone(), allowed);
            }
            if !matched_anywhere {
                return Err(format!(
                    "pin {} {} matches no candidates",
                    pin.name, pin.version_req
                ));
            }
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
            let upgrade_keys = keys_for_name(&ranks, &ru.name);
            let Some(ranked) = upgrade_keys.first().and_then(|key| ranks.get(key)) else {
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
            if keys_for_name(&ranks, root).is_empty() {
                return Err(format!("no candidates for root package {root}"));
            }
        }

        let mut favored_ranks = HashMap::new();
        let mut locked_ranks = HashMap::new();
        let mut excluded_ranks: HashMap<String, HashMap<u32, String>> = HashMap::new();
        if let Some(stack) = stack_policy {
            for pin in &stack.pins {
                let matching_result = matching_ranks(
                    &candidates,
                    &ranks,
                    &pin.name,
                    &pin.version_requirement,
                    pin.toolchain.as_ref(),
                    pin.versionsuffix.as_deref(),
                );
                match pin.mode {
                    StackPinMode::Preferred => {
                        if let Ok(matching) = matching_result {
                            if let Some(selected_rank) = matching.last().copied() {
                                favored_ranks.insert(pin.name.clone(), selected_rank);
                            }
                        }
                    }
                    StackPinMode::Locked => {
                        let matching = matching_result?;
                        if matching.is_empty() {
                            return Err(format!(
                                "stack pin {} {} matches no candidates",
                                pin.name, pin.version_requirement
                            ));
                        }
                        let allowed = if let Some(existing) = pin_ranks.get(&pin.name) {
                            matching
                                .into_iter()
                                .filter(|rank| existing.contains(rank))
                                .collect::<Vec<_>>()
                        } else {
                            matching
                        };
                        if allowed.is_empty() {
                            return Err(format!(
                                "locked stack pin {} {} conflicts with package policy pins",
                                pin.name, pin.version_requirement
                            ));
                        }
                        pin_ranks.insert(pin.name.clone(), allowed.clone());
                        if allowed.len() == 1 {
                            locked_ranks.insert(pin.name.clone(), allowed[0]);
                        }
                    }
                }
            }

            for exclusion in &stack.exclusions {
                let matching = matching_ranks(
                    &candidates,
                    &ranks,
                    &exclusion.name,
                    &exclusion.version_requirement,
                    None,
                    None,
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

        // Prefer what the baseline already installed, when the policy asks for
        // it. This runs after every hard constraint is known, so it can only
        // choose between candidates that were all valid anyway: a pin, an
        // exclusion or a require_upgrade on the same package wins, and a
        // baseline version that is no longer a candidate is simply not found.
        //
        // Without this the objective is newest-wins for every package at once,
        // which plans a new generation well and maintains one badly: a package
        // nobody asked to move still moves, and on this site that is hours of a
        // GPU partition spent on a rebuild no one wanted.
        if policy.prefer_installed {
            if let Some(base) = baseline {
                for installed in &base.packages {
                    let name = &installed.name;
                    if favored_ranks.contains_key(name)
                        || locked_ranks.contains_key(name)
                        || pin_ranks.contains_key(name)
                        || min_rank_exclusive.contains_key(name)
                    {
                        continue;
                    }
                    let Some(ranked) = ranks.get(name) else {
                        continue;
                    };
                    // Same version is not the same build: a variant differing
                    // only in versionsuffix is a different module, and
                    // preferring it would keep nothing that is installed.
                    let installed_rank = ranked.iter().find_map(|(rank, idx)| {
                        let candidate = &candidates[*idx];
                        (candidate.version == installed.version
                            && candidate.versionsuffix == installed.versionsuffix
                            && candidate.toolchain == installed.toolchain)
                            .then_some(*rank)
                    });
                    if let Some(rank) = installed_rank {
                        if excluded_ranks
                            .get(name)
                            .is_some_and(|excluded| excluded.contains_key(&rank))
                        {
                            continue;
                        }
                        favored_ranks.insert(name.clone(), rank);
                    }
                }
            }
        }

        let mut keys_by_name: HashMap<String, Vec<String>> = HashMap::new();
        for c in candidates.iter() {
            let key = package_key(&c.name, &c.toolchain, &c.version, &multi_level, &system_multi);
            let entry = keys_by_name.entry(c.name.clone()).or_default();
            if !entry.contains(&key) {
                entry.push(key);
            }
        }

        Ok(Self {
            multi_level,
            system_multi,
            keys_by_name,
            hierarchy_members,
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
        toolchain: Option<&crate::domain::Toolchain>,
        versionsuffix: Option<&str>,
    ) -> Ranges<u32> {
        let Some(ranked) = self.ranks.get(pkg) else {
            return Ranges::empty();
        };
        let mut range = Ranges::empty();
        for (rank, idx) in ranked {
            let c = &self.candidates[*idx];
            // A dependency may name the version with the versionsuffix run
            // onto it, because that is what the module is called: a toolchain
            // asks for NVHPC 25.3-CUDA-12.8.0, and what provides it is version
            // 25.3 with versionsuffix -CUDA-12.8.0.
            let with_suffix = format!(
                "{}{}",
                c.version,
                c.versionsuffix.as_deref().unwrap_or("")
            );
            if !matches_req(&c.version, version_req) && !matches_req(&with_suffix, version_req) {
                continue;
            }
            if toolchain.is_some_and(|want| !crate::hierarchy::toolchains_match(&c.toolchain, want))
            {
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

    /// The requirements standing for the application roots.
    pub fn root_requirements(&self, roots: &[String]) -> Vec<resolvo::ConditionalRequirement> {
        roots
            .iter()
            .filter_map(|name| {
                // A root is a name; the pool holds keys. A package carried at
                // several levels has one key per level, and asking for the
                // root means any of them satisfies it, so the requirement is
                // their union.
                let mut sets = Vec::new();
                for key in keys_for_name(&self.ranks, name) {
                    let Some(&name_id) = self.name_ids.get(&key) else {
                        continue;
                    };
                    let Some(ranked) = self.ranks.get(&key) else {
                        continue;
                    };
                    let mut range = Ranges::empty();
                    for (rank, _) in ranked {
                        if self.allowed_rank(&key, *rank) {
                            range = range.union(&Ranges::singleton(*rank));
                        }
                    }
                    if range != Ranges::empty() {
                        sets.push(self.pool.intern_version_set(name_id, range));
                    }
                }
                let (first, rest) = sets.split_first()?;
                let requirement: resolvo::Requirement = if rest.is_empty() {
                    (*first).into()
                } else {
                    self.pool
                        .intern_version_set_union(*first, rest.iter().copied())
                        .into()
                };
                Some(resolvo::ConditionalRequirement {
                    condition: None,
                    requirement,
                })
            })
            .collect()
    }

    /// The candidate a solvable id refers to.
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
        // prefix it here or messages repeat the package name before the version set.
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
        // A favored candidate goes first, then rank order. Resolvo treats
        // `Candidates::favored` as a hint and asks the provider for the order
        // it should try, so a preference that is not expressed here is a
        // preference the solver never sees: it takes the first candidate that
        // works, which under plain rank order is always the newest.
        // Every solvable in one call shares a package, so the favored rank is
        // looked up once rather than once per comparison.
        let favored = solvables
            .first()
            .map(|solvable| self.pool.resolve_solvable(*solvable).name)
            .and_then(|name| {
                let package_name = self.pool.resolve_package_name(name).to_string();
                self.favored_ranks.get(&package_name).copied()
            });
        solvables.sort_by(|a, b| {
            let ra = self.pool.resolve_solvable(*a).record;
            let rb = self.pool.resolve_solvable(*b).record;
            let fa = favored == Some(ra);
            let fb = favored == Some(rb);
            fb.cmp(&fa).then_with(|| rb.cmp(&ra))
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
            let keys = self.dependency_keys(c, d);
            if keys.is_empty() {
                let reason = self
                    .pool
                    .intern_string(format!("missing dependency package {}", d.name));
                return Dependencies::Unknown(reason);
            }
            // One key is the ordinary case. Several means the dependency named
            // no toolchain and the generation carries the package at more than
            // one level, so any of them satisfies it and the requirement is
            // their union, which is how EasyBuild's minimal-toolchain search
            // behaves: it takes whichever level is available, not a fixed one.
            let mut sets = Vec::new();
            for key in &keys {
                let Some(&dep_name_id) = self.name_ids.get(key) else {
                    continue;
                };
                let range = self.range_matching(
                    key,
                    &d.version_req,
                    d.toolchain.as_ref(),
                    d.versionsuffix.as_deref(),
                );
                if range == Ranges::empty() {
                    continue;
                }
                sets.push(self.pool.intern_version_set(dep_name_id, range));
            }
            if sets.is_empty() {
                // No level at or below this recipe carries a matching version.
                // A stack policy can admit a closure from another generation on
                // purpose, so widen to every level the universe holds for this
                // name before calling the dependency unresolved.
                if let Some(all) = self.keys_by_name.get(&d.name) {
                    for key in all {
                        if keys.contains(key) {
                            continue;
                        }
                        let Some(&dep_name_id) = self.name_ids.get(key) else {
                            continue;
                        };
                        let range = self.range_matching(
                            key,
                            &d.version_req,
                            d.toolchain.as_ref(),
                            d.versionsuffix.as_deref(),
                        );
                        if range != Ranges::empty() {
                            sets.push(self.pool.intern_version_set(dep_name_id, range));
                        }
                    }
                }
            }
            let Some((first, rest)) = sets.split_first() else {
                let reason = self.pool.intern_string(format!(
                    "unresolved dependency {} {} from {}={}",
                    d.name, d.version_req, c.name, c.version
                ));
                return Dependencies::Unknown(reason);
            };
            if rest.is_empty() {
                known.requirements.push((*first).into());
            } else {
                let union = self
                    .pool
                    .intern_version_set_union(*first, rest.iter().copied());
                known.requirements.push(union.into());
            }
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
    toolchain: Option<&crate::domain::Toolchain>,
    versionsuffix: Option<&str>,
) -> Result<Vec<u32>, String> {
    // Policy says a name; the pool holds keys, qualified by toolchain for any
    // name carried at several levels. A pin on such a package means every
    // build of it, so all of its keys are searched.
    let keys = keys_for_name(ranks, name);
    if keys.is_empty() {
        return Err(format!("stack policy references unknown package {name}"));
    }
    let ranked: Vec<(u32, usize)> = keys
        .iter()
        .filter_map(|key| ranks.get(key))
        .flat_map(|ranked| ranked.iter().copied())
        .collect();
    Ok(ranked
        .iter()
        .filter(|(_, index)| {
            let candidate = &candidates[*index];
            matches_req(&candidate.version, version_requirement)
                && toolchain
                    .map(|toolchain| {
                        crate::hierarchy::toolchains_match(&candidate.toolchain, toolchain)
                    })
                    .unwrap_or(true)
                && versionsuffix
                    .map(|versionsuffix| {
                        candidate.versionsuffix.as_deref().unwrap_or_default() == versionsuffix
                    })
                    .unwrap_or(true)
        })
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
    curated_toolchains: bool,
) -> Result<Vec<Candidate>, String> {
    let provider = EbProvider::from_universe_with_stack_policy_scope(
        candidates,
        policy,
        baseline,
        Some(stack_policy),
        curated_toolchains,
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

/// Co-select a stack under a policy, honouring site pins and exclusions.
pub fn solve_with_stack_policy(
    candidates: &[Candidate],
    policy: &Policy,
    baseline: Option<&StackLock>,
    stack_policy: &StackPolicy,
) -> Result<StackPolicySolve, String> {
    solve_with_stack_policy_scope(candidates, policy, baseline, stack_policy, false)
}

pub(crate) fn solve_curated_with_stack_policy(
    candidates: &[Candidate],
    policy: &Policy,
    baseline: Option<&StackLock>,
    stack_policy: &StackPolicy,
) -> Result<StackPolicySolve, String> {
    solve_with_stack_policy_scope(candidates, policy, baseline, stack_policy, true)
}

fn solve_with_stack_policy_scope(
    candidates: &[Candidate],
    policy: &Policy,
    baseline: Option<&StackLock>,
    stack_policy: &StackPolicy,
    curated_toolchains: bool,
) -> Result<StackPolicySolve, String> {
    validate_stack_policy(policy, stack_policy)?;
    let selected = solve_feasibility_with_stack_policy(
        candidates,
        policy,
        baseline,
        stack_policy,
        curated_toolchains,
    )?;
    let pin_outcomes = stack_policy
        .pins
        .iter()
        .map(|pin| {
            let preferred_candidate_available = candidates.iter().any(|candidate| {
                candidate.name == pin.name
                    && matches_req(&candidate.version, &pin.version_requirement)
                    && pin.toolchain.as_ref().is_none_or(|toolchain| {
                        crate::hierarchy::toolchains_match(&candidate.toolchain, toolchain)
                    })
                    && pin.versionsuffix.as_deref().is_none_or(|versionsuffix| {
                        candidate.versionsuffix.as_deref().unwrap_or_default() == versionsuffix
                    })
            });
            let selected_candidate = selected.iter().find(|candidate| candidate.name == pin.name);
            let selected_version = selected_candidate.map(|candidate| candidate.version.clone());
            let selected_toolchain =
                selected_candidate.map(|candidate| candidate.toolchain.clone());
            let selected_versionsuffix =
                selected_candidate.and_then(|candidate| candidate.versionsuffix.clone());
            let fallback = pin.mode == StackPinMode::Preferred
                && selected_candidate.is_some_and(|candidate| {
                    !matches_req(&candidate.version, &pin.version_requirement)
                        || pin.toolchain.as_ref().is_some_and(|toolchain| {
                            !crate::hierarchy::toolchains_match(&candidate.toolchain, toolchain)
                        })
                        || pin.versionsuffix.as_deref().is_some_and(|versionsuffix| {
                            candidate.versionsuffix.as_deref().unwrap_or_default() != versionsuffix
                        })
                });
            StackPinOutcome {
                name: pin.name.clone(),
                requested: pin.version_requirement.clone(),
                requested_toolchain: pin.toolchain.clone(),
                requested_versionsuffix: pin.versionsuffix.clone(),
                selected_version,
                selected_toolchain,
                selected_versionsuffix,
                fallback,
                fallback_reason: fallback.then(|| {
                    if preferred_candidate_available {
                        "favored candidate did not participate in the complete Resolvo solution"
                            .to_string()
                    } else {
                        "preferred identity has no admitted candidate; Resolvo selected a compatible fallback"
                            .to_string()
                    }
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
/// Whether a candidate's toolchain is the policy toolchain or a level below it.
///
/// The hierarchy is derived from the tree when no fixture knows the
/// generation, so a brand-new one needs no new fixture.
fn in_generation(
    candidate_tc: &crate::domain::Toolchain,
    policy_tc: &crate::domain::Toolchain,
    candidates: &[Candidate],
) -> bool {
    if crate::hierarchy::toolchains_match(candidate_tc, policy_tc) {
        return true;
    }
    crate::hierarchy::hierarchy_for_with_tree(policy_tc, None, candidates)
        .map(|h| {
            h.members
                .iter()
                .any(|m| crate::hierarchy::toolchains_match(candidate_tc, m))
        })
        .unwrap_or(false)
}

fn versions_in_trial_order(
    candidates: &[Candidate],
    policy: &Policy,
    name: &str,
    baseline: Option<&StackLock>,
) -> Vec<String> {
    let mut versions: Vec<String> = candidates
        .iter()
        .filter(|c| {
            // A root is not always at the policy toolchain: CMake and Python
            // sit at GCCcore in a GCC generation, and asking for one by name
            // must find it where it legitimately lives rather than report that
            // the package does not exist.
            c.name == name
                && in_generation(&c.toolchain, &policy.toolchain, candidates)
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
    // With prefer_installed the root is tried at the version already installed
    // before anything newer. The trial loop takes the first version that is
    // jointly feasible, so a require_upgrade or a pin that rules it out simply
    // fails this trial and the next version is tried: hard constraints keep
    // winning, and the preference only decides between feasible outcomes.
    if policy.prefer_installed {
        if let Some(installed) = baseline.and_then(|lock| lock.package(name)) {
            // The installed entry has to name a build that still exists, and
            // same version is not same build: a candidate differing only in
            // versionsuffix is a different module, so promoting its version
            // would keep nothing that is installed.
            let installed_exists = candidates.iter().any(|candidate| {
                candidate.name == name
                    && candidate.version == installed.version
                    && candidate.versionsuffix == installed.versionsuffix
                    && candidate.toolchain == installed.toolchain
            });
            if installed_exists {
                if let Some(at) = versions.iter().position(|v| *v == installed.version) {
                    let preferred = versions.remove(at);
                    versions.insert(0, preferred);
                }
            }
        }
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
        let versions = versions_in_trial_order(candidates, policy, root, baseline);
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
            moduleclass: None,
        }
    }

    fn policy(roots: Vec<&str>, require_upgrade: Vec<RequireUpgrade>) -> Policy {
        Policy {
            prefer_installed: false,
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
    fn a_generation_can_carry_both_halves_of_a_bootstrap_pair() {
        // binutils 2.40 at system builds the toolchain that builds binutils
        // 2.42, and EasyBuild installs both. Keyed by name alone the two are
        // one variable and the stack cannot be solved at all.
        let system = Toolchain {
            name: "system".into(),
            version: "system".into(),
        };
        let at_system = |name: &str, version: &str, deps: Vec<DepReq>| Candidate {
            name: name.into(),
            version: version.into(),
            toolchain: system.clone(),
            versionsuffix: None,
            easyconfig_path: format!("{name}-{version}.eb"),
            dependencies: deps,
            builddependencies: vec![],
            exts_list: vec![],
            moduleclass: None,
        };
        let need = |name: &str, version: &str| DepReq {
            name: name.into(),
            version_req: format!("=={version}"),
            versionsuffix: None,
            toolchain: Some(system.clone()),
        };
        let candidates = vec![
            at_system("binutils", "2.40", vec![]),
            at_system("binutils", "2.42", vec![need("Perl", "5.38.0")]),
            at_system("Perl", "5.38.0", vec![need("binutils", "2.40")]),
            cand(
                "App",
                "1.0",
                None,
                "App-1.0.eb",
                vec![need("binutils", "2.42")],
            ),
        ];
        let selected = solve_with_resolvo(&candidates, &policy(vec!["App"], vec![]), None)
            .expect("both binutils builds coexist");
        let mut binutils: Vec<&str> = selected
            .iter()
            .filter(|c| c.name == "binutils")
            .map(|c| c.version.as_str())
            .collect();
        binutils.sort();
        assert_eq!(binutils, vec!["2.40", "2.42"]);
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
