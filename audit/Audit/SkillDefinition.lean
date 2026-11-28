import Assay
import Sanchoris.SkillDefinition

open Plausible Sanchoris.SkillDefinition

instance : Inhabited StateType := ⟨.pass⟩

def genOf [Inhabited α] (xs : List α) : Gen α := do
  let i ← Gen.choose Nat 0 (xs.length - 1) (by omega)
  pure (xs.getD i.val default)

def genList (most : Nat) (gen : Gen α) : Gen (List α) := do
  let n ← Gen.choose Nat 0 most (by omega)
  (List.range n.val).mapM fun _ => gen

def genOption (gen : Gen α) : Gen (Option α) := do
  if (← Gen.choose Nat 0 3 (by omega)).val = 0 then pure none else some <$> gen

def genName : Gen String :=
  genOf ["A", "B", "C"]

def genErrorEquals : Gen (List String) :=
  genOf [["Command.Failed"], ["Command.Failed"], ["States.ALL"], ["Command.Failed", "States.ALL"]]

/-- Usually true: the generated definitions are mostly valid, with a few changes. -/
def genKeep : Gen Bool := do
  pure ((← Gen.choose Nat 0 7 (by omega)).val != 0)

def genKeys (vocabulary : List String) : Gen (List String) :=
  vocabulary.filterM fun _ => genKeep

def genTarget (n : String) : Gen (Option String) := do
  if ← genKeep then pure (some n) else genOption genName

def requiredKeys : StateType → List String
  | .command => ["Run", "TimeoutSeconds"]
  | .wait => ["Seconds"]
  | .choice => ["Choices"]
  | .fail => ["Error", "Cause"]
  | .pass => []
  | .succeed => []

def genRule (vocabulary : List String) (next : String) : Gen Rule := do
  pure { keys := ← genKeys vocabulary, errorEquals := ← genErrorEquals, next := ← genTarget next }

/-- A State that goes on to `next`, or ends when `next` is `none`. -/
def genState (name : String) (next : Option String) : Gen State := do
  let type ← match next with
    | some _ => genOf [StateType.command, .pass, .wait, .choice]
    | none => genOf [StateType.succeed, .fail, .command, .pass]
  let target := next.getD "A"
  let goesOn := next.isSome
  let keys ← genKeys ("Type" :: requiredKeys type)
  let keepType ← genKeep
  let nextName ← genTarget target
  let keepEnd ← genKeep
  let choice ← genRule ["Next"] target
  let keepDefault ← genKeep
  let defaultName ← genOption genName
  let retriers ← genList 1 (genRule ["ErrorEquals", "IntervalSeconds", "MaxAttempts", "BackoffRate"] target)
  let catchers ← genList 1 (genRule ["ErrorEquals", "Next"] target)
  pure {
    name, keys
    type := if keepType then some type else none
    next := if goesOn && type != .choice then nextName else none
    isEnd := if goesOn then !keepEnd else type != .succeed && type != .fail
    choices := if type == .choice then [choice] else []
    default := if type == .choice && !keepDefault then defaultName else none
    retriers := if type == .command then retriers else []
    catchers := if type == .command then catchers else [] }

def genDefinition : Gen Definition := do
  let count := (← Gen.choose Nat 0 3 (by omega)).val
  let pool := ["A", "B", "C"].take count
  let states ← (List.range count).mapM fun i =>
    genState (pool.getD i "A") (if i + 1 < count then some (pool.getD (i + 1) "A") else none)
  let keys ← genKeys ["Inputs", "StartAt", "States", "TimeoutSeconds"]
  let startAt ← genTarget "A"
  let inOrder ← genKeep
  pure { keys, startAt, states := if inOrder then states else states.reverse }

instance : Arbitrary Definition := ⟨genDefinition⟩
instance : Shrinkable Definition := ⟨fun _ => []⟩

set_option assay.samples 300 in
#assay Sanchoris.SkillDefinition.validate
