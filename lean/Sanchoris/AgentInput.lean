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

/-- What follows the end of a turn: a compaction asked for during the turn (`compact`), a turn with
the inputs that waited (`turn`), or nothing (`idle`). -/
inductive Next where
  | compact
  | turn
  | idle
  deriving DecidableEq, Repr

/-- The step after a turn ends, for an agent that is `stopping`, has a `compact` waiting, and
holds `queued` inputs. -/
def afterTurn (stopping compactQueued : Bool) (queued : Nat) : Next :=
  if stopping then .idle else if compactQueued then .compact else if 0 < queued then .turn else .idle

theorem afterTurn_turn_iff (stopping compactQueued : Bool) (queued : Nat) :
    afterTurn stopping compactQueued queued = .turn ↔
      stopping = false ∧ compactQueued = false ∧ 0 < queued := by
  by_cases h : 0 < queued <;> cases stopping <;> cases compactQueued <;> simp [afterTurn, h]

theorem afterTurn_compact_iff (stopping compactQueued : Bool) (queued : Nat) :
    afterTurn stopping compactQueued queued = .compact ↔
      stopping = false ∧ compactQueued = true := by
  by_cases h : 0 < queued <;> cases stopping <;> cases compactQueued <;> simp [afterTurn, h]

theorem afterTurn_idle_iff (stopping compactQueued : Bool) (queued : Nat) :
    afterTurn stopping compactQueued queued = .idle ↔
      stopping = true ∨ (compactQueued = false ∧ queued = 0) := by
  by_cases h : 0 < queued <;> cases stopping <;> cases compactQueued <;> simp [afterTurn, h] <;> omega

end Sanchoris.AgentInput
