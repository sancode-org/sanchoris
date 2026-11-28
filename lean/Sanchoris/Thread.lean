import Sanchoris.Host

namespace Sanchoris.Thread

open Sanchoris.Host

/-- Why a thread waits for its Workspace to start (`PendingReason`). -/
inductive PendingReason where
  | blockedBy
  | concurrency
  | planLimit
  | hostFull
  | subscription
  deriving DecidableEq, Repr

/-- What a `WAITING` thread waits for (`WaitReason`). -/
inductive WaitReason where
  | skill
  | children
  | usageLimit
  | host
  deriving DecidableEq, Repr

/-- The `Limit.*` errors: the limits of one thread of the Plan. -/
inductive Limit where
  | turns
  | costUsd
  | timeoutSeconds
  | heartbeatSeconds
  deriving DecidableEq, Repr

/-- Why a thread waits for a person (`Error` of `NEEDS_YOU`). -/
inductive Error where
  | limit (limit : Limit)
  | workerFailed
  | usageLimit
  | workspaceStopped
  | scriptFailed
  | credentialMissing
  | hostMissing
  deriving DecidableEq, Repr

/-- A turn of a `RUNNING` thread: asked for and not started yet (`preparing`), or running
(`active`). -/
inductive Turn where
  | preparing
  | active
  deriving DecidableEq, Repr

/-- The state of a thread (`skill.md`「スレッドの状態」). -/
inductive State where
  | pending (reason : PendingReason)
  | running (turn : Turn)
  | waiting (reason : WaitReason)
  | needsYou (error : Error)
  | succeeded
  | aborted
  deriving DecidableEq, Repr

def isRunning : State → Bool
  | .running _ => true
  | _ => false

def isPending : State → Bool
  | .pending _ => true
  | _ => false

def isWaiting : State → Bool
  | .waiting _ => true
  | _ => false

/-- Where an input comes from: a person (a message of a person or of the parent thread), or
sanchoris (a notification of a Skill or a child thread, the failure of a script, the resumption
after the usage limit). -/
inductive Source where
  | person
  | system
  deriving DecidableEq, Repr

/-- What happens to an input (`skill.md`「スレッドへの入力」): it goes into the running turn
(`steer`), waits to be sent later (`queue`), starts a new turn (`startTurn`), is refused
(`reject`), or is not sent (`drop`). -/
inductive Delivery where
  | steer
  | queue
  | startTurn
  | reject
  | drop
  deriving DecidableEq, Repr

/-- How an input from `source` is delivered to a thread in `state`. -/
def deliver : State → Source → Delivery
  | .running .active, _ => .steer
  | .running .preparing, _ => .queue
  | .pending _, _ => .queue
  | .waiting .skill, _ => .startTurn
  | .waiting .children, _ => .startTurn
  | .waiting .usageLimit, _ => .queue
  | .waiting .host, _ => .queue
  | .needsYou _, .person => .startTurn
  | .needsYou _, .system => .queue
  | .succeeded, _ => .startTurn
  | .aborted, .person => .reject
  | .aborted, .system => .drop

/-- Whether the thread takes the input, now or later. -/
def Delivery.accepted : Delivery → Bool
  | .reject => false
  | .drop => false
  | _ => true

theorem deliver_accepted_iff (state : State) (source : Source) :
    (deliver state source).accepted = true ↔ state ≠ .aborted := by
  cases state <;> cases source <;> (try rename_i x; cases x) <;> simp [deliver, Delivery.accepted]

/-- Whether a thread waits for a person (`NEEDS_YOU`). -/
def needsPerson : State → Bool
  | .needsYou _ => true
  | _ => false

theorem deliver_startTurn_iff (state : State) (source : Source) :
    deliver state source = .startTurn ↔
      state = .waiting .skill ∨ state = .waiting .children ∨ state = .succeeded ∨
        needsPerson state = true ∧ source = .person := by
  cases state <;> cases source <;> (try rename_i x; cases x) <;> simp [deliver, needsPerson]

