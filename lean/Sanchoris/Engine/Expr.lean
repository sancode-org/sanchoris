import Sanchoris.Engine.Json

namespace Sanchoris.Engine

/-! ### Paths

A path is `$` (the State's input) or `$$` (the context), followed by `.name`, `['name']` and
`[index]` segments. -/

inductive Seg where
  | key (name : String)
  | index (n : Nat)
  deriving DecidableEq, Repr

structure Path where
  context : Bool
  segs : List Seg
  deriving DecidableEq, Repr

/-- The characters that end a `.name` segment. -/
def endsName (c : Char) : Bool :=
  c = '.' || c = '[' || c = ',' || c = ')' || Json.isSpace c

def parseSegs : Nat → List Char → List Seg → Option (List Seg × List Char)
  | 0, _, _ => none
  | fuel + 1, '.' :: rest, acc =>
    let name := rest.takeWhile (!endsName ·)
    if name.isEmpty then none
    else parseSegs fuel (rest.drop name.length) (.key (String.ofList name) :: acc)
  | fuel + 1, '[' :: '\'' :: rest, acc =>
    let name := rest.takeWhile (· != '\'')
    match rest.drop name.length with
    | '\'' :: ']' :: rest' => parseSegs fuel rest' (.key (String.ofList name) :: acc)
    | _ => none
  | fuel + 1, '[' :: rest, acc =>
    let ds := rest.takeWhile Json.isDigit
    match rest.drop ds.length with
    | ']' :: rest' =>
      if ds.isEmpty then none else parseSegs fuel rest' (.index (Json.digitsValue ds) :: acc)
    | _ => none
  | _ + 1, cs, acc => some (acc.reverse, cs)

/-- A path at the start of the text, and the text after it. -/
def parsePathPrefix (cs : List Char) : Option (Path × List Char) :=
  match cs with
  | '$' :: '$' :: rest => (parseSegs (rest.length + 1) rest []).map fun (s, r) => (⟨true, s⟩, r)
  | '$' :: rest => (parseSegs (rest.length + 1) rest []).map fun (s, r) => (⟨false, s⟩, r)
  | _ => none

def parsePath (s : String) : Option Path :=
  match parsePathPrefix s.toList with
  | some (p, []) => some p
  | _ => none

def getSegs : Json → List Seg → Option Json
  | j, [] => some j
  | j, .key k :: rest => (j.field? k).bind (getSegs · rest)
  | .arr items, .index n :: rest => items[n]?.bind (getSegs · rest)
  | _, .index _ :: _ => none

/-- The value a path names in the input or the context. -/
def Path.get (p : Path) (input context : Json) : Option Json :=
  getSegs (if p.context then context else input) p.segs

/-- Puts `v` at the keys of `segs`, making the objects that are missing. `none` when a segment
is an index or a value on the way is not an object. -/
def setSegs : Json → List Seg → Json → Option Json
  | _, [], v => some v
  | .obj fields, .key k :: rest, v =>
    let inner := (fields.find? (·.1 == k)).map (·.2) |>.getD (.obj [])
    (setSegs inner rest v).map fun x => .obj (Json.setField fields k x)
  | .null, .key k :: rest, v => (setSegs (.obj []) rest v).map fun x => .obj [(k, x)]
  | _, _, _ => none

/-- The input with a result at `ResultPath`: `none` (`ResultPath: null`) keeps the input. -/
def applyResultPath (resultPath : Option String) (input result : Json) : Except String Json :=
  match resultPath with
  | none => .ok input
  | some text =>
    match parsePath text with
    | some ⟨false, segs⟩ =>
      match setSegs input segs result with
      | some v => .ok v
      | none => .error s!"ResultPath {text} does not name a place in the input"
    | _ => .error s!"ResultPath {text} is not a path of the input"

/-! ### Intrinsic functions -/

/-- A piece of a string literal: a character, or `{}` (a place of `States.Format`). -/
inductive Piece where
  | char (c : Char)
  | hole
  deriving DecidableEq, Repr

inductive Expr where
  | path (p : Path)
  | str (pieces : List Piece)
  | num (n : Num)
  | json (v : Json)
  | call (name : String) (args : List Expr)
  deriving Repr, Inhabited

def piecesString (ps : List Piece) : String :=
  String.ofList (ps.flatMap fun | .char c => [c] | .hole => ['{', '}'])

/-- A string literal after its opening `'`: `\'`, `\{`, `\}` and `\\` escape a character. -/
def parseLiteral : List Char → List Piece → Option (List Piece × List Char)
  | [], _ => none
  | '\'' :: rest, acc => some (acc.reverse, rest)
  | '\\' :: c :: rest, acc => parseLiteral rest (.char c :: acc)
  | '{' :: '}' :: rest, acc => parseLiteral rest (.hole :: acc)
  | c :: rest, acc => parseLiteral rest (.char c :: acc)

def isNameChar (c : Char) : Bool :=
  c.isAlphanum || c = '.'

mutual
def parseExpr : Nat → List Char → Option (Expr × List Char)
  | 0, _ => none
  | fuel + 1, cs =>
    match Json.skipSpace cs with
    | '$' :: rest => (parsePathPrefix ('$' :: rest)).map fun (p, r) => (.path p, r)
    | '\'' :: rest => (parseLiteral rest []).map fun (ps, r) => (.str ps, r)
    | 't' :: 'r' :: 'u' :: 'e' :: rest => some (.json (.bool true), rest)
    | 'f' :: 'a' :: 'l' :: 's' :: 'e' :: rest => some (.json (.bool false), rest)
    | 'n' :: 'u' :: 'l' :: 'l' :: rest => some (.json .null, rest)
    | cs =>
      if cs.head?.any fun c => c = '-' || Json.isDigit c then
        (Json.parseNumber cs).map fun (n, r) => (.num n, r)
      else
        let name := cs.takeWhile isNameChar
        match Json.skipSpace (cs.drop name.length) with
        | '(' :: rest =>
          match Json.skipSpace rest with
          | ')' :: rest => some (.call (String.ofList name) [], rest)
          | rest => (parseArgs fuel rest []).map fun (args, r) => (.call (String.ofList name) args, r)
        | _ => none
def parseArgs : Nat → List Char → List Expr → Option (List Expr × List Char)
  | 0, _, _ => none
  | fuel + 1, cs, acc =>
    match parseExpr fuel cs with
    | none => none
    | some (e, rest) =>
      match Json.skipSpace rest with
      | ',' :: rest => parseArgs fuel rest (e :: acc)
      | ')' :: rest => some ((e :: acc).reverse, rest)
      | _ => none
end

/-- The text that `States.Format` puts in a place: a string as it is, any other value as JSON. -/
def formatArg : Json → String
  | .str s => s
  | v => v.render

def fill : List Piece → List String → Except String String
  | [], [] => .ok ""
  | [], _ :: _ => .error "States.Format has more arguments than places"
  | .hole :: _, [] => .error "States.Format has more places than arguments"
  | .hole :: ps, a :: args => (fill ps args).map (a ++ ·)
  | .char c :: ps, args => (fill ps args).map (String.singleton c ++ ·)

mutual
def evalExpr (input context : Json) : Expr → Except String Json
  | .path p =>
    match p.get input context with
    | some v => .ok v
    | none => .error "a path names no value"
  | .str ps => .ok (.str (piecesString ps))
  | .num n => .ok (.num n)
  | .json v => .ok v
  | .call "States.Format" (.str template :: args) => do
    let vs ← evalArgs input context args
    let s ← fill template (vs.map formatArg)
    pure (.str s)
  | .call "States.JsonToString" [arg] => do
    let v ← evalExpr input context arg
    pure (.str v.render)
  | .call "States.StringToJson" [arg] => do
    match ← evalExpr input context arg with
    | .str s =>
      match Json.parse s with
      | some v => pure v
      | none => .error "States.StringToJson takes JSON text"
    | _ => .error "States.StringToJson takes a string"
  | .call "States.MathAdd" [a, b] => do
    let x ← evalExpr input context a
    let y ← evalExpr input context b
    match x, y with
    | .num m, .num n =>
      if m.toInt?.isSome && n.toInt?.isSome then pure (.num (m.add n))
      else .error "States.MathAdd takes integers"
    | _, _ => .error "States.MathAdd takes numbers"
  | .call name _ => .error s!"{name} is not an intrinsic function with these arguments"
def evalArgs (input context : Json) : List Expr → Except String (List Json)
  | [] => .ok []
  | e :: es => do
    let v ← evalExpr input context e
    let vs ← evalArgs input context es
    pure (v :: vs)
end

/-- The value of a `.$` key: a path, or a call of an intrinsic function. -/
def evalValue (text : String) (input context : Json) : Except String Json :=
  let cs := text.toList
  match parseExpr (cs.length + 1) cs with
  | some (e, rest) =>
    if (Json.skipSpace rest).isEmpty then evalExpr input context e
    else .error s!"{text} is not a path or an intrinsic function"
  | none => .error s!"{text} is not a path or an intrinsic function"

mutual
/-- A template with its `.$` keys replaced by their values, as `Parameters` is. -/
def resolve (input context : Json) : Json → Except String Json
  | .obj fields => (resolveFields input context fields).map .obj
  | .arr items => (resolveItems input context items).map .arr
  | v => .ok v
def resolveItems (input context : Json) : List Json → Except String (List Json)
  | [] => .ok []
  | x :: xs => do
    let v ← resolve input context x
    let vs ← resolveItems input context xs
    pure (v :: vs)
def resolveFields (input context : Json) : List (String × Json) → Except String (List (String × Json))
  | [] => .ok []
  | (k, x) :: xs => do
    let rest ← resolveFields input context xs
    if k.endsWith ".$" then
      match x with
      | .str text =>
        let v ← evalValue text input context
        pure ((String.ofList (k.toList.take (k.length - 2)), v) :: rest)
      | _ => .error s!"{k} takes a path or an intrinsic function"
    else
      let v ← resolve input context x
      pure ((k, v) :: rest)
end

/-! ### Choice rules -/

inductive Rel where
  | eq
  | lt
  | gt
  | le
  | ge
  deriving DecidableEq, Repr

inductive Test where
  | stringEquals (s : String)
  | stringMatches (pattern : String)
  | numeric (rel : Rel) (n : Num)
  | booleanEquals (b : Bool)
  | isPresent (b : Bool)
  | isNull (b : Bool)
  deriving DecidableEq, Repr

inductive Cond where
  | all (conds : List Cond)
  | any (conds : List Cond)
  | not (cond : Cond)
  | test (path : String) (test : Test)
  deriving Repr, Inhabited

inductive Tok where
  | lit (c : Char)
  | star
  deriving DecidableEq, Repr

/-- The pattern of `StringMatches`: `*` matches any text, `\*` and `\\` are `*` and `\`. -/
def tokens : List Char → List Tok
  | '\\' :: c :: rest => .lit c :: tokens rest
  | '*' :: rest => .star :: tokens rest
  | c :: rest => .lit c :: tokens rest
  | [] => []

def wild : List Tok → List Char → Bool
  | [], s => s.isEmpty
  | .star :: p, s =>
    wild p s || match s with
      | [] => false
      | _ :: s' => wild (.star :: p) s'
  | .lit c :: p, d :: s => c == d && wild p s
  | .lit _ :: _, [] => false
termination_by p s => (p.length, s.length)

def Rel.holds (rel : Rel) (o : Ordering) : Bool :=
  match rel with
  | .eq => o == .eq
  | .lt => o == .lt
  | .gt => o == .gt
  | .le => o != .gt
  | .ge => o != .lt

def Test.holds (t : Test) (v : Json) : Bool :=
  match t, v with
  | .stringEquals s, .str x => s == x
  | .stringMatches p, .str x => wild (tokens p.toList) x.toList
  | .numeric rel n, .num x => rel.holds (x.cmp n)
  | .booleanEquals b, .bool x => b == x
  | .isNull b, v => (v == .null) == b
  | _, _ => false

mutual
def Cond.eval (input context : Json) : Cond → Except String Bool
  | .all cs => Cond.evalAll input context cs
  | .any cs => Cond.evalAny input context cs
  | .not c => (Cond.eval input context c).map (!·)
  | .test path t =>
    match parsePath path with
    | none => .error s!"Variable {path} is not a path"
    | some p =>
      match t, p.get input context with
      | .isPresent b, v => .ok (v.isSome == b)
      | _, none => .error s!"Variable {path} names no value"
      | t, some v => .ok (t.holds v)
def Cond.evalAll (input context : Json) : List Cond → Except String Bool
  | [] => .ok true
  | c :: cs => do
    if ← Cond.eval input context c then Cond.evalAll input context cs else pure false
def Cond.evalAny (input context : Json) : List Cond → Except String Bool
  | [] => .ok false
  | c :: cs => do
    if ← Cond.eval input context c then pure true else Cond.evalAny input context cs
end

/-! ### Timestamps

Times are milliseconds since 1970-01-01T00:00:00Z. -/

def daysFromCivil (y m d : Nat) : Nat :=
  let y := if m ≤ 2 then y - 1 else y
  let era := y / 400
  let yoe := y - era * 400
  let mp := if m > 2 then m - 3 else m + 9
  let doy := (153 * mp + 2) / 5 + d - 1
  let doe := yoe * 365 + yoe / 4 - yoe / 100 + doy
  era * 146097 + doe - 719468

def civilFromDays (days : Nat) : Nat × Nat × Nat :=
  let z := days + 719468
  let era := z / 146097
  let doe := z - era * 146097
  let yoe := (doe - doe / 1460 + doe / 36524 - doe / 146096) / 365
  let doy := doe - (365 * yoe + yoe / 4 - yoe / 100)
  let mp := (5 * doy + 2) / 153
  let d := doy - (153 * mp + 2) / 5 + 1
  let m := if mp < 10 then mp + 3 else mp - 9
  (if m ≤ 2 then yoe + era * 400 + 1 else yoe + era * 400, m, d)

def pad (width n : Nat) : String :=
  let s := Num.digits n
  String.ofList (List.replicate (width - s.length) '0') ++ s

/-- The time in the form `2026-11-28T09:30:00.000Z`. -/
def formatTime (ms : Nat) : String :=
  let (y, m, d) := civilFromDays (ms / 86400000)
  let t := ms % 86400000
  s!"{pad 4 y}-{pad 2 m}-{pad 2 d}T{pad 2 (t / 3600000)}:{pad 2 (t / 60000 % 60)}:{pad 2 (t / 1000 % 60)}.{pad 3 (t % 1000)}Z"

def fixedDigits (n : Nat) (cs : List Char) : Option (Nat × List Char) :=
  let ds := cs.take n
  if ds.length = n && ds.all Json.isDigit then some (Json.digitsValue ds, cs.drop n) else none

/-- An RFC 3339 time with a `Z` or `±HH:MM` offset, from 1970 on. -/
def parseTime (s : String) : Option Nat := do
  let cs := s.toList
  let (y, cs) ← fixedDigits 4 cs
  let cs ← match cs with | '-' :: r => some r | _ => none
  let (mo, cs) ← fixedDigits 2 cs
  let cs ← match cs with | '-' :: r => some r | _ => none
  let (d, cs) ← fixedDigits 2 cs
  let cs ← match cs with | 'T' :: r => some r | 't' :: r => some r | _ => none
  let (h, cs) ← fixedDigits 2 cs
  let cs ← match cs with | ':' :: r => some r | _ => none
  let (mi, cs) ← fixedDigits 2 cs
  let cs ← match cs with | ':' :: r => some r | _ => none
  let (sec, cs) ← fixedDigits 2 cs
  let (ms, cs) := match cs with
    | '.' :: r =>
      let ds := r.takeWhile Json.isDigit
      (Json.digitsValue ((ds ++ ['0', '0', '0']).take 3), r.drop ds.length)
    | _ => (0, cs)
  let offset : Int ← match cs with
    | ['Z'] | ['z'] => some 0
    | sign :: rest =>
      match fixedDigits 2 rest with
      | some (oh, ':' :: rest') =>
        match fixedDigits 2 rest' with
        | some (om, []) =>
          let v : Int := (oh * 60 + om) * 60000
          if sign = '+' then some v else if sign = '-' then some (-v) else none
        | _ => none
      | _ => none
    | [] => none
  if y < 1970 || mo < 1 || mo > 12 || d < 1 || d > 31 || h > 23 || mi > 59 || sec > 60 then none
  else
    let local_ : Int := ((daysFromCivil y mo d * 24 + h) * 60 + mi) * 60000 + sec * 1000 + ms
    let utc := local_ - offset
    if utc < 0 then none else some utc.toNat

end Sanchoris.Engine
