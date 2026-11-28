import Sanchoris.Engine.Expr

/-!
# The engine of predictable skills

The interpreter of `skill.yaml` (`skill.md`「実行エンジン」). `start` and `step` take a definition,
a SkillRun and an event, and return the new SkillRun with everything the transition writes: the
StateEntries, the commands to run in the Workspace, the timers, the notifications of the thread
and the outputs of `continue` for `sanchoris-agent skill await`. Times are milliseconds since
the epoch.
-/

namespace Sanchoris.Engine

/-! ## The definition -/

/-- What a transition out of a State sends to the thread. -/
structure Prompt where
  transition : Bool
  stdout : Bool
  stderr : Bool
  deriving DecidableEq, Repr, Inhabited

structure Retrier where
  errorEquals : List String
  intervalSeconds : Nat
  maxAttempts : Nat
  backoffRate : Num
  maxDelaySeconds : Option Nat
  deriving DecidableEq, Repr, Inhabited

/-- A Catcher; `resultPath` is `none` for `ResultPath: null`. -/
structure Catcher where
  errorEquals : List String
  resultPath : Option String
  next : String
  deriving DecidableEq, Repr, Inhabited

/-- A `Command` State: `run` is the command, or the path or intrinsic function of `Run.$` when
`runIsPath`. -/
structure Command where
  run : String
  runIsPath : Bool
  timeoutSeconds : Nat
  resultPath : Option String
  retry : List Retrier
  catchers : List Catcher
  deriving DecidableEq, Repr, Inhabited

inductive Body where
  | command (c : Command)
  | pass (result : Option Json) (parameters : Option Json) (resultPath : Option String)
  | choice (choices : List (Cond × String)) (default : Option String)
  | waitSeconds (seconds : Nat)
  | waitUntil (timestamp : String)
  | succeed
  | fail (error cause : String)
  deriving Repr, Inhabited

/-- A State; `next` is `none` for `End: true` and for the types that have no `Next`. -/
structure State where
  name : String
  body : Body
  next : Option String
  prompt : Prompt
  deriving Repr, Inhabited

structure Definition where
  skill : String
  startAt : String
  timeoutSeconds : Nat
  states : List State
  deriving Repr, Inhabited

def Definition.lookup (d : Definition) (name : String) : Option State :=
  d.states.find? (·.name == name)

/-! ## SkillRuns and StateEntries -/

inductive RunStatus where
  | running
  | waiting
  | succeeded
  | failed
  | aborted
  deriving DecidableEq, Repr, Inhabited

def RunStatus.finished : RunStatus → Bool
  | .succeeded | .failed | .aborted => true
  | .running | .waiting => false

inductive EntryStatus where
  | running
  | waiting
  | succeeded
  | failed
  | caught
  | timedOut
  | aborted
  deriving DecidableEq, Repr, Inhabited

/-- Where `sanchoris-agent skill await` of a StateEntry is: not started, waiting for a
`continue` (with the order of the `await` in the thread), or given an Output. -/
inductive Await where
  | idle
  | waiting (order : Nat)
  | delivered (output : Json)
  deriving DecidableEq, Repr, Inhabited

/-- What an open StateEntry waits for. -/
inductive Phase where
  | executing (deadline : Nat)
  | backoff (due : Nat)
  | sleeping (due : Nat)
  | closed
  deriving DecidableEq, Repr, Inhabited

/-- The result of a command in the Workspace (`exec_result`). -/
structure ExecOutput where
  exitCode : Int
  stdout : String
  stderr : String
  timedOut : Bool
  deriving DecidableEq, Repr, Inhabited

/-- A StateEntry. `seq` is its place in the SkillRun from 1, `visit` how many times the SkillRun
has entered the State with this one, and `attempts` the attempts of a `Command` so far. -/
structure Entry where
  seq : Nat
  state : String
  visit : Nat
  status : EntryStatus
  enteredAt : Nat
  exitedAt : Option Nat
  input : Json
  output : Option Json
  error : Option String
  cause : Option String
  attempts : Nat
  retryCounts : List Nat
  phase : Phase
  await : Await
  result : Option ExecOutput
  next : Option String
  deriving DecidableEq, Repr, Inhabited

