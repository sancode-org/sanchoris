import Sanchoris.Engine

/-!
# Theorems of the engine

The properties of the transitions of `Sanchoris.Engine` that the control plane relies on
(`architecture.md`「Lean で書く範囲」).
-/

namespace Sanchoris.Engine

@[simp] theorem default_outputs : (default : Write).outputs = [] := rfl

@[simp] theorem default_entries : (default : Write).entries = [] := rfl

theorem step_exec (d : Definition) (r : Run) (seq attempt : Nat) (o : ExecOutput) (now : Nat) :
    step d r (.exec seq attempt o) now = onExec d r seq attempt o now := rfl

theorem step_timer (d : Definition) (r : Run) (t : Timer) (now : Nat) :
    step d r (.timer t) now = onTimer d r t now := rfl

theorem step_await (d : Definition) (r : Run) (order now : Nat) :
    step d r (.await order) now = onAwait d r order := rfl

theorem step_continue (d : Definition) (r : Run) (v : Json) (now : Nat) :
    step d r (.continue v) now = onContinue d r v := rfl

theorem step_abort (d : Definition) (r : Run) (now : Nat) :
    step d r .abort now = onAbort r now := rfl

theorem handles_iff (errorEquals : List String) (error : String) :
    handles errorEquals error = true ↔
      error ≠ runtimeError ∧ (error ∈ errorEquals ∨ "States.ALL" ∈ errorEquals) := by
  simp [handles]

/-- `States.Runtime` goes past every Retrier and Catcher. -/
theorem firstHandler_runtime (rules : List (List String)) :
    firstHandler rules runtimeError = none := by
  simp [firstHandler, handles]

theorem firstHandler_nil (error : String) : firstHandler [] error = none := rfl

theorem firstHandler_cons (r : List String) (rules : List (List String)) (error : String) :
    firstHandler (r :: rules) error =
      if handles r error then some 0 else (firstHandler rules error).map (· + 1) := by
  simp only [firstHandler, List.findIdx?_cons]

/-- The rules as the validation of `skill.yaml` leaves them: a rule with `States.ALL` has it alone
and is the last rule. -/
def allLast (rules : List (List String)) : Prop :=
  ∀ i, i < rules.length → "States.ALL" ∈ rules.getD i [] →
    rules.getD i [] = ["States.ALL"] ∧ i + 1 = rules.length

instance (rules : List (List String)) : Decidable (allLast rules) := by
  unfold allLast; infer_instance

theorem firstHandler_spec (rules : List (List String)) (error : String) (i : Nat)
    (h : firstHandler rules error = some i) :
    i < rules.length ∧ handles (rules.getD i []) error = true ∧
      ∀ j, j < i → handles (rules.getD j []) error = false := by
  induction rules generalizing i with
  | nil => simp [firstHandler_nil] at h
  | cons r rules ih =>
    rw [firstHandler_cons] at h
    by_cases hr : handles r error = true
    · simp only [hr, ite_true, Option.some.injEq] at h
      subst h
      simp [hr]
    · simp only [hr, Bool.false_eq_true, ite_false, Option.map_eq_some_iff] at h
      obtain ⟨k, hk, rfl⟩ := h
      obtain ⟨h1, h2, h3⟩ := ih k hk
      refine ⟨by simp; omega, by simpa using h2, ?_⟩
      intro j hj
      cases j with
      | zero => simpa using hr
      | succ j => simpa using h3 j (by omega)

/-- A rule with `States.ALL` handles an error only as the last rule, when no rule before it
names the error. -/
theorem firstHandler_statesAll (rules : List (List String)) (error : String) (h : allLast rules) :
    (firstHandler rules error).all (fun i =>
      !(rules.getD i []).contains "States.ALL" ||
        (i + 1 == rules.length && (rules.take i).all fun r => !r.contains error)) = true := by
  cases hf : firstHandler rules error with
  | none => rfl
  | some i =>
    obtain ⟨hi, _, hbefore⟩ := firstHandler_spec rules error i hf
    simp only [Option.all_some, Bool.or_eq_true, Bool.not_eq_true', Bool.and_eq_true, beq_iff_eq,
      List.all_eq_true, List.contains_eq_mem, decide_eq_false_iff_not]
    by_cases hall : "States.ALL" ∈ rules.getD i []
    · right
      refine ⟨(h i hi hall).2, ?_⟩
      intro r hr
      obtain ⟨j, hj, rfl⟩ := List.getElem_of_mem hr
      have hji : j < i := by simp at hj; omega
      have := hbefore j hji
      simp only [handles, Bool.and_eq_false_iff, bne_eq_false_iff_eq, Bool.or_eq_false_iff,
        List.contains_eq_mem, decide_eq_false_iff_not] at this
      have hlen : j < rules.length := by omega
      simp only [List.getElem_take]
      intro hmem
      rcases this with hrt | ⟨hne, _⟩
      · subst hrt
        rw [firstHandler_runtime] at hf
        exact absurd hf (by simp)
      · exact hne (by simpa [List.getD_eq_getElem?_getD, hlen] using hmem)
    · left
      simpa using hall