theorem deliver_queue_iff (state : State) (source : Source) :
    deliver state source = .queue ↔
      state = .running .preparing ∨ state = .waiting .usageLimit ∨ state = .waiting .host ∨
        isPending state = true ∨ needsPerson state = true ∧ source = .system := by
  cases state <;> cases source <;> (try rename_i x; cases x) <;>
    simp [deliver, needsPerson, isPending]

theorem deliver_reject_iff (state : State) (source : Source) :
    deliver state source = .reject ↔ state = .aborted ∧ source = .person := by
  cases state <;> cases source <;> (try rename_i x; cases x) <;> simp [deliver]

theorem deliver_steer_iff (state : State) (source : Source) :
    deliver state source = .steer ↔ state = .running .active := by
  cases state <;> cases source <;> (try rename_i x; cases x) <;> simp [deliver]

/-- What happens to a thread. -/
inductive Event where
  /-- Its Workspace started, or was restored. -/
  | workspaceReady
  /-- The Worker started the turn that was asked for. -/
  | turnStarted
  | turnEnded
  | turnFailed
  | limitExceeded (limit : Limit)
  /-- The Worker reached its usage limit; `auto` is `UsageLimit.Resume: auto`. -/
  | usageLimit (auto : Bool)
  /-- `ResumeAt` passed, or the Host came back online. -/
  | resume
  | hostOffline
  | workspaceStopped
  /-- A `RunOnCreate` script with `Async: false` failed. -/
  | scriptFailed
  | credentialMissing
  | hostMissing
  /-- There is no room to restore the Workspace. -/
  | noRoom (reason : PendingReason)
  | input (source : Source)
  | stop
  deriving DecidableEq, Repr

/-- What a thread has when an event happens: its SkillRuns that run (`RUNNING`, `WAITING`), its
`subagent` children that run (`PENDING`, `RUNNING`, `WAITING`), and the inputs that wait. -/
structure Context where
  runningSkills : Nat
  runningChildren : Nat
  queued : Nat
  deriving DecidableEq, Repr

/-- The state after a turn ends (`skill.md`「スレッドの完了」). Inputs that waited start the next
turn. -/
def afterTurn (c : Context) : State :=
  if 0 < c.queued then .running .preparing
  else if 0 < c.runningSkills then .waiting .skill
  else if 0 < c.runningChildren then .waiting .children
  else .succeeded

theorem afterTurn_next_iff (c : Context) :
    afterTurn c = .running .preparing ↔ 0 < c.queued := by
  unfold afterTurn; repeat' (first | split | omega | simp_all)

theorem afterTurn_skill_iff (c : Context) :
    afterTurn c = .waiting .skill ↔ c.queued = 0 ∧ 0 < c.runningSkills := by
  unfold afterTurn; repeat' (first | split | omega | simp_all)

theorem afterTurn_children_iff (c : Context) :
    afterTurn c = .waiting .children ↔
      c.queued = 0 ∧ c.runningSkills = 0 ∧ 0 < c.runningChildren := by
  unfold afterTurn; repeat' (first | split | omega | simp_all)

theorem afterTurn_succeeded_iff (c : Context) :
    afterTurn c = .succeeded ↔ c.queued = 0 ∧ c.runningSkills = 0 ∧ c.runningChildren = 0 := by
  unfold afterTurn; repeat' (first | split | omega | simp_all)