/-- A SkillRun. `hidden` holds the States left by transitions that sent no notification, for the
next notification. -/
structure Run where
  id : Nat
  skill : String
  status : RunStatus
  input : Json
  output : Option Json
  error : Option String
  cause : Option String
  startedAt : Nat
  finishedAt : Option Nat
  seq : Nat
  visits : List (String × Nat)
  current : Option Entry
  pendingContinue : Option Json
  hidden : List String
  deriving DecidableEq, Repr, Inhabited

/-! ## Events and writes -/

inductive Timer where
  | run
  | state (seq attempt : Nat)
  | retry (seq attempt : Nat)
  | wait (seq : Nat)
  deriving DecidableEq, Repr, Inhabited

inductive Event where
  | exec (seq attempt : Nat) (output : ExecOutput)
  | timer (timer : Timer)
  | await (order : Nat)
  | continue (output : Json)
  | abort
  deriving Repr, Inhabited

/-- A command to run in the Workspace; its idempotency key is the SkillRun, `seq` and
`attempt`. -/
structure Exec where
  seq : Nat
  attempt : Nat
  command : String
  stdin : String
  timeoutSeconds : Nat
  deriving DecidableEq, Repr, Inhabited

/-- A notification of the thread. `path` is the States the SkillRun went through since the last
notification. -/
inductive Notice where
  | transition (path : List String) (exitCode : Option Int) (stdout stderr : Option String)
  | succeeded (path : List String) (output : Json)
  | failed (path : List String) (error cause : String)
  deriving DecidableEq, Repr, Inhabited

/-- What one transition writes. A StateEntry that changes twice is in `entries` twice; the last
one is its value. -/
structure Write where
  entries : List Entry
  execs : List Exec
  timers : List (Timer × Nat)
  notices : List Notice
  outputs : List Json
  deriving DecidableEq, Repr, Inhabited

inductive Rejection where
  | finished
  | continuePending
  | notAwaiting
  deriving DecidableEq, Repr, Inhabited

inductive Outcome where
  | applied (run : Run) (write : Write)
  | ignored
  | rejected (reason : Rejection)
  deriving DecidableEq, Repr, Inhabited

/-! ## Errors -/

def runtimeError : String := "States.Runtime"
def timeoutError : String := "States.Timeout"

/-- Whether a Retrier or Catcher with `errorEquals` handles an error: it names the error, or it
has `States.ALL` and the error is not `States.Runtime`. -/
def handles (errorEquals : List String) (error : String) : Bool :=
  error != runtimeError && (errorEquals.contains error || errorEquals.contains "States.ALL")

/-- The first rule that handles an error. -/
def firstHandler (rules : List (List String)) (error : String) : Option Nat :=
  rules.findIdx? (handles · error)

/-- The wait before the attempt after `n` retries: `IntervalSeconds * BackoffRate ^ n` seconds,
at most `MaxDelaySeconds`, in milliseconds. -/
def retryDelay (r : Retrier) (n : Nat) : Nat :=
  let base : Int := r.intervalSeconds * 1000 * r.backoffRate.mantissa ^ n
  let e : Int := r.backoffRate.exponent * n
  let ms := (if 0 ≤ e then base * 10 ^ e.toNat else base / 10 ^ (-e).toNat).toNat
  match r.maxDelaySeconds with
  | some most => min ms (most * 1000)
  | none => ms

/-- What the Retriers do with an error: wait and retry with the Retrier `index`, whose count goes
up by one, or nothing (no Retrier handles the error, or the first one that does has used its
`MaxAttempts`). -/
def retryOf (retriers : List Retrier) (counts : List Nat) (error : String) : Option (Nat × List Nat) :=
  match firstHandler (retriers.map (·.errorEquals)) error with
  | none => none
  | some i =>
    let n := counts.getD i 0
    if n < (retriers.getD i default).maxAttempts then some (i, counts.set i (n + 1)) else none

