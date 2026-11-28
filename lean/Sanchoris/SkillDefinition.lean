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

private theorem mem_missingOf {p : Place} {keys : List String} {r : List (List String)} {i : Issue}
    (h : i ∈ missingOf p keys r) : ∃ fs, i = .missingField p fs := by
  simp only [missingOf, List.mem_filterMap] at h
  obtain ⟨fs, _, h⟩ := h
  split at h
  · cases h
  · exact ⟨fs, (Option.some.inj h).symm⟩

private theorem mem_flatMap_missingOf {l : List (Rule × Nat)} {f : Rule × Nat → Place}
    {req : List (List String)} {i : Issue}
    (h : i ∈ l.flatMap fun x => missingOf (f x) x.1.keys req) :
    ∃ p fs, i = .missingField p fs := by
  simp only [List.mem_flatMap] at h
  obtain ⟨x, _, h⟩ := h
  obtain ⟨fs, rfl⟩ := mem_missingOf h
  exact ⟨_, fs, rfl⟩

private theorem mem_missingFields {d : Definition} {i : Issue} (h : i ∈ missingFields d) :
    ∃ p fs, i = .missingField p fs := by
  simp only [missingFields, List.mem_append, List.mem_flatMap] at h
  rcases h with h | ⟨s, _, h⟩
  · obtain ⟨fs, rfl⟩ := mem_missingOf h
    exact ⟨_, fs, rfl⟩
  · simp only [stateMissing, List.mem_flatten, List.mem_cons, List.not_mem_nil, or_false] at h
    obtain ⟨l, hl, hm⟩ := h
    rcases hl with rfl | rfl | rfl | rfl | rfl
    · obtain ⟨fs, rfl⟩ := mem_missingOf hm
      exact ⟨_, fs, rfl⟩
    · split at hm
      · rcases List.mem_append.mp hm with hm | hm
        · obtain ⟨fs, rfl⟩ := mem_missingOf hm
          exact ⟨_, fs, rfl⟩
        · split at hm
          · exact ⟨_, _, List.mem_singleton.mp hm⟩
          · cases hm
      · cases hm
    · exact mem_flatMap_missingOf hm
    · exact mem_flatMap_missingOf hm
    · exact mem_flatMap_missingOf hm

