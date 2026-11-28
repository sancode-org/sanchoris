import Assay
import Sanchoris.Engine.Theorems

open Plausible Sanchoris.Engine

/-- Names of errors and States, so that random rules, errors and definitions refer to each
other. -/
instance (priority := high) : Arbitrary String :=
  ⟨Gen.elements ["States.ALL", "States.Runtime", "States.Timeout", "Command.Failed", "A", "B"]
    (by decide)⟩

set_option assay.maxSize 6 in
set_option assay.samples 300 in
#assay Sanchoris.Engine.step using Sanchoris.Engine.step_exec Sanchoris.Engine.step_timer
  Sanchoris.Engine.step_await Sanchoris.Engine.step_continue Sanchoris.Engine.step_abort
  Sanchoris.Engine.step_defined

set_option assay.maxSize 6 in
set_option assay.samples 300 in
#assay Sanchoris.Engine.start using Sanchoris.Engine.start_run Sanchoris.Engine.start_timer
  Sanchoris.Engine.start_first_entry

set_option assay.timeout 120000 in
#assay Sanchoris.Engine.parentOf Sanchoris.Engine.latest using
  Sanchoris.Engine.parentOf_eq_none_iff Sanchoris.Engine.parentOf_last
  Sanchoris.Engine.latest_eq_none_iff Sanchoris.Engine.latest_mem Sanchoris.Engine.latest_max
  Sanchoris.Engine.latest_first

set_option assay.maxSize 6 in
#assay Sanchoris.Engine.waiterOf using Sanchoris.Engine.waiterOf_isSome
  Sanchoris.Engine.waiterOf_fields

#assay Sanchoris.Engine.handles Sanchoris.Engine.firstHandler using Sanchoris.Engine.handles_iff
  Sanchoris.Engine.firstHandler_nil Sanchoris.Engine.firstHandler_cons
  Sanchoris.Engine.firstHandler_runtime Sanchoris.Engine.firstHandler_statesAll

#assay Sanchoris.Engine.retryOf using Sanchoris.Engine.retryOf_index Sanchoris.Engine.retryOf_counts
  Sanchoris.Engine.retryOf_isSome Sanchoris.Engine.retryOf_withinMax

#assay Sanchoris.Engine.continueStep using Sanchoris.Engine.continueStep_deliver_iff
  Sanchoris.Engine.continueStep_reject_iff

#assay Sanchoris.Engine.awaitStep using Sanchoris.Engine.awaitStep_delivered
  Sanchoris.Engine.awaitStep_take Sanchoris.Engine.awaitStep_wait