/-! ## Nesting -/

/-- A SkillRun of the thread that waits in `sanchoris-agent skill await`. -/
structure Waiter where
  run : Nat
  state : String
  visit : Nat
  order : Nat
  deriving DecidableEq, Repr, Inhabited

/-- The parent of a SkillRun: the SkillRun, its State and the entry into that State. -/
structure Parent where
  run : Nat
  state : String
  visit : Nat
  deriving DecidableEq, Repr, Inhabited

def Waiter.parent (w : Waiter) : Parent := ⟨w.run, w.state, w.visit⟩

/-- The waiter that entered `await` last; the first of them when two have the same order. -/
def latest : List Waiter → Option Waiter
  | [] => none
  | w :: ws =>
    match latest ws with
    | none => some w
    | some v => if w.order < v.order then some v else some w

/-- The parent of a SkillRun that the thread starts while `waiters` wait in `await`
(`skill.md`「Skill の入れ子」); `none` puts it right under the thread. -/
def parentOf (waiters : List Waiter) : Option Parent :=
  (latest waiters).map Waiter.parent

/-- The SkillRun as a waiter, when its StateEntry waits in `await`. -/
def waiterOf (r : Run) : Option Waiter :=
  match r.status.finished, r.current with
  | false, some e =>
    match e.await with
    | .waiting order => some ⟨r.id, e.state, e.visit, order⟩
    | _ => none
  | _, _ => none

/-! ## `continue` and `await` -/

/-- What a `continue` with an Output does, given the `await` of the StateEntry and the waiting
`continue` of the SkillRun: give it to the `await`, keep it, or fail because one waits. -/
inductive ContinueStep where
  | deliver
  | keep
  | reject
  deriving DecidableEq, Repr, Inhabited

def continueStep (a : Await) (pending : Bool) : ContinueStep :=
  match a with
  | .waiting _ => .deliver
  | _ => if pending then .reject else .keep

/-- What an `await` does: return the Output given to the StateEntry before, return the waiting
`continue`, or wait. -/
inductive AwaitStep where
  | replay (output : Json)
  | take (output : Json)
  | wait
  deriving DecidableEq, Repr, Inhabited

def awaitStep (a : Await) (pending : Option Json) : AwaitStep :=
  match a, pending with
  | .delivered v, _ => .replay v
  | _, some v => .take v
  | _, none => .wait

/-! ## The machine -/

/-- A SkillRun and what the transition has written so far. -/
structure Acc where
  run : Run
  write : Write
  deriving Inhabited

def Acc.entry (acc : Acc) (e : Entry) : Acc :=
  { acc with write := { acc.write with entries := acc.write.entries ++ [e] } }

def Acc.notice (acc : Acc) (n : Notice) : Acc :=
  { acc with write := { acc.write with notices := acc.write.notices ++ [n] } }

def Acc.timer (acc : Acc) (t : Timer) (at_ : Nat) : Acc :=
  { acc with write := { acc.write with timers := acc.write.timers ++ [(t, at_)] } }

def Acc.exec (acc : Acc) (x : Exec) : Acc :=
  { acc with write := { acc.write with execs := acc.write.execs ++ [x] } }

def Acc.output (acc : Acc) (v : Json) : Acc :=
  { acc with write := { acc.write with outputs := acc.write.outputs ++ [v] } }

/-- Makes `e` the open StateEntry. -/
def Acc.open_ (acc : Acc) (e : Entry) (status : RunStatus) : Acc :=
  { acc.entry e with run := { acc.run with current := some e, status } }

/-- Closes the StateEntry `e`. -/
def Acc.close (acc : Acc) (e : Entry) (now : Nat) (status : EntryStatus) (output : Option Json)
    (error cause : Option String) (next : Option String) : Acc :=
  let e := { e with status, exitedAt := some now, phase := .closed, output, error, cause, next }
  { acc.entry e with run := { acc.run with current := none } }