/-- The state of a thread after an event. -/
def step (s : State) (e : Event) (c : Context) : State :=
  match e with
  | .stop => .aborted
  | .input source => if deliver s source = .startTurn then .running .preparing else s
  | .workspaceReady => if isPending s then .running .preparing else s
  | .turnStarted => if s = .running .preparing then .running .active else s
  | .turnEnded => if isRunning s then afterTurn c else s
  | .turnFailed => if isRunning s then .needsYou .workerFailed else s
  | .limitExceeded l => if isRunning s then .needsYou (.limit l) else s
  | .usageLimit auto =>
    if isRunning s then (if auto then .waiting .usageLimit else .needsYou .usageLimit) else s
  | .scriptFailed => if isRunning s then .needsYou .scriptFailed else s
  | .resume =>
    if s = .waiting .usageLimit ∨ s = .waiting .host then .running .preparing else s
  | .hostOffline => if isRunning s || isWaiting s then .waiting .host else s
  | .workspaceStopped =>
    if isPending s || isRunning s || isWaiting s then .needsYou .workspaceStopped else s
  | .credentialMissing => if s = .aborted then s else .needsYou .credentialMissing
  | .hostMissing => if s = .aborted then s else .needsYou .hostMissing
  | .noRoom r => if isRunning s then .pending r else s

theorem step_stop (s : State) (c : Context) : step s .stop c = .aborted := by
  cases s <;> simp [step]

theorem step_input (s : State) (source : Source) (c : Context) :
    step s (.input source) c =
      if deliver s source = .startTurn then .running .preparing else s := by
  cases s <;> cases source <;> (try rename_i x; cases x) <;> simp [step, deliver]

theorem step_workspaceReady (s : State) (c : Context) :
    step s .workspaceReady c = if isPending s then .running .preparing else s := by
  cases s <;> (try rename_i x; cases x) <;> simp [step, isPending]

theorem step_turnStarted (s : State) (c : Context) :
    step s .turnStarted c = if s = .running .preparing then .running .active else s := by
  cases s <;> (try rename_i x; cases x) <;> simp [step]

theorem step_turnEnded (s : State) (c : Context) :
    step s .turnEnded c = if isRunning s then afterTurn c else s := by
  cases s <;> (try rename_i x; cases x) <;> simp [step, isRunning]

theorem step_turnFailed (s : State) (c : Context) :
    step s .turnFailed c = if isRunning s then .needsYou .workerFailed else s := by
  cases s <;> (try rename_i x; cases x) <;> simp [step, isRunning]

theorem step_limitExceeded (s : State) (l : Limit) (c : Context) :
    step s (.limitExceeded l) c = if isRunning s then .needsYou (.limit l) else s := by
  cases s <;> (try rename_i x; cases x) <;> simp [step, isRunning]

theorem step_usageLimit (s : State) (auto : Bool) (c : Context) :
    step s (.usageLimit auto) c =
      if isRunning s then (if auto then .waiting .usageLimit else .needsYou .usageLimit) else s := by
  cases s <;> cases auto <;> (try rename_i x; cases x) <;> simp [step, isRunning]

theorem step_scriptFailed (s : State) (c : Context) :
    step s .scriptFailed c = if isRunning s then .needsYou .scriptFailed else s := by
  cases s <;> (try rename_i x; cases x) <;> simp [step, isRunning]

theorem step_resume (s : State) (c : Context) :
    step s .resume c =
      if s = .waiting .usageLimit ∨ s = .waiting .host then .running .preparing else s := by
  cases s <;> (try rename_i x; cases x) <;> simp [step]

theorem step_hostOffline (s : State) (c : Context) :
    step s .hostOffline c = if isRunning s || isWaiting s then .waiting .host else s := by
  cases s <;> (try rename_i x; cases x) <;> simp [step, isRunning, isWaiting]

theorem step_workspaceStopped (s : State) (c : Context) :
    step s .workspaceStopped c =
      if isPending s || isRunning s || isWaiting s then .needsYou .workspaceStopped else s := by
  cases s <;> (try rename_i x; cases x) <;> simp [step, isRunning, isWaiting, isPending]

theorem step_credentialMissing (s : State) (c : Context) :
    step s .credentialMissing c = if s = .aborted then s else .needsYou .credentialMissing := by
  cases s <;> (try rename_i x; cases x) <;> simp [step]

