# Campaign Execution Boundary Documentation

*Generated from the current repository state (commit `fba00c15e0f223ea7d400ecb4dfb0218987b155e`).*

## 1. Direct Executions performed by `src/campaign.rs`

| Action | Command / Function | Conditions | File:Line Reference |
|--------|-------------------|------------|---------------------|
| Read campaign bundle manifest | `read_json(&manifest_path)` | Always after acquiring lock | `src/campaign.rs:236-237` |
| Extract package name & version from manifest | `manifest.pointer("/package/name")` and `manifest.pointer("/package/version")` | Must exist in manifest | `src/campaign.rs:239-245` |
| Load recipes list | `load_campaign_recipes(&request.bundle)` | Must return at least one recipe | `src/campaign.rs:246` |
| Discover Resolvo profile locks | `discover_files(&request.bundle.join("locks"), "json")` | Must find at least one lock file | `src/campaign.rs:252-256` |
| Initialise or load campaign state JSON | `read_json(&request.state_path)` or construct new `CampaignState` | If state file exists, schema must match | `src/campaign.rs:259-268` |
| Record interrupted previous attempt (if any) | `record_interrupted_attempt(&mut state, &request.state_path)` | When previous state was `Running` | `src/campaign.rs:580-615` |
| Write updated state to disk (atomic) | `write_state(&request.state_path, &state)` | After each state transition | Multiple locations, e.g. `src/campaign.rs:300`, `src/campaign.rs:350`, `src/campaign.rs:390`, `src/campaign.rs:402`, `src/campaign.rs:423`, `src/campaign.rs:460`, `src/campaign.rs:474`, `src/campaign.rs:510`, `src/campaign.rs:550`, `src/campaign.rs:560`, `src/campaign.rs:576` |
| Pre‑flight packaging gate for each recipe | `packaging_gate(&resolved, &[])` | Runs before any build; on error creates a `BuildFinding` with class `Checksum` or `Unknown` | `src/campaign.rs:313-342` |
| Stage bundle on target | `request.target.stage_bundle(&request.bundle)` | May fail with transport error | `src/campaign.rs:355-393` |
| Build each recipe on target | `request.target.build_command_with_robot_paths(&staged_recipe.display().to_string(), &[staged_overlay])` then `command.execute()` | Executes EasyBuild command; on failure creates a `BuildFinding` | `src/campaign.rs:407-426` |
| Verify binaries for each profile | `request.target.verification_command(&program, &args)` then `command.execute()` | Runs verification commands; on failure creates a `BuildFinding` | `src/campaign.rs:498-514` |
| Finalise campaign state as `Completed` | Set `state.status = CampaignStatus::Completed` and write state | After all builds and verification succeed | `src/campaign.rs:564-576` |

## 2. Files Written (but not executed) and Their Consumers

| File | Description | Who/What Consumes It |
|------|-------------|----------------------|
| `state_path` (e.g. `campaign.json`) | Persistent JSON representation of the campaign state, including history, findings, claims | Human operators, other campaign runs, debugging tools |
| `state_path.lock` and `state_path.lock.guard` | Exclusive lock metadata to prevent concurrent campaigns | `CampaignLock::acquire` ensures only one process holds the lock |
| Temporary `*.tmp` files during state writes | Atomic write‑then‑rename to avoid partial state files | The `write_state` helper (`src/campaign.rs:1089-1102`) |
| `campaign.lock` metadata JSON (`*.lock`) | Records host, PID, start ticks for the lock holder | Lock acquisition / release logic (`CampaignLock::acquire` and `Drop`) |
| `campaign.findings` entries inside the state JSON | Records failures, interruptions, and their evidences | Human reviewers, `eb-stack campaign find` commands |
| `campaign.history` entries inside the state JSON | Chronological log of actions (e.g., "build evaluation on <target>") | Auditing, debugging |

## 3. Refusals (Error Paths) – Where They Are Raised

### In `src/campaign.rs`

| Error Variant | Situation | Where Raised (File:Line) |
|---------------|-----------|--------------------------|
| `CampaignError::InvalidBundle` | Missing required fields in bundle (e.g., no recipes, no locks, missing manifest entries) | `src/campaign.rs:248-256`, `src/campaign.rs:241-245` |
| `CampaignError::UnsupportedSchema` | State file schema version does not match `CAMPAIGN_SCHEMA_VERSION` | `src/campaign.rs:262-263` |
| `CampaignError::StateIdentity` | Existing state file refers to a different package/version than bundle | `src/campaign.rs:264-267` |
| `CampaignError::Busy` | Another process holds the campaign lock (`CampaignLock::acquire`) | `src/campaign.rs:1110-1118` (inside `CampaignLock::acquire`) |
| `CampaignError::Target` (wraps `TargetError`) | Target command could not be started (e.g., SSH transport, Slurm executor, Docker runtime) | `src/campaign.rs:415-426` via `record_target_command_failure` |
| `CampaignError::Io` / `Json` | File system or JSON (de)serialization failures | Various `write_state` / `read_json` calls (e.g., `src/campaign.rs:1089-1102`) |
| `CampaignError::FindingNotFound`, `FindingOwned`, `FindingState` | Operations on campaign findings that are invalid (e.g., claiming an already‑owned finding) | `src/campaign.rs:618-655` (`claim_finding`) and `src/campaign.rs:657-689` (`resolve_finding`) |

### In `src/foreign.rs`

| Error Variant | Situation | Where Raised (File:Line) |
|---------------|-----------|--------------------------|
| `ForeignError::Parse` | Recipe could not be parsed as YAML or Python AST | `src/foreign.rs:456-462` (conda parsing) and `src/foreign.rs:1296-1300` (Spack parsing) |
| `ForeignError::Unsupported` | File extension/name not recognised as a supported foreign format | `src/foreign.rs:269-283` (`detect_foreign_format`) |
| `ForeignError::Io` | Underlying file read error | `src/foreign.rs:298-301` (`parse_foreign_path`) |

## 4. Operator Model (Human / Agent Requirements)

| Requirement | Explanation |
|-------------|-------------|
| **Easyconfigs tree** | The campaign needs a populated Easyconfigs directory (`EB_EASYCONFIGS`) that matches the bundle’s `easyconfigs/` sub‑directory. This is used by `resolve_easyconfig_file` and by the target’s EasyBuild robot paths. |
| **Target configuration** | A `BuildTarget` (local, SSH, Slurm, Podman/Docker) must be provided in `CampaignRequest`. It defines how commands are executed, transport, executor, and runtime. |
| **Credentials / Access** | For remote targets (SSH, container registries) the operator must ensure appropriate authentication (SSH keys, token files) are available in the environment; the code does not manage credentials itself. |
| **State file location** | `request.state_path` must be writable by the operator. The system guarantees exclusive access via a lock file (`*.lock`) and atomic writes (`*.tmp` → final). |
| **Bundle integrity** | The operator must provide a valid bundle directory containing `package.plan.json`, `easyconfigs/`, and `locks/`. The system validates these before proceeding and aborts with `InvalidBundle` if missing. |
| **Guaranteed outcomes** | *Atomic state persistence*: each state transition is written atomically and recorded in the history. *Exclusive execution*: only one campaign can run against a given state file at a time. *Failure classification*: any command failure is classified into a `BuildFindingClass` with a deterministic disposition, enabling automated retry or human judgment. |

---

*This document is generated from the source code.*
