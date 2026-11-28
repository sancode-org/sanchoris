import Assay
import Sanchoris.Thread

/-! The audit of `Sanchoris.Thread.command`, over names that include `compact`. -/

open Plausible Sanchoris.Thread

instance (priority := high) : Arbitrary String :=
  ⟨Gen.elements ["compact", "where", "review"] (by decide)⟩

instance : Arbitrary (List String) :=
  ⟨Gen.elements [[], ["where"], ["where", "review"], ["compact"]] (by decide)⟩

#assay Sanchoris.Thread.command using Sanchoris.Thread.command_compact_iff
  Sanchoris.Thread.command_launch_iff Sanchoris.Thread.command_none