def Acc.succeed (acc : Acc) (now : Nat) (path : List String) (output : Json) : Acc :=
  let acc := acc.notice (.succeeded path output)
  { acc with run := { acc.run with status := .succeeded, output := some output, finishedAt := some now,
                                   current := none, hidden := [] } }

def Acc.failRun (acc : Acc) (now : Nat) (path : List String) (error cause : String) : Acc :=
  let acc := acc.notice (.failed path error cause)
  { acc with run := { acc.run with status := .failed, error := some error, cause := some cause,
                                   finishedAt := some now, current := none, hidden := [] } }

/-- The context object `$$`. -/
def context (r : Run) (e : Entry) : Json :=
  .obj [("Execution", .obj [("Id", .num (.ofInt r.id)), ("Name", .str (toString r.id)),
                            ("Input", r.input), ("StartTime", .str (formatTime r.startedAt))]),
        ("State", .obj [("Name", .str e.state), ("EnteredTime", .str (formatTime e.enteredAt)),
                        ("RetryCount", .num (.ofInt (e.attempts - 1 : Nat)))])]

/-- The result of a command: `ExitCode`, `Stdout`, `Stderr`, and `Json` when the stdout is JSON. -/
def execJson (o : ExecOutput) : List (String × Json) :=
  [("ExitCode", .num (.ofInt o.exitCode)), ("Stdout", .str o.stdout), ("Stderr", .str o.stderr)] ++
    match Json.parse o.stdout with
    | some v => [("Json", v)]
    | none => []

/-- The command of an attempt of a `Command` State. -/
def commandText (c : Command) (input ctx : Json) : Except String String :=
  if c.runIsPath then
    match evalValue c.run input ctx with
    | .ok (.str s) => .ok s
    | .ok _ => .error "Run.$ does not give a string"
    | .error m => .error m
  else .ok c.run

/-- Sends an attempt of a `Command` State to the Workspace, or fails the SkillRun when its
command cannot be made. -/
def Acc.attempt (acc : Acc) (c : Command) (e : Entry) (attempt now : Nat) : Acc :=
  let e := { e with attempts := attempt, phase := .executing (now + c.timeoutSeconds * 1000),
                    status := .running, await := .idle }
  match commandText c e.input (context acc.run e) with
  | .error m =>
    let acc := acc.close e now .failed none (some runtimeError) (some m) none
    acc.failRun now (acc.run.hidden ++ [e.state]) runtimeError m
  | .ok command =>
    let acc := acc.open_ e .running
    let acc := acc.exec ⟨e.seq, attempt, command, e.input.render, c.timeoutSeconds⟩
    acc.timer (.state e.seq attempt) (now + c.timeoutSeconds * 1000)

/-- The notification of a transition out of `s` to `next`, or the States to add to the next one. -/
def Acc.transitionNotice (acc : Acc) (s : State) (next : String) (result : Option ExecOutput) : Acc :=
  if s.prompt.transition then
    let n := Notice.transition (acc.run.hidden ++ [s.name, next]) (result.map (·.exitCode))
      (if s.prompt.stdout then result.map (·.stdout) else none)
      (if s.prompt.stderr then result.map (·.stderr) else none)
    let acc := acc.notice n
    { acc with run := { acc.run with hidden := [] } }
  else { acc with run := { acc.run with hidden := acc.run.hidden ++ [s.name] } }

def visitOf (visits : List (String × Nat)) (name : String) : Nat :=
  ((visits.find? (·.1 == name)).map (·.2)).getD 0

def bumpVisit (visits : List (String × Nat)) (name : String) : List (String × Nat) :=
  if visits.any (·.1 == name) then visits.map fun (k, n) => if k == name then (k, n + 1) else (k, n)
  else visits ++ [(name, 1)]

