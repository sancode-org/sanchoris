import Assay
import Sanchoris.Host

open Plausible Sanchoris.Host

instance (priority := high) : Arbitrary String := ⟨Gen.elements ["a", "b"] (by decide)⟩

instance : Arbitrary HostState := ⟨do
  pure ⟨← Arbitrary.arbitrary, ← Arbitrary.arbitrary, (← Gen.choose Nat 0 3 (by omega)).val,
    (← Gen.choose Nat 0 3 (by omega)).val⟩⟩
instance : Shrinkable HostState := ⟨fun _ => []⟩

#assay Sanchoris.Host.canStartWorkspace Sanchoris.Host.place
