namespace Sanchoris.SkillDefinition

/-- The type of a State of `skill.yaml`. -/
inductive StateType where
  | command
  | pass
  | choice
  | wait
  | succeed
  | fail
  deriving DecidableEq, Repr

/-- An element of `Choices`, `Retry` or `Catch`, with the keys written in it. -/
structure Rule where
  keys : List String
  errorEquals : List String
  next : Option String
  deriving DecidableEq, Repr

/-- A State of `skill.yaml`. `type` is `none` when `Type` is missing or names no type, and
`isEnd` is whether the State has `End: true`. -/
structure State where
  name : String
  keys : List String
  type : Option StateType
  next : Option String
  isEnd : Bool
  choices : List Rule
  default : Option String
  retriers : List Rule
  catchers : List Rule
  deriving DecidableEq, Repr

/-- The root of `skill.yaml`: its keys, `StartAt` and the States in the order they are written. -/
structure Definition where
  keys : List String
  startAt : Option String
  states : List State
  deriving DecidableEq, Repr

/-- Where an issue is: the root, a State, or an element of a State's `Choices`, `Retry` or
`Catch`, counted from 0. -/
inductive Place where
  | root
  | state (name : String)
  | choice (state : String) (index : Nat)
  | retrier (state : String) (index : Nat)
  | catcher (state : String) (index : Nat)
  deriving DecidableEq, Repr

/-- A failed check of `skill.yaml`. `missingField` names the keys of which one is required. -/
inductive Issue where
  | missingField (place : Place) (fields : List String)
  | unknownState (place : Place) (field : String) (target : String)
  | unreachable (state : String)
  | noTerminal
  | misplacedStatesAll (place : Place)
  deriving DecidableEq, Repr

/-- The State names that a State refers to, with where and in which key. -/
def targets (s : State) : List (Place × String × String) :=
  [ s.next.toList.map fun t => (.state s.name, "Next", t),
    s.choices.zipIdx.filterMap fun (r, i) => r.next.map fun t => (.choice s.name i, "Next", t),
    s.default.toList.map fun t => (.state s.name, "Default", t),
    s.catchers.zipIdx.filterMap fun (r, i) => r.next.map fun t => (.catcher s.name i, "Next", t) ].flatten

/-- The States a State can go to next: `Next`, the `Next` of `Choices`, `Default` and the `Next`
of `Catch`. -/
def successors (s : State) : List String :=
  (targets s).map fun p => p.2.2

def isTerminal (s : State) : Bool :=
  s.type == some .succeed || s.type == some .fail || s.isEnd

/-- A State name is reachable when it is `StartAt` or a successor of a reachable State. -/
inductive Reachable (d : Definition) : String → Prop where
  | start {n : String} : d.startAt = some n → Reachable d n
  | step {s : State} {b : String} :
      s ∈ d.states → Reachable d s.name → b ∈ successors s → Reachable d b

theorem Reachable.startAt {d : Definition} {n : String} (h : Reachable d n) :
    ∃ m, d.startAt = some m := by
  induction h with
  | start hn => exact ⟨_, hn⟩
  | step _ _ _ ih => exact ih

def succOf (d : Definition) (n : String) : List String :=
  d.states.flatMap fun s => if s.name = n then successors s else []

/-- The names that reachability can reach: `StartAt` and every successor. -/
def candidates (d : Definition) : List String :=
  d.startAt.toList ++ d.states.flatMap successors

def expand (d : Definition) (seen : List String) : List String :=
  (seen.flatMap (succOf d)).filter fun x => !seen.contains x

theorem mem_succOf {d : Definition} {n x : String} :
    x ∈ succOf d n ↔ ∃ s ∈ d.states, s.name = n ∧ x ∈ successors s := by
  simp only [succOf, List.mem_flatMap]
  constructor
  · rintro ⟨s, hs, hx⟩
    by_cases h : s.name = n
    · exact ⟨s, hs, h, by simpa [h] using hx⟩
    · simp [h] at hx
  · rintro ⟨s, hs, h, hx⟩
    exact ⟨s, hs, by simpa [h] using hx⟩

theorem contains_false {s : List String} {x : String} : s.contains x = false ↔ x ∉ s := by
  simp

theorem contains_true {s : List String} {x : String} : s.contains x = true ↔ x ∈ s := by
  simp