/-- Counts of retries within `MaxAttempts`, one for each Retrier. -/
def withinMax (retriers : List Retrier) (counts : List Nat) : Prop :=
  ∀ j, j < retriers.length → counts.getD j 0 ≤ (retriers.getD j default).maxAttempts

instance (retriers : List Retrier) (counts : List Nat) : Decidable (withinMax retriers counts) := by
  unfold withinMax; infer_instance

/-- A retry chooses the first Retrier that handles the error. -/
theorem retryOf_index (retriers : List Retrier) (counts : List Nat) (error : String) :
    (retryOf retriers counts error).all (fun p =>
      firstHandler (retriers.map (·.errorEquals)) error == some p.1) = true := by
  unfold retryOf
  cases firstHandler (retriers.map (·.errorEquals)) error with
  | none => rfl
  | some i => dsimp only; split <;> simp_all [List.getD_eq_getElem?_getD] <;> omega

/-- A retry counts one more retry of its Retrier. -/
theorem retryOf_counts (retriers : List Retrier) (counts : List Nat) (error : String) :
    (retryOf retriers counts error).all (fun p =>
      p.2 == counts.set p.1 (counts.getD p.1 0 + 1)) = true := by
  unfold retryOf
  cases firstHandler (retriers.map (·.errorEquals)) error with
  | none => rfl
  | some i => dsimp only; split <;> simp_all [List.getD_eq_getElem?_getD] <;> omega

/-- A Retrier that handles the error retries until it has used its `MaxAttempts`. -/
theorem retryOf_isSome (retriers : List Retrier) (counts : List Nat) (error : String) :
    (retryOf retriers counts error).isSome =
      (firstHandler (retriers.map (·.errorEquals)) error).any fun i =>
        decide (counts.getD i 0 < (retriers.getD i default).maxAttempts) := by
  unfold retryOf
  cases firstHandler (retriers.map (·.errorEquals)) error with
  | none => rfl
  | some i => dsimp only; split <;> simp_all [List.getD_eq_getElem?_getD] <;> omega

