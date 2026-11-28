import Assay
import Sanchoris.Thread

/-! The generators of the types of `Sanchoris.Thread`. -/

open Plausible Sanchoris.Host Sanchoris.Thread

def pick {α : Type} (xs : List α) (h : 0 < xs.length) : Gen α := Gen.elements xs h

instance : Arbitrary PendingReason :=
  ⟨pick [.blockedBy, .concurrency, .planLimit, .hostFull, .subscription] (by decide)⟩
instance : Shrinkable PendingReason := ⟨fun _ => []⟩
instance : Arbitrary WaitReason := ⟨pick [.skill, .children, .usageLimit, .host] (by decide)⟩
instance : Shrinkable WaitReason := ⟨fun _ => []⟩
instance : Arbitrary Limit := ⟨pick [.turns, .costUsd, .timeoutSeconds, .heartbeatSeconds] (by decide)⟩
instance : Shrinkable Limit := ⟨fun _ => []⟩
instance : Arbitrary Error :=
  ⟨pick [.limit .turns, .limit .heartbeatSeconds, .workerFailed, .usageLimit, .workspaceStopped,
    .scriptFailed, .credentialMissing, .hostMissing] (by decide)⟩
instance : Shrinkable Error := ⟨fun _ => []⟩
instance : Arbitrary Source := ⟨pick [.person, .system] (by decide)⟩
instance : Shrinkable Source := ⟨fun _ => []⟩

instance : Arbitrary State :=
  ⟨pick [.pending .blockedBy, .pending .concurrency, .running .preparing, .running .active,
    .waiting .skill, .waiting .children, .waiting .usageLimit, .waiting .host,
    .needsYou .workerFailed, .needsYou (.limit .turns), .succeeded, .aborted] (by decide)⟩
instance : Shrinkable State := ⟨fun _ => []⟩

instance : Arbitrary Event :=
  ⟨pick [.workspaceReady, .turnStarted, .turnEnded, .turnFailed, .limitExceeded .costUsd,
    .usageLimit true, .usageLimit false, .resume, .hostOffline, .workspaceStopped, .scriptFailed,
    .credentialMissing, .hostMissing, .noRoom .concurrency, .input .person, .input .system,
    .stop] (by decide)⟩
instance : Shrinkable Event := ⟨fun _ => []⟩

def small : Gen Nat := do pure (← Gen.choose Nat 0 2 (by omega)).val

instance : Arbitrary Context := ⟨do pure ⟨← small, ← small, ← small⟩⟩
instance : Shrinkable Context := ⟨fun _ => []⟩

instance : Arbitrary Subscription :=
  ⟨pick [.none, .trialing, .active, .pastDue, .canceled, .unpaid] (by decide)⟩
instance : Shrinkable Subscription := ⟨fun _ => []⟩

def limit : Gen (Option Nat) := do
  if (← Gen.choose Nat 0 2 (by omega)).val = 0 then pure none else some <$> small

instance : Arbitrary PlanLimits := ⟨do pure ⟨← limit, ← limit, ← small⟩⟩
instance : Shrinkable PlanLimits := ⟨fun _ => []⟩
instance : Arbitrary Used := ⟨do pure ⟨← small, ← small, ← small⟩⟩
instance : Shrinkable Used := ⟨fun _ => []⟩

instance : Arbitrary Demand :=
  ⟨do pure ⟨← Arbitrary.arbitrary, ← Arbitrary.arbitrary, ← Arbitrary.arbitrary,
    ← Arbitrary.arbitrary, ← Arbitrary.arbitrary, ← Arbitrary.arbitrary⟩⟩
instance : Shrinkable Demand := ⟨fun _ => []⟩

instance (priority := high) : Arbitrary String := ⟨pick ["a", "b"] (by decide)⟩

instance : Arbitrary Placement := ⟨pick [.cloud, .host "a", .host "b", .full, .missing] (by decide)⟩
instance : Shrinkable Placement := ⟨fun _ => []⟩

