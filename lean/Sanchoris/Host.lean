namespace Sanchoris.Host

def canStartWorkspace (online : Bool) (running maxWorkspaces : Nat) : Bool :=
  online && decide (running < maxWorkspaces)

theorem canStartWorkspace_iff (online : Bool) (running maxWorkspaces : Nat) :
    canStartWorkspace online running maxWorkspaces = true ↔
      online = true ∧ running < maxWorkspaces := by
  simp [canStartWorkspace]

theorem canStartWorkspace_within_limit (online : Bool) (running maxWorkspaces : Nat) :
    canStartWorkspace online running maxWorkspaces = true → running + 1 ≤ maxWorkspaces := by
  simp [canStartWorkspace]
  omega

/-- A Host of a User as the placement sees it. -/
structure HostState where
  name : String
  online : Bool
  running : Nat
  maxWorkspaces : Nat
  deriving DecidableEq, Repr

/-- Where a Workspace goes: the cloud, a Host by its name, nowhere because the Host has no room
or is `offline` (`full`), or nowhere because no Host has the name (`missing`). -/
inductive Placement where
  | cloud
  | host (name : String)
  | full
  | missing
  deriving DecidableEq, Repr

/-- The placement of a Workspace whose Project names `target` (`Workspace.Host`; `none` is
`cloud`), among the Hosts of the User. -/
def place (target : Option String) (hosts : List HostState) : Placement :=
  match target with
  | none => .cloud
  | some name =>
    match hosts.find? (fun h => h.name == name) with
    | none => .missing
    | some h => if canStartWorkspace h.online h.running h.maxWorkspaces then .host name else .full

/-- A Workspace goes to a Host only when that Host is `online` and has room below
`MaxWorkspaces`. -/
theorem place_host (target : Option String) (hosts : List HostState) (name : String) :
    place target hosts = .host name →
      target = some name ∧ ∃ h ∈ hosts, h.name = name ∧ h.online = true ∧
        h.running + 1 ≤ h.maxWorkspaces := by
  unfold place
  cases target with
  | none => simp
  | some n =>
    simp only
    cases hf : hosts.find? (fun h => h.name == n) with
    | none => simp
    | some h =>
      simp only
      split
      · rename_i hc
        intro he
        injection he with he
        subst he
        have hm := List.mem_of_find?_eq_some hf
        have hn := List.find?_some hf
        simp at hn
        exact ⟨rfl, h, hm, hn, (canStartWorkspace_iff _ _ _).mp hc |>.1,
          canStartWorkspace_within_limit _ _ _ hc⟩
      · simp

/-- An `offline` Host gets no Workspace. -/
theorem place_offline (target : Option String) (hosts : List HostState) (name : String)
    (offline : ∀ h ∈ hosts, h.name = name → h.online = false) :
    place target hosts ≠ .host name := by
  intro hp
  obtain ⟨_, h, hm, hn, ho, _⟩ := place_host target hosts name hp
  simp [offline h hm hn] at ho

theorem place_cloud_iff (target : Option String) (hosts : List HostState) :
    place target hosts = .cloud ↔ target = none := by
  unfold place
  cases target with
  | none => simp
  | some n =>
    simp only
    cases hosts.find? (fun h => h.name == n) <;> simp
    split <;> simp

theorem place_missing_iff (target : Option String) (hosts : List HostState) :
    place target hosts = .missing ↔ ∃ n, target = some n ∧ ∀ h ∈ hosts, h.name ≠ n := by
  unfold place
  cases target with
  | none => simp
  | some n =>
    simp only
    cases hf : hosts.find? (fun h => h.name == n) with
    | none =>
      simp at hf
      simpa using hf
    | some h =>
      simp only
      have hm := List.mem_of_find?_eq_some hf
      have hn := List.find?_some hf
      simp at hn
      constructor
      · split <;> simp
      · intro ⟨m, hm', hall⟩
        injection hm' with hm'
        subst hm'
        exact absurd hn (hall h hm)

end Sanchoris.Host
