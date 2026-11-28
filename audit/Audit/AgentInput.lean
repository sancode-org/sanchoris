import Assay
import Sanchoris.AgentInput

open Plausible Sanchoris.AgentInput

instance : Arbitrary Turn := ⟨Gen.elements [.idle, .preparing, .running] (by decide)⟩
instance : Shrinkable Turn := ⟨fun _ => []⟩
instance : Arbitrary Delivery := ⟨Gen.elements [.steer, .turn] (by decide)⟩
instance : Shrinkable Delivery := ⟨fun _ => []⟩

#assay Sanchoris.AgentInput.deliver Sanchoris.AgentInput.afterTurn
