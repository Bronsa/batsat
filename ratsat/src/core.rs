use std::cmp;
use system::{cpu_time, mem_used_peak};
use {lbool, Lit, Var};
use intmap::{Comparator, Heap, HeapData, PartialComparator};
use clause::{CRef, ClauseAllocator, ClauseRef, DeletePred, LSet, OccLists, OccListsData, VMap};

#[derive(Debug)]
pub struct Solver {
    // Extra results: (read-only member variable)
    /// If problem is satisfiable, this vector contains the model (if any).
    model: Vec<lbool>,
    /// If problem is unsatisfiable (possibly under assumptions),
    /// this vector represent the final conflict clause expressed in the assumptions.
    conflict: LSet,

    // Mode of operation:
    verbosity: i32,
    var_decay: f64,
    clause_decay: f64,
    random_var_freq: f64,
    random_seed: f64,
    luby_restart: bool,
    /// Controls conflict clause minimization (0=none, 1=basic, 2=deep).
    ccmin_mode: i32,
    /// Controls the level of phase saving (0=none, 1=limited, 2=full).
    phase_saving: i32,
    /// Use random polarities for branching heuristics.
    rnd_pol: bool,
    /// Initialize variable activities with a small random value.
    rnd_init_act: bool,
    /// The fraction of wasted memory allowed before a garbage collection is triggered.
    garbage_frac: f64,
    /// Minimum number to set the learnts limit to.
    min_learnts_lim: i32,

    /// The initial restart limit. (default 100)
    restart_first: i32,
    /// The factor with which the restart limit is multiplied in each restart. (default 1.5)
    restart_inc: f64,
    /// The intitial limit for learnt clauses is a factor of the original clauses. (default 1 / 3)
    learntsize_factor: f64,
    /// The limit for learnt clauses is multiplied with this factor each restart. (default 1.1)
    learntsize_inc: f64,

    learntsize_adjust_start_confl: i32,
    learntsize_adjust_inc: f64,

    // Statistics: (read-only member variable)
    solves: u64,
    starts: u64,
    decisions: u64,
    rnd_decisions: u64,
    propagations: u64,
    conflicts: u64,
    dec_vars: u64,
    // v.num_clauses: u64,
    // v.num_learnts: u64,
    // v.clauses_literals: u64,
    // v.learnts_literals: u64,
    max_literals: u64,
    tot_literals: u64,

    // Solver state:
    /// List of problem clauses.
    clauses: Vec<CRef>,
    /// List of learnt clauses.
    learnts: Vec<CRef>,
    // /// Assignment stack; stores all assigments made in the order they were made.
    // v.trail: Vec<Lit>,
    // /// Separator indices for different decision levels in 'trail'.
    // v.trail_lim: Vec<i32>,
    /// Current set of assumptions provided to solve by the user.
    assumptions: Vec<Lit>,

    /// A heuristic measurement of the activity of a variable.
    activity: VMap<f64>,
    // /// The current assignments.
    // v.assigns: VMap<lbool>,
    /// The preferred polarity of each variable.
    polarity: VMap<bool>,
    /// The users preferred polarity of each variable.
    user_pol: VMap<lbool>,
    /// Declares if a variable is eligible for selection in the decision heuristic.
    decision: VMap<bool>,
    // /// Stores reason and level for each variable.
    // v.vardata: VMap<VarData>,
    /// 'watches[lit]' is a list of constraints watching 'lit' (will go there if literal becomes true).
    watches_data: OccListsData<Lit, Watcher>,
    /// A priority queue of variables ordered with respect to the variable activity.
    order_heap_data: HeapData<Var>,
    /// If FALSE, the constraints are already unsatisfiable. No part of the solver state may be used!
    ok: bool,
    /// Amount to bump next clause with.
    cla_inc: f64,
    /// Amount to bump next variable with.
    var_inc: f64,
    /// Head of queue (as index into the trail -- no more explicit propagation queue in MiniSat).
    qhead: i32,
    /// Number of top-level assignments since last execution of 'simplify()'.
    simp_db_assigns: i32,
    /// Remaining number of propagations that must be made before next execution of 'simplify()'.
    simp_db_props: i64,
    /// Set by 'search()'.
    progress_estimate: f64,
    /// Indicates whether possibly inefficient linear scan for satisfied clauses should be performed in 'simplify'.
    remove_satisfied: bool,
    /// Next variable to be created.
    next_var: Var,
    ca: ClauseAllocator,