/-- The first Choice rule that holds, or the error of the rules. -/
def chooseNext (choices : List (Cond × String)) (input ctx : Json) : Except String (Option String) :=
  match choices with
  | [] => .ok none
  | (c, next) :: rest => do
    if ← c.eval input ctx then pure (some next) else chooseNext rest input ctx

/-- How many States a transition goes through at most. -/
def maxStates : Nat := 256

mutual
/-- Enters the State `name` with `input`, and goes on through the States that finish at once. -/
def enter : Nat → Definition → Nat → Acc → String → Json → Acc
  | 0, _, now, acc, name, _ =>
    acc.failRun now (acc.run.hidden ++ [name]) runtimeError
      s!"the SkillRun went through {maxStates} States without a Command or a Wait"
  | fuel + 1, d, now, acc, name, input =>
    match d.lookup name with
    | none => acc.failRun now acc.run.hidden runtimeError s!"{name} names no State"
    | some s =>
      let visits := bumpVisit acc.run.visits name
      let e : Entry := {
        seq := acc.run.seq + 1, state := name, visit := visitOf visits name, status := .running,
        enteredAt := now, exitedAt := none, input, output := none, error := none, cause := none,
        attempts := 0, retryCounts := [], phase := .closed, await := .idle, result := none,
        next := none }
      let acc := { acc with run := { acc.run with seq := e.seq, visits } }
      let ctx := context acc.run e
      match s.body with
      | .command c =>
        acc.attempt c { e with retryCounts := c.retry.map fun _ => 0 } 1 now
      | .pass result parameters resultPath =>
        let value : Except String Json := match result, parameters with
          | some r, _ => .ok r
          | none, some p => resolve input ctx p
          | none, none => .ok input
        match value.bind (applyResultPath resultPath input) with
        | .error m => (acc.close e now .failed none (some runtimeError) (some m) none).failRun now
            (acc.run.hidden ++ [name]) runtimeError m
        | .ok output => transit fuel d now acc s e output none
      | .choice choices default =>
        match chooseNext choices input ctx with
        | .error m => (acc.close e now .failed none (some runtimeError) (some m) none).failRun now
            (acc.run.hidden ++ [name]) runtimeError m
        | .ok (some next) => transit fuel d now acc { s with next := some next } e input none
        | .ok none =>
          match default with
          | some next => transit fuel d now acc { s with next := some next } e input none
          | none =>
            let m := s!"no Choice rule of {name} matched and it has no Default"
            (acc.close e now .failed none (some "States.NoChoiceMatched") (some m) none).failRun now
              (acc.run.hidden ++ [name]) "States.NoChoiceMatched" m
      | .waitSeconds seconds =>
        let due := now + seconds * 1000
        (acc.open_ { e with status := .waiting, phase := .sleeping due } .waiting).timer (.wait e.seq) due
      | .waitUntil timestamp =>
        match parseTime timestamp with
        | none =>
          let m := s!"Timestamp {timestamp} is not an RFC 3339 time"
          (acc.close e now .failed none (some runtimeError) (some m) none).failRun now
            (acc.run.hidden ++ [name]) runtimeError m
        | some due =>
          (acc.open_ { e with status := .waiting, phase := .sleeping due } .waiting).timer (.wait e.seq) due
      | .succeed =>
        (acc.close e now .succeeded (some input) none none none).succeed now (acc.run.hidden ++ [name]) input
      | .fail error cause =>
        (acc.close e now .failed none (some error) (some cause) none).failRun now
          (acc.run.hidden ++ [name]) error cause

/-- Leaves the StateEntry `e` of `s` with `output`, to `s.next` or to the end of the SkillRun. -/
def transit : Nat → Definition → Nat → Acc → State → Entry → Json → Option ExecOutput → Acc
  | fuel, d, now, acc, s, e, output, result =>
    let e := { e with result := result.or e.result }
    match s.next with
    | none =>
      let acc := acc.close e now .succeeded (some output) none none none
      acc.succeed now (acc.run.hidden ++ [s.name]) output
    | some next =>
      let acc := acc.close e now .succeeded (some output) none none (some next)
      enter fuel d now (acc.transitionNotice s next result) next output
