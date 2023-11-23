use {
    crate::{
        invoke_context::{BuiltinFunctionWithContext, InvokeContext},
        timings::ExecuteDetailsTimings,
    },
    itertools::Itertools,
    log::{debug, error, log_enabled, trace},
    percentage::PercentageInteger,
    solana_measure::measure::Measure,
    solana_rbpf::{
        elf::Executable,
        program::{BuiltinProgram, FunctionRegistry},
        verifier::RequisiteVerifier,
        vm::Config,
    },
    solana_sdk::{
        bpf_loader, bpf_loader_deprecated, bpf_loader_upgradeable,
        clock::{Epoch, Slot},
        loader_v4,
        pubkey::Pubkey,
        saturating_add_assign,
    },
    std::{
        collections::HashMap,
        fmt::{Debug, Formatter},
        sync::{
            atomic::{AtomicU64, Ordering},
            Arc, Mutex, RwLock,
        },
    },
};

pub type ProgramRuntimeEnvironment = Arc<BuiltinProgram<InvokeContext<'static>>>;
pub const MAX_LOADED_ENTRY_COUNT: usize = 256;
pub const DELAY_VISIBILITY_SLOT_OFFSET: Slot = 1;

/// Relationship between two fork IDs
#[derive(Copy, Clone, Debug, PartialEq)]
pub enum BlockRelation {
    /// The slot is on the same fork and is an ancestor of the other slot
    Ancestor,
    /// The two slots are equal and are on the same fork
    Equal,
    /// The slot is on the same fork and is a descendant of the other slot
    Descendant,
    /// The slots are on two different forks and may have had a common ancestor at some point
    Unrelated,
    /// Either one or both of the slots are either older than the latest root, or are in future
    Unknown,
}

/// Maps relationship between two slots.
pub trait ForkGraph {
    /// Returns the BlockRelation of A to B
    fn relationship(&self, a: Slot, b: Slot) -> BlockRelation;

    /// Returns the epoch of the given slot
    fn slot_epoch(&self, _slot: Slot) -> Option<Epoch> {
        Some(0)
    }
}

/// Provides information about current working slot, and its ancestors
pub trait WorkingSlot {
    /// Returns the current slot
    fn current_slot(&self) -> Slot;

    /// Returns the epoch of the current slot
    fn current_epoch(&self) -> Epoch;

    /// Returns true if the `other` slot is an ancestor of self, false otherwise
    fn is_ancestor(&self, other: Slot) -> bool;
}