    released_vars: Vec<Var>,
    free_vars: Vec<Var>,

    // Temporaries (to reduce allocation overhead). Each variable is prefixed by the method in which it is
    // used, exept 'seen' wich is used in several places.
    seen: VMap<bool>,
    // analyze_stack: Vec<ShrinkStackElem>,
    analyze_toclear: Vec<Lit>,
    add_tmp: Vec<Lit>,

    max_learnts: f64,
    learntsize_adjust_confl: f64,
    learntsize_adjust_cnt: i32,

    // Resource contraints:
    conflict_budget: i64,
    propagation_budget: i64,
    asynch_interrupt: bool,

    v: SolverV,
}
#[derive(Debug)]
struct SolverV {
    /// The current assignments.
    assigns: VMap<lbool>,
    /// Assignment stack; stores all assigments made in the order they were made.
    trail: Vec<Lit>,
    /// Separator indices for different decision levels in 'trail'.
    trail_lim: Vec<i32>,
    /// Stores reason and level for each variable.
    vardata: VMap<VarData>,

    num_clauses: u64,
    num_learnts: u64,
    clauses_literals: u64,
    learnts_literals: u64,
}

impl Default for Solver {
    fn default() -> Self {
        Self {
            // Parameters (user settable):
            model: vec![],
            conflict: LSet::new(),
            verbosity: 0,
            var_decay: 0.95,
            clause_decay: 0.999,
            random_var_freq: 0.0,
            random_seed: 91648253.0,
            luby_restart: true,
            ccmin_mode: 2,
            phase_saving: 2,
            rnd_pol: false,
            rnd_init_act: false,
            garbage_frac: 0.20,
            min_learnts_lim: 0,
            restart_first: 100,
            restart_inc: 2.0,

            // Parameters (the rest):
            learntsize_factor: 1.0 / 3.0,
            learntsize_inc: 1.1,

            // Parameters (experimental):
            learntsize_adjust_start_confl: 100,
            learntsize_adjust_inc: 1.5,

            // Statistics: (formerly in 'SolverStats')
            solves: 0,
            starts: 0,
            decisions: 0,
            rnd_decisions: 0,
            propagations: 0,
            conflicts: 0,
            dec_vars: 0,
            // v.num_clauses: 0,
            // v.num_learnts: 0,
            // v.clauses_literals: 0,
            // v.learnts_literals: 0,
            max_literals: 0,
            tot_literals: 0,

            clauses: vec![],
            learnts: vec![],
            // v.trail: vec![],
            // v.trail_lim: vec![],
            assumptions: vec![],
            activity: VMap::new(),
            // v.assigns: VMap::new(),
            polarity: VMap::new(),
            user_pol: VMap::new(),
            decision: VMap::new(),
            // v.vardata: VMap::new(),
            watches_data: OccListsData::new(),
            order_heap_data: HeapData::new(),
            ok: true,
            cla_inc: 1.0,
            var_inc: 1.0,
            qhead: 0,
            simp_db_assigns: -1,
            simp_db_props: 0,
            progress_estimate: 0.0,
            remove_satisfied: true,
            next_var: Var::from_idx(0),

            ca: ClauseAllocator::new(),
            released_vars: vec![],
            free_vars: vec![],
            seen: VMap::new(),
            // analyze_stack: vec![],
            analyze_toclear: vec![],
            add_tmp: vec![],
            max_learnts: 0.0,
            learntsize_adjust_confl: 0.0,
            learntsize_adjust_cnt: 0,

            // Resource constraints:
            conflict_budget: -1,
            propagation_budget: -1,
            asynch_interrupt: false,

            v: SolverV {
                assigns: VMap::new(),
                trail: vec![],
                trail_lim: vec![],
                vardata: VMap::new(),
                num_clauses: 0,
                num_learnts: 0,
                clauses_literals: 0,
                learnts_literals: 0,
            },
        }
    }
}