end

/-- Takes an error of the open StateEntry `e` of `s`: retry, catch, or fail the SkillRun. -/
def raise (d : Definition) (now : Nat) (acc : Acc) (s : State) (e : Entry) (error cause : String)
    (result : Option ExecOutput) : Acc :=
  let e := { e with result := result.or e.result }
  let closed : EntryStatus := if error == timeoutError then .timedOut else .failed
  let failAll := fun (acc : Acc) =>
    (acc.close e now closed none (some error) (some cause) none).failRun now
      (acc.run.hidden ++ [s.name]) error cause
  match s.body with
  | .command c =>
    match retryOf c.retry e.retryCounts error with
    | some (i, counts) =>
      let due := now + retryDelay (c.retry.getD i default) (e.retryCounts.getD i 0)
      let e := { e with status := .waiting, phase := .backoff due, retryCounts := counts,
                        error := some error, cause := some cause }
      (acc.open_ e .waiting).timer (.retry e.seq (e.attempts + 1)) due
    | none =>
      match firstHandler (c.catchers.map (·.errorEquals)) error with
      | none => failAll acc
      | some i =>
        let k := c.catchers.getD i default
        let value := Json.obj ([("Error", .str error), ("Cause", .str cause)] ++
          (result.map fun o => (execJson o).take 3).getD [])
        match applyResultPath k.resultPath e.input value with
        | .error m =>
          (acc.close e now .failed none (some runtimeError) (some m) none).failRun now
            (acc.run.hidden ++ [s.name]) runtimeError m
        | .ok output =>
          let status : EntryStatus := if error == timeoutError then .timedOut else .caught
          let acc := acc.close e now status (some output) (some error) (some cause) (some k.next)
          enter maxStates d now (acc.transitionNotice { s with next := some k.next } k.next result)
            k.next output
  | _ => failAll acc

/-- The open StateEntry and its State, when the SkillRun runs and the entry names a State. -/
def openState (d : Definition) (r : Run) : Option (State × Entry) :=
  if r.status.finished then none
  else
    match r.current with
    | none => none
    | some e => (d.lookup e.state).map fun s => (s, e)

def onExec (d : Definition) (r : Run) (seq attempt : Nat) (o : ExecOutput) (now : Nat) : Outcome :=
  match openState d r with
  | some (s, e) =>
    match s.body, e.phase with
    | .command c, .executing _ =>
      if e.seq = seq ∧ e.attempts = attempt then
        let acc : Acc := ⟨r, default⟩
        if o.timedOut then
          let acc := raise d now acc s e timeoutError "the command ran longer than TimeoutSeconds" (some o)
          .applied acc.run acc.write
        else if o.exitCode ≠ 0 then
          let acc := raise d now acc s e "Command.Failed" s!"the command exited with code {o.exitCode}" (some o)
          .applied acc.run acc.write
        else
          match applyResultPath c.resultPath e.input (.obj (execJson o)) with
          | .error m =>
            let acc := (acc.close { e with result := some o } now .failed none (some runtimeError)
              (some m) none).failRun now (r.hidden ++ [s.name]) runtimeError m
            .applied acc.run acc.write
          | .ok output =>
            let acc := transit maxStates d now acc s e output (some o)
            .applied acc.run acc.write
      else .ignored
    | _, _ => .ignored
  | none => .ignored