theorem step_hostMissing (s : State) (c : Context) :
    step s .hostMissing c = if s = .aborted then s else .needsYou .hostMissing := by
  cases s <;> (try rename_i x; cases x) <;> simp [step]

theorem step_noRoom (s : State) (r : PendingReason) (c : Context) :
    step s (.noRoom r) c = if isRunning s then .pending r else s := by
  cases s <;> (try rename_i x; cases x) <;> simp [step, isRunning]

theorem step_aborted (e : Event) (c : Context) : step .aborted e c = .aborted := by
  cases e <;> (try rename_i x; cases x) <;> simp [step, deliver, isRunning, isPending, isWaiting]

theorem step_aborted_iff (s : State) (e : Event) (c : Context) (h : s ≠ .aborted) :
    step s e c = .aborted ↔ e = .stop := by
  cases s <;> cases e <;> (try rename_i x y; cases x <;> cases y) <;>
    (try rename_i x; cases x) <;> simp_all [step, deliver, afterTurn, isRunning, isPending, isWaiting] <;>
    repeat' (first | split | simp_all)

/-- A thread that is not `SUCCEEDED` becomes `SUCCEEDED` only when a turn ends with no SkillRun
and no `subagent` child that runs, and no input that waits. -/
theorem step_succeeded_iff (s : State) (e : Event) (c : Context) (h : s ≠ .succeeded) :
    step s e c = .succeeded ↔
      isRunning s = true ∧ e = .turnEnded ∧ c.queued = 0 ∧ c.runningSkills = 0 ∧
        c.runningChildren = 0 := by
  cases s <;> cases e <;> (try rename_i x y; cases x <;> cases y) <;>
    (try rename_i x; cases x) <;> simp_all [step, deliver, afterTurn, isRunning, isPending, isWaiting] <;>
    repeat' (first | split | omega | simp_all)

/-- A thread with a SkillRun or a `subagent` child that runs does not become `SUCCEEDED`. -/
theorem step_not_succeeded (s : State) (e : Event) (c : Context) (h : s ≠ .succeeded)
    (running : 0 < c.runningSkills ∨ 0 < c.runningChildren) : step s e c ≠ .succeeded := by
  intro hs
  have := (step_succeeded_iff s e c h).mp hs
  omega

/-- An input to an `ABORTED` thread changes nothing. -/
theorem step_input_aborted (source : Source) (c : Context) :
    step .aborted (.input source) c = .aborted := step_aborted _ _

/-- The subscription of a User. -/
inductive Subscription where
  | none
  | trialing
  | active
  | pastDue
  | canceled
  | unpaid
  deriving DecidableEq, Repr

/-- A User starts threads when its subscription is `trialing` or `active`, or its Plan is Admin. -/
def subscribed (subscription : Subscription) (admin : Bool) : Bool :=
  admin || subscription == .trialing || subscription == .active

theorem subscribed_iff (subscription : Subscription) (admin : Bool) :
    subscribed subscription admin = true ↔
      admin = true ∨ subscription = .trialing ∨ subscription = .active := by
  cases subscription <;> cases admin <;> simp [subscribed] <;> decide

/-- The limits of a Plan for a period; `none` has no limit. -/
structure PlanLimits where
  threads : Option Nat
  workspaceSeconds : Option Nat
  maxConcurrency : Nat
  deriving DecidableEq, Repr

/-- What a User used in the period. -/
structure Used where
  threads : Nat
  workspaceSeconds : Nat
  runningWorkspaces : Nat
  deriving DecidableEq, Repr

def below : Option Nat → Nat → Bool
  | none, _ => true
  | some limit, used => decide (used < limit)

/-- Whether the User can start one more thread in the period: it is below the threads and the
Workspace time of its Plan. -/
def withinPlan (limits : PlanLimits) (used : Used) : Bool :=
  below limits.threads used.threads && below limits.workspaceSeconds used.workspaceSeconds