impl Solver {
    pub fn set_verbosity(&mut self, verbosity: i32) {
        debug_assert!(0 <= verbosity && verbosity <= 2);
        self.verbosity = verbosity;
    }
    pub fn num_clauses(&self) -> u32 {
        self.v.num_clauses as u32
    }
    pub fn verbosity(&self) -> i32 {
        self.verbosity
    }

    pub fn set_decision_var(&mut self, v: Var, b: bool) {
        if b && !self.decision[v] {
            self.dec_vars += 1;
        } else if !b && self.decision[v] {
            self.dec_vars -= 1;
        }
        self.decision[v] = b;
        self.insert_var_order(v);
    }

    fn insert_var_order(&mut self, x: Var) {
        if !self.order_heap().in_heap(x) && self.decision[x] {
            self.order_heap().insert(x);
        }
    }

    pub fn num_vars(&self) -> u32 {
        self.next_var.idx()
    }

    /// Print some current statistics to standard output.
    pub fn print_stats(&self) {
        let cpu_time = cpu_time();
        let mem_used = mem_used_peak();
        println!("restarts              : {}", self.starts);
        println!(
            "conflicts             : {:<12}   ({:.0} /sec)",
            self.conflicts,
            self.conflicts as f64 / cpu_time
        );
        println!(
            "decisions             : {:<12}   ({:4.2} % random) ({:.0} /sec)",
            self.decisions,
            self.rnd_decisions as f32 * 100.0 / self.decisions as f32,
            self.decisions as f64 / cpu_time as f64
        );
        println!(
            "propagations          : {:<12}   ({:.0} /sec)",
            self.propagations,
            self.propagations as f64 / cpu_time
        );
        println!(
            "conflict literals     : {:<12}   ({:4.2} % deleted)",
            self.tot_literals,
            (self.max_literals - self.tot_literals) as f64 * 100.0 / self.max_literals as f64
        );
        if mem_used != 0.0 {
            println!("Memory used           : {:.2} MB", mem_used);
        }
        println!("CPU time              : {} s", cpu_time);
    }

    /// Creates a new SAT variable in the solver. If 'decision' is cleared, variable will not be
    /// used as a decision variable (NOTE! This has effects on the meaning of a SATISFIABLE result).
    pub fn new_var(&mut self, upol: lbool, dvar: bool) -> Var {
        let v = self.free_vars.pop().unwrap_or_else(|| {
            let v = self.next_var;
            self.next_var = Var::from_idx(self.next_var.idx() + 1);
            v
        });
        self.watches().init(Lit::new(v, false));
        self.watches().init(Lit::new(v, true));
        self.v.assigns.insert_default(v, lbool::UNDEF);
        self.v
            .vardata
            .insert_default(v, VarData::new(CRef::UNDEF, 0));
        if self.rnd_init_act {
            self.activity
                .insert_default(v, drand(&mut self.random_seed) * 0.00001);
        } else {
            self.activity.insert_default(v, 0.0);
        }
        self.seen.insert_default(v, false);
        self.polarity.insert_default(v, true);
        self.user_pol.insert_default(v, upol);
        self.decision.reserve_default(v);
        let len = self.v.trail.len();
        self.v.trail.reserve(v.idx() as usize + 1 - len);
        self.set_decision_var(v, dvar);
        v
    }

    pub fn new_var_default(&mut self) -> Var {
        self.new_var(lbool::UNDEF, true)
    }
    pub fn add_clause_reuse(&mut self, clause: &mut Vec<Lit>) -> bool {
        // eprintln!("add_clause({:?})", clause);
        debug_assert_eq!(self.v.decision_level(), 0);
        if !self.ok {
            return false;
        }
        clause.sort();
        let mut last_lit = Lit::UNDEF;
        let mut j = 0;
        for i in 0..clause.len() {
            let value = self.v.value_lit(clause[i]);
            if value == lbool::TRUE || clause[i] == !last_lit {
                return true;
            } else if value != lbool::FALSE && clause[i] != last_lit {
                last_lit = clause[i];
                clause[j] = clause[i];
                j += 1;
            }
        }
        clause.resize(j, Lit::UNDEF);
        if clause.len() == 0 {
            self.ok = false;
            return false;
        } else if clause.len() == 1 {
            self.v.unchecked_enqueue(clause[0], CRef::UNDEF);
        } else {
            let cr = self.ca.alloc_with_learnt(&clause, false);
            self.clauses.push(cr);
            self.attach_clause(cr);
        }

        true
    }