theorem countP_lt {U s s' : List String} (hsub : ∀ y, y ∈ s → y ∈ s') {x : String}
    (hxU : x ∈ U) (hx' : x ∈ s') (hx : x ∉ s) :
    U.countP (fun y => !s'.contains y) < U.countP (fun y => !s.contains y) := by
  induction U with
  | nil => simp at hxU
  | cons a U ih =>
    simp only [List.countP_cons]
    have hmono : U.countP (fun y => !s'.contains y) ≤ U.countP (fun y => !s.contains y) := by
      apply List.countP_mono_left
      intro y _ hy
      simp only [Bool.not_eq_true', contains_false] at hy ⊢
      exact fun h => hy (hsub y h)
    have hhead : (if (!s'.contains a) = true then 1 else 0) ≤
        (if (!s.contains a) = true then 1 else 0) := by
      by_cases ha : a ∈ s
      · have ha' : a ∈ s' := hsub a ha
        have e1 : s.contains a = true := contains_true.mpr ha
        have e2 : s'.contains a = true := contains_true.mpr ha'
        simp only [e1, e2, Bool.not_true, Bool.false_eq_true, ite_false, Nat.le_refl]
      · have e1 : s.contains a = false := contains_false.mpr ha
        simp only [e1, Bool.not_false, ite_true]
        split <;> omega
    rcases List.mem_cons.mp hxU with h | h
    · subst h
      have e1 : s'.contains x = true := contains_true.mpr hx'
      have e2 : s.contains x = false := contains_false.mpr hx
      simp only [e1, e2, Bool.not_true, Bool.not_false, Bool.false_eq_true, ite_false, ite_true]
      omega
    · have := ih h
      omega

/-- The names reachable from `seen`: `expand` is repeated until it adds no name. -/
def closure (d : Definition) (seen : List String) : List String :=
  let fresh := expand d seen
  if _h : fresh = [] then seen else closure d (seen ++ fresh)
termination_by (candidates d).countP fun y => !seen.contains y
decreasing_by
  obtain ⟨x, hx⟩ := List.exists_mem_of_ne_nil fresh _h
  simp only [fresh, expand, List.mem_filter, List.mem_flatMap, Bool.not_eq_true',
    contains_false] at hx
  obtain ⟨⟨n, _, hxn⟩, hxs⟩ := hx
  apply countP_lt (x := x)
  · intro y hy
    exact List.mem_append_left _ hy
  · obtain ⟨s, hs, _, hxs'⟩ := mem_succOf.mp hxn
    exact List.mem_append_right _ (List.mem_flatMap.mpr ⟨s, hs, hxs'⟩)
  · apply List.mem_append_right
    simp only [expand, List.mem_filter, Bool.not_eq_true', contains_false]
    exact ⟨List.mem_flatMap.mpr ⟨n, by assumption, hxn⟩, hxs⟩
  · exact hxs

theorem subset_closure (d : Definition) (seen : List String) :
    ∀ x ∈ seen, x ∈ closure d seen := by
  fun_induction closure d seen with
  | case1 seen fresh h => intro x hx; exact hx
  | case2 seen fresh h ih => intro x hx; exact ih x (List.mem_append_left _ hx)

theorem closure_closed (d : Definition) (seen : List String) :
    ∀ n ∈ closure d seen, ∀ x ∈ succOf d n, x ∈ closure d seen := by
  fun_induction closure d seen with
  | case1 seen fresh h =>
    intro n hn x hx
    refine Classical.byContradiction fun hxs => ?_
    have : x ∈ fresh := by
      simp only [fresh, expand, List.mem_filter, List.mem_flatMap, Bool.not_eq_true',
        contains_false]
      exact ⟨⟨n, hn, hx⟩, hxs⟩
    simp [h] at this
  | case2 seen fresh h ih => exact ih

theorem closure_sound (d : Definition) (seen : List String) (hseen : ∀ x ∈ seen, Reachable d x) :
    ∀ x ∈ closure d seen, Reachable d x := by
  fun_induction closure d seen with
  | case1 seen fresh h => exact hseen
  | case2 seen fresh h ih =>
    apply ih
    intro x hx
    rcases List.mem_append.mp hx with hx | hx
    · exact hseen x hx
    · simp only [fresh, expand, List.mem_filter, List.mem_flatMap] at hx
      obtain ⟨⟨n, hn, hxn⟩, _⟩ := hx
      obtain ⟨s, hs, hsn, hxs⟩ := mem_succOf.mp hxn
      exact Reachable.step hs (hsn ▸ hseen n hn) hxs

/-- The names reachable from `StartAt`. -/
def reachableNames (d : Definition) : List String :=
  closure d d.startAt.toList

theorem mem_reachableNames_iff (d : Definition) (n : String) :
    n ∈ reachableNames d ↔ Reachable d n := by
  constructor
  · apply closure_sound
    intro x hx
    exact Reachable.start (by simpa using hx)
  · intro h
    induction h with
    | start hn => exact subset_closure d _ _ (by simp [hn])
    | step hs _ hb ih =>
      exact closure_closed d _ _ ih _ (mem_succOf.mpr ⟨_, hs, rfl, hb⟩)

instance (d : Definition) (n : String) : Decidable (Reachable d n) :=
  decidable_of_iff _ (mem_reachableNames_iff d n)

/-- For each group of keys, `missingField` when the object has none of them. -/
def missingOf (place : Place) (keys : List String) (required : List (List String)) : List Issue :=
  required.filterMap fun fields =>
    if fields.any keys.contains then none else some (.missingField place fields)

/-- The groups of keys that a State of a type requires, besides `Type`, `Next` and `End`. -/
def requiredOf : StateType → List (List String)
  | .command => [["Run", "Run.$"], ["TimeoutSeconds"]]
  | .wait => [["Seconds", "Timestamp"]]
  | .choice => [["Choices"]]
  | .fail => [["Error"], ["Cause"]]
  | .pass => []
  | .succeed => []

/-- Whether a State of the type goes on with `Next` or ends with `End: true`. -/
def continues : StateType → Bool
  | .command => true
  | .pass => true
  | .wait => true
  | .choice => false
  | .succeed => false
  | .fail => false

def rootRequired : List (List String) :=
  [["Inputs"], ["StartAt"], ["States"], ["TimeoutSeconds"]]

def retrierRequired : List (List String) :=
  [["ErrorEquals"], ["IntervalSeconds"], ["MaxAttempts"], ["BackoffRate"]]

def catcherRequired : List (List String) :=
  [["ErrorEquals"], ["Next"]]

def choiceRequired : List (List String) :=
  [["Next"]]

def stateMissing (s : State) : List Issue :=
  [ missingOf (.state s.name) s.keys [["Type"]],
    match s.type with
    | some t =>
      missingOf (.state s.name) s.keys (requiredOf t) ++
        (if continues t && s.next.isNone && !s.isEnd then [.missingField (.state s.name) ["Next", "End"]]
        else [])
    | none => [],
    s.choices.zipIdx.flatMap fun (r, i) => missingOf (.choice s.name i) r.keys choiceRequired,
    s.retriers.zipIdx.flatMap fun (r, i) => missingOf (.retrier s.name i) r.keys retrierRequired,
    s.catchers.zipIdx.flatMap fun (r, i) => missingOf (.catcher s.name i) r.keys catcherRequired ].flatten

/-- The required keys that are missing, in the root and in every State. -/
def missingFields (d : Definition) : List Issue :=
  missingOf .root d.keys rootRequired ++ d.states.flatMap stateMissing

def names (d : Definition) : List String :=
  d.states.map (·.name)

/-- `StartAt`, `Next`, the `Next` of `Choices`, `Default` and the `Next` of `Catch` that name no
State. -/
def unknownStates (d : Definition) : List Issue :=
  (d.startAt.toList.filterMap fun t =>
      if (names d).contains t then none else some (.unknownState .root "StartAt" t)) ++
    d.states.flatMap fun s =>
      (targets s).filterMap fun (p, f, t) =>
        if (names d).contains t then none else some (.unknownState p f t)

/-- The States that `StartAt` does not reach. -/
def unreachableStates (d : Definition) : List Issue :=
  d.states.filterMap fun s =>
    if (reachableNames d).contains s.name then none else some (.unreachable s.name)

/-- `States.ALL` that is not alone in `ErrorEquals` or not in the last element of `Retry` or
`Catch`. -/
def misplacedOf (place : String → Nat → Place) (s : State) (rules : List Rule) : List Issue :=
  rules.zipIdx.filterMap fun (r, i) =>
    if r.errorEquals.contains "States.ALL" && (r.errorEquals.length != 1 || i + 1 != rules.length) then
      some (.misplacedStatesAll (place s.name i))
    else none

def misplacedStatesAll (d : Definition) : List Issue :=
  d.states.flatMap fun s => misplacedOf .retrier s s.retriers ++ misplacedOf .catcher s s.catchers

/-- Every failed check of `skill.yaml`: the required keys, the references to States, reachability
from `StartAt`, a terminal State (`End`, `Succeed` or `Fail`) and the place of `States.ALL`. -/
def validate (d : Definition) : List Issue :=
  [ missingFields d,
    unknownStates d,
    unreachableStates d,
    if d.states.any isTerminal then [] else [.noTerminal],
    misplacedStatesAll d ].flatten

private theorem parts_nil {d : Definition} (h : validate d = []) :
    missingFields d = [] ∧ unknownStates d = [] ∧ unreachableStates d = [] ∧
      d.states.any isTerminal = true ∧ misplacedStatesAll d = [] := by
  simp only [validate, List.flatten_eq_nil_iff, List.mem_cons, List.not_mem_nil, or_false,
    forall_eq_or_imp, forall_eq] at h
  obtain ⟨h1, h2, h3, h4, h5⟩ := h
  refine ⟨h1, h2, h3, ?_, h5⟩
  cases ht : d.states.any isTerminal
  · simp [ht] at h4
  · rfl

private theorem reachable_terminal (d : Definition) (h : validate d = []) :
    (∀ s ∈ d.states, Reachable d s.name) ∧
      ∃ s ∈ d.states, isTerminal s = true ∧ Reachable d s.name := by
  obtain ⟨_, _, h3, h4, _⟩ := parts_nil h
  have hall : ∀ s ∈ d.states, Reachable d s.name := by
    intro s hs
    simp only [unreachableStates, List.filterMap_eq_nil_iff] at h3
    have := h3 s hs
    cases hr : (reachableNames d).contains s.name
    · simp only [hr, Bool.false_eq_true, ite_false, reduceCtorEq] at this
    · exact (mem_reachableNames_iff d s.name).mp (contains_true.mp hr)
  refine ⟨hall, ?_⟩
  obtain ⟨s, hs, ht⟩ := List.any_eq_true.mp h4
  exact ⟨s, hs, ht, hall s hs⟩

private theorem targets_defined (d : Definition) (h : validate d = []) :
    (∀ n, d.startAt = some n → n ∈ names d) ∧ ∀ s ∈ d.states, ∀ t ∈ successors s, t ∈ names d := by
  obtain ⟨_, h2, _, _, _⟩ := parts_nil h
  simp only [unknownStates, List.append_eq_nil_iff, List.filterMap_eq_nil_iff,
    List.flatMap_eq_nil_iff] at h2
  obtain ⟨hstart, hstates⟩ := h2
  constructor
  · intro n hn
    have := hstart n (by simp [hn])
    cases hc : (names d).contains n
    · simp only [hc, Bool.false_eq_true, ite_false, reduceCtorEq] at this
    · exact contains_true.mp hc
  · intro s hs t ht
    simp only [successors, List.mem_map] at ht
    obtain ⟨⟨p, f, t'⟩, hp, rfl⟩ := ht
    have := hstates s hs (p, f, t') hp
    cases hc : (names d).contains t'
    · simp only [hc, Bool.false_eq_true, ite_false, reduceCtorEq] at this
    · exact contains_true.mp hc

/-- A definition that passes the checks reaches every State from `StartAt`. -/
theorem validate_all_reachable (d : Definition) (h : validate d = []) :
    d.states.all (fun s => decide (Reachable d s.name)) = true := by
  simpa [List.all_eq_true] using (reachable_terminal d h).1

/-- A definition that passes the checks has a terminal State, and `StartAt` reaches it. -/
theorem validate_reaches_terminal (d : Definition) (h : validate d = []) :
    d.states.any (fun s => isTerminal s && decide (Reachable d s.name)) = true := by
  obtain ⟨s, hs, ht, hr⟩ := (reachable_terminal d h).2
  exact List.any_eq_true.mpr ⟨s, hs, by simp [ht, hr]⟩

/-- In a definition that passes the checks, `StartAt` names a State. -/
theorem validate_startAt_defined (d : Definition) (h : validate d = []) :
    d.startAt.any (names d).contains = true := by
  obtain ⟨s, hs, _, hr⟩ := (reachable_terminal d h).2
  obtain ⟨n, hn⟩ := hr.startAt
  simpa [hn] using (targets_defined d h).1 n hn

/-- In a definition that passes the checks, every successor of a State names a State. -/
theorem validate_successors_defined (d : Definition) (h : validate d = []) :
    d.states.all (fun s => (successors s).all (names d).contains) = true := by
  simp only [List.all_eq_true]
  intro s hs t ht
  exact contains_true.mpr ((targets_defined d h).2 s hs t ht)

/-- `validate` reports `noTerminal` exactly when no State is terminal. -/
private theorem noTerminal_mem (d : Definition) :
    Issue.noTerminal ∈ validate d ↔ ∀ s ∈ d.states, isTerminal s = false := by
  simp [validate, missingFields, missingOf, stateMissing, unknownStates, unreachableStates,
    misplacedStatesAll, misplacedOf]
  intro x _ h
  split at h
  · simp only [List.mem_append, List.mem_filterMap] at h
    rcases h with ⟨_, _, h⟩ | h
    · split at h <;> simp at h
    · split at h <;> simp at h
  · simp at h

/-- `validate` reports `noTerminal` exactly when no State is terminal. -/
theorem noTerminal_mem_validate (d : Definition) :
    (validate d).contains .noTerminal = d.states.all (fun s => !isTerminal s) := by
  have := noTerminal_mem d
  apply Bool.eq_iff_iff.mpr
  simpa [List.all_eq_true] using this

end Sanchoris.SkillDefinition
