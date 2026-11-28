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

end Sanchoris.Host