    /// Simplify the clause database according to the current top-level assigment. Currently, the only
    /// thing done here is the removal of satisfied clauses, but more things can be put here.
    pub fn simplify(&mut self) -> bool {
        debug_assert_eq!(self.v.decision_level(), 0);

        if !self.ok || self.propagate() != CRef::UNDEF {
            self.ok = false;
            return false;
        }

        if self.v.num_assigns() as i32 == self.simp_db_assigns || self.simp_db_props > 0 {
            return true;
        }

        // Remove satisfied clauses:
        self.remove_satisfied(true);
        if self.remove_satisfied {
            // Can be turned off.
            self.remove_satisfied(false);

            // TODO: what todo in if 'remove_satisfied' is false?

            // Remove all released variables from the trail:
            for &rvar in &self.released_vars {
                debug_assert_eq!(self.seen[rvar], false);
                self.seen[rvar] = true;
            }

            {
                let seen = &self.seen;
                self.v.trail.retain(|&lit| !seen[lit.var()]);
            }
            // eprintln!(
            //     "trail.size()= {}, qhead = {}",
            //     self.v.trail.len(),
            //     self.qhead
            // );
            self.qhead = self.v.trail.len() as i32;

            for &rvar in &self.released_vars {
                self.seen[rvar] = false;
            }

            // Released variables are now ready to be reused:
            self.free_vars.extend(self.released_vars.drain(..));
        }
        self.check_garbage();
        self.rebuild_order_heap();

        self.simp_db_assigns = self.v.num_assigns() as i32;
        // (shouldn't depend on stats really, but it will do for now)
        self.simp_db_props = (self.v.clauses_literals + self.v.learnts_literals) as i64;

        true
    }

    /// Shrink 'cs' to contain only non-satisfied clauses.
    // fn remove_satisfied(&mut self, cs: &mut Vec<CRef>) {
    fn remove_satisfied(&mut self, shrink_learnts: bool) {
        let cs: &mut Vec<CRef> = if shrink_learnts {
            &mut self.learnts
        } else {
            &mut self.clauses
        };
        let ca = &mut self.ca;
        let watches_data = &mut self.watches_data;
        let self_v = &mut self.v;
        cs.retain(|&cr| {
            let satisfied = self_v.satisfied(ca.get_ref(cr));
            if satisfied {
                self_v.remove_clause(ca, watches_data, cr)
            } else {
                let amount = {
                    let mut c = ca.get_mut(cr);
                    // Trim clause:
                    debug_assert_eq!(self_v.value_lit(c[0]), lbool::UNDEF);
                    debug_assert_eq!(self_v.value_lit(c[1]), lbool::UNDEF);
                    let mut k = 2;
                    let orig_size = c.size();
                    let mut end = c.size();
                    while k < end {
                        if self_v.value_lit(c[k]) == lbool::FALSE {
                            end -= 1;
                            c[k] = c[end];
                        } else {
                            k += 1;
                        }
                    }
                    c.shrink(end);
                    orig_size - end
                };
                // It was not in MiniSAT, but it is needed for correct wasted calculation.
                ca.free_amount(amount);
            }
            !satisfied
        });
    }

    fn rebuild_order_heap(&mut self) {
        let mut vs = vec![];
        for v in (0..self.num_vars()).map(Var::from_idx) {
            if self.decision[v] && self.v.value(v) == lbool::UNDEF {
                vs.push(v);
            }
        }
        self.order_heap().build(&vs);
    }

    fn attach_clause(&mut self, cr: CRef) {
        let (c0, c1, learnt, size) = {
            let c = self.ca.get_ref(cr);
            debug_assert!(c.size() > 1);
            (c[0], c[1], c.learnt(), c.size())
        };
        self.watches()[!c0].push(Watcher::new(cr, c1));
        self.watches()[!c1].push(Watcher::new(cr, c0));
        if learnt {
            self.v.num_learnts += 1;
            self.v.learnts_literals += size as u64;
        } else {
            self.v.num_clauses += 1;
            self.v.clauses_literals += size as u64;
        }
    }

