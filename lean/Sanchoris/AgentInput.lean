namespace Sanchoris.AgentInput

/-- Where the Worker of a Workspace is: no turn (`idle`), a turn asked for that the Worker has
not started yet (`preparing`), or a turn the Worker runs (`running`). -/
inductive Turn where
  | idle
  | preparing
  | running
  deriving DecidableEq, Repr

/-- How the control plane asks an `input` to be delivered (`skill.md`「スレッドへの入力」). -/
inductive Delivery where
  | steer
  | turn
  deriving DecidableEq, Repr

/-- What `sanchoris-agent` does with an `input`: put it into the running turn (`steer`), start a
turn with it (`startTurn`), or keep it for the input of the next turn (`queue`). -/
inductive Action where
  | steer
  | startTurn
  | queue
  deriving DecidableEq, Repr

/-- The action for an `input` that arrives while the Worker is at `turn`, asked to be delivered
by `delivery`, to a Worker that takes steers when `steerable`. -/
def deliver (turn : Turn) (delivery : Delivery) (steerable : Bool) : Action :=
  match turn, delivery with
  | .idle, _ => .startTurn
  | .running, .steer => if steerable then .steer else .queue
  | _, _ => .queue

theorem deliver_steer_iff (turn : Turn) (delivery : Delivery) (steerable : Bool) :
    deliver turn delivery steerable = .steer ↔
      turn = .running ∧ delivery = .steer ∧ steerable = true := by
  cases turn <;> cases delivery <;> cases steerable <;> simp [deliver]

theorem deliver_startTurn_iff (turn : Turn) (delivery : Delivery) (steerable : Bool) :
    deliver turn delivery steerable = .startTurn ↔ turn = .idle := by
  cases turn <;> cases delivery <;> cases steerable <;> simp [deliver]

theorem deliver_queue_iff (turn : Turn) (delivery : Delivery) (steerable : Bool) :
    deliver turn delivery steerable = .queue ↔
      turn ≠ .idle ∧ ¬(turn = .running ∧ delivery = .steer ∧ steerable = true) := by
  cases turn <;> cases delivery <;> cases steerable <;> simp [deliver]

end Sanchoris.AgentInput