private theorem mem_unknownStates {d : Definition} {i : Issue} (h : i ∈ unknownStates d) :
    ∃ p f t, i = .unknownState p f t ∧ (names d).contains t = false := by
  simp only [unknownStates, List.mem_append, List.mem_filterMap, List.mem_flatMap] at h
  rcases h with ⟨t, _, h⟩ | ⟨s, _, ⟨p, f, t⟩, _, h⟩
  · by_cases hc : (names d).contains t = true
    · simp_all
    · have hc' : (names d).contains t = false := by simpa using hc
      simp only [hc', Bool.false_eq_true, ite_false, Option.some.injEq] at h
      exact ⟨_, _, t, h.symm, hc'⟩
  · by_cases hc : (names d).contains t = true
    · simp_all
    · have hc' : (names d).contains t = false := by simpa using hc
      simp only [hc', Bool.false_eq_true, ite_false, Option.some.injEq] at h
      exact ⟨p, f, t, h.symm, hc'⟩

private theorem mem_misplacedStatesAll {d : Definition} {i : Issue} (h : i ∈ misplacedStatesAll d) :
    ∃ p, i = .misplacedStatesAll p := by
  simp only [misplacedStatesAll, List.mem_flatMap, List.mem_append] at h
  obtain ⟨s, _, h | h⟩ := h <;>
  · simp only [misplacedOf, List.mem_filterMap] at h
    obtain ⟨_, _, h⟩ := h
    split at h
    · exact ⟨_, (Option.some.inj h).symm⟩
    · cases h

private theorem mem_unreachableStates {d : Definition} {n : String} :
    Issue.unreachable n ∈ unreachableStates d ↔ ∃ s ∈ d.states, s.name = n ∧ ¬ Reachable d n := by
  simp only [unreachableStates, List.mem_filterMap]
  constructor
  · rintro ⟨s, hs, h⟩
    cases hr : (reachableNames d).contains s.name
    · simp only [hr, Bool.false_eq_true, ite_false, Option.some.injEq,
        Issue.unreachable.injEq] at h
      subst h
      refine ⟨s, hs, rfl, fun hreach => ?_⟩
      exact absurd ((mem_reachableNames_iff d s.name).mpr hreach) (contains_false.mp hr)
    · simp only [hr, ite_true, reduceCtorEq] at h
  · rintro ⟨s, hs, rfl, hn⟩
    refine ⟨s, hs, ?_⟩
    have : (reachableNames d).contains s.name = false := by
      cases hc : (reachableNames d).contains s.name
      · rfl
      · exact absurd ((mem_reachableNames_iff d s.name).mp (contains_true.mp hc)) hn
    simp only [this, Bool.false_eq_true, ite_false]

private theorem mem_validate {d : Definition} {i : Issue} :
    i ∈ validate d ↔ i ∈ missingFields d ∨ i ∈ unknownStates d ∨ i ∈ unreachableStates d ∨
      (i = .noTerminal ∧ d.states.any isTerminal = false) ∨ i ∈ misplacedStatesAll d := by
  simp only [validate, List.mem_flatten, List.mem_cons, List.not_mem_nil, or_false]
  constructor
  · rintro ⟨l, hl, hm⟩
    rcases hl with rfl | rfl | rfl | rfl | rfl
    · exact .inl hm
    · exact .inr (.inl hm)
    · exact .inr (.inr (.inl hm))
    · cases ht : d.states.any isTerminal
      · simp only [ht, Bool.false_eq_true, ite_false, List.mem_singleton] at hm
        exact .inr (.inr (.inr (.inl ⟨hm, rfl⟩)))
      · simp [ht] at hm
    · exact .inr (.inr (.inr (.inr hm)))
  · rintro (h | h | h | ⟨rfl, ht⟩ | h)
    · exact ⟨_, .inl rfl, h⟩
    · exact ⟨_, .inr (.inl rfl), h⟩
    · exact ⟨_, .inr (.inr (.inl rfl)), h⟩
    · exact ⟨_, .inr (.inr (.inr (.inl rfl))), by simp [ht]⟩
    · exact ⟨_, .inr (.inr (.inr (.inr rfl))), h⟩

private theorem mem_validate_unreachable (d : Definition) (n : String) :
    Issue.unreachable n ∈ validate d ↔ ∃ s ∈ d.states, s.name = n ∧ ¬ Reachable d n := by
  rw [mem_validate, ← mem_unreachableStates]
  constructor
  · rintro (h | h | h | ⟨h, _⟩ | h)
    · obtain ⟨_, _, h⟩ := mem_missingFields h; cases h
    · obtain ⟨_, _, _, h, _⟩ := mem_unknownStates h; cases h
    · exact h
    · cases h
    · obtain ⟨_, h⟩ := mem_misplacedStatesAll h; cases h
  · exact fun h => .inr (.inr (.inl h))

/-- `validate` reports a State as unreachable exactly when `StartAt` does not reach it. -/
theorem unreachable_mem_validate (d : Definition) :
    d.states.all (fun s =>
      (validate d).contains (.unreachable s.name) = !decide (Reachable d s.name)) = true := by
  simp only [List.all_eq_true, decide_eq_true_eq]
  intro s hs
  apply Bool.eq_iff_iff.mpr
  simp only [List.contains_iff_mem, Bool.not_eq_true', decide_eq_false_iff_not]
  rw [mem_validate_unreachable]
  exact ⟨fun ⟨_, _, _, h⟩ => h, fun h => ⟨s, hs, rfl, h⟩⟩

private theorem mem_unreachableStates_kind {d : Definition} {i : Issue} (h : i ∈ unreachableStates d) :
    ∃ n, i = .unreachable n := by
  simp only [unreachableStates, List.mem_filterMap] at h
  obtain ⟨s, _, h⟩ := h
  split at h
  · cases h
  · exact ⟨_, (Option.some.inj h).symm⟩

private theorem mem_validate_unknownState (d : Definition) (s : State) (hs : s ∈ d.states)
    (p : Place) (f t : String) (ht : (p, f, t) ∈ targets s) :
    Issue.unknownState p f t ∈ validate d ↔ (names d).contains t = false := by
  rw [mem_validate]
  constructor
  · rintro (h | h | h | ⟨h, _⟩ | h)
    · obtain ⟨_, _, h⟩ := mem_missingFields h; cases h
    · obtain ⟨_, _, t', h, hc⟩ := mem_unknownStates h
      cases h
      exact hc
    · obtain ⟨_, h⟩ := mem_unreachableStates_kind h; cases h
    · cases h
    · obtain ⟨_, h⟩ := mem_misplacedStatesAll h; cases h
  · intro hc
    refine .inr (.inl ?_)
    simp only [unknownStates, List.mem_append, List.mem_flatMap, List.mem_filterMap]
    refine .inr ⟨s, hs, (p, f, t), ht, ?_⟩
    simp only [hc, Bool.false_eq_true, ite_false]

/-- `validate` reports a `Next`, a `Next` of `Choices`, a `Default` or a `Next` of `Catch` exactly
when it names no State. -/
theorem unknownState_mem_validate (d : Definition) :
    d.states.all (fun s => (targets s).all fun x =>
      (validate d).contains (.unknownState x.1 x.2.1 x.2.2) = !(names d).contains x.2.2) = true := by
  simp only [List.all_eq_true, decide_eq_true_eq]
  intro s hs ⟨p, f, t⟩ ht
  apply Bool.eq_iff_iff.mpr
  simp only [List.contains_iff_mem, Bool.not_eq_true']
  exact mem_validate_unknownState d s hs p f t ht

/-- `validate` reports `StartAt` exactly when it names no State. -/
theorem startAt_unknown_mem_validate (d : Definition) :
    d.startAt.all (fun t =>
      (validate d).contains (.unknownState .root "StartAt" t) = !(names d).contains t) = true := by
  cases hstart : d.startAt with
  | none => rfl
  | some t =>
    simp only [Option.all_some, decide_eq_true_eq]
    apply Bool.eq_iff_iff.mpr
    simp only [List.contains_iff_mem, Bool.not_eq_true']
    rw [mem_validate]
    constructor
    · rintro (h | h | h | ⟨h, _⟩ | h)
      · obtain ⟨_, _, h⟩ := mem_missingFields h; cases h
      · obtain ⟨_, _, t', h, hc⟩ := mem_unknownStates h
        cases h
        exact hc
      · obtain ⟨_, h⟩ := mem_unreachableStates_kind h; cases h
      · cases h
      · obtain ⟨_, h⟩ := mem_misplacedStatesAll h; cases h
    · intro hc
      refine .inr (.inl ?_)
      simp only [unknownStates, List.mem_append, List.mem_flatMap, List.mem_filterMap]
      refine .inl ⟨t, by simp [hstart], ?_⟩
      simp only [hc, Bool.false_eq_true, ite_false]

private theorem missingField_mem_missingOf {p q : Place} {keys fs : List String}
    {req : List (List String)} :
    Issue.missingField q fs ∈ missingOf p keys req ↔
      q = p ∧ fs ∈ req ∧ fs.any keys.contains = false := by
  simp only [missingOf, List.mem_filterMap]
  constructor
  · rintro ⟨fs', hfs, h⟩
    cases hc : fs'.any keys.contains
    · simp only [hc, Bool.false_eq_true, ite_false, Option.some.injEq, Issue.missingField.injEq] at h
      obtain ⟨rfl, rfl⟩ := h
      exact ⟨rfl, hfs, hc⟩
    · simp [hc] at h
  · rintro ⟨rfl, hfs, hc⟩
    exact ⟨fs, hfs, by simp [hc]⟩

private theorem eq_of_mem_zipIdx {l : List α} {x y : α × Nat} (hx : x ∈ l.zipIdx)
    (hy : y ∈ l.zipIdx) (h : y.2 = x.2) : y = x := by
  rw [List.mem_zipIdx_iff_getElem?] at hx hy
  rw [h, hx] at hy
  exact Prod.ext (Option.some.inj hy).symm h

private theorem eq_of_name {l : List State} (hl : (l.map (·.name)).Nodup) {s t : State}
    (hs : s ∈ l) (ht : t ∈ l) (h : t.name = s.name) : t = s := by
  induction l with
  | nil => simp at hs
  | cons a l ih =>
    simp only [List.map_cons, List.nodup_cons, List.mem_map, not_exists, not_and] at hl
    rcases List.mem_cons.mp hs with hs | hs <;> rcases List.mem_cons.mp ht with ht | ht
    · rw [hs, ht]
    · exact absurd (hs ▸ h) (hl.1 t ht)
    · exact absurd (ht ▸ h).symm (hl.1 s hs)
    · exact ih hl.2 hs ht

/-- The places of the issues of a State: the State, or an element of its `Choices`, `Retry` or
`Catch`. -/
private theorem stateMissing_place {t : State} {p : Place} {fs : List String}
    (h : Issue.missingField p fs ∈ stateMissing t) :
    p = .state t.name ∨ ∃ k, p = .choice t.name k ∨ p = .retrier t.name k ∨ p = .catcher t.name k := by
  simp only [stateMissing, List.mem_flatten, List.mem_cons, List.not_mem_nil, or_false] at h
  obtain ⟨l, hl, hm⟩ := h
  rcases hl with rfl | rfl | rfl | rfl | rfl
  · exact .inl (missingField_mem_missingOf.mp hm).1
  · split at hm
    · rcases List.mem_append.mp hm with hm | hm
      · exact .inl (missingField_mem_missingOf.mp hm).1
      · split at hm
        · simp only [List.mem_singleton, Issue.missingField.injEq] at hm
          exact .inl hm.1
        · cases hm
    · cases hm
  · obtain ⟨x, _, hm⟩ := List.mem_flatMap.mp hm
    exact .inr ⟨x.2, .inl (missingField_mem_missingOf.mp hm).1⟩
  · obtain ⟨x, _, hm⟩ := List.mem_flatMap.mp hm
    exact .inr ⟨x.2, .inr (.inl (missingField_mem_missingOf.mp hm).1)⟩
  · obtain ⟨x, _, hm⟩ := List.mem_flatMap.mp hm
    exact .inr ⟨x.2, .inr (.inr (missingField_mem_missingOf.mp hm).1)⟩

/-- In a definition whose States have distinct names, an issue at a place of a State comes from
that State. -/
private theorem missingField_mem_missingFields {d : Definition} (hd : (names d).Nodup)
    {s : State} (hs : s ∈ d.states) {p : Place} {fs : List String}
    (hp : p = .state s.name ∨ ∃ k, p = .choice s.name k ∨ p = .retrier s.name k ∨
      p = .catcher s.name k) :
    Issue.missingField p fs ∈ missingFields d ↔ Issue.missingField p fs ∈ stateMissing s := by
  simp only [missingFields, List.mem_append, List.mem_flatMap]
  constructor
  · rintro (h | ⟨t, ht, h⟩)
    · obtain ⟨rfl, -⟩ := missingField_mem_missingOf.mp h
      rcases hp with h | ⟨k, h | h | h⟩ <;> cases h
    · have hn : t.name = s.name := by
        rcases stateMissing_place h with h' | ⟨k', h' | h' | h'⟩ <;>
          rcases hp with h'' | ⟨k, h'' | h'' | h''⟩ <;> rw [h''] at h' <;>
          first
            | cases h'
            | (injection h' with h1; exact h1.symm)
            | (injection h' with h1 h2; exact h1.symm)
      rwa [← eq_of_name hd hs ht hn]
  · exact fun h => .inr ⟨s, hs, h⟩

/-- `missingFields` reports a group of keys of the root exactly when the root has none of them. -/
theorem missingFields_root_iff (d : Definition) :
    rootRequired.all (fun fs =>
      (missingFields d).contains (.missingField .root fs) = !fs.any d.keys.contains) = true := by
  simp only [List.all_eq_true, decide_eq_true_eq]
  intro fs hfs
  apply Bool.eq_iff_iff.mpr
  simp only [List.contains_iff_mem, Bool.not_eq_true', missingFields, List.mem_append,
    List.mem_flatMap, missingField_mem_missingOf]
  constructor
  · rintro (⟨-, -, h⟩ | ⟨t, -, h⟩)
    · exact h
    · rcases stateMissing_place h with h | ⟨k, h | h | h⟩ <;> cases h
  · exact fun h => .inl ⟨trivial, hfs, h⟩

/-- `missingFields` reports the keys missing from the root before those of the States. -/
theorem missingFields_root_first (d : Definition) :
    (missingOf .root d.keys rootRequired).isPrefixOf (missingFields d) = true := by
  simp [missingFields, List.isPrefixOf_iff_prefix]

/-- `missingFields` reports a State without `Type` exactly when it has no `Type`. -/
theorem missingFields_type_iff (d : Definition) (hd : (names d).Nodup) :
    d.states.all (fun s =>
      (missingFields d).contains (.missingField (.state s.name) ["Type"]) =
        !s.keys.contains "Type") = true := by
  simp only [List.all_eq_true, decide_eq_true_eq]
  intro s hs
  apply Bool.eq_iff_iff.mpr
  simp only [List.contains_iff_mem, Bool.not_eq_true']
  rw [missingField_mem_missingFields hd hs (.inl rfl)]
  cases ht : s.type with
  | none => simp [stateMissing, ht, missingField_mem_missingOf, List.mem_flatMap]
  | some t =>
    cases t <;> simp [stateMissing, ht, missingField_mem_missingOf, List.mem_flatMap, requiredOf]

/-- `missingFields` reports a group of keys that the type of a State requires exactly when the
State has none of them. -/
theorem missingFields_required_iff (d : Definition) (hd : (names d).Nodup) :
    d.states.all (fun s => s.type.all fun t => (requiredOf t).all fun fs =>
      (missingFields d).contains (.missingField (.state s.name) fs) =
        !fs.any s.keys.contains) = true := by
  simp only [List.all_eq_true]
  intro s hs
  cases ht : s.type with
  | none => rfl
  | some t =>
    simp only [Option.all_some, List.all_eq_true, decide_eq_true_eq]
    intro fs hfs
    apply Bool.eq_iff_iff.mpr
    simp only [List.contains_iff_mem, Bool.not_eq_true']
    rw [missingField_mem_missingFields hd hs (.inl rfl)]
    cases t <;> simp only [requiredOf, List.mem_cons, List.not_mem_nil, or_false] at hfs <;>
      rcases hfs with rfl | rfl <;>
      simp [stateMissing, ht, missingField_mem_missingOf, List.mem_flatMap, requiredOf]

/-- `missingFields` reports a State without `Next` and `End` exactly when its type goes on and it
has neither. -/
theorem missingFields_nextOrEnd_iff (d : Definition) (hd : (names d).Nodup) :
    d.states.all (fun s =>
      (missingFields d).contains (.missingField (.state s.name) ["Next", "End"]) =
        (s.type.any continues && s.next.isNone && !s.isEnd)) = true := by
  simp only [List.all_eq_true, decide_eq_true_eq]
  intro s hs
  apply Bool.eq_iff_iff.mpr
  simp only [List.contains_iff_mem]
  rw [missingField_mem_missingFields hd hs (.inl rfl)]
  cases ht : s.type with
  | none => simp [stateMissing, ht, missingField_mem_missingOf, List.mem_flatMap]
  | some t =>
    cases t <;> simp [stateMissing, ht, missingField_mem_missingOf, List.mem_flatMap, requiredOf,
      continues]

private theorem missingField_mem_rules {l : List Rule} {mk : Nat → Place}
    (hmk : ∀ i j, mk i = mk j → i = j) {req : List (List String)} {x : Rule × Nat}
    (hx : x ∈ l.zipIdx) {fs : List String} :
    Issue.missingField (mk x.2) fs ∈ l.zipIdx.flatMap (fun y => missingOf (mk y.2) y.1.keys req) ↔
      fs ∈ req ∧ fs.any x.1.keys.contains = false := by
  simp only [List.mem_flatMap, missingField_mem_missingOf]
  constructor
  · rintro ⟨y, hy, h, h'⟩
    rwa [← eq_of_mem_zipIdx hx hy (hmk _ _ h).symm]
  · exact fun h => ⟨x, hx, rfl, h⟩

/-- The issues of a State at an element of its `Choices`, `Retry` or `Catch` are those of the
element. -/
private theorem rule_mem_stateMissing {t : State} {p : Place} {fs : List String}
    (hp : ∃ k, p = .choice t.name k ∨ p = .retrier t.name k ∨ p = .catcher t.name k) :
    Issue.missingField p fs ∈ stateMissing t ↔
      Issue.missingField p fs ∈
          t.choices.zipIdx.flatMap (fun x => missingOf (.choice t.name x.2) x.1.keys choiceRequired) ∨
        Issue.missingField p fs ∈
          t.retriers.zipIdx.flatMap (fun x => missingOf (.retrier t.name x.2) x.1.keys retrierRequired) ∨
        Issue.missingField p fs ∈
          t.catchers.zipIdx.flatMap (fun x => missingOf (.catcher t.name x.2) x.1.keys catcherRequired) := by
  have hstate : p ≠ .state t.name := by
    rintro rfl
    rcases hp with ⟨k, h | h | h⟩ <;> cases h
  simp only [stateMissing, List.mem_flatten, List.mem_cons, List.not_mem_nil, or_false]
  constructor
  · rintro ⟨l, hl, hm⟩
    rcases hl with rfl | rfl | rfl | rfl | rfl
    · exact absurd (missingField_mem_missingOf.mp hm).1 hstate
    · split at hm
      · rcases List.mem_append.mp hm with hm | hm
        · exact absurd (missingField_mem_missingOf.mp hm).1 hstate
        · split at hm
          · simp only [List.mem_singleton, Issue.missingField.injEq] at hm
            exact absurd hm.1 hstate
          · cases hm
      · cases hm
    · exact .inl hm
    · exact .inr (.inl hm)
    · exact .inr (.inr hm)
  · rintro (h | h | h)
    · exact ⟨_, .inr (.inr (.inl rfl)), h⟩
    · exact ⟨_, .inr (.inr (.inr (.inl rfl))), h⟩
    · exact ⟨_, .inr (.inr (.inr (.inr rfl))), h⟩

/-- `missingFields` reports an element of `Choices` without `Next` exactly when it has no `Next`. -/
theorem missingFields_choice_iff (d : Definition) (hd : (names d).Nodup) :
    d.states.all (fun s => s.choices.zipIdx.all fun x =>
      (missingFields d).contains (.missingField (.choice s.name x.2) ["Next"]) =
        !x.1.keys.contains "Next") = true := by
  simp only [List.all_eq_true, decide_eq_true_eq]
  intro s hs x hx
  apply Bool.eq_iff_iff.mpr
  simp only [List.contains_iff_mem, Bool.not_eq_true']
  rw [missingField_mem_missingFields hd hs (.inr ⟨x.2, .inl rfl⟩),
    rule_mem_stateMissing ⟨x.2, .inl rfl⟩,
    missingField_mem_rules (mk := Place.choice s.name) (fun _ _ h => by cases h; rfl) hx]
  simp [List.mem_flatMap, missingField_mem_missingOf, choiceRequired]

/-- `missingFields` reports a group of keys of an element of `Retry` exactly when the element has
none of them. -/
theorem missingFields_retrier_iff (d : Definition) (hd : (names d).Nodup) :
    d.states.all (fun s => s.retriers.zipIdx.all fun x => retrierRequired.all fun fs =>
      (missingFields d).contains (.missingField (.retrier s.name x.2) fs) =
        !fs.any x.1.keys.contains) = true := by
  simp only [List.all_eq_true, decide_eq_true_eq]
  intro s hs x hx fs hfs
  apply Bool.eq_iff_iff.mpr
  simp only [List.contains_iff_mem, Bool.not_eq_true']
  rw [missingField_mem_missingFields hd hs (.inr ⟨x.2, .inr (.inl rfl)⟩),
    rule_mem_stateMissing ⟨x.2, .inr (.inl rfl)⟩,
    missingField_mem_rules (mk := Place.retrier s.name) (fun _ _ h => by cases h; rfl) hx]
  simp [List.mem_flatMap, missingField_mem_missingOf, hfs]

/-- `missingFields` reports a group of keys of an element of `Catch` exactly when the element has
none of them. -/
theorem missingFields_catcher_iff (d : Definition) (hd : (names d).Nodup) :
    d.states.all (fun s => s.catchers.zipIdx.all fun x => catcherRequired.all fun fs =>
      (missingFields d).contains (.missingField (.catcher s.name x.2) fs) =
        !fs.any x.1.keys.contains) = true := by
  simp only [List.all_eq_true, decide_eq_true_eq]
  intro s hs x hx fs hfs
  apply Bool.eq_iff_iff.mpr
  simp only [List.contains_iff_mem, Bool.not_eq_true']
  rw [missingField_mem_missingFields hd hs (.inr ⟨x.2, .inr (.inr rfl)⟩),
    rule_mem_stateMissing ⟨x.2, .inr (.inr rfl)⟩,
    missingField_mem_rules (mk := Place.catcher s.name) (fun _ _ h => by cases h; rfl) hx]
  simp [List.mem_flatMap, missingField_mem_missingOf, hfs]

private theorem misplaced_mem_misplacedOf {mk : String → Nat → Place} {s : State}
    {rules : List Rule} {q : Place} :
    Issue.misplacedStatesAll q ∈ misplacedOf mk s rules ↔
      ∃ x ∈ rules.zipIdx, q = mk s.name x.2 ∧
        (x.1.errorEquals.contains "States.ALL" &&
          (x.1.errorEquals.length != 1 || x.2 + 1 != rules.length)) = true := by
  simp only [misplacedOf, List.mem_filterMap]
  constructor
  · rintro ⟨x, hx, h⟩
    split at h
    · exact ⟨x, hx, (Issue.misplacedStatesAll.inj (Option.some.inj h)).symm, by assumption⟩
    · cases h
  · rintro ⟨x, hx, rfl, h⟩
    exact ⟨x, hx, by simp only [h, ↓reduceIte]⟩

private theorem statesAll_misplaced (ee : List String) (n m : Nat) :
    (ee.contains "States.ALL" && (ee.length != 1 || n != m)) =
      (ee.contains "States.ALL" && !(ee == ["States.ALL"] && n == m)) := by
  rcases ee with _ | ⟨a, _ | ⟨b, l⟩⟩
  · rfl
  · by_cases ha : a = "States.ALL"
    · subst ha
      simp [bne]
    · simp [Ne.symm ha]
  · simp [bne]

/-- `misplacedStatesAll` reports an element of `Retry` exactly when its `ErrorEquals` has
`States.ALL` and is not `[States.ALL]` in the last element. -/
theorem misplacedStatesAll_retrier_iff (d : Definition) (hd : (names d).Nodup) :
    d.states.all (fun s => s.retriers.zipIdx.all fun x =>
      (misplacedStatesAll d).contains (.misplacedStatesAll (.retrier s.name x.2)) =
        (x.1.errorEquals.contains "States.ALL" &&
          !(x.1.errorEquals == ["States.ALL"] && x.2 + 1 == s.retriers.length))) = true := by
  simp only [List.all_eq_true, decide_eq_true_eq]
  intro s hs x hx
  rw [← statesAll_misplaced]
  apply Bool.eq_iff_iff.mpr
  simp only [List.contains_iff_mem, misplacedStatesAll, List.mem_flatMap, List.mem_append,
    misplaced_mem_misplacedOf]
  constructor
  · rintro ⟨t, ht, ⟨y, hy, h, hc⟩ | ⟨y, hy, h, hc⟩⟩
    · injection h with h1 h2
      obtain rfl := eq_of_name hd hs ht h1.symm
      obtain rfl := eq_of_mem_zipIdx hx hy h2.symm
      exact hc
    · cases h
  · exact fun h => ⟨s, hs, .inl ⟨x, hx, rfl, h⟩⟩

/-- `misplacedStatesAll` reports an element of `Catch` exactly when its `ErrorEquals` has
`States.ALL` and is not `[States.ALL]` in the last element. -/
theorem misplacedStatesAll_catcher_iff (d : Definition) (hd : (names d).Nodup) :
    d.states.all (fun s => s.catchers.zipIdx.all fun x =>
      (misplacedStatesAll d).contains (.misplacedStatesAll (.catcher s.name x.2)) =
        (x.1.errorEquals.contains "States.ALL" &&
          !(x.1.errorEquals == ["States.ALL"] && x.2 + 1 == s.catchers.length))) = true := by
  simp only [List.all_eq_true, decide_eq_true_eq]
  intro s hs x hx
  rw [← statesAll_misplaced]
  apply Bool.eq_iff_iff.mpr
  simp only [List.contains_iff_mem, misplacedStatesAll, List.mem_flatMap, List.mem_append,
    misplaced_mem_misplacedOf]
  constructor
  · rintro ⟨t, ht, ⟨y, hy, h, hc⟩ | ⟨y, hy, h, hc⟩⟩
    · cases h
    · injection h with h1 h2
      obtain rfl := eq_of_name hd hs ht h1.symm
      obtain rfl := eq_of_mem_zipIdx hx hy h2.symm
      exact hc
  · exact fun h => ⟨s, hs, .inr ⟨x, hx, rfl, h⟩⟩

/-- `misplacedStatesAll` reports the elements of the `Retry` of a State before those of its
`Catch`. -/
theorem misplacedStatesAll_retry_before_catch (d : Definition) :
    d.states.all (fun s =>
      (misplacedOf .retrier s s.retriers ++ misplacedOf .catcher s s.catchers).isSublist
        (misplacedStatesAll d)) = true := by
  simp only [List.all_eq_true, List.isSublist_iff_sublist]
  intro s hs
  simp only [misplacedStatesAll, List.flatMap_def]
  exact List.sublist_flatten_of_mem (List.mem_map_of_mem hs)

end Sanchoris.SkillDefinition