#[derive(Default)]
pub enum LoadedProgramType {
    /// Tombstone for undeployed, closed or unloadable programs
    FailedVerification(ProgramRuntimeEnvironment),
    #[default]
    Closed,
    DelayVisibility,
    /// Successfully verified but not currently compiled, used to track usage statistics when a compiled program is evicted from memory.
    Unloaded(ProgramRuntimeEnvironment),
    LegacyV0(Executable<InvokeContext<'static>>),
    LegacyV1(Executable<InvokeContext<'static>>),
    Typed(Executable<InvokeContext<'static>>),
    #[cfg(test)]
    TestLoaded(ProgramRuntimeEnvironment),
    Builtin(BuiltinProgram<InvokeContext<'static>>),
}

impl Debug for LoadedProgramType {
    fn fmt(&self, f: &mut Formatter<'_>) -> std::fmt::Result {
        match self {
            LoadedProgramType::FailedVerification(_) => {
                write!(f, "LoadedProgramType::FailedVerification")
            }
            LoadedProgramType::Closed => write!(f, "LoadedProgramType::Closed"),
            LoadedProgramType::DelayVisibility => write!(f, "LoadedProgramType::DelayVisibility"),
            LoadedProgramType::Unloaded(_) => write!(f, "LoadedProgramType::Unloaded"),
            LoadedProgramType::LegacyV0(_) => write!(f, "LoadedProgramType::LegacyV0"),
            LoadedProgramType::LegacyV1(_) => write!(f, "LoadedProgramType::LegacyV1"),
            LoadedProgramType::Typed(_) => write!(f, "LoadedProgramType::Typed"),
            #[cfg(test)]
            LoadedProgramType::TestLoaded(_) => write!(f, "LoadedProgramType::TestLoaded"),
            LoadedProgramType::Builtin(_) => write!(f, "LoadedProgramType::Builtin"),
        }
    }
}

impl LoadedProgramType {
    /// Returns a reference to its environment if it has one
    pub fn get_environment(&self) -> Option<&ProgramRuntimeEnvironment> {
        match self {
            LoadedProgramType::LegacyV0(program)
            | LoadedProgramType::LegacyV1(program)
            | LoadedProgramType::Typed(program) => Some(program.get_loader()),
            LoadedProgramType::FailedVerification(env) | LoadedProgramType::Unloaded(env) => {
                Some(env)
            }
            #[cfg(test)]
            LoadedProgramType::TestLoaded(environment) => Some(environment),
            _ => None,
        }
    }
}

#[derive(Debug, Default)]
pub struct LoadedProgram {
    /// The program of this entry
    pub program: LoadedProgramType,
    /// Size of account that stores the program and program data
    pub account_size: usize,
    /// Slot in which the program was (re)deployed
    pub deployment_slot: Slot,
    /// Slot in which this entry will become active (can be in the future)
    pub effective_slot: Slot,
    /// Optional expiration slot for this entry, after which it is treated as non-existent
    pub maybe_expiration_slot: Option<Slot>,
    /// How often this entry was used by a transaction
    pub tx_usage_counter: AtomicU64,
    /// How often this entry was used by a transaction
    pub ix_usage_counter: AtomicU64,
}

#[derive(Debug, Default)]
pub struct Stats {
    pub hits: AtomicU64,
    pub misses: AtomicU64,
    pub evictions: HashMap<Pubkey, u64>,
    pub insertions: AtomicU64,
    pub replacements: AtomicU64,
    pub one_hit_wonders: AtomicU64,
    pub prunes_orphan: AtomicU64,
    pub prunes_expired: AtomicU64,
    pub prunes_environment: AtomicU64,
    pub empty_entries: AtomicU64,
}

impl Stats {
    /// Logs the measurement values
    pub fn submit(&self, slot: Slot) {
        let hits = self.hits.load(Ordering::Relaxed);
        let misses = self.misses.load(Ordering::Relaxed);
        let insertions = self.insertions.load(Ordering::Relaxed);
        let replacements = self.replacements.load(Ordering::Relaxed);
        let one_hit_wonders = self.one_hit_wonders.load(Ordering::Relaxed);
        let evictions: u64 = self.evictions.values().sum();
        let prunes_orphan = self.prunes_orphan.load(Ordering::Relaxed);
        let prunes_expired = self.prunes_expired.load(Ordering::Relaxed);
        let prunes_environment = self.prunes_environment.load(Ordering::Relaxed);
        let empty_entries = self.empty_entries.load(Ordering::Relaxed);
        datapoint_info!(
            "loaded-programs-cache-stats",
            ("slot", slot, i64),
            ("hits", hits, i64),
            ("misses", misses, i64),
            ("evictions", evictions, i64),
            ("insertions", insertions, i64),
            ("replacements", replacements, i64),
            ("one_hit_wonders", one_hit_wonders, i64),
            ("prunes_orphan", prunes_orphan, i64),
            ("prunes_expired", prunes_expired, i64),
            ("prunes_environment", prunes_environment, i64),
            ("empty_entries", empty_entries, i64),
        );
        debug!(
            "Loaded Programs Cache Stats -- Hits: {}, Misses: {}, Evictions: {}, Insertions: {}, Replacements: {}, One-Hit-Wonders: {}, Prunes-Orphan: {}, Prunes-Expired: {}, Prunes-Environment: {}, Empty: {}",
            hits, misses, evictions, insertions, replacements, one_hit_wonders, prunes_orphan, prunes_expired, prunes_environment, empty_entries
        );
        if log_enabled!(log::Level::Trace) && !self.evictions.is_empty() {
            let mut evictions = self.evictions.iter().collect::<Vec<_>>();
            evictions.sort_by_key(|e| e.1);
            let evictions = evictions
                .into_iter()
                .rev()
                .map(|(program_id, evictions)| {
                    format!("  {:<44}  {}", program_id.to_string(), evictions)
                })
                .collect::<Vec<_>>();
            let evictions = evictions.join("\n");
            trace!(
                "Eviction Details:\n  {:<44}  {}\n{}",
                "Program",
                "Count",
                evictions
            );
        }
    }

    pub fn reset(&mut self) {
        *self = Stats::default();
    }
}

#[derive(Debug, Default)]
pub struct LoadProgramMetrics {
    pub program_id: String,
    pub register_syscalls_us: u64,
    pub load_elf_us: u64,
    pub verify_code_us: u64,
    pub jit_compile_us: u64,
}

impl LoadProgramMetrics {
    pub fn submit_datapoint(&self, timings: &mut ExecuteDetailsTimings) {
        saturating_add_assign!(
            timings.create_executor_register_syscalls_us,
            self.register_syscalls_us
        );
        saturating_add_assign!(timings.create_executor_load_elf_us, self.load_elf_us);
        saturating_add_assign!(timings.create_executor_verify_code_us, self.verify_code_us);
        saturating_add_assign!(timings.create_executor_jit_compile_us, self.jit_compile_us);
        datapoint_trace!(
            "create_executor_trace",
            ("program_id", self.program_id, String),
            ("register_syscalls_us", self.register_syscalls_us, i64),
            ("load_elf_us", self.load_elf_us, i64),
            ("verify_code_us", self.verify_code_us, i64),
            ("jit_compile_us", self.jit_compile_us, i64),
        );
    }
}

impl PartialEq for LoadedProgram {
    fn eq(&self, other: &Self) -> bool {
        self.effective_slot == other.effective_slot
            && self.deployment_slot == other.deployment_slot
            && self.is_tombstone() == other.is_tombstone()
    }
}

impl LoadedProgram {
    /// Creates a new user program
    pub fn new(
        loader_key: &Pubkey,
        program_runtime_environment: ProgramRuntimeEnvironment,
        deployment_slot: Slot,
        effective_slot: Slot,
        maybe_expiration_slot: Option<Slot>,
        elf_bytes: &[u8],
        account_size: usize,
        metrics: &mut LoadProgramMetrics,
    ) -> Result<Self, Box<dyn std::error::Error>> {
        Self::new_internal(
            loader_key,
            program_runtime_environment,
            deployment_slot,
            effective_slot,
            maybe_expiration_slot,
            elf_bytes,
            account_size,
            metrics,
            false, /* reloading */
        )
    }

    /// Reloads a user program, *without* running the verifier.
    ///
    /// # Safety
    ///
    /// This method is unsafe since it assumes that the program has already been verified. Should
    /// only be called when the program was previously verified and loaded in the cache, but was
    /// unloaded due to inactivity. It should also be checked that the `program_runtime_environment`
    /// hasn't changed since it was unloaded.
    pub unsafe fn reload(
        loader_key: &Pubkey,
        program_runtime_environment: Arc<BuiltinProgram<InvokeContext<'static>>>,
        deployment_slot: Slot,
        effective_slot: Slot,
        maybe_expiration_slot: Option<Slot>,
        elf_bytes: &[u8],
        account_size: usize,
        metrics: &mut LoadProgramMetrics,
    ) -> Result<Self, Box<dyn std::error::Error>> {
        Self::new_internal(
            loader_key,
            program_runtime_environment,
            deployment_slot,
            effective_slot,
            maybe_expiration_slot,
            elf_bytes,
            account_size,
            metrics,
            true, /* reloading */
        )
    }

    fn new_internal(
        loader_key: &Pubkey,
        program_runtime_environment: Arc<BuiltinProgram<InvokeContext<'static>>>,
        deployment_slot: Slot,
        effective_slot: Slot,
        maybe_expiration_slot: Option<Slot>,
        elf_bytes: &[u8],
        account_size: usize,
        metrics: &mut LoadProgramMetrics,
        reloading: bool,
    ) -> Result<Self, Box<dyn std::error::Error>> {
        let mut load_elf_time = Measure::start("load_elf_time");
        // The following unused_mut exception is needed for architectures that do not
        // support JIT compilation.
        #[allow(unused_mut)]
        let mut executable = Executable::load(elf_bytes, program_runtime_environment.clone())?;
        load_elf_time.stop();
        metrics.load_elf_us = load_elf_time.as_us();

        if !reloading {
            let mut verify_code_time = Measure::start("verify_code_time");
            executable.verify::<RequisiteVerifier>()?;
            verify_code_time.stop();
            metrics.verify_code_us = verify_code_time.as_us();
        }

        #[cfg(all(not(target_os = "windows"), target_arch = "x86_64"))]
        {
            let mut jit_compile_time = Measure::start("jit_compile_time");
            executable.jit_compile()?;
            jit_compile_time.stop();
            metrics.jit_compile_us = jit_compile_time.as_us();
        }

        // Allowing mut here, since it may be needed for jit compile, which is under a config flag
        #[allow(unused_mut)]
        let mut program = if bpf_loader_deprecated::check_id(loader_key) {
            LoadedProgramType::LegacyV0(executable)
        } else if bpf_loader::check_id(loader_key) || bpf_loader_upgradeable::check_id(loader_key) {
            LoadedProgramType::LegacyV1(executable)
        } else if loader_v4::check_id(loader_key) {
            LoadedProgramType::Typed(executable)
        } else {
            panic!();
        };

        Ok(Self {
            deployment_slot,
            account_size,
            effective_slot,
            maybe_expiration_slot,
            tx_usage_counter: AtomicU64::new(0),
            program,
            ix_usage_counter: AtomicU64::new(0),
        })
    }

    pub fn to_unloaded(&self) -> Option<Self> {
        Some(Self {
            program: LoadedProgramType::Unloaded(self.program.get_environment()?.clone()),
            account_size: self.account_size,
            deployment_slot: self.deployment_slot,
            effective_slot: self.effective_slot,
            maybe_expiration_slot: self.maybe_expiration_slot,
            tx_usage_counter: AtomicU64::new(self.tx_usage_counter.load(Ordering::Relaxed)),
            ix_usage_counter: AtomicU64::new(self.tx_usage_counter.load(Ordering::Relaxed)),
        })
    }

    /// Creates a new built-in program
    pub fn new_builtin(
        deployment_slot: Slot,
        account_size: usize,
        builtin_function: BuiltinFunctionWithContext,
    ) -> Self {
        let mut function_registry = FunctionRegistry::default();
        function_registry
            .register_function_hashed(*b"entrypoint", builtin_function)
            .unwrap();
        Self {
            deployment_slot,
            account_size,
            effective_slot: deployment_slot,
            maybe_expiration_slot: None,
            tx_usage_counter: AtomicU64::new(0),
            program: LoadedProgramType::Builtin(BuiltinProgram::new_builtin(function_registry)),
            ix_usage_counter: AtomicU64::new(0),
        }
    }

    pub fn new_tombstone(slot: Slot, reason: LoadedProgramType) -> Self {
        let maybe_expiration_slot = matches!(reason, LoadedProgramType::DelayVisibility)
            .then_some(slot.saturating_add(DELAY_VISIBILITY_SLOT_OFFSET));
        let tombstone = Self {
            program: reason,
            account_size: 0,
            deployment_slot: slot,
            effective_slot: slot,
            maybe_expiration_slot,
            tx_usage_counter: AtomicU64::default(),
            ix_usage_counter: AtomicU64::default(),
        };
        debug_assert!(tombstone.is_tombstone());
        tombstone
    }

    pub fn is_tombstone(&self) -> bool {
        matches!(
            self.program,
            LoadedProgramType::FailedVerification(_)
                | LoadedProgramType::Closed
                | LoadedProgramType::DelayVisibility
        )
    }

    fn is_implicit_delay_visibility_tombstone(&self, slot: Slot) -> bool {
        !matches!(self.program, LoadedProgramType::Builtin(_))
            && self.effective_slot.saturating_sub(self.deployment_slot)
                == DELAY_VISIBILITY_SLOT_OFFSET
            && slot >= self.deployment_slot
            && slot < self.effective_slot
    }
}

#[derive(Clone, Debug)]
pub struct ProgramRuntimeEnvironments {
    /// Globally shared RBPF config and syscall registry for runtime V1
    pub program_runtime_v1: ProgramRuntimeEnvironment,
    /// Globally shared RBPF config and syscall registry for runtime V2
    pub program_runtime_v2: ProgramRuntimeEnvironment,
}

impl Default for ProgramRuntimeEnvironments {
    fn default() -> Self {
        let empty_loader = Arc::new(BuiltinProgram::new_loader(
            Config::default(),
            FunctionRegistry::default(),
        ));
        Self {
            program_runtime_v1: empty_loader.clone(),
            program_runtime_v2: empty_loader,
        }
    }
}

pub struct LoadedPrograms<FG: ForkGraph> {
    /// A two level index:
    ///
    /// Pubkey is the address of a program, multiple versions can coexists simultaneously under the same address (in different slots).
    entries: HashMap<Pubkey, Vec<Arc<LoadedProgram>>>,
    /// The slot of the last rerooting
    pub latest_root_slot: Slot,
    /// The epoch of the last rerooting
    pub latest_root_epoch: Epoch,
    /// Environments of the current epoch
    pub environments: ProgramRuntimeEnvironments,
    /// Anticipated replacement for `environments` at the next epoch
    ///
    /// This is `None` during most of an epoch, and only `Some` around the boundaries (at the end and beginning of an epoch).
    /// More precisely, it starts with the recompilation phase a few hundred slots before the epoch boundary,
    /// and it ends with the first rerooting after the epoch boundary.
    pub upcoming_environments: Option<ProgramRuntimeEnvironments>,
    /// List of loaded programs which should be recompiled before the next epoch (but don't have to).
    pub programs_to_recompile: Vec<(Pubkey, Arc<LoadedProgram>)>,
    pub stats: Stats,
    pub fork_graph: Option<Arc<RwLock<FG>>>,
}

impl<FG: ForkGraph> Debug for LoadedPrograms<FG> {
    fn fmt(&self, f: &mut Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("LoadedPrograms")
            .field("root slot", &self.latest_root_slot)
            .field("root epoch", &self.latest_root_epoch)
            .field("stats", &self.stats)
            .field("cache", &self.entries)
            .finish()
    }
}

impl<FG: ForkGraph> Default for LoadedPrograms<FG> {
    fn default() -> Self {
        Self {
            entries: HashMap::new(),
            latest_root_slot: 0,
            latest_root_epoch: 0,
            environments: ProgramRuntimeEnvironments::default(),
            upcoming_environments: None,
            programs_to_recompile: Vec::default(),
            stats: Stats::default(),
            fork_graph: None,
        }
    }
}

#[derive(Clone, Debug, Default)]
pub struct LoadedProgramsForTxBatch {
    /// Pubkey is the address of a program.
    /// LoadedProgram is the corresponding program entry valid for the slot in which a transaction is being executed.
    entries: HashMap<Pubkey, Arc<LoadedProgram>>,
    slot: Slot,
    pub environments: ProgramRuntimeEnvironments,
}

pub struct ExtractedPrograms {
    pub loaded: LoadedProgramsForTxBatch,
    pub missing: HashMap<Pubkey, (u64, bool)>,
}

impl LoadedProgramsForTxBatch {
    pub fn new(slot: Slot, environments: ProgramRuntimeEnvironments) -> Self {
        Self {
            entries: HashMap::new(),
            slot,
            environments,
        }
    }

    /// Refill the cache with a single entry. It's typically called during transaction loading, and
    /// transaction processing (for program management instructions).
    /// It replaces the existing entry (if any) with the provided entry. The return value contains
    /// `true` if an entry existed.
    /// The function also returns the newly inserted value.
    pub fn replenish(
        &mut self,
        key: Pubkey,
        entry: Arc<LoadedProgram>,
    ) -> (bool, Arc<LoadedProgram>) {
        (self.entries.insert(key, entry.clone()).is_some(), entry)
    }

    pub fn find(&self, key: &Pubkey) -> Option<Arc<LoadedProgram>> {
        self.entries.get(key).map(|entry| {
            if entry.is_implicit_delay_visibility_tombstone(self.slot) {
                // Found a program entry on the current fork, but it's not effective
                // yet. It indicates that the program has delayed visibility. Return
                // the tombstone to reflect that.
                Arc::new(LoadedProgram::new_tombstone(
                    entry.deployment_slot,
                    LoadedProgramType::DelayVisibility,
                ))
            } else {
                entry.clone()
            }
        })
    }

    pub fn slot(&self) -> Slot {
        self.slot
    }

    pub fn set_slot_for_tests(&mut self, slot: Slot) {
        self.slot = slot;
    }

    pub fn merge(&mut self, other: &Self) {
        other.entries.iter().for_each(|(key, entry)| {
            self.replenish(*key, entry.clone());
        })
    }
}

pub enum LoadedProgramMatchCriteria {
    DeployedOnOrAfterSlot(Slot),
    Tombstone,
    NoCriteria,
}

impl<FG: ForkGraph> LoadedPrograms<FG> {
    pub fn set_fork_graph(&mut self, fork_graph: Arc<RwLock<FG>>) {
        self.fork_graph = Some(fork_graph);
    }

    /// Returns the current environments depending on the given epoch
    pub fn get_environments_for_epoch(&self, epoch: Epoch) -> &ProgramRuntimeEnvironments {
        if epoch != self.latest_root_epoch {
            if let Some(upcoming_environments) = self.upcoming_environments.as_ref() {
                return upcoming_environments;
            }
        }
        &self.environments
    }

    /// Refill the cache with a single entry. It's typically called during transaction loading,
    /// when the cache doesn't contain the entry corresponding to program `key`.
    /// The function dedupes the cache, in case some other thread replenished the entry in parallel.
    pub fn replenish(
        &mut self,
        key: Pubkey,
        entry: Arc<LoadedProgram>,
    ) -> (bool, Arc<LoadedProgram>) {
        let second_level = self.entries.entry(key).or_default();
        let index = second_level
            .iter()
            .position(|at| at.effective_slot >= entry.effective_slot);
        if let Some((existing, entry_index)) =
            index.and_then(|index| second_level.get(index).map(|value| (value, index)))
        {
            if existing.deployment_slot == entry.deployment_slot
                && existing.effective_slot == entry.effective_slot
            {
                if matches!(existing.program, LoadedProgramType::Unloaded(_)) {
                    // The unloaded program is getting reloaded
                    // Copy over the usage counter to the new entry
                    let mut usage_count = existing.tx_usage_counter.load(Ordering::Relaxed);
                    saturating_add_assign!(
                        usage_count,
                        entry.tx_usage_counter.load(Ordering::Relaxed)
                    );
                    entry.tx_usage_counter.store(usage_count, Ordering::Relaxed);
                    entry.ix_usage_counter.store(
                        existing.ix_usage_counter.load(Ordering::Relaxed),
                        Ordering::Relaxed,
                    );
                    second_level.remove(entry_index);
                } else if existing.is_tombstone() != entry.is_tombstone() {
                    // Either the old entry is tombstone and the new one is not.
                    // (Let's give the new entry a chance).
                    // Or, the old entry is not a tombstone and the new one is a tombstone.
                    // (Remove the old entry, as the tombstone makes it obsolete).
                    second_level.remove(entry_index);
                } else {
                    self.stats.replacements.fetch_add(1, Ordering::Relaxed);
                    return (true, existing.clone());
                }
            }
        }
        self.stats.insertions.fetch_add(1, Ordering::Relaxed);
        second_level.insert(index.unwrap_or(second_level.len()), entry.clone());
        (false, entry)
    }

    /// Assign the program `entry` to the given `key` in the cache.
    /// This is typically called when a deployed program is managed (un-/re-/deployed) via
    /// loader instructions. Because of the cooldown, entires can not have the same
    /// deployment_slot and effective_slot.
    pub fn assign_program(&mut self, key: Pubkey, entry: Arc<LoadedProgram>) -> Arc<LoadedProgram> {
        let (was_occupied, entry) = self.replenish(key, entry);
        debug_assert!(!was_occupied);
        entry
    }

    pub fn prune_by_deployment_slot(&mut self, slot: Slot) {
        self.entries.retain(|_key, second_level| {
            *second_level = second_level
                .iter()
                .filter(|entry| entry.deployment_slot != slot)
                .cloned()
                .collect();
            !second_level.is_empty()
        });
        self.remove_programs_with_no_entries();
    }

    /// Before rerooting the blockstore this removes all superfluous entries
    pub fn prune(&mut self, new_root_slot: Slot, new_root_epoch: Epoch) {
        let Some(fork_graph) = self.fork_graph.clone() else {
            error!("Program cache doesn't have fork graph.");
            return;
        };
        let Ok(fork_graph) = fork_graph.read() else {
            error!("Failed to lock fork graph for reading.");
            return;
        };
        let mut recompilation_phase_ends = false;
        if self.latest_root_epoch != new_root_epoch {
            self.latest_root_epoch = new_root_epoch;
            if let Some(upcoming_environments) = self.upcoming_environments.take() {
                recompilation_phase_ends = true;
                self.environments = upcoming_environments;
                self.programs_to_recompile.clear();
            }
        }
        for second_level in self.entries.values_mut() {
            // Remove entries un/re/deployed on orphan forks
            let mut first_ancestor_found = false;
            let mut first_ancestor_env = None;
            *second_level = second_level
                .iter()
                .rev()
                .filter(|entry| {
                    let relation = fork_graph.relationship(entry.deployment_slot, new_root_slot);
                    if entry.deployment_slot >= new_root_slot {
                        matches!(relation, BlockRelation::Equal | BlockRelation::Descendant)
                    } else if matches!(relation, BlockRelation::Ancestor)
                        || entry.deployment_slot <= self.latest_root_slot
                    {
                        if !first_ancestor_found {
                            first_ancestor_found = true;
                            first_ancestor_env = entry.program.get_environment();
                            return true;
                        }
                        // Do not prune the entry if the runtime environment of the entry is different
                        // than the entry that was previously found (stored in first_ancestor_env).
                        // Different environment indicates that this entry might belong to an older
                        // epoch that had a different environment (e.g. different feature set).
                        // Once the root moves to the new/current epoch, the entry will get pruned.
                        // But, until then the entry might still be getting used by an older slot.
                        if let Some(entry_env) = entry.program.get_environment() {
                            if let Some(env) = first_ancestor_env {
                                if !Arc::ptr_eq(entry_env, env) {
                                    return true;
                                }
                            }
                        }
                        self.stats.prunes_orphan.fetch_add(1, Ordering::Relaxed);
                        false
                    } else {
                        self.stats.prunes_orphan.fetch_add(1, Ordering::Relaxed);
                        false
                    }
                })
                .filter(|entry| {
                    // Remove expired
                    if let Some(expiration) = entry.maybe_expiration_slot {
                        if expiration <= new_root_slot {
                            self.stats.prunes_expired.fetch_add(1, Ordering::Relaxed);
                            return false;
                        }
                    }
                    // Remove outdated environment of previous feature set
                    if recompilation_phase_ends
                        && !Self::matches_environment(entry, &self.environments)
                    {
                        self.stats
                            .prunes_environment
                            .fetch_add(1, Ordering::Relaxed);
                        return false;
                    }
                    true
                })
                .cloned()
                .collect();
            second_level.reverse();
        }
        self.remove_programs_with_no_entries();
        debug_assert!(self.latest_root_slot <= new_root_slot);
        self.latest_root_slot = new_root_slot;
    }

    fn matches_environment(
        entry: &Arc<LoadedProgram>,
        environments: &ProgramRuntimeEnvironments,
    ) -> bool {
        let Some(environment) = entry.program.get_environment() else {
            return true;
        };
        Arc::ptr_eq(environment, &environments.program_runtime_v1)
            || Arc::ptr_eq(environment, &environments.program_runtime_v2)
    }

    fn matches_loaded_program_criteria(
        program: &Arc<LoadedProgram>,
        criteria: &LoadedProgramMatchCriteria,
    ) -> bool {
        match criteria {
            LoadedProgramMatchCriteria::DeployedOnOrAfterSlot(slot) => {
                program.deployment_slot >= *slot
            }
            LoadedProgramMatchCriteria::Tombstone => program.is_tombstone(),
            LoadedProgramMatchCriteria::NoCriteria => true,
        }
    }

    fn is_entry_usable(
        entry: &Arc<LoadedProgram>,
        current_slot: Slot,
        match_criteria: &LoadedProgramMatchCriteria,
    ) -> bool {
        if entry
            .maybe_expiration_slot
            .map(|expiration_slot| expiration_slot <= current_slot)
            .unwrap_or(false)
        {
            // Found an entry that's already expired. Any further entries in the list
            // are older than the current one. So treat the program as missing in the
            // cache and return early.
            return false;
        }

        Self::matches_loaded_program_criteria(entry, match_criteria)
    }

    /// Extracts a subset of the programs relevant to a transaction batch
    /// and returns which program accounts the accounts DB needs to load.
    pub fn extract<S: WorkingSlot>(
        &self,
        working_slot: &S,
        keys: impl Iterator<Item = (Pubkey, (LoadedProgramMatchCriteria, u64))>,
    ) -> Arc<Mutex<ExtractedPrograms>> {
        let environments = self.get_environments_for_epoch(working_slot.current_epoch());
        let current_slot = working_slot.current_slot();
        let extracted = Arc::new(Mutex::new(ExtractedPrograms {
            loaded: LoadedProgramsForTxBatch {
                entries: HashMap::new(),
                slot: current_slot,
                environments: environments.clone(),
            },
            missing: HashMap::new(),
        }));
        let mut extracting = extracted.lock().unwrap();
        extracting.loaded.entries = keys
            .filter_map(|(key, (match_criteria, count))| {
                let mut reloading = false;
                if let Some(second_level) = self.entries.get(&key) {
                    for entry in second_level.iter().rev() {
                        let is_ancestor = if let Some(fork_graph) = &self.fork_graph {
                            fork_graph
                                .read()
                                .map(|fork_graph_r| {
                                    matches!(
                                        fork_graph_r
                                            .relationship(entry.deployment_slot, current_slot),
                                        BlockRelation::Ancestor
                                    )
                                })
                                .unwrap_or(false)
                        } else {
                            working_slot.is_ancestor(entry.deployment_slot)
                        };

                        if entry.deployment_slot <= self.latest_root_slot
                            || entry.deployment_slot == current_slot
                            || is_ancestor
                        {
                            if current_slot >= entry.effective_slot {
                                if !Self::is_entry_usable(entry, current_slot, &match_criteria)
                                    || !Self::matches_environment(entry, environments)
                                {
                                    break;
                                }

                                if let LoadedProgramType::Unloaded(_environment) = &entry.program {
                                    reloading = true;
                                    break;
                                }

                                let mut usage_count =
                                    entry.tx_usage_counter.load(Ordering::Relaxed);
                                saturating_add_assign!(usage_count, count);
                                entry.tx_usage_counter.store(usage_count, Ordering::Relaxed);
                                return Some((key, entry.clone()));
                            } else if entry.is_implicit_delay_visibility_tombstone(current_slot) {
                                // Found a program entry on the current fork, but it's not effective
                                // yet. It indicates that the program has delayed visibility. Return
                                // the tombstone to reflect that.
                                return Some((
                                    key,
                                    Arc::new(LoadedProgram::new_tombstone(
                                        entry.deployment_slot,
                                        LoadedProgramType::DelayVisibility,
                                    )),
                                ));
                            }
                        }
                    }
                }
                extracting.missing.insert(key, (count, reloading));
                None
            })
            .collect::<HashMap<Pubkey, Arc<LoadedProgram>>>();
        self.stats
            .misses
            .fetch_add(extracting.missing.len() as u64, Ordering::Relaxed);
        self.stats
            .hits
            .fetch_add(extracting.loaded.entries.len() as u64, Ordering::Relaxed);
        drop(extracting);
        extracted
    }

    pub fn merge(&mut self, tx_batch_cache: &LoadedProgramsForTxBatch) {
        tx_batch_cache.entries.iter().for_each(|(key, entry)| {
            self.replenish(*key, entry.clone());
        })
    }

    /// Returns the list of loaded programs which are verified and compiled sorted by `tx_usage_counter`.
    ///
    /// Entries from program runtime v1 and v2 can be individually filtered.
    pub fn get_entries_sorted_by_tx_usage(
        &self,
        include_program_runtime_v1: bool,
        include_program_runtime_v2: bool,
    ) -> Vec<(Pubkey, Arc<LoadedProgram>)> {
        self.entries
            .iter()
            .flat_map(|(id, list)| {
                list.iter()
                    .filter_map(move |program| match program.program {
                        LoadedProgramType::LegacyV0(_) | LoadedProgramType::LegacyV1(_)
                            if include_program_runtime_v1 =>
                        {
                            Some((*id, program.clone()))
                        }
                        LoadedProgramType::Typed(_) if include_program_runtime_v2 => {
                            Some((*id, program.clone()))
                        }
                        #[cfg(test)]
                        LoadedProgramType::TestLoaded(_) => Some((*id, program.clone())),
                        _ => None,
                    })
            })
            .sorted_by_cached_key(|(_id, program)| program.tx_usage_counter.load(Ordering::Relaxed))
            .collect()
    }

    /// Unloads programs which were used infrequently
    pub fn sort_and_unload(&mut self, shrink_to: PercentageInteger) {
        let sorted_candidates = self.get_entries_sorted_by_tx_usage(true, true);
        let num_to_unload = sorted_candidates
            .len()
            .saturating_sub(shrink_to.apply_to(MAX_LOADED_ENTRY_COUNT));
        self.unload_program_entries(sorted_candidates.iter().take(num_to_unload));
        self.remove_programs_with_no_entries();
    }

    /// Removes all the entries at the given keys, if they exist
    pub fn remove_programs(&mut self, keys: impl Iterator<Item = Pubkey>) {
        for k in keys {
            self.entries.remove(&k);
        }
    }

    fn unload_program(&mut self, id: &Pubkey) {
        if let Some(entries) = self.entries.get_mut(id) {
            entries.iter_mut().for_each(|entry| {
                if let Some(unloaded) = entry.to_unloaded() {
                    *entry = Arc::new(unloaded);
                    self.stats
                        .evictions
                        .entry(*id)
                        .and_modify(|c| saturating_add_assign!(*c, 1))
                        .or_insert(1);
                }
            });
        }
    }

    pub fn unload_all_programs(&mut self) {
        let keys = self.entries.keys().copied().collect::<Vec<Pubkey>>();
        keys.iter().for_each(|key| self.unload_program(key));
    }

    fn unload_program_entries<'a>(
        &mut self,
        remove: impl Iterator<Item = &'a (Pubkey, Arc<LoadedProgram>)>,
    ) {
        for (id, program) in remove {
            if let Some(entries) = self.entries.get_mut(id) {
                if let Some(candidate) = entries.iter_mut().find(|entry| entry == &program) {
                    if let Some(unloaded) = candidate.to_unloaded() {
                        if candidate.tx_usage_counter.load(Ordering::Relaxed) == 1 {
                            self.stats.one_hit_wonders.fetch_add(1, Ordering::Relaxed);
                        }
                        self.stats
                            .evictions
                            .entry(*id)
                            .and_modify(|c| saturating_add_assign!(*c, 1))
                            .or_insert(1);
                        *candidate = Arc::new(unloaded);
                    }
                }
            }
        }
    }

    fn remove_programs_with_no_entries(&mut self) {
        let num_programs_before_removal = self.entries.len();
        self.entries.retain(|_, programs| !programs.is_empty());
        if self.entries.len() < num_programs_before_removal {
            self.stats.empty_entries.fetch_add(
                num_programs_before_removal.saturating_sub(self.entries.len()) as u64,
                Ordering::Relaxed,
            );
        }
    }
}

#[cfg(RUSTC_WITH_SPECIALIZATION)]
impl solana_frozen_abi::abi_example::AbiExample for LoadedProgram {
    fn example() -> Self {
        // LoadedProgram isn't serializable by definition.
        Self::default()
    }
}

#[cfg(RUSTC_WITH_SPECIALIZATION)]
impl<FG: ForkGraph> solana_frozen_abi::abi_example::AbiExample for LoadedPrograms<FG> {
    fn example() -> Self {
        // LoadedPrograms isn't serializable by definition.
        Self::default()
    }
}

#[cfg(test)]
mod tests {
    use {
        crate::loaded_programs::{
            BlockRelation, ExtractedPrograms, ForkGraph, LoadedProgram, LoadedProgramMatchCriteria,
            LoadedProgramType, LoadedPrograms, ProgramRuntimeEnvironment,
            ProgramRuntimeEnvironments, WorkingSlot, DELAY_VISIBILITY_SLOT_OFFSET,
        },
        assert_matches::assert_matches,
        percentage::Percentage,
        solana_rbpf::program::BuiltinProgram,
        solana_sdk::{
            clock::{Epoch, Slot},
            pubkey::Pubkey,
        },
        std::{
            ops::ControlFlow,
            sync::{
                atomic::{AtomicU64, Ordering},
                Arc, Mutex, RwLock,
            },
        },
    };

    static MOCK_ENVIRONMENT: std::sync::OnceLock<ProgramRuntimeEnvironment> =
        std::sync::OnceLock::<ProgramRuntimeEnvironment>::new();

    fn new_mock_cache<FG: ForkGraph>() -> LoadedPrograms<FG> {
        let mut cache = LoadedPrograms::default();
        cache.environments.program_runtime_v1 = MOCK_ENVIRONMENT
            .get_or_init(|| Arc::new(BuiltinProgram::new_mock()))
            .clone();
        cache
    }

    fn new_test_loaded_program(deployment_slot: Slot, effective_slot: Slot) -> Arc<LoadedProgram> {
        new_test_loaded_program_with_usage(deployment_slot, effective_slot, AtomicU64::default())
    }

    fn new_test_loaded_program_with_usage(
        deployment_slot: Slot,
        effective_slot: Slot,
        usage_counter: AtomicU64,
    ) -> Arc<LoadedProgram> {
        new_test_loaded_program_with_usage_and_expiry(
            deployment_slot,
            effective_slot,
            usage_counter,
            None,
        )
    }

    fn new_test_loaded_program_with_usage_and_expiry(
        deployment_slot: Slot,
        effective_slot: Slot,
        usage_counter: AtomicU64,
        expiry: Option<Slot>,
    ) -> Arc<LoadedProgram> {
        Arc::new(LoadedProgram {
            program: LoadedProgramType::TestLoaded(MOCK_ENVIRONMENT.get().unwrap().clone()),
            account_size: 0,
            deployment_slot,
            effective_slot,
            maybe_expiration_slot: expiry,
            tx_usage_counter: usage_counter,
            ix_usage_counter: AtomicU64::default(),
        })
    }

    fn new_test_builtin_program(deployment_slot: Slot, effective_slot: Slot) -> Arc<LoadedProgram> {
        Arc::new(LoadedProgram {
            program: LoadedProgramType::Builtin(BuiltinProgram::new_mock()),
            account_size: 0,
            deployment_slot,
            effective_slot,
            maybe_expiration_slot: None,
            tx_usage_counter: AtomicU64::default(),
            ix_usage_counter: AtomicU64::default(),
        })
    }

    fn set_tombstone<FG: ForkGraph>(
        cache: &mut LoadedPrograms<FG>,
        key: Pubkey,
        slot: Slot,
        reason: LoadedProgramType,
    ) -> Arc<LoadedProgram> {
        cache.assign_program(key, Arc::new(LoadedProgram::new_tombstone(slot, reason)))
    }

    fn insert_unloaded_program<FG: ForkGraph>(
        cache: &mut LoadedPrograms<FG>,
        key: Pubkey,
        slot: Slot,
    ) -> Arc<LoadedProgram> {
        let unloaded = Arc::new(
            LoadedProgram {
                program: LoadedProgramType::TestLoaded(
                    cache.environments.program_runtime_v1.clone(),
                ),
                account_size: 0,
                deployment_slot: slot,
                effective_slot: slot.saturating_add(1),
                maybe_expiration_slot: None,
                tx_usage_counter: AtomicU64::default(),
                ix_usage_counter: AtomicU64::default(),
            }
            .to_unloaded()
            .expect("Failed to unload the program"),
        );
        cache.replenish(key, unloaded).1
    }

    fn num_matching_entries<P, FG>(cache: &LoadedPrograms<FG>, predicate: P) -> usize
    where
        P: Fn(&LoadedProgramType) -> bool,
        FG: ForkGraph,
    {
        cache
            .entries
            .values()
            .map(|programs| {
                programs
                    .iter()
                    .filter(|program| predicate(&program.program))
                    .count()
            })
            .sum()
    }

    #[test]
    fn test_eviction() {
        let mut programs = vec![];
        let mut num_total_programs: usize = 0;

        let mut cache = new_mock_cache::<TestForkGraph>();

        let program1 = Pubkey::new_unique();
        let program1_deployment_slots = [0, 10, 20];
        let program1_usage_counters = [4, 5, 25];
        program1_deployment_slots
            .iter()
            .enumerate()
            .for_each(|(i, deployment_slot)| {
                let usage_counter = *program1_usage_counters.get(i).unwrap_or(&0);
                cache.replenish(
                    program1,
                    new_test_loaded_program_with_usage(
                        *deployment_slot,
                        (*deployment_slot) + 2,
                        AtomicU64::new(usage_counter),
                    ),
                );
                num_total_programs += 1;
                programs.push((program1, *deployment_slot, usage_counter));
            });

        let env = Arc::new(BuiltinProgram::new_mock());
        for slot in 21..31 {
            set_tombstone(
                &mut cache,
                program1,
                slot,
                LoadedProgramType::FailedVerification(env.clone()),
            );
        }

        for slot in 31..41 {
            insert_unloaded_program(&mut cache, program1, slot);
        }

        let program2 = Pubkey::new_unique();
        let program2_deployment_slots = [5, 11];
        let program2_usage_counters = [0, 2];
        program2_deployment_slots
            .iter()
            .enumerate()
            .for_each(|(i, deployment_slot)| {
                let usage_counter = *program2_usage_counters.get(i).unwrap_or(&0);
                cache.replenish(
                    program2,
                    new_test_loaded_program_with_usage(
                        *deployment_slot,
                        (*deployment_slot) + 2,
                        AtomicU64::new(usage_counter),
                    ),
                );
                num_total_programs += 1;
                programs.push((program2, *deployment_slot, usage_counter));
            });

        for slot in 21..31 {
            set_tombstone(
                &mut cache,
                program2,
                slot,
                LoadedProgramType::DelayVisibility,
            );
        }

        for slot in 31..41 {
            insert_unloaded_program(&mut cache, program2, slot);
        }

        let program3 = Pubkey::new_unique();
        let program3_deployment_slots = [0, 5, 15];
        let program3_usage_counters = [100, 3, 20];
        program3_deployment_slots
            .iter()
            .enumerate()
            .for_each(|(i, deployment_slot)| {
                let usage_counter = *program3_usage_counters.get(i).unwrap_or(&0);
                cache.replenish(
                    program3,
                    new_test_loaded_program_with_usage(
                        *deployment_slot,
                        (*deployment_slot) + 2,
                        AtomicU64::new(usage_counter),
                    ),
                );
                num_total_programs += 1;
                programs.push((program3, *deployment_slot, usage_counter));
            });

        for slot in 21..31 {
            set_tombstone(&mut cache, program3, slot, LoadedProgramType::Closed);
        }

        for slot in 31..41 {
            insert_unloaded_program(&mut cache, program3, slot);
        }

        programs.sort_by_key(|(_id, _slot, usage_count)| *usage_count);

        let num_loaded = num_matching_entries(&cache, |program_type| {
            matches!(program_type, LoadedProgramType::TestLoaded(_))
        });
        let num_unloaded = num_matching_entries(&cache, |program_type| {
            matches!(program_type, LoadedProgramType::Unloaded(_))
        });
        let num_tombstones = num_matching_entries(&cache, |program_type| {
            matches!(
                program_type,
                LoadedProgramType::DelayVisibility
                    | LoadedProgramType::FailedVerification(_)
                    | LoadedProgramType::Closed
            )
        });

        assert_eq!(num_loaded, 8);
        assert_eq!(num_unloaded, 30);
        assert_eq!(num_tombstones, 30);

        // Evicting to 2% should update cache with
        // * 5 active entries
        // * 33 unloaded entries (3 active programs will get unloaded)
        // * 30 tombstones (tombstones are not evicted)
        cache.sort_and_unload(Percentage::from(2));
        // Check that every program is still in the cache.
        programs.iter().for_each(|entry| {
            assert!(cache.entries.get(&entry.0).is_some());
        });

        let unloaded = cache
            .entries
            .iter()
            .flat_map(|(id, cached_programs)| {
                cached_programs.iter().filter_map(|program| {
                    matches!(program.program, LoadedProgramType::Unloaded(_))
                        .then_some((*id, program.tx_usage_counter.load(Ordering::Relaxed)))
                })
            })
            .collect::<Vec<(Pubkey, u64)>>();

        for index in 0..3 {
            let expected = programs.get(index).expect("Missing program");
            assert!(unloaded.contains(&(expected.0, expected.2)));
        }

        let num_loaded = num_matching_entries(&cache, |program_type| {
            matches!(program_type, LoadedProgramType::TestLoaded(_))
        });
        let num_unloaded = num_matching_entries(&cache, |program_type| {
            matches!(program_type, LoadedProgramType::Unloaded(_))
        });
        let num_tombstones = num_matching_entries(&cache, |program_type| {
            matches!(
                program_type,
                LoadedProgramType::DelayVisibility
                    | LoadedProgramType::FailedVerification(_)
                    | LoadedProgramType::Closed
            )
        });

        assert_eq!(num_loaded, 5);
        assert_eq!(num_unloaded, 33);
        assert_eq!(num_tombstones, 30);
    }

    #[test]
    fn test_usage_count_of_unloaded_program() {
        let mut cache = new_mock_cache::<TestForkGraph>();

        let program = Pubkey::new_unique();
        let num_total_programs = 6;
        (0..num_total_programs).for_each(|i| {
            cache.replenish(
                program,
                new_test_loaded_program_with_usage(i, i + 2, AtomicU64::new(i + 10)),
            );
        });

        // This will unload the program deployed at slot 0, with usage count = 10
        cache.sort_and_unload(Percentage::from(2));

        let num_unloaded = num_matching_entries(&cache, |program_type| {
            matches!(program_type, LoadedProgramType::Unloaded(_))
        });
        assert_eq!(num_unloaded, 1);

        cache.entries.values().for_each(|programs| {
            programs.iter().for_each(|program| {
                if matches!(program.program, LoadedProgramType::Unloaded(_)) {
                    // Test that the usage counter is retained for the unloaded program
                    assert_eq!(program.tx_usage_counter.load(Ordering::Relaxed), 10);
                    assert_eq!(program.deployment_slot, 0);
                    assert_eq!(program.effective_slot, 2);
                }
            })
        });

        // Replenish the program that was just unloaded. Use 0 as the usage counter. This should be
        // updated with the usage counter from the unloaded program.
        cache.replenish(
            program,
            new_test_loaded_program_with_usage(0, 2, AtomicU64::new(0)),
        );

        cache.entries.values().for_each(|programs| {
            programs.iter().for_each(|program| {
                if matches!(program.program, LoadedProgramType::Unloaded(_))
                    && program.deployment_slot == 0
                    && program.effective_slot == 2
                {
                    // Test that the usage counter was correctly updated.
                    assert_eq!(program.tx_usage_counter.load(Ordering::Relaxed), 10);
                }
            })
        });
    }

    #[test]
    fn test_replace_tombstones() {
        let mut cache = new_mock_cache::<TestForkGraph>();
        let program1 = Pubkey::new_unique();
        let env = Arc::new(BuiltinProgram::new_mock());
        set_tombstone(
            &mut cache,
            program1,
            10,
            LoadedProgramType::FailedVerification(env),
        );

        let loaded_program = new_test_loaded_program(10, 10);
        let (existing, program) = cache.replenish(program1, loaded_program.clone());
        assert!(!existing);
        assert_eq!(program, loaded_program);
    }

    #[test]
    fn test_tombstone() {
        let env = Arc::new(BuiltinProgram::new_mock());
        let tombstone =
            LoadedProgram::new_tombstone(0, LoadedProgramType::FailedVerification(env.clone()));
        assert_matches!(tombstone.program, LoadedProgramType::FailedVerification(_));
        assert!(tombstone.is_tombstone());
        assert_eq!(tombstone.deployment_slot, 0);
        assert_eq!(tombstone.effective_slot, 0);

        let tombstone = LoadedProgram::new_tombstone(100, LoadedProgramType::Closed);
        assert_matches!(tombstone.program, LoadedProgramType::Closed);
        assert!(tombstone.is_tombstone());
        assert_eq!(tombstone.deployment_slot, 100);
        assert_eq!(tombstone.effective_slot, 100);

        let mut cache = new_mock_cache::<TestForkGraph>();
        let program1 = Pubkey::new_unique();
        let tombstone = set_tombstone(
            &mut cache,
            program1,
            10,
            LoadedProgramType::FailedVerification(env.clone()),
        );
        let second_level = &cache
            .entries
            .get(&program1)
            .expect("Failed to find the entry");
        assert_eq!(second_level.len(), 1);
        assert!(second_level.first().unwrap().is_tombstone());
        assert_eq!(tombstone.deployment_slot, 10);
        assert_eq!(tombstone.effective_slot, 10);

        // Add a program at slot 50, and a tombstone for the program at slot 60
        let program2 = Pubkey::new_unique();
        assert!(
            !cache
                .replenish(program2, new_test_builtin_program(50, 51))
                .0
        );
        let second_level = &cache
            .entries
            .get(&program2)
            .expect("Failed to find the entry");
        assert_eq!(second_level.len(), 1);
        assert!(!second_level.first().unwrap().is_tombstone());

        let tombstone = set_tombstone(
            &mut cache,
            program2,
            60,
            LoadedProgramType::FailedVerification(env),
        );
        let second_level = &cache
            .entries
            .get(&program2)
            .expect("Failed to find the entry");
        assert_eq!(second_level.len(), 2);
        assert!(!second_level.first().unwrap().is_tombstone());
        assert!(second_level.get(1).unwrap().is_tombstone());
        assert!(tombstone.is_tombstone());
        assert_eq!(tombstone.deployment_slot, 60);
        assert_eq!(tombstone.effective_slot, 60);
    }

    struct TestForkGraph {
        relation: BlockRelation,
    }
    impl ForkGraph for TestForkGraph {
        fn relationship(&self, _a: Slot, _b: Slot) -> BlockRelation {
            self.relation
        }
    }

    #[test]
    fn test_prune_empty() {
        let mut cache = new_mock_cache::<TestForkGraph>();
        let fork_graph = Arc::new(RwLock::new(TestForkGraph {
            relation: BlockRelation::Unrelated,
        }));

        cache.set_fork_graph(fork_graph);

        cache.prune(0, 0);
        assert!(cache.entries.is_empty());

        cache.prune(10, 0);
        assert!(cache.entries.is_empty());

        let mut cache = new_mock_cache::<TestForkGraph>();
        let fork_graph = Arc::new(RwLock::new(TestForkGraph {
            relation: BlockRelation::Ancestor,
        }));

        cache.set_fork_graph(fork_graph);

        cache.prune(0, 0);
        assert!(cache.entries.is_empty());

        cache.prune(10, 0);
        assert!(cache.entries.is_empty());

        let mut cache = new_mock_cache::<TestForkGraph>();
        let fork_graph = Arc::new(RwLock::new(TestForkGraph {
            relation: BlockRelation::Descendant,
        }));

        cache.set_fork_graph(fork_graph);

        cache.prune(0, 0);
        assert!(cache.entries.is_empty());

        cache.prune(10, 0);
        assert!(cache.entries.is_empty());

        let mut cache = new_mock_cache::<TestForkGraph>();
        let fork_graph = Arc::new(RwLock::new(TestForkGraph {
            relation: BlockRelation::Unknown,
        }));
        cache.set_fork_graph(fork_graph);

        cache.prune(0, 0);
        assert!(cache.entries.is_empty());

        cache.prune(10, 0);
        assert!(cache.entries.is_empty());
    }

    #[test]
    fn test_prune_different_env() {
        let mut cache = new_mock_cache::<TestForkGraph>();

        let fork_graph = Arc::new(RwLock::new(TestForkGraph {
            relation: BlockRelation::Ancestor,
        }));

        cache.set_fork_graph(fork_graph);

        let program1 = Pubkey::new_unique();
        let loaded_program = new_test_loaded_program(10, 10);
        let (existing, program) = cache.replenish(program1, loaded_program.clone());
        assert!(!existing);
        assert_eq!(program, loaded_program);

        let new_env = Arc::new(BuiltinProgram::new_mock());
        cache.upcoming_environments = Some(ProgramRuntimeEnvironments {
            program_runtime_v1: new_env.clone(),
            program_runtime_v2: new_env.clone(),
        });
        let updated_program = Arc::new(LoadedProgram {
            program: LoadedProgramType::TestLoaded(new_env.clone()),
            account_size: 0,
            deployment_slot: 20,
            effective_slot: 20,
            maybe_expiration_slot: None,
            tx_usage_counter: AtomicU64::default(),
            ix_usage_counter: AtomicU64::default(),
        });
        let (existing, program) = cache.replenish(program1, updated_program.clone());
        assert!(!existing);
        assert_eq!(program, updated_program);

        // Test that there are 2 entries for the program
        assert_eq!(
            cache
                .entries
                .get(&program1)
                .expect("failed to find the program")
                .len(),
            2
        );

        cache.prune(21, cache.latest_root_epoch);

        // Test that prune didn't remove the entry, since environments are different.
        assert_eq!(
            cache
                .entries
                .get(&program1)
                .expect("failed to find the program")
                .len(),
            2
        );

        cache.prune(22, cache.latest_root_epoch.saturating_add(1));

        let entries = cache
            .entries
            .get(&program1)
            .expect("failed to find the program");
        // Test that prune removed 1 entry, since epoch changed
        assert_eq!(entries.len(), 1);

        let entry = entries.first().expect("Failed to get the program").clone();
        // Test that the correct entry remains in the cache
        assert_eq!(entry, updated_program);
    }

    #[derive(Default)]
    struct TestForkGraphSpecific {
        forks: Vec<Vec<Slot>>,
    }

    impl TestForkGraphSpecific {
        fn insert_fork(&mut self, fork: &[Slot]) {
            let mut fork = fork.to_vec();
            fork.sort();
            self.forks.push(fork)
        }
    }

    impl ForkGraph for TestForkGraphSpecific {
        fn relationship(&self, a: Slot, b: Slot) -> BlockRelation {
            match self.forks.iter().try_for_each(|fork| {
                let relation = fork
                    .iter()
                    .position(|x| *x == a)
                    .and_then(|a_pos| {
                        fork.iter().position(|x| *x == b).and_then(|b_pos| {
                            (a_pos == b_pos)
                                .then_some(BlockRelation::Equal)
                                .or_else(|| (a_pos < b_pos).then_some(BlockRelation::Ancestor))
                                .or(Some(BlockRelation::Descendant))
                        })
                    })
                    .unwrap_or(BlockRelation::Unrelated);

                if relation != BlockRelation::Unrelated {
                    return ControlFlow::Break(relation);
                }

                ControlFlow::Continue(())
            }) {
                ControlFlow::Break(relation) => relation,
                _ => BlockRelation::Unrelated,
            }
        }
    }

    struct TestWorkingSlot(pub Slot);

    impl WorkingSlot for TestWorkingSlot {
        fn current_slot(&self) -> Slot {
            self.0
        }

        fn current_epoch(&self) -> Epoch {
            0
        }

        fn is_ancestor(&self, _other: Slot) -> bool {
            false
        }
    }

    fn match_slot(
        extracted: &Arc<Mutex<ExtractedPrograms>>,
        program: &Pubkey,
        deployment_slot: Slot,
        working_slot: Slot,
    ) -> bool {
        let extracted = extracted.lock().unwrap();
        assert_eq!(extracted.loaded.slot, working_slot);
        extracted
            .loaded
            .find(program)
            .map(|entry| entry.deployment_slot == deployment_slot)
            .unwrap_or(false)
    }

    fn match_missing(
        extracted: &Arc<Mutex<ExtractedPrograms>>,
        key: &Pubkey,
        reload: bool,
    ) -> bool {
        let extracted = extracted.lock().unwrap();
        extracted
            .missing
            .get(key)
            .filter(|(_count, reloading)| *reloading == reload)
            .is_some()
    }

    #[test]
    fn test_fork_extract_and_prune() {
        let mut cache = new_mock_cache::<TestForkGraphSpecific>();

        // Fork graph created for the test
        //                   0
        //                 /   \
        //                10    5
        //                |     |
        //                20    11
        //                |     | \
        //                22   15  25
        //                      |   |
        //                     16  27
        //                      |
        //                     19
        //                      |
        //                     23

        let mut fork_graph = TestForkGraphSpecific::default();
        fork_graph.insert_fork(&[0, 10, 20, 22]);
        fork_graph.insert_fork(&[0, 5, 11, 15, 16, 18, 19, 21, 23]);
        fork_graph.insert_fork(&[0, 5, 11, 25, 27]);

        let fork_graph = Arc::new(RwLock::new(fork_graph));
        cache.set_fork_graph(fork_graph);

        let program1 = Pubkey::new_unique();
        assert!(!cache.replenish(program1, new_test_loaded_program(0, 1)).0);
        assert!(!cache.replenish(program1, new_test_loaded_program(10, 11)).0);
        assert!(!cache.replenish(program1, new_test_loaded_program(20, 21)).0);

        // Test: inserting duplicate entry return pre existing entry from the cache
        assert!(cache.replenish(program1, new_test_loaded_program(20, 21)).0);

        let program2 = Pubkey::new_unique();
        assert!(!cache.replenish(program2, new_test_loaded_program(5, 6)).0);
        assert!(
            !cache
                .replenish(
                    program2,
                    new_test_loaded_program(11, 11 + DELAY_VISIBILITY_SLOT_OFFSET)
                )
                .0
        );

        let program3 = Pubkey::new_unique();
        assert!(!cache.replenish(program3, new_test_loaded_program(25, 26)).0);

        let program4 = Pubkey::new_unique();
        assert!(!cache.replenish(program4, new_test_loaded_program(0, 1)).0);
        assert!(!cache.replenish(program4, new_test_loaded_program(5, 6)).0);
        // The following is a special case, where effective slot is 3 slots in the future
        assert!(
            !cache
                .replenish(
                    program4,
                    new_test_loaded_program(15, 15 + DELAY_VISIBILITY_SLOT_OFFSET)
                )
                .0
        );

        // Current fork graph
        //                   0
        //                 /   \
        //                10    5
        //                |     |
        //                20    11
        //                |     | \
        //                22   15  25
        //                      |   |
        //                     16  27
        //                      |
        //                     19
        //                      |
        //                     23

        // Testing fork 0 - 10 - 12 - 22 with current slot at 22
        let extracted = cache.extract(
            &TestWorkingSlot(22),
            vec![
                (program1, (LoadedProgramMatchCriteria::NoCriteria, 1)),
                (program2, (LoadedProgramMatchCriteria::NoCriteria, 2)),
                (program3, (LoadedProgramMatchCriteria::NoCriteria, 3)),
                (program4, (LoadedProgramMatchCriteria::NoCriteria, 4)),
            ]
            .into_iter(),
        );

        assert!(match_slot(&extracted, &program1, 20, 22));
        assert!(match_slot(&extracted, &program4, 0, 22));

        assert!(match_missing(&extracted, &program2, false));
        assert!(match_missing(&extracted, &program3, false));

        // Testing fork 0 - 5 - 11 - 15 - 16 with current slot at 16
        let extracted = cache.extract(
            &TestWorkingSlot(15),
            vec![
                (program1, (LoadedProgramMatchCriteria::NoCriteria, 1)),
                (program2, (LoadedProgramMatchCriteria::NoCriteria, 1)),
                (program3, (LoadedProgramMatchCriteria::NoCriteria, 1)),
                (program4, (LoadedProgramMatchCriteria::NoCriteria, 1)),
            ]
            .into_iter(),
        );

        assert!(match_slot(&extracted, &program1, 0, 15));
        assert!(match_slot(&extracted, &program2, 11, 15));

        // The effective slot of program4 deployed in slot 15 is 19. So it should not be usable in slot 16.
        // A delay visibility tombstone should be returned here.
        let tombstone = extracted
            .lock()
            .unwrap()
            .loaded
            .find(&program4)
            .expect("Failed to find the tombstone");
        assert_matches!(tombstone.program, LoadedProgramType::DelayVisibility);
        assert_eq!(tombstone.deployment_slot, 15);

        assert!(match_missing(&extracted, &program3, false));

        // Testing the same fork above, but current slot is now 18 (equal to effective slot of program4).
        let extracted = cache.extract(
            &TestWorkingSlot(18),
            vec![
                (program1, (LoadedProgramMatchCriteria::NoCriteria, 1)),
                (program2, (LoadedProgramMatchCriteria::NoCriteria, 1)),
                (program3, (LoadedProgramMatchCriteria::NoCriteria, 1)),
                (program4, (LoadedProgramMatchCriteria::NoCriteria, 1)),
            ]
            .into_iter(),
        );

        assert!(match_slot(&extracted, &program1, 0, 18));
        assert!(match_slot(&extracted, &program2, 11, 18));

        // The effective slot of program4 deployed in slot 15 is 18. So it should be usable in slot 18.
        assert!(match_slot(&extracted, &program4, 15, 18));

        assert!(match_missing(&extracted, &program3, false));

        // Testing the same fork above, but current slot is now 23 (future slot than effective slot of program4).
        let extracted = cache.extract(
            &TestWorkingSlot(23),
            vec![
                (program1, (LoadedProgramMatchCriteria::NoCriteria, 1)),
                (program2, (LoadedProgramMatchCriteria::NoCriteria, 1)),
                (program3, (LoadedProgramMatchCriteria::NoCriteria, 1)),
                (program4, (LoadedProgramMatchCriteria::NoCriteria, 1)),
            ]
            .into_iter(),
        );

        assert!(match_slot(&extracted, &program1, 0, 23));
        assert!(match_slot(&extracted, &program2, 11, 23));

        // The effective slot of program4 deployed in slot 15 is 19. So it should be usable in slot 23.
        assert!(match_slot(&extracted, &program4, 15, 23));

        assert!(match_missing(&extracted, &program3, false));

        // Testing fork 0 - 5 - 11 - 15 - 16 with current slot at 11
        let extracted = cache.extract(
            &TestWorkingSlot(11),
            vec![
                (program1, (LoadedProgramMatchCriteria::NoCriteria, 1)),
                (program2, (LoadedProgramMatchCriteria::NoCriteria, 1)),
                (program3, (LoadedProgramMatchCriteria::NoCriteria, 1)),
                (program4, (LoadedProgramMatchCriteria::NoCriteria, 1)),
            ]
            .into_iter(),
        );

        assert!(match_slot(&extracted, &program1, 0, 11));
        // program2 was updated at slot 11, but is not effective till slot 12. The result should contain a tombstone.
        let tombstone = extracted
            .lock()
            .unwrap()
            .loaded
            .find(&program2)
            .expect("Failed to find the tombstone");
        assert_matches!(tombstone.program, LoadedProgramType::DelayVisibility);
        assert_eq!(tombstone.deployment_slot, 11);
        assert!(match_slot(&extracted, &program4, 5, 11));

        assert!(match_missing(&extracted, &program3, false));

        // The following is a special case, where there's an expiration slot
        let test_program = Arc::new(LoadedProgram {
            program: LoadedProgramType::DelayVisibility,
            account_size: 0,
            deployment_slot: 19,
            effective_slot: 19,
            maybe_expiration_slot: Some(21),
            tx_usage_counter: AtomicU64::default(),
            ix_usage_counter: AtomicU64::default(),
        });
        assert!(!cache.replenish(program4, test_program).0);

        // Testing fork 0 - 5 - 11 - 15 - 16 - 19 - 21 - 23 with current slot at 19
        let extracted = cache.extract(
            &TestWorkingSlot(19),
            vec![
                (program1, (LoadedProgramMatchCriteria::NoCriteria, 1)),
                (program2, (LoadedProgramMatchCriteria::NoCriteria, 1)),
                (program3, (LoadedProgramMatchCriteria::NoCriteria, 1)),
                (program4, (LoadedProgramMatchCriteria::NoCriteria, 1)),
            ]
            .into_iter(),
        );

        assert!(match_slot(&extracted, &program1, 0, 19));
        assert!(match_slot(&extracted, &program2, 11, 19));
        // Program4 deployed at slot 19 should not be expired yet
        assert!(match_slot(&extracted, &program4, 19, 19));

        assert!(match_missing(&extracted, &program3, false));

        // Testing fork 0 - 5 - 11 - 15 - 16 - 19 - 21 - 23 with current slot at 21
        // This would cause program4 deployed at slot 19 to be expired.
        let extracted = cache.extract(
            &TestWorkingSlot(21),
            vec![
                (program1, (LoadedProgramMatchCriteria::NoCriteria, 1)),
                (program2, (LoadedProgramMatchCriteria::NoCriteria, 1)),
                (program3, (LoadedProgramMatchCriteria::NoCriteria, 1)),
                (program4, (LoadedProgramMatchCriteria::NoCriteria, 1)),
            ]
            .into_iter(),
        );

        assert!(match_slot(&extracted, &program1, 0, 21));
        assert!(match_slot(&extracted, &program2, 11, 21));

        assert!(match_missing(&extracted, &program3, false));
        assert!(match_missing(&extracted, &program4, false));

        // Remove the expired entry to let the rest of the test continue
        if let Some(programs) = cache.entries.get_mut(&program4) {
            programs.pop();
        }

        cache.prune(5, 0);

        // Fork graph after pruning
        //                   0
        //                   |
        //                   5
        //                   |
        //                   11
        //                   | \
        //                  15  25
        //                   |   |
        //                  16  27
        //                   |
        //                  19
        //                   |
        //                  23

        // Testing fork 11 - 15 - 16- 19 - 22 with root at 5 and current slot at 22
        let extracted = cache.extract(
            &TestWorkingSlot(21),
            vec![
                (program1, (LoadedProgramMatchCriteria::NoCriteria, 1)),
                (program2, (LoadedProgramMatchCriteria::NoCriteria, 1)),
                (program3, (LoadedProgramMatchCriteria::NoCriteria, 1)),
                (program4, (LoadedProgramMatchCriteria::NoCriteria, 1)),
            ]
            .into_iter(),
        );

        // Since the fork was pruned, we should not find the entry deployed at slot 20.
        assert!(match_slot(&extracted, &program1, 0, 21));
        assert!(match_slot(&extracted, &program2, 11, 21));
        assert!(match_slot(&extracted, &program4, 15, 21));

        assert!(match_missing(&extracted, &program3, false));

        // Testing fork 0 - 5 - 11 - 25 - 27 with current slot at 27
        let extracted = cache.extract(
            &TestWorkingSlot(27),
            vec![
                (program1, (LoadedProgramMatchCriteria::NoCriteria, 1)),
                (program2, (LoadedProgramMatchCriteria::NoCriteria, 1)),
                (program3, (LoadedProgramMatchCriteria::NoCriteria, 1)),
                (program4, (LoadedProgramMatchCriteria::NoCriteria, 1)),
            ]
            .into_iter(),
        );

        assert!(match_slot(&extracted, &program1, 0, 27));
        assert!(match_slot(&extracted, &program2, 11, 27));
        assert!(match_slot(&extracted, &program3, 25, 27));
        assert!(match_slot(&extracted, &program4, 5, 27));

        cache.prune(15, 0);

        // Fork graph after pruning
        //                  0
        //                  |
        //                  5
        //                  |
        //                  11
        //                  |
        //                  15
        //                  |
        //                  16
        //                  |
        //                  19
        //                  |
        //                  23

        // Testing fork 16, 19, 23, with root at 15, current slot at 23
        let extracted = cache.extract(
            &TestWorkingSlot(23),
            vec![
                (program1, (LoadedProgramMatchCriteria::NoCriteria, 1)),
                (program2, (LoadedProgramMatchCriteria::NoCriteria, 1)),
                (program3, (LoadedProgramMatchCriteria::NoCriteria, 1)),
                (program4, (LoadedProgramMatchCriteria::NoCriteria, 1)),
            ]
            .into_iter(),
        );

        assert!(match_slot(&extracted, &program1, 0, 23));
        assert!(match_slot(&extracted, &program2, 11, 23));
        assert!(match_slot(&extracted, &program4, 15, 23));

        // program3 was deployed on slot 25, which has been pruned
        assert!(match_missing(&extracted, &program3, false));
    }

    #[test]
    fn test_extract_using_deployment_slot() {
        let mut cache = new_mock_cache::<TestForkGraphSpecific>();

        // Fork graph created for the test
        //                   0
        //                 /   \
        //                10    5
        //                |     |
        //                20    11
        //                |     | \
        //                22   15  25
        //                      |   |
        //                     16  27
        //                      |
        //                     19
        //                      |
        //                     23

        let mut fork_graph = TestForkGraphSpecific::default();
        fork_graph.insert_fork(&[0, 10, 20, 22]);
        fork_graph.insert_fork(&[0, 5, 11, 12, 15, 16, 18, 19, 21, 23]);
        fork_graph.insert_fork(&[0, 5, 11, 25, 27]);

        let fork_graph = Arc::new(RwLock::new(fork_graph));
        cache.set_fork_graph(fork_graph);

        let program1 = Pubkey::new_unique();
        assert!(!cache.replenish(program1, new_test_loaded_program(0, 1)).0);
        assert!(!cache.replenish(program1, new_test_loaded_program(20, 21)).0);

        let program2 = Pubkey::new_unique();
        assert!(!cache.replenish(program2, new_test_loaded_program(5, 6)).0);
        assert!(!cache.replenish(program2, new_test_loaded_program(11, 12)).0);

        let program3 = Pubkey::new_unique();
        assert!(!cache.replenish(program3, new_test_loaded_program(25, 26)).0);

        // Testing fork 0 - 5 - 11 - 15 - 16 - 19 - 21 - 23 with current slot at 19
        let extracted = cache.extract(
            &TestWorkingSlot(12),
            vec![
                (program1, (LoadedProgramMatchCriteria::NoCriteria, 1)),
                (program2, (LoadedProgramMatchCriteria::NoCriteria, 1)),
                (program3, (LoadedProgramMatchCriteria::NoCriteria, 1)),
            ]
            .into_iter(),
        );

        assert!(match_slot(&extracted, &program1, 0, 12));
        assert!(match_slot(&extracted, &program2, 11, 12));

        assert!(match_missing(&extracted, &program3, false));

        // Test the same fork, but request the program modified at a later slot than what's in the cache.
        let extracted = cache.extract(
            &TestWorkingSlot(12),
            vec![
                (
                    program1,
                    (LoadedProgramMatchCriteria::DeployedOnOrAfterSlot(5), 1),
                ),
                (
                    program2,
                    (LoadedProgramMatchCriteria::DeployedOnOrAfterSlot(5), 1),
                ),
                (program3, (LoadedProgramMatchCriteria::NoCriteria, 1)),
            ]
            .into_iter(),
        );

        assert!(match_slot(&extracted, &program2, 11, 12));

        assert!(match_missing(&extracted, &program1, false));
        assert!(match_missing(&extracted, &program3, false));
    }

    #[test]
    fn test_extract_unloaded() {
        let mut cache = new_mock_cache::<TestForkGraphSpecific>();

        // Fork graph created for the test
        //                   0
        //                 /   \
        //                10    5
        //                |     |
        //                20    11
        //                |     | \
        //                22   15  25
        //                      |   |
        //                     16  27
        //                      |
        //                     19
        //                      |
        //                     23

        let mut fork_graph = TestForkGraphSpecific::default();
        fork_graph.insert_fork(&[0, 10, 20, 22]);
        fork_graph.insert_fork(&[0, 5, 11, 15, 16, 19, 21, 23]);
        fork_graph.insert_fork(&[0, 5, 11, 25, 27]);

        let fork_graph = Arc::new(RwLock::new(fork_graph));
        cache.set_fork_graph(fork_graph);

        let program1 = Pubkey::new_unique();
        assert!(!cache.replenish(program1, new_test_loaded_program(0, 1)).0);
        assert!(!cache.replenish(program1, new_test_loaded_program(20, 21)).0);

        let program2 = Pubkey::new_unique();
        assert!(!cache.replenish(program2, new_test_loaded_program(5, 6)).0);
        assert!(!cache.replenish(program2, new_test_loaded_program(11, 12)).0);

        let program3 = Pubkey::new_unique();
        // Insert an unloaded program with correct/cache's environment at slot 25
        let _ = insert_unloaded_program(&mut cache, program3, 25);

        // Insert another unloaded program with a different environment at slot 20
        // Since this entry's environment won't match cache's environment, looking up this
        // entry should return missing instead of unloaded entry.
        assert!(
            !cache
                .replenish(
                    program3,
                    Arc::new(
                        new_test_loaded_program(20, 21)
                            .to_unloaded()
                            .expect("Failed to create unloaded program")
                    )
                )
                .0
        );

        // Testing fork 0 - 5 - 11 - 15 - 16 - 19 - 21 - 23 with current slot at 19
        let extracted = cache.extract(
            &TestWorkingSlot(19),
            vec![
                (program1, (LoadedProgramMatchCriteria::NoCriteria, 1)),
                (program2, (LoadedProgramMatchCriteria::NoCriteria, 1)),
                (program3, (LoadedProgramMatchCriteria::NoCriteria, 1)),
            ]
            .into_iter(),
        );

        assert!(match_slot(&extracted, &program1, 0, 19));
        assert!(match_slot(&extracted, &program2, 11, 19));

        assert!(match_missing(&extracted, &program3, false));

        // Testing fork 0 - 5 - 11 - 25 - 27 with current slot at 27
        let extracted = cache.extract(
            &TestWorkingSlot(27),
            vec![
                (program1, (LoadedProgramMatchCriteria::NoCriteria, 1)),
                (program2, (LoadedProgramMatchCriteria::NoCriteria, 1)),
                (program3, (LoadedProgramMatchCriteria::NoCriteria, 1)),
            ]
            .into_iter(),
        );

        assert!(match_slot(&extracted, &program1, 0, 27));
        assert!(match_slot(&extracted, &program2, 11, 27));

        assert!(match_missing(&extracted, &program3, true));

        // Testing fork 0 - 10 - 20 - 22 with current slot at 22
        let extracted = cache.extract(
            &TestWorkingSlot(22),
            vec![
                (program1, (LoadedProgramMatchCriteria::NoCriteria, 1)),
                (program2, (LoadedProgramMatchCriteria::NoCriteria, 1)),
                (program3, (LoadedProgramMatchCriteria::NoCriteria, 1)),
            ]
            .into_iter(),
        );

        assert!(match_slot(&extracted, &program1, 20, 22));

        assert!(match_missing(&extracted, &program2, false));
        assert!(match_missing(&extracted, &program3, true));
    }

    #[test]
    fn test_prune_expired() {
        let mut cache = new_mock_cache::<TestForkGraphSpecific>();

        // Fork graph created for the test
        //                   0
        //                 /   \
        //                10    5
        //                |     |
        //                20    11
        //                |     | \
        //                22   15  25
        //                      |   |
        //                     16  27
        //                      |
        //                     19
        //                      |
        //                     23

        let mut fork_graph = TestForkGraphSpecific::default();
        fork_graph.insert_fork(&[0, 10, 20, 22]);
        fork_graph.insert_fork(&[0, 5, 11, 12, 15, 16, 18, 19, 21, 23]);
        fork_graph.insert_fork(&[0, 5, 11, 25, 27]);
        let fork_graph = Arc::new(RwLock::new(fork_graph));
        cache.set_fork_graph(fork_graph);

        let program1 = Pubkey::new_unique();
        assert!(!cache.replenish(program1, new_test_loaded_program(10, 11)).0);
        assert!(!cache.replenish(program1, new_test_loaded_program(20, 21)).0);

        let program2 = Pubkey::new_unique();
        assert!(!cache.replenish(program2, new_test_loaded_program(5, 6)).0);
        assert!(!cache.replenish(program2, new_test_loaded_program(11, 12)).0);

        let program3 = Pubkey::new_unique();
        assert!(!cache.replenish(program3, new_test_loaded_program(25, 26)).0);

        // The following is a special case, where there's an expiration slot
        let test_program = Arc::new(LoadedProgram {
            program: LoadedProgramType::DelayVisibility,
            account_size: 0,
            deployment_slot: 11,
            effective_slot: 11,
            maybe_expiration_slot: Some(15),
            tx_usage_counter: AtomicU64::default(),
            ix_usage_counter: AtomicU64::default(),
        });
        assert!(!cache.replenish(program1, test_program).0);

        // Testing fork 0 - 5 - 11 - 15 - 16 - 19 - 21 - 23 with current slot at 19
        let extracted = cache.extract(
            &TestWorkingSlot(12),
            vec![
                (program1, (LoadedProgramMatchCriteria::NoCriteria, 1)),
                (program2, (LoadedProgramMatchCriteria::NoCriteria, 1)),
                (program3, (LoadedProgramMatchCriteria::NoCriteria, 1)),
            ]
            .into_iter(),
        );

        // Program1 deployed at slot 11 should not be expired yet
        assert!(match_slot(&extracted, &program1, 11, 12));
        assert!(match_slot(&extracted, &program2, 11, 12));

        assert!(match_missing(&extracted, &program3, false));

        // Testing fork 0 - 5 - 11 - 12 - 15 - 16 - 19 - 21 - 23 with current slot at 15
        // This would cause program4 deployed at slot 15 to be expired.
        let extracted = cache.extract(
            &TestWorkingSlot(15),
            vec![
                (program1, (LoadedProgramMatchCriteria::NoCriteria, 1)),
                (program2, (LoadedProgramMatchCriteria::NoCriteria, 1)),
                (program3, (LoadedProgramMatchCriteria::NoCriteria, 1)),
            ]
            .into_iter(),
        );

        assert!(match_slot(&extracted, &program2, 11, 15));

        assert!(match_missing(&extracted, &program1, false));
        assert!(match_missing(&extracted, &program3, false));

        // Test that the program still exists in the cache, even though it is expired.
        assert_eq!(
            cache
                .entries
                .get(&program1)
                .expect("Didn't find program1")
                .len(),
            3
        );

        // New root 5 should not evict the expired entry for program1
        cache.prune(5, 0);
        assert_eq!(
            cache
                .entries
                .get(&program1)
                .expect("Didn't find program1")
                .len(),
            1
        );

        // New root 15 should evict the expired entry for program1
        cache.prune(15, 0);
        assert!(cache.entries.get(&program1).is_none());
    }

    #[test]
    fn test_fork_prune_find_first_ancestor() {
        let mut cache = new_mock_cache::<TestForkGraphSpecific>();

        // Fork graph created for the test
        //                   0
        //                 /   \
        //                10    5
        //                |
        //                20

        // Deploy program on slot 0, and slot 5.
        // Prune the fork that has slot 5. The cache should still have the program
        // deployed at slot 0.
        let mut fork_graph = TestForkGraphSpecific::default();
        fork_graph.insert_fork(&[0, 10, 20]);
        fork_graph.insert_fork(&[0, 5]);
        let fork_graph = Arc::new(RwLock::new(fork_graph));
        cache.set_fork_graph(fork_graph);

        let program1 = Pubkey::new_unique();
        assert!(!cache.replenish(program1, new_test_loaded_program(0, 1)).0);
        assert!(!cache.replenish(program1, new_test_loaded_program(5, 6)).0);

        cache.prune(10, 0);

        let extracted = cache.extract(
            &TestWorkingSlot(20),
            vec![(program1, (LoadedProgramMatchCriteria::NoCriteria, 1))].into_iter(),
        );

        // The cache should have the program deployed at slot 0
        assert_eq!(
            extracted
                .lock()
                .unwrap()
                .loaded
                .entries
                .get(&program1)
                .expect("Did not find the program")
                .deployment_slot,
            0
        );
    }

    #[test]
    fn test_prune_by_deployment_slot() {
        let mut cache = new_mock_cache::<TestForkGraphSpecific>();

        // Fork graph created for the test
        //                   0
        //                 /   \
        //                10    5
        //                |
        //                20

        // Deploy program on slot 0, and slot 5.
        // Prune the fork that has slot 5. The cache should still have the program
        // deployed at slot 0.
        let mut fork_graph = TestForkGraphSpecific::default();
        fork_graph.insert_fork(&[0, 10, 20]);
        fork_graph.insert_fork(&[0, 5, 6]);
        let fork_graph = Arc::new(RwLock::new(fork_graph));
        cache.set_fork_graph(fork_graph);

        let program1 = Pubkey::new_unique();
        assert!(!cache.replenish(program1, new_test_loaded_program(0, 1)).0);
        assert!(!cache.replenish(program1, new_test_loaded_program(5, 6)).0);

        let program2 = Pubkey::new_unique();
        assert!(!cache.replenish(program2, new_test_loaded_program(10, 11)).0);

        let extracted = cache.extract(
            &TestWorkingSlot(20),
            vec![
                (program1, (LoadedProgramMatchCriteria::NoCriteria, 1)),
                (program2, (LoadedProgramMatchCriteria::NoCriteria, 1)),
            ]
            .into_iter(),
        );

        assert!(match_slot(&extracted, &program1, 0, 20));
        assert!(match_slot(&extracted, &program2, 10, 20));

        let extracted = cache.extract(
            &TestWorkingSlot(6),
            vec![
                (program1, (LoadedProgramMatchCriteria::NoCriteria, 1)),
                (program2, (LoadedProgramMatchCriteria::NoCriteria, 1)),
            ]
            .into_iter(),
        );

        assert!(match_slot(&extracted, &program1, 5, 6));
        assert!(match_missing(&extracted, &program2, false));

        // Pruning slot 5 will remove program1 entry deployed at slot 5.
        // On fork chaining from slot 5, the entry deployed at slot 0 will become visible.
        cache.prune_by_deployment_slot(5);

        let extracted = cache.extract(
            &TestWorkingSlot(20),
            vec![
                (program1, (LoadedProgramMatchCriteria::NoCriteria, 1)),
                (program2, (LoadedProgramMatchCriteria::NoCriteria, 1)),
            ]
            .into_iter(),
        );

        assert!(match_slot(&extracted, &program1, 0, 20));
        assert!(match_slot(&extracted, &program2, 10, 20));

        let extracted = cache.extract(
            &TestWorkingSlot(6),
            vec![
                (program1, (LoadedProgramMatchCriteria::NoCriteria, 1)),
                (program2, (LoadedProgramMatchCriteria::NoCriteria, 1)),
            ]
            .into_iter(),
        );

        assert!(match_slot(&extracted, &program1, 0, 6));
        assert!(match_missing(&extracted, &program2, false));

        // Pruning slot 10 will remove program2 entry deployed at slot 10.
        // As there is no other entry for program2, extract() will return it as missing.
        cache.prune_by_deployment_slot(10);

        let extracted = cache.extract(
            &TestWorkingSlot(20),
            vec![
                (program1, (LoadedProgramMatchCriteria::NoCriteria, 1)),
                (program2, (LoadedProgramMatchCriteria::NoCriteria, 1)),
            ]
            .into_iter(),
        );

        assert!(match_slot(&extracted, &program1, 0, 20));
        assert!(match_missing(&extracted, &program2, false));
    }

    #[test]
    fn test_usable_entries_for_slot() {
        new_mock_cache::<TestForkGraph>();
        let tombstone = Arc::new(LoadedProgram::new_tombstone(0, LoadedProgramType::Closed));

        assert!(LoadedPrograms::<TestForkGraph>::is_entry_usable(
            &tombstone,
            0,
            &LoadedProgramMatchCriteria::NoCriteria
        ));

        assert!(LoadedPrograms::<TestForkGraph>::is_entry_usable(
            &tombstone,
            1,
            &LoadedProgramMatchCriteria::Tombstone
        ));

        assert!(LoadedPrograms::<TestForkGraph>::is_entry_usable(
            &tombstone,
            1,
            &LoadedProgramMatchCriteria::NoCriteria
        ));

        assert!(LoadedPrograms::<TestForkGraph>::is_entry_usable(
            &tombstone,
            1,
            &LoadedProgramMatchCriteria::DeployedOnOrAfterSlot(0)
        ));

        assert!(!LoadedPrograms::<TestForkGraph>::is_entry_usable(
            &tombstone,
            1,
            &LoadedProgramMatchCriteria::DeployedOnOrAfterSlot(1)
        ));

        let program = new_test_loaded_program(0, 1);

        assert!(LoadedPrograms::<TestForkGraph>::is_entry_usable(
            &program,
            0,
            &LoadedProgramMatchCriteria::NoCriteria
        ));

        assert!(!LoadedPrograms::<TestForkGraph>::is_entry_usable(
            &program,
            1,
            &LoadedProgramMatchCriteria::Tombstone
        ));

        assert!(LoadedPrograms::<TestForkGraph>::is_entry_usable(
            &program,
            1,
            &LoadedProgramMatchCriteria::NoCriteria
        ));

        assert!(LoadedPrograms::<TestForkGraph>::is_entry_usable(
            &program,
            1,
            &LoadedProgramMatchCriteria::DeployedOnOrAfterSlot(0)
        ));

        assert!(!LoadedPrograms::<TestForkGraph>::is_entry_usable(
            &program,
            1,
            &LoadedProgramMatchCriteria::DeployedOnOrAfterSlot(1)
        ));

        let program = Arc::new(new_test_loaded_program_with_usage_and_expiry(
            0,
            1,
            AtomicU64::default(),
            Some(2),
        ));

        assert!(LoadedPrograms::<TestForkGraph>::is_entry_usable(
            &program,
            0,
            &LoadedProgramMatchCriteria::NoCriteria
        ));

        assert!(LoadedPrograms::<TestForkGraph>::is_entry_usable(
            &program,
            1,
            &LoadedProgramMatchCriteria::NoCriteria
        ));

        assert!(!LoadedPrograms::<TestForkGraph>::is_entry_usable(
            &program,
            1,
            &LoadedProgramMatchCriteria::Tombstone
        ));

        assert!(!LoadedPrograms::<TestForkGraph>::is_entry_usable(
            &program,
            2,
            &LoadedProgramMatchCriteria::NoCriteria
        ));

        assert!(LoadedPrograms::<TestForkGraph>::is_entry_usable(
            &program,
            1,
            &LoadedProgramMatchCriteria::DeployedOnOrAfterSlot(0)
        ));

        assert!(!LoadedPrograms::<TestForkGraph>::is_entry_usable(
            &program,
            1,
            &LoadedProgramMatchCriteria::DeployedOnOrAfterSlot(1)
        ));
    }
}