    /// Propagates all enqueued facts. If a conflict arises, the conflicting clause is returned,
    /// otherwise CRef_Undef.
    ///
    /// # Post-conditions:
    ///
    /// - the propagation queue is empty, even if there was a conflict.
    fn propagate(&mut self) -> CRef {
        // These macros are to avoid false sharing of references.
        let mut confl = CRef::UNDEF;
        let mut num_props: u32 = 0;

        while (self.qhead as usize) < self.v.trail.len() {
            // 'p' is enqueued fact to propagate.
            let p = self.v.trail[self.qhead as usize];
            self.qhead += 1;
            let watches_data_ptr: *mut OccListsData<_, _> = &mut self.watches_data;
            // let ws = self.watches().lookup_mut(p);
            let ws = self.watches_data
                .lookup_mut_pred(p, &WatcherDeleted { ca: &self.ca });
            let mut i: usize = 0;
            let mut j: usize = 0;
            let end: usize = ws.len();
            num_props += 1;
            while i < end {
                // Try to avoid inspecting the clause:
                let blocker = ws[i].blocker;
                if self.v.value_lit(blocker) == lbool::TRUE {
                    ws[j] = ws[i];
                    j += 1;
                    i += 1;
                    continue;
                }

                // Make sure the false literal is data[1]:
                let cr = ws[i].cref;
                let mut c = self.ca.get_mut(cr);
                let false_lit = !p;
                if c[0] == false_lit {
                    c[0] = c[1];
                    c[1] = false_lit;
                }
                debug_assert_eq!(c[1], false_lit);
                i += 1;

                // If 0th watch is true, then clause is already satisfied.
                let first = c[0];
                let w = Watcher::new(cr, first);
                if first != blocker && self.v.value_lit(first) == lbool::TRUE {
                    ws[j] = w;
                    j += 1;
                    continue;
                }

                // Look for new watch:
                for k in 2..c.size() {
                    if self.v.value_lit(c[k]) != lbool::FALSE {
                        c[1] = c[k];
                        c[k] = false_lit;

                        // self.watches()[!c[1]].push(w);
                        assert_ne!(!c[1], p);
                        unsafe { &mut (*watches_data_ptr)[!c[1]] }.push(w);
                    }
                }

                // Did not find watch -- clause is unit under assignment:
                ws[j] = w;
                j += 1;
                if self.v.value_lit(first) == lbool::FALSE {
                    confl = cr;
                    self.qhead = self.v.trail.len() as i32;
                    // Copy the remaining watches:
                    while i < end {
                        ws[j] = ws[i];
                        j += 1;
                        i += 1;
                    }
                } else {
                    self.v.unchecked_enqueue(first, cr);
                }
            }
            let dummy = Watcher {
                cref: CRef::UNDEF,
                blocker: Lit::UNDEF,
            };
            ws.resize(i - j, dummy);
        }
        self.propagations += num_props as u64;
        self.simp_db_props -= num_props as i64;

        confl
    }

    fn check_garbage(&mut self) {
        if self.ca.wasted() as f64 > self.ca.len() as f64 * self.garbage_frac {
            self.garbage_collect();
        }
    }