def onTimer (d : Definition) (r : Run) (t : Timer) (now : Nat) : Outcome :=
  match t with
  | .run =>
    if r.status.finished || now < r.startedAt + d.timeoutSeconds * 1000 then .ignored
    else
      let acc : Acc := ⟨r, default⟩
      let cause := "the SkillRun ran longer than TimeoutSeconds"
      let acc := match r.current with
        | some e => acc.close e now .failed none (some timeoutError) (some cause) none
        | none => acc
      let acc := acc.failRun now (r.hidden ++ (r.current.map (·.state)).toList) timeoutError cause
      .applied acc.run acc.write
  | .state seq attempt =>
    match openState d r with
    | some (s, e) =>
      match e.phase with
      | .executing deadline =>
        if e.seq = seq ∧ e.attempts = attempt ∧ deadline ≤ now then
          let acc := raise d now ⟨r, default⟩ s e timeoutError
            "the State ran longer than TimeoutSeconds" none
          .applied acc.run acc.write
        else .ignored
      | _ => .ignored
    | none => .ignored
  | .retry seq attempt =>
    match openState d r with
    | some (s, e) =>
      match s.body, e.phase with
      | .command c, .backoff due =>
        if e.seq = seq ∧ e.attempts + 1 = attempt ∧ due ≤ now then
          let acc := (⟨r, default⟩ : Acc).attempt c e attempt now
          .applied acc.run acc.write
        else .ignored
      | _, _ => .ignored
    | none => .ignored
  | .wait seq =>
    match openState d r with
    | some (s, e) =>
      match e.phase with
      | .sleeping due =>
        if e.seq = seq ∧ due ≤ now then
          let acc := transit maxStates d now ⟨r, default⟩ s e e.input none
          .applied acc.run acc.write
        else .ignored
      | _ => .ignored
    | none => .ignored

def onAwait (d : Definition) (r : Run) (order : Nat) : Outcome :=
  if r.status.finished then .rejected .finished
  else
    match openState d r with
    | some (s, e) =>
      match s.body, e.phase with
      | .command _, .executing _ =>
        let acc : Acc := ⟨r, default⟩
        match awaitStep e.await r.pendingContinue with
        | .replay v => .applied r (acc.output v).write
        | .take v =>
          let acc := (acc.open_ { e with await := .delivered v } r.status).output v
          .applied { acc.run with pendingContinue := none } acc.write
        | .wait =>
          let acc := acc.open_ { e with await := .waiting order } r.status
          .applied acc.run acc.write
      | _, _ => .rejected .notAwaiting
    | none => .rejected .notAwaiting

def onContinue (d : Definition) (r : Run) (v : Json) : Outcome :=
  if r.status.finished then .rejected .finished
  else
    let acc : Acc := ⟨r, default⟩
    match openState d r with
    | some (_, e) =>
      match continueStep e.await r.pendingContinue.isSome with
      | .deliver =>
        let acc := (acc.open_ { e with await := .delivered v } r.status).output v
        .applied acc.run acc.write
      | .keep => .applied { r with pendingContinue := some v } default
      | .reject => .rejected .continuePending
    | none => .rejected .notAwaiting

def onAbort (r : Run) (now : Nat) : Outcome :=
  if r.status.finished then .ignored
  else
    let acc : Acc := ⟨r, default⟩
    let acc := match r.current with
      | some e => acc.close e now .aborted none none none none
      | none => acc
    .applied { acc.run with status := .aborted, finishedAt := some now, current := none } acc.write

/-- Starts the SkillRun `id` of the definition with `input` at `now`. -/
def start (d : Definition) (id : Nat) (input : Json) (now : Nat) : Outcome :=
  let r : Run := {
    id, skill := d.skill, status := .running, input, output := none, error := none, cause := none,
    startedAt := now, finishedAt := none, seq := 0, visits := [], current := none,
    pendingContinue := none, hidden := [] }
  let acc := (⟨r, default⟩ : Acc).timer .run (now + d.timeoutSeconds * 1000)
  let acc := enter maxStates d now acc d.startAt input
  .applied acc.run acc.write

/-- Takes an event of the SkillRun at `now`. -/
def step (d : Definition) (r : Run) (event : Event) (now : Nat) : Outcome :=
  match event with
  | .exec seq attempt o => onExec d r seq attempt o now
  | .timer t => onTimer d r t now
  | .await order => onAwait d r order
  | .continue v => onContinue d r v
  | .abort => onAbort r now

end Sanchoris.Engine