theorem withinPlan_iff (limits : PlanLimits) (used : Used) :
    withinPlan limits used = true ↔
      (∀ n, limits.threads = some n → used.threads < n) ∧
        (∀ n, limits.workspaceSeconds = some n → used.workspaceSeconds < n) := by
  cases h1 : limits.threads <;> cases h2 : limits.workspaceSeconds <;>
    simp [withinPlan, below, h1, h2]

/-- What a thread needs to get its Workspace. `counted` is a thread counted in the period already
(one that restores its Workspace), which the Plan limit of the period does not stop. -/
structure Demand where
  blocked : Bool
  subscription : Subscription
  admin : Bool
  counted : Bool
  limits : PlanLimits
  used : Used
  deriving DecidableEq, Repr

def isHost : Placement → Bool
  | .host _ => true
  | _ => false

/-- What happens to a thread that asks for its Workspace. -/
inductive Start where
  /-- The Workspace starts where `placement` says; a cloud one is counted in the concurrency. -/
  | start (placement : Placement)
  | pending (reason : PendingReason)
  | needsYou (error : Error)
  deriving DecidableEq, Repr

/-- The start of a thread for its demand and the placement of its Workspace. The reasons are
taken in this order: an open blocked-by issue, the subscription, the Plan limit of the period,
then the place of the Workspace (the concurrency for the cloud, the room of the Host). -/
def decideStart (d : Demand) (p : Placement) : Start :=
  if d.blocked then .pending .blockedBy
  else if !subscribed d.subscription d.admin then .pending .subscription
  else if !d.counted && !withinPlan d.limits d.used then .pending .planLimit
  else
    match p with
    | .cloud =>
      if d.used.runningWorkspaces < d.limits.maxConcurrency then .start .cloud
      else .pending .concurrency
    | .host name => .start (.host name)
    | .full => .pending .hostFull
    | .missing => .needsYou .hostMissing

/-- A new thread of a User at the Plan limit of the period does not start. -/
theorem decideStart_planLimit (d : Demand) (p : Placement) (h : d.counted = false)
    (full : withinPlan d.limits d.used = false) : ∀ q, decideStart d p ≠ .start q := by
  intro q
  cases hb : d.blocked <;> cases hs : subscribed d.subscription d.admin <;>
    simp [decideStart, hb, hs, h, full]

theorem decideStart_start_iff (d : Demand) (p : Placement) (q : Placement) :
    decideStart d p = .start q ↔
      d.blocked = false ∧ subscribed d.subscription d.admin = true ∧
        (d.counted = true ∨ withinPlan d.limits d.used = true) ∧ q = p ∧
        (p = .cloud ∧ d.used.runningWorkspaces < d.limits.maxConcurrency ∨ isHost p = true) := by
  cases hb : d.blocked <;> cases hs : subscribed d.subscription d.admin <;>
    cases hc : d.counted <;> cases hw : withinPlan d.limits d.used <;>
    cases p <;> simp [decideStart, hb, hs, hc, hw, isHost] <;>
    repeat' (first | split | omega | simp_all | (constructor <;> intro <;> simp_all))

/-- A cloud Workspace starts only below the concurrency of the Plan. -/
theorem decideStart_cloud (d : Demand) (p : Placement) :
    decideStart d p = .start .cloud → d.used.runningWorkspaces + 1 ≤ d.limits.maxConcurrency := by
  intro h
  have := (decideStart_start_iff d p .cloud).mp h
  obtain ⟨_, _, _, hq, hp⟩ := this
  subst hq
  rcases hp with ⟨_, hlt⟩ | hn
  · omega
  · simp [isHost] at hn

/-- A Workspace starts on a Host only where the placement put it. -/
theorem decideStart_host (d : Demand) (p : Placement) (name : String) :
    decideStart d p = .start (.host name) → p = .host name := by
  intro h
  exact ((decideStart_start_iff d p _).mp h).2.2.2.1.symm