    fn garbage_collect(&mut self) {
        // Initialize the next region to a size corresponding to the estimated utilization degree. This
        // is not precise but should avoid some unnecessary reallocations for the new region:
        let mut to = ClauseAllocator::with_start_cap(self.ca.len() - self.ca.wasted());

        self.reloc_all(&mut to);
        if self.verbosity >= 2 {
            println!(
                "|  Garbage collection:   {:12} bytes => {:12} bytes             |",
                self.ca.len() * ClauseAllocator::UNIT_SIZE,
                to.len() * ClauseAllocator::UNIT_SIZE
            );
        }
        self.ca = to;
    }
    fn reloc_all(&mut self, to: &mut ClauseAllocator) {
        macro_rules! is_removed {
            ($ca:expr, $cr:expr) => {
                $ca.get_ref($cr).mark() == 1
            };
        }
        // All watchers:
        //
        self.watches().clean_all();
        for v in (0..self.num_vars()).map(Var::from_idx) {
            for s in 0..2 {
                let p = Lit::new(v, s != 0);
                for watch in &mut self.watches_data[p] {
                    self.ca.reloc(&mut watch.cref, to);
                }
            }
        }

        // All reasons:
        //
        for &lit in &self.v.trail {
            let v = lit.var();

            // Note: it is not safe to call 'locked()' on a relocated clause. This is why we keep
            // 'dangling' reasons here. It is safe and does not hurt.
            let reason = self.v.reason(v);
            if reason != CRef::UNDEF {
                let cond = {
                    let c = self.ca.get_ref(reason);
                    c.reloced() || self.v.locked(&self.ca, c)
                };
                if cond {
                    debug_assert!(!is_removed!(self.ca, reason));
                    self.ca.reloc(&mut self.v.vardata[v].reason, to);
                }
            }
        }

        // All learnt:
        //
        {
            // let ca = &mut self.ca;
            // self.learnts.drain_filter(|cr| {
            //     if !is_removed!(ca, *cr) {
            //         ca.reloc(cr, to);
            //         false
            //     } else {
            //         true
            //     }
            // }).count();
            let mut j = 0;
            for i in 0..self.learnts.len() {
                let cr = self.learnts[i];
                if !is_removed!(self.ca, cr) {
                    self.learnts[j] = cr;
                    j += 1;
                }
            }
            self.learnts.resize(j, CRef::UNDEF);
        }

        // All original:
        //
        {
            // let ca = &mut self.ca;
            // self.clauses.drain_filter(|cr| {
            //     if !is_removed!(ca, *cr) {
            //         ca.reloc(cr, to);
            //         false
            //     } else {
            //         true
            //     }
            // }).count();
            let mut j = 0;
            for i in 0..self.clauses.len() {
                let cr = self.clauses[i];
                if !is_removed!(self.ca, cr) {
                    self.clauses[j] = cr;
                    j += 1;
                }
            }
            self.clauses.resize(j, CRef::UNDEF);
        }
    }

    fn order_heap(&mut self) -> Heap<Var, VarOrder> {
        self.order_heap_data.promote(VarOrder {
            activity: &self.activity,
        })
    }
    fn watches(&mut self) -> OccLists<Lit, Watcher, WatcherDeleted> {
        self.watches_data.promote(WatcherDeleted { ca: &self.ca })
    }
}

impl SolverV {
    pub fn num_assigns(&self) -> u32 {
        self.trail.len() as u32
    }

    pub fn value(&self, x: Var) -> lbool {
        self.assigns[x]
    }
    pub fn value_lit(&self, x: Lit) -> lbool {
        self.assigns[x.var()] ^ x.sign()
    }

    /// Detach a clause to watcher lists.
    fn detach_clause_strict(
        &mut self,
        ca: &mut ClauseAllocator,
        watches_data: &mut OccListsData<Lit, Watcher>,
        cr: CRef,
        strict: bool,
    ) {
        let (c0, c1, csize, clearnt) = {
            let c = ca.get_ref(cr);
            (c[0], c[1], c.size(), c.learnt())
        };
        debug_assert!(csize > 1);

        let mut watches = watches_data.promote(WatcherDeleted { ca });

        // Strict or lazy detaching:
        if strict {
            // watches[!c0].remove_item(&Watcher::new(cr, c1));
            // watches[!c1].remove_item(&Watcher::new(cr, c0));
            let pos = watches[!c0]
                .iter()
                .position(|x| x == &Watcher::new(cr, c1))
                .expect("Watcher not found");
            watches[!c0].remove(pos);
            let pos = watches[!c1]
                .iter()
                .position(|x| x == &Watcher::new(cr, c0))
                .expect("Watcher not found");
            watches[!c1].remove(pos);
        } else {
            watches.smudge(!c0);
            watches.smudge(!c1);
        }

        if clearnt {
            self.num_learnts -= 1;
            self.learnts_literals -= csize as u64;
        } else {
            self.num_clauses -= 1;
            self.clauses_literals -= csize as u64;
        }
    }
    fn detach_clause(
        &mut self,
        ca: &mut ClauseAllocator,
        watches_data: &mut OccListsData<Lit, Watcher>,
        cr: CRef,
    ) {
        self.detach_clause_strict(ca, watches_data, cr, false)
    }
    /// Detach and free a clause.
    fn remove_clause(
        &mut self,
        ca: &mut ClauseAllocator,
        watches_data: &mut OccListsData<Lit, Watcher>,
        cr: CRef,
    ) {
        self.detach_clause(ca, watches_data, cr);
        {
            let c = ca.get_ref(cr);
            // Don't leave pointers to free'd memory!
            if self.locked(ca, c) {
                self.vardata[c[0].var()].reason = CRef::UNDEF;
            }
        }
        ca.get_mut(cr).set_mark(1);
        ca.free(cr);
    }