/-- The retries of each Retrier stay within its `MaxAttempts`. -/
theorem retryOf_withinMax (retriers : List Retrier) (counts : List Nat) (error : String)
    (h : withinMax retriers counts) :
    (retryOf retriers counts error).all (fun p => decide (withinMax retriers p.2)) = true := by
  unfold retryOf
  cases firstHandler (retriers.map (·.errorEquals)) error with
  | none => rfl
  | some i =>
    dsimp only
    split
    · rename_i hlt
      simp only [Option.all_some, decide_eq_true_eq]
      intro j hj
      have hj' := h j hj
      simp only [List.getD_eq_getElem?_getD] at hj' hlt ⊢
      by_cases hij : i = j
      · subst hij
        by_cases hi : i < counts.length
        · simp [hi] at hlt ⊢
          omega
        · simp [hi]
      · simp [hij, hj']
    · rfl

/-- The wait of a retry is at most `MaxDelaySeconds`. -/
theorem retryDelay_le (r : Retrier) (n most : Nat) (h : r.maxDelaySeconds = some most) :
    retryDelay r n ≤ most * 1000 := by
  simp only [retryDelay, h]
  exact Nat.min_le_right _ _

theorem latest_nil : latest [] = none := rfl

theorem latest_eq_none_iff (waiters : List Waiter) : latest waiters = none ↔ waiters = [] := by
  cases waiters with
  | nil => simp [latest]
  | cons w ws =>
    simp only [latest, reduceCtorEq, iff_false]
    cases latest ws with
    | none => simp
    | some v => by_cases h : w.order < v.order <;> simp [h]

theorem latest_mem (waiters : List Waiter) : (latest waiters).all (· ∈ waiters) = true := by
  induction waiters with
  | nil => rfl
  | cons w ws ih =>
    simp only [latest]
    cases hl : latest ws with
    | none => simp
    | some v =>
      simp only [hl, Option.all_some, decide_eq_true_eq] at ih
      by_cases h : w.order < v.order <;> simp [h, ih]

theorem latest_max (waiters : List Waiter) :
    (latest waiters).all (fun v => waiters.all (·.order ≤ v.order)) = true := by
  induction waiters with
  | nil => rfl
  | cons w ws ih =>
    simp only [latest]
    cases hl : latest ws with
    | none =>
      rw [latest_eq_none_iff] at hl
      simp [hl]
    | some v =>
      simp only [hl, Option.all_some, List.all_eq_true, decide_eq_true_eq] at ih
      by_cases h : w.order < v.order
      · simp only [h, ite_true, Option.all_some, List.all_cons, decide_eq_true_eq,
          Bool.and_eq_true, List.all_eq_true]
        exact ⟨by omega, ih⟩
      · simp only [h, ite_false, Option.all_some, List.all_cons, decide_eq_true_eq,
          Bool.and_eq_true, List.all_eq_true, Nat.le_refl, true_and]
        intro x hx
        have := ih x hx
        omega

/-- Of the waiters with the latest order, the first one is chosen. -/
theorem latest_first (waiters : List Waiter) :
    (latest waiters).all (fun v => (waiters.takeWhile (· != v)).all (·.order < v.order)) = true := by
  induction waiters with
  | nil => rfl
  | cons w ws ih =>
    simp only [latest]
    cases hl : latest ws with
    | none => simp
    | some v =>
      simp only [hl, Option.all_some] at ih
      by_cases h : w.order < v.order
      · have hne : (w != v) = true := by
          simp only [bne_iff_ne, ne_eq]
          intro hwv
          subst hwv
          omega
        simp only [h, ite_true, Option.all_some, List.takeWhile_cons, hne, List.all_cons,
          decide_eq_true_eq, Bool.and_eq_true]
        exact ⟨trivial, ih⟩
      · simp [h]

/-- With no SkillRun in `await`, a new SkillRun is right under the thread. -/
theorem parentOf_eq_none_iff (waiters : List Waiter) (_h : (waiters.map (·.order)).Nodup) :
    parentOf waiters = none ↔ waiters = [] := by
  simp [parentOf, latest_eq_none_iff]

/-- A new SkillRun is a child of the State entry of the SkillRun that entered `await` last. -/
theorem parentOf_last (waiters : List Waiter) (_h : (waiters.map (·.order)).Nodup) :
    (parentOf waiters).all (fun p =>
      waiters.any fun w => w.parent == p && waiters.all (·.order ≤ w.order)) = true := by
  have hmem := latest_mem waiters
  have hmax := latest_max waiters
  simp only [parentOf]
  cases hl : latest waiters with
  | none => rfl
  | some v =>
    simp only [hl, Option.all_some, decide_eq_true_eq] at hmem hmax
    simp only [Option.map_some, Option.all_some, List.any_eq_true, Bool.and_eq_true, beq_iff_eq]
    exact ⟨v, hmem, rfl, hmax⟩

def Await.isWaiting : Await → Bool
  | .waiting _ => true
  | _ => false

def Await.isDelivered : Await → Bool
  | .delivered _ => true
  | _ => false

/-- A SkillRun is a waiter exactly while it runs and its StateEntry waits in `await`. -/
theorem waiterOf_isSome (r : Run) :
    (waiterOf r).isSome = (!r.status.finished && r.current.any (·.await.isWaiting)) := by
  unfold waiterOf
  split
  · split <;> simp_all [Await.isWaiting]
  · rename_i h
    cases hf : r.status.finished <;> cases hc : r.current <;> simp_all

theorem waiterOf_fields (r : Run) :
    (waiterOf r).all (fun w => w.run == r.id && r.current.any fun e =>
      e.state == w.state && e.visit == w.visit && e.await == .waiting w.order) = true := by
  unfold waiterOf
  split <;> (try split) <;> simp_all

/-- A `continue` goes to an `await` that waits. -/
theorem continueStep_deliver_iff (a : Await) (pending : Bool) :
    continueStep a pending = .deliver ↔ a.isWaiting = true := by
  cases a <;> cases pending <;> simp [continueStep, Await.isWaiting]

/-- With no `await` waiting, a second `continue` fails. -/
theorem continueStep_reject_iff (a : Await) (pending : Bool) :
    continueStep a pending = .reject ↔ a.isWaiting = false ∧ pending = true := by
  cases a <;> cases pending <;> simp [continueStep, Await.isWaiting]

/-- An `await` that ran again returns the Output its StateEntry was given. -/
theorem awaitStep_delivered (v : Json) (pending : Option Json) :
    awaitStep (.delivered v) pending = .replay v := rfl

/-- An `await` takes a `continue` that came before it. -/
theorem awaitStep_take (a : Await) (v : Json) (h : a.isDelivered = false) :
    awaitStep a (some v) = .take v := by
  cases a <;> simp_all [awaitStep, Await.isDelivered]

theorem awaitStep_wait (a : Await) (h : a.isDelivered = false) : awaitStep a none = .wait := by
  cases a <;> simp_all [awaitStep, Await.isDelivered]

def Outcome.run? : Outcome → Option Run
  | .applied r _ => some r
  | _ => none

def Outcome.write? : Outcome → Option Write
  | .applied _ w => some w
  | _ => none

/-- A `continue` that comes before the `await` of a command is returned by that `await`. -/
theorem await_returns_earlier_continue (d : Definition) (r : Run) (s : State) (e : Entry)
    (c : Command) (deadline order now : Nat) (v : Json) (hs : openState d r = some (s, e))
    (hc : s.body = .command c) (hp : e.phase = .executing deadline) (ha : e.await = .idle)
    (hn : r.pendingContinue = none) :
    ((step d r (.continue v) now).run?.bind fun r' =>
      (step d r' (.await order) now).write?.map (·.outputs)) = some [v] := by
  have hf : r.status.finished = false := by
    unfold openState at hs; split at hs <;> simp_all
  have hs' : openState d { r with pendingContinue := some v } = some (s, e) := by
    unfold openState at hs ⊢; exact hs
  simp [step, onContinue, hf, hs, ha, hn, continueStep, Outcome.run?, onAwait, hs', hc, hp,
    awaitStep, Outcome.write?, Acc.open_, Acc.entry, Acc.output]

/-- A `continue` that comes while the `await` of a command waits goes to it at once. -/
theorem continue_goes_to_waiting_await (d : Definition) (r : Run) (s : State) (e : Entry)
    (o now : Nat) (v : Json) (hs : openState d r = some (s, e)) (ha : e.await = .waiting o) :
    (step d r (.continue v) now).write?.map (·.outputs) = some [v] := by
  have hf : r.status.finished = false := by
    unfold openState at hs; split at hs <;> simp_all
  simp [step, onContinue, hf, hs, ha, continueStep, Outcome.write?, Acc.open_, Acc.entry,
    Acc.output]

/-- A SkillRun that runs past its `TimeoutSeconds` fails with `States.Timeout`. -/
theorem run_timeout (d : Definition) (r : Run) (now : Nat) (hf : r.status.finished = false)
    (hn : r.startedAt + d.timeoutSeconds * 1000 ≤ now) :
    (step d r (.timer .run) now).run?.map (fun r' => (r'.status, r'.error)) =
      some (.failed, some timeoutError) := by
  have : ¬ now < r.startedAt + d.timeoutSeconds * 1000 := by omega
  cases hc : r.current <;>
    simp [step, onTimer, hf, this, hc, Outcome.run?, Acc.failRun, Acc.notice, Acc.close, Acc.entry]

/-! ### What a transition keeps

`Grows a b`: `b` is `a` after more of a transition: the same SkillRun, with the writes of `a`
and more after them. -/

def Grows (a b : Acc) : Prop :=
  b.run.id = a.run.id ∧ b.run.skill = a.run.skill ∧ b.run.input = a.run.input ∧
    b.run.startedAt = a.run.startedAt ∧ a.write.timers <+: b.write.timers ∧
    a.write.entries <+: b.write.entries ∧ a.write.notices <+: b.write.notices

theorem Grows.refl (a : Acc) : Grows a a := by
  simp [Grows]

theorem Grows.trans {a b c : Acc} (h1 : Grows a b) (h2 : Grows b c) : Grows a c := by
  obtain ⟨a1, a2, a3, a4, a5, a6, a7⟩ := h1
  obtain ⟨b1, b2, b3, b4, b5, b6, b7⟩ := h2
  exact ⟨b1.trans a1, b2.trans a2, b3.trans a3, b4.trans a4, a5.trans b5, a6.trans b6, a7.trans b7⟩

theorem grows_entry (acc : Acc) (e : Entry) : Grows acc (acc.entry e) := by
  simp [Grows, Acc.entry, List.prefix_append]

theorem grows_notice (acc : Acc) (n : Notice) : Grows acc (acc.notice n) := by
  simp [Grows, Acc.notice, List.prefix_append]

theorem grows_timer (acc : Acc) (t : Timer) (at_ : Nat) : Grows acc (acc.timer t at_) := by
  simp [Grows, Acc.timer, List.prefix_append]

theorem grows_exec (acc : Acc) (x : Exec) : Grows acc (acc.exec x) := by
  simp [Grows, Acc.exec]

theorem grows_open (acc : Acc) (e : Entry) (st : RunStatus) : Grows acc (acc.open_ e st) := by
  simp [Grows, Acc.open_, Acc.entry, List.prefix_append]

theorem grows_close (acc : Acc) (e : Entry) (now : Nat) (st : EntryStatus) (o : Option Json)
    (er c n : Option String) : Grows acc (acc.close e now st o er c n) := by
  simp [Grows, Acc.close, Acc.entry, List.prefix_append]

theorem grows_succeed (acc : Acc) (now : Nat) (p : List String) (o : Json) :
    Grows acc (acc.succeed now p o) := by
  simp [Grows, Acc.succeed, Acc.notice, List.prefix_append]

theorem grows_failRun (acc : Acc) (now : Nat) (p : List String) (er c : String) :
    Grows acc (acc.failRun now p er c) := by
  simp [Grows, Acc.failRun, Acc.notice, List.prefix_append]

theorem grows_transitionNotice (acc : Acc) (s : State) (n : String) (o : Option ExecOutput) :
    Grows acc (acc.transitionNotice s n o) := by
  unfold Acc.transitionNotice
  split
  · simp [Grows, Acc.notice, List.prefix_append]
  · simp [Grows]

theorem grows_attempt (acc : Acc) (c : Command) (e : Entry) (attempt now : Nat) :
    Grows acc (acc.attempt c e attempt now) := by
  unfold Acc.attempt
  dsimp only
  split
  · exact (grows_close ..).trans (grows_failRun ..)
  · exact ((grows_open ..).trans (grows_exec ..)).trans (grows_timer ..)

theorem grows_seq (acc : Acc) (seq : Nat) (visits : List (String × Nat)) :
    Grows acc { acc with run := { acc.run with seq, visits } } := by
  simp [Grows]

theorem enter_grows (d : Definition) (now : Nat) (fuel : Nat) :
    (∀ acc name input, Grows acc (enter fuel d now acc name input)) ∧
      (∀ acc s e output result, Grows acc (transit fuel d now acc s e output result)) := by
  induction fuel with
  | zero =>
    have h0 : ∀ acc name input, Grows acc (enter 0 d now acc name input) := by
      intro acc name input; simp only [enter]; exact grows_failRun ..
    refine ⟨h0, fun acc s e output result => ?_⟩
    unfold transit
    dsimp only
    split
    · exact (grows_close ..).trans (grows_succeed ..)
    · exact ((grows_close ..).trans (grows_transitionNotice ..)).trans (h0 ..)
  | succ n ih =>
    have h1 : ∀ acc name input, Grows acc (enter (n + 1) d now acc name input) := by
      intro acc name input
      unfold enter
      split
      · exact grows_failRun ..
      · dsimp only
        refine (grows_seq acc (acc.run.seq + 1) (bumpVisit acc.run.visits name)).trans ?_
        split
        · exact grows_attempt ..
        · split
          · exact (grows_close ..).trans (grows_failRun ..)
          · exact ih.2 ..
        · split
          · exact (grows_close ..).trans (grows_failRun ..)
          · exact ih.2 ..
          · split
            · exact ih.2 ..
            · exact (grows_close ..).trans (grows_failRun ..)
        · exact (grows_open ..).trans (grows_timer ..)
        · split
          · exact (grows_close ..).trans (grows_failRun ..)
          · exact (grows_open ..).trans (grows_timer ..)
        · exact (grows_close ..).trans (grows_succeed ..)
        · exact (grows_close ..).trans (grows_failRun ..)
    refine ⟨h1, fun acc s e output result => ?_⟩
    unfold transit
    dsimp only
    split
    · exact (grows_close ..).trans (grows_succeed ..)
    · exact ((grows_close ..).trans (grows_transitionNotice ..)).trans (h1 ..)

/-- A new SkillRun has its ID, skill, input and start. -/
theorem start_run (d : Definition) (id : Nat) (input : Json) (now : Nat) :
    (start d id input now).run?.map (fun r => (r.id, r.skill, r.input, r.startedAt)) =
      some (id, d.skill, input, now) := by
  have h := fun acc => (enter_grows d now maxStates).1 acc d.startAt input
  unfold start
  dsimp only [Outcome.run?, Option.map_some]
  rw [(h _).1, (h _).2.1, (h _).2.2.1, (h _).2.2.2.1]
  rfl

/-- The first timer of a new SkillRun is its `TimeoutSeconds`. -/
theorem start_timer (d : Definition) (id : Nat) (input : Json) (now : Nat) :
    (start d id input now).write?.map (·.timers.head?) =
      some (some (.run, now + d.timeoutSeconds * 1000)) := by
  have h := fun acc => (enter_grows d now maxStates).1 acc d.startAt input
  unfold start
  dsimp only [Outcome.write?, Option.map_some]
  obtain ⟨rest, hr⟩ := (h _).2.2.2.2.1
  rw [← hr]
  rfl

/-! ### Transitions go only to States of the definition -/

/-- The open StateEntry, if any, names a State of the definition. -/
def Defined (d : Definition) (r : Run) : Prop :=
  ∀ e, r.current = some e → (d.lookup e.state).isSome

theorem openState_spec {d : Definition} {r : Run} {s : State} {e : Entry}
    (h : openState d r = some (s, e)) :
    r.status.finished = false ∧ r.current = some e ∧ d.lookup e.state = some s := by
  unfold openState at h
  split at h
  · simp at h
  · split at h
    · simp at h
    · rename_i e' hc
      simp only [Option.map_eq_some_iff, Prod.mk.injEq] at h
      obtain ⟨s', hs, rfl, rfl⟩ := h
      simp_all

theorem defined_none (d : Definition) (r : Run) (h : r.current = none) : Defined d r := by
  intro e he; simp [h] at he

theorem defined_close (d : Definition) (acc : Acc) (e : Entry) (now : Nat) (st : EntryStatus)
    (o : Option Json) (er c n : Option String) : Defined d (acc.close e now st o er c n).run :=
  defined_none _ _ rfl

theorem defined_succeed (d : Definition) (acc : Acc) (now : Nat) (p : List String) (o : Json) :
    Defined d (acc.succeed now p o).run :=
  defined_none _ _ rfl

theorem defined_failRun (d : Definition) (acc : Acc) (now : Nat) (p : List String) (er c : String) :
    Defined d (acc.failRun now p er c).run :=
  defined_none _ _ rfl

theorem defined_open (d : Definition) (acc : Acc) (e : Entry) (st : RunStatus)
    (h : (d.lookup e.state).isSome) : Defined d (acc.open_ e st).run := by
  intro e' he'
  simp only [Acc.open_, Acc.entry, Option.some.injEq] at he'
  exact he' ▸ h

theorem defined_timer (d : Definition) (acc : Acc) (t : Timer) (at_ : Nat)
    (h : Defined d acc.run) : Defined d (acc.timer t at_).run := h

theorem defined_exec (d : Definition) (acc : Acc) (x : Exec) (h : Defined d acc.run) :
    Defined d (acc.exec x).run := h

theorem defined_output (d : Definition) (acc : Acc) (v : Json) (h : Defined d acc.run) :
    Defined d (acc.output v).run := h

theorem defined_attempt (d : Definition) (acc : Acc) (c : Command) (e : Entry) (attempt now : Nat)
    (h : (d.lookup e.state).isSome) : Defined d (acc.attempt c e attempt now).run := by
  unfold Acc.attempt
  dsimp only
  split
  · exact defined_failRun _ _ _ _ _ _
  · exact defined_timer _ _ _ _ (defined_exec _ _ _ (defined_open _ _ _ _ h))

theorem lookup_name {d : Definition} {name : String} {s : State} (h : d.lookup name = some s) :
    (d.lookup name).isSome := by
  simp [h]

theorem enter_defined (d : Definition) (now : Nat) (fuel : Nat) :
    (∀ acc name input, Defined d (enter fuel d now acc name input).run) ∧
      (∀ acc s e output result, Defined d (transit fuel d now acc s e output result).run) := by
  induction fuel with
  | zero =>
    have h0 : ∀ acc name input, Defined d (enter 0 d now acc name input).run := by
      intro acc name input; simp only [enter]; exact defined_failRun _ _ _ _ _ _
    refine ⟨h0, fun acc s e output result => ?_⟩
    unfold transit
    dsimp only
    split
    · exact defined_succeed _ _ _ _ _
    · exact h0 _ _ _
  | succ n ih =>
    have h1 : ∀ acc name input, Defined d (enter (n + 1) d now acc name input).run := by
      intro acc name input
      unfold enter
      split
      · exact defined_failRun _ _ _ _ _ _
      · rename_i s hs
        have hl := lookup_name hs
        dsimp only
        split
        · exact defined_attempt _ _ _ _ _ _ hl
        · split
          · exact defined_failRun _ _ _ _ _ _
          · exact ih.2 _ _ _ _ _
        · split
          · exact defined_failRun _ _ _ _ _ _
          · exact ih.2 _ _ _ _ _
          · split
            · exact ih.2 _ _ _ _ _
            · exact defined_failRun _ _ _ _ _ _
        · exact defined_timer _ _ _ _ (defined_open _ _ _ _ hl)
        · split
          · exact defined_failRun _ _ _ _ _ _
          · exact defined_timer _ _ _ _ (defined_open _ _ _ _ hl)
        · exact defined_succeed _ _ _ _ _
        · exact defined_failRun _ _ _ _ _ _
    refine ⟨h1, fun acc s e output result => ?_⟩
    unfold transit
    dsimp only
    split
    · exact defined_succeed _ _ _ _ _
    · exact h1 _ _ _

theorem raise_defined (d : Definition) (now : Nat) (acc : Acc) (s : State) (e : Entry)
    (er c : String) (o : Option ExecOutput) (h : (d.lookup e.state).isSome) :
    Defined d (raise d now acc s e er c o).run := by
  unfold raise
  dsimp only
  split
  · split
    · exact defined_timer _ _ _ _ (defined_open _ _ _ _ h)
    · split
      · exact defined_failRun _ _ _ _ _ _
      · split
        · exact defined_failRun _ _ _ _ _ _
        · exact (enter_defined d now maxStates).1 _ _ _
  · exact defined_failRun _ _ _ _ _ _

/-- Every transition leaves the SkillRun in a State of its definition, or in none. -/
theorem step_defined (d : Definition) (r : Run) (event : Event) (now : Nat) :
    (step d r event now).run?.all (fun r' => r'.current.all fun e => (d.lookup e.state).isSome) =
      true := by
  suffices h : ∀ r', (step d r event now).run? = some r' → Defined d r' by
    cases hr : (step d r event now).run? with
    | none => rfl
    | some r' =>
      simp only [Option.all_some]
      cases hc : r'.current with
      | none => rfl
      | some e => simpa using h r' hr e hc
  intro r' hr
  cases event with
  | exec seq attempt o =>
    simp only [step, onExec] at hr
    split at hr
    · rename_i s e hs
      have hl : (d.lookup e.state).isSome := by simp [(openState_spec hs).2.2]
      split at hr
      · split at hr
        · split at hr
          · simp only [Outcome.run?, Option.some.injEq] at hr
            exact hr ▸ raise_defined _ _ _ _ _ _ _ _ hl
          · split at hr
            · simp only [Outcome.run?, Option.some.injEq] at hr
              exact hr ▸ raise_defined _ _ _ _ _ _ _ _ hl
            · split at hr
              · simp only [Outcome.run?, Option.some.injEq] at hr
                exact hr ▸ defined_failRun _ _ _ _ _ _
              · simp only [Outcome.run?, Option.some.injEq] at hr
                exact hr ▸ (enter_defined d now maxStates).2 _ _ _ _ _
        · simp [Outcome.run?] at hr
      · simp [Outcome.run?] at hr
    · simp [Outcome.run?] at hr
  | timer t =>
    simp only [step, onTimer] at hr
    split at hr
    · split at hr
      · simp [Outcome.run?] at hr
      · simp only [Outcome.run?, Option.some.injEq] at hr
        exact hr ▸ defined_failRun _ _ _ _ _ _
    · split at hr
      · rename_i s e hs
        have hl : (d.lookup e.state).isSome := by simp [(openState_spec hs).2.2]
        split at hr
        · split at hr
          · simp only [Outcome.run?, Option.some.injEq] at hr
            exact hr ▸ raise_defined _ _ _ _ _ _ _ _ hl
          · simp [Outcome.run?] at hr
        · simp [Outcome.run?] at hr
      · simp [Outcome.run?] at hr
    · split at hr
      · rename_i s e hs
        have hl : (d.lookup e.state).isSome := by simp [(openState_spec hs).2.2]
        split at hr
        · split at hr
          · simp only [Outcome.run?, Option.some.injEq] at hr
            exact hr ▸ defined_attempt _ _ _ _ _ _ hl
          · simp [Outcome.run?] at hr
        · simp [Outcome.run?] at hr
      · simp [Outcome.run?] at hr
    · split at hr
      · split at hr
        · split at hr
          · simp only [Outcome.run?, Option.some.injEq] at hr
            exact hr ▸ (enter_defined d now maxStates).2 _ _ _ _ _
          · simp [Outcome.run?] at hr
        · simp [Outcome.run?] at hr
      · simp [Outcome.run?] at hr
  | await order =>
    simp only [step, onAwait] at hr
    split at hr
    · simp [Outcome.run?] at hr
    · split at hr
      · rename_i s e hs
        obtain ⟨-, hc, hs'⟩ := openState_spec hs
        have hl : (d.lookup e.state).isSome := by simp [hs']
        split at hr
        · split at hr
          · simp only [Outcome.run?, Option.some.injEq] at hr
            subst hr
            intro e' he'
            rw [hc] at he'
            exact (Option.some.inj he') ▸ hl
          · simp only [Outcome.run?, Option.some.injEq] at hr
            subst hr
            exact defined_output _ _ _ (defined_open _ _ _ _ hl)
          · simp only [Outcome.run?, Option.some.injEq] at hr
            exact hr ▸ defined_open _ _ _ _ hl
        · simp [Outcome.run?] at hr
      · simp [Outcome.run?] at hr
  | «continue» v =>
    simp only [step, onContinue] at hr
    split at hr
    · simp [Outcome.run?] at hr
    · split at hr
      · rename_i s e hs
        obtain ⟨-, hc, hs'⟩ := openState_spec hs
        have hl : (d.lookup e.state).isSome := by simp [hs']
        split at hr
        · simp only [Outcome.run?, Option.some.injEq] at hr
          exact hr ▸ defined_output _ _ _ (defined_open _ _ _ _ hl)
        · simp only [Outcome.run?, Option.some.injEq] at hr
          subst hr
          intro e' he'
          simp only [hc, Option.some.injEq] at he'
          exact he' ▸ hl
        · simp [Outcome.run?] at hr
      · simp [Outcome.run?] at hr
  | abort =>
    simp only [step, onAbort] at hr
    split at hr
    · simp [Outcome.run?] at hr
    · simp only [Outcome.run?, Option.some.injEq] at hr
      exact hr ▸ defined_none _ _ rfl

/-! ### StateEntries are numbered from 1 -/

/-- `b` writes, after the StateEntries of `a`, first a StateEntry numbered `seq`. -/
def FirstEntry (a b : Acc) (seq : Nat) : Prop :=
  ∃ e rest, b.write.entries = a.write.entries ++ e :: rest ∧ e.seq = seq

theorem FirstEntry.grow {a b c : Acc} {seq : Nat} (h : FirstEntry a b seq) (g : Grows b c) :
    FirstEntry a c seq := by
  obtain ⟨e, rest, he, hs⟩ := h
  obtain ⟨-, -, -, -, -, ⟨more, hm⟩, -⟩ := g
  exact ⟨e, rest ++ more, by rw [← hm, he]; simp, hs⟩

theorem firstEntry_close (acc : Acc) (e : Entry) (now : Nat) (st : EntryStatus) (o : Option Json)
    (er c n : Option String) : FirstEntry acc (acc.close e now st o er c n) e.seq := by
  unfold FirstEntry Acc.close Acc.entry
  exact ⟨_, [], rfl, rfl⟩

theorem firstEntry_open (acc : Acc) (e : Entry) (st : RunStatus) :
    FirstEntry acc (acc.open_ e st) e.seq := by
  unfold FirstEntry Acc.open_ Acc.entry
  exact ⟨_, [], rfl, rfl⟩

theorem firstEntry_attempt (acc : Acc) (c : Command) (e : Entry) (attempt now : Nat) :
    FirstEntry acc (acc.attempt c e attempt now) e.seq := by
  unfold Acc.attempt
  dsimp only
  split
  · refine FirstEntry.grow ?_ (grows_failRun _ _ _ _ _)
    exact firstEntry_close _ _ _ _ _ _ _ _
  · refine FirstEntry.grow (FirstEntry.grow ?_ (grows_exec _ _)) (grows_timer _ _ _)
    exact firstEntry_open _ _ _

theorem firstEntry_transit (d : Definition) (fuel now : Nat) (acc : Acc) (s : State) (e : Entry)
    (o : Json) (r : Option ExecOutput) : FirstEntry acc (transit fuel d now acc s e o r) e.seq := by
  unfold transit
  dsimp only
  split
  · refine FirstEntry.grow ?_ (grows_succeed _ _ _ _)
    exact firstEntry_close _ _ _ _ _ _ _ _
  · refine FirstEntry.grow (FirstEntry.grow ?_ (grows_transitionNotice _ _ _ _))
      ((enter_grows d now fuel).1 _ _ _)
    exact firstEntry_close _ _ _ _ _ _ _ _

/-- Entering a State writes its StateEntry, numbered after the last one, first. -/
theorem firstEntry_enter (d : Definition) (fuel now : Nat) (acc : Acc) (name : String)
    (input : Json) (s : State) (hs : d.lookup name = some s) :
    FirstEntry acc (enter (fuel + 1) d now acc name input) (acc.run.seq + 1) := by
  unfold enter
  rw [hs]
  dsimp only
  split
  · exact firstEntry_attempt _ _ _ _ _
  · split
    · refine FirstEntry.grow ?_ (grows_failRun _ _ _ _ _)
      exact firstEntry_close _ _ _ _ _ _ _ _
    · exact firstEntry_transit _ _ _ _ _ _ _ _
  · split
    · refine FirstEntry.grow ?_ (grows_failRun _ _ _ _ _)
      exact firstEntry_close _ _ _ _ _ _ _ _
    · exact firstEntry_transit _ _ _ _ _ _ _ _
    · split
      · exact firstEntry_transit _ _ _ _ _ _ _ _
      · refine FirstEntry.grow ?_ (grows_failRun _ _ _ _ _)
        exact firstEntry_close _ _ _ _ _ _ _ _
  · refine FirstEntry.grow ?_ (grows_timer _ _ _)
    exact firstEntry_open _ _ _
  · split
    · refine FirstEntry.grow ?_ (grows_failRun _ _ _ _ _)
      exact firstEntry_close _ _ _ _ _ _ _ _
    · refine FirstEntry.grow ?_ (grows_timer _ _ _)
      exact firstEntry_open _ _ _
  · refine FirstEntry.grow ?_ (grows_succeed _ _ _ _)
    exact firstEntry_close _ _ _ _ _ _ _ _
  · refine FirstEntry.grow ?_ (grows_failRun _ _ _ _ _)
    exact firstEntry_close _ _ _ _ _ _ _ _

/-- The first StateEntry of a SkillRun is number 1. -/
theorem start_first_entry (d : Definition) (id : Nat) (input : Json) (now : Nat) :
    (start d id input now).write?.all (fun w => w.entries.head?.all (·.seq == 1)) = true := by
  unfold start
  dsimp only [Outcome.write?, Option.all_some]
  cases hs : d.lookup d.startAt with
  | none =>
    simp only [maxStates, enter, hs]
    rfl
  | some s =>
    obtain ⟨e, rest, he, hseq⟩ := firstEntry_enter d (maxStates - 1) now _ d.startAt input s hs
    simp only [maxStates] at he ⊢
    rw [he]
    simp [Acc.timer, hseq]

end Sanchoris.Engine