theorem decideStart_pending_iff (d : Demand) (p : Placement) (r : PendingReason) :
    decideStart d p = .pending r ↔
      (d.blocked = true ∧ r = .blockedBy) ∨
      (d.blocked = false ∧ subscribed d.subscription d.admin = false ∧ r = .subscription) ∨
      (d.blocked = false ∧ subscribed d.subscription d.admin = true ∧ d.counted = false ∧
        withinPlan d.limits d.used = false ∧ r = .planLimit) ∨
      (d.blocked = false ∧ subscribed d.subscription d.admin = true ∧
        (d.counted = true ∨ withinPlan d.limits d.used = true) ∧
        (p = .cloud ∧ d.limits.maxConcurrency ≤ d.used.runningWorkspaces ∧ r = .concurrency ∨
          p = .full ∧ r = .hostFull)) := by
  cases hb : d.blocked <;> cases hs : subscribed d.subscription d.admin <;>
    cases hc : d.counted <;> cases hw : withinPlan d.limits d.used <;>
    cases p <;> cases r <;> simp [decideStart, hb, hs, hc, hw] <;>
    repeat' (first | split | omega | simp_all)

/-- What a message of a User is (`skill.md`「User の起動」): the compact of the Worker, the
launch of a predictable Skill of the thread, or a message to the thread. -/
inductive Command where
  | compact
  | launch (name : String)
  | message
  deriving DecidableEq, Repr

/-- Reads a message by the name after its leading `/` (`none` without one), whether text
follows the name, and the predictable Skills of the thread. -/
def command (name : Option String) (rest : Bool) (skills : List String) : Command :=
  match name with
  | none => .message
  | some n =>
    if n = "compact" ∧ rest = false then .compact
    else if skills.contains n then .launch n
    else .message

/-- `/compact` alone is the compact of the Worker. -/
theorem command_compact_iff (name : Option String) (rest : Bool) (skills : List String) :
    command name rest skills = .compact ↔ name = some "compact" ∧ rest = false := by
  cases name with
  | none => simp [command]
  | some m =>
    by_cases hc : m = "compact" ∧ rest = false
    · obtain ⟨rfl, rfl⟩ := hc
      simp [command]
    · have hn : ¬(some m = some "compact" ∧ rest = false) := fun ⟨h1, h2⟩ =>
        hc ⟨Option.some.inj h1, h2⟩
      by_cases hm : skills.contains m = true <;>
        simp only [command, hc, hm, hn, ↓reduceIte, reduceCtorEq] <;> simp

/-- A User launches only a predictable Skill of the thread, named after the `/`. -/
theorem command_launch_iff (name : Option String) (rest : Bool) (skills : List String)
    (n : String) :
    command name rest skills = .launch n ↔
      name = some n ∧ n ∈ skills ∧ ¬(n = "compact" ∧ rest = false) := by
  cases name with
  | none => simp [command]
  | some m =>
    by_cases hc : m = "compact" ∧ rest = false
    · obtain ⟨rfl, rfl⟩ := hc
      simp only [command, and_self, ↓reduceIte, reduceCtorEq, false_iff]
      intro ⟨h1, _, h3⟩
      simp_all
    · by_cases hm : skills.contains m = true
      · simp only [command, hc, hm, ↓reduceIte, Command.launch.injEq, Option.some.injEq]
        constructor
        · intro h
          subst h
          exact ⟨rfl, by simpa using hm, hc⟩
        · intro ⟨h, _, _⟩
          exact h
      · simp only [command, hc, hm, ↓reduceIte, reduceCtorEq, false_iff, Option.some.injEq]
        intro ⟨h, h2, _⟩
        subst h
        exact hm (by simpa using h2)

/-- A message without a leading `/` goes to the thread. -/
theorem command_none (rest : Bool) (skills : List String) :
    command none rest skills = .message := rfl

end Sanchoris.Thread