    pub fn satisfied(&self, c: ClauseRef) -> bool {
        c.iter().any(|&lit| self.value_lit(lit) == lbool::TRUE)
    }

    pub fn decision_level(&self) -> u32 {
        self.trail_lim.len() as u32
    }

    fn reason(&self, x: Var) -> CRef {
        self.vardata[x].reason
    }

    fn unchecked_enqueue(&mut self, p: Lit, from: CRef) {
        debug_assert_eq!(self.value_lit(p), lbool::UNDEF);
        self.assigns[p.var()] = lbool::new(!p.sign());
        self.vardata[p.var()] = VarData::new(from, self.decision_level() as i32);
        self.trail.push(p);
    }

    /// Returns TRUE if a clause is a reason for some implication in the current state.
    fn locked(&self, ca: &ClauseAllocator, c: ClauseRef) -> bool {
        let reason = self.reason(c[0].var());
        self.value_lit(c[0]) == lbool::TRUE && reason != CRef::UNDEF && ca.get_ref(reason) == c
    }
    // inline bool     Solver::locked          (const Clause& c) const { return value(c[0]) == l_True && reason(var(c[0])) != CRef_Undef && ca.lea(reason(var(c[0]))) == &c; }
}

#[derive(Debug, Clone, Copy)]
struct VarData {
    reason: CRef,
    level: i32,
}

impl Default for VarData {
    fn default() -> Self {
        Self {
            reason: CRef::UNDEF,
            level: 0,
        }
    }
}

impl VarData {
    fn new(reason: CRef, level: i32) -> Self {
        Self { reason, level }
    }
}

#[derive(Debug, Clone, Copy)]
struct Watcher {
    cref: CRef,
    blocker: Lit,
}

impl Watcher {
    fn new(cref: CRef, blocker: Lit) -> Self {
        Self { cref, blocker }
    }
}

impl PartialEq for Watcher {
    fn eq(&self, rhs: &Self) -> bool {
        self.cref == rhs.cref
    }
}
impl Eq for Watcher {}

struct VarOrder<'a> {
    activity: &'a VMap<f64>,
}

impl<'a> PartialComparator<Var> for VarOrder<'a> {
    fn partial_cmp(&self, lhs: &Var, rhs: &Var) -> Option<cmp::Ordering> {
        Some(self.cmp(lhs, rhs))
    }
}
impl<'a> Comparator<Var> for VarOrder<'a> {
    fn cmp(&self, lhs: &Var, rhs: &Var) -> cmp::Ordering {
        PartialOrd::partial_cmp(&self.activity[*rhs], &self.activity[*lhs]).expect("NaN activity")
    }
}

struct WatcherDeleted<'a> {
    ca: &'a ClauseAllocator,
}

impl<'a> DeletePred<Watcher> for WatcherDeleted<'a> {
    fn deleted(&self, w: &Watcher) -> bool {
        self.ca.get_ref(w.cref).mark() == 1
    }
}

/// Generate a random double:
fn drand(seed: &mut f64) -> f64 {
    *seed *= 1389796.0;
    let q = (*seed / 2147483647.0) as i32;
    *seed -= q as f64 * 2147483647.0;
    return *seed / 2147483647.0;
}
