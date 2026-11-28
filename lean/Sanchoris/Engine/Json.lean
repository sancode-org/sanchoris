namespace Sanchoris.Engine

/-- A JSON number `mantissa * 10 ^ exponent`. Numbers are compared by value; `Num.normalize`
gives the form with no trailing zeros in the mantissa, which the engine keeps. -/
structure Num where
  mantissa : Int
  exponent : Int
  deriving DecidableEq, Repr, Inhabited

/-- A JSON value. Objects keep their keys in order. -/
inductive Json where
  | null
  | bool (b : Bool)
  | num (n : Num)
  | str (s : String)
  | arr (items : List Json)
  | obj (fields : List (String × Json))
  deriving Repr, Inhabited

mutual
def Json.decEq : (a b : Json) → Decidable (a = b)
  | .null, .null => isTrue rfl
  | .bool a, .bool b =>
    if h : a = b then isTrue (h ▸ rfl) else isFalse (by intro e; cases e; exact h rfl)
  | .num a, .num b =>
    if h : a = b then isTrue (h ▸ rfl) else isFalse (by intro e; cases e; exact h rfl)
  | .str a, .str b =>
    if h : a = b then isTrue (h ▸ rfl) else isFalse (by intro e; cases e; exact h rfl)
  | .arr a, .arr b =>
    match Json.decEqItems a b with
    | isTrue h => isTrue (h ▸ rfl)
    | isFalse h => isFalse (by intro e; cases e; exact h rfl)
  | .obj a, .obj b =>
    match Json.decEqFields a b with
    | isTrue h => isTrue (h ▸ rfl)
    | isFalse h => isFalse (by intro e; cases e; exact h rfl)
  | .null, .bool _ | .null, .num _ | .null, .str _ | .null, .arr _ | .null, .obj _
  | .bool _, .null | .bool _, .num _ | .bool _, .str _ | .bool _, .arr _ | .bool _, .obj _
  | .num _, .null | .num _, .bool _ | .num _, .str _ | .num _, .arr _ | .num _, .obj _
  | .str _, .null | .str _, .bool _ | .str _, .num _ | .str _, .arr _ | .str _, .obj _
  | .arr _, .null | .arr _, .bool _ | .arr _, .num _ | .arr _, .str _ | .arr _, .obj _
  | .obj _, .null | .obj _, .bool _ | .obj _, .num _ | .obj _, .str _ | .obj _, .arr _ =>
    isFalse (by intro e; cases e)
def Json.decEqItems : (a b : List Json) → Decidable (a = b)
  | [], [] => isTrue rfl
  | x :: xs, y :: ys =>
    match Json.decEq x y, Json.decEqItems xs ys with
    | isTrue h1, isTrue h2 => isTrue (h1 ▸ h2 ▸ rfl)
    | isFalse h, _ => isFalse (by intro e; cases e; exact h rfl)
    | _, isFalse h => isFalse (by intro e; cases e; exact h rfl)
  | [], _ :: _ | _ :: _, [] => isFalse (by intro e; cases e)
def Json.decEqFields : (a b : List (String × Json)) → Decidable (a = b)
  | [], [] => isTrue rfl
  | (k, x) :: xs, (l, y) :: ys =>
    if hk : k = l then
      match Json.decEq x y, Json.decEqFields xs ys with
      | isTrue h1, isTrue h2 => isTrue (hk ▸ h1 ▸ h2 ▸ rfl)
      | isFalse h, _ => isFalse (by intro e; cases e; exact h rfl)
      | _, isFalse h => isFalse (by intro e; cases e; exact h rfl)
    else isFalse (by intro e; cases e; exact hk rfl)
  | [], _ :: _ | _ :: _, [] => isFalse (by intro e; cases e)
end

instance : DecidableEq Json := Json.decEq

namespace Num

def ofInt (n : Int) : Num := ⟨n, 0⟩

/-- Removes the trailing zeros of the mantissa; zero has exponent 0. -/
def normalize (n : Num) : Num :=
  if n.mantissa = 0 then ⟨0, 0⟩ else go n.mantissa.natAbs.succ n
where
  go : Nat → Num → Num
    | 0, n => n
    | fuel + 1, n => if n.mantissa % 10 = 0 then go fuel ⟨n.mantissa / 10, n.exponent + 1⟩ else n

/-- The mantissas of two numbers scaled to their smaller exponent. -/
def align (a b : Num) : Int × Int :=
  let e := min a.exponent b.exponent
  (a.mantissa * 10 ^ (a.exponent - e).toNat, b.mantissa * 10 ^ (b.exponent - e).toNat)

def cmp (a b : Num) : Ordering :=
  let (x, y) := align a b
  compare x y

def add (a b : Num) : Num :=
  let (x, y) := align a b
  normalize ⟨x + y, min a.exponent b.exponent⟩

/-- The integer a number is, if it is one. -/
def toInt? (n : Num) : Option Int :=
  if 0 ≤ n.exponent then some (n.mantissa * 10 ^ n.exponent.toNat)
  else
    let d := (10 : Int) ^ (-n.exponent).toNat
    if n.mantissa % d = 0 then some (n.mantissa / d) else none

def digits (n : Nat) : String :=
  String.ofList (Nat.toDigits 10 n)

/-- The number in decimal notation, without an exponent. -/
def render (n : Num) : String :=
  let sign := if n.mantissa < 0 then "-" else ""
  let m := n.mantissa.natAbs
  if 0 ≤ n.exponent then sign ++ digits (m * 10 ^ n.exponent.toNat)
  else
    let k := (-n.exponent).toNat
    let ds := (Nat.toDigits 10 m)
    let ds := List.replicate (k + 1 - ds.length) '0' ++ ds
    let whole := ds.take (ds.length - k)
    let frac := ds.drop (ds.length - k)
    sign ++ String.ofList whole ++ "." ++ String.ofList frac

end Num

namespace Json

def field? (j : Json) (key : String) : Option Json :=
  match j with
  | .obj fields => (fields.find? (·.1 == key)).map (·.2)
  | _ => none

/-- Sets a field of an object, in place when the key is there and at the end otherwise. -/
def setField (fields : List (String × Json)) (key : String) (v : Json) : List (String × Json) :=
  if fields.any (·.1 == key) then fields.map fun (k, x) => if k == key then (k, v) else (k, x)
  else fields ++ [(key, v)]

def hexDigit (n : Nat) : Char :=
  if n < 10 then Char.ofNat (48 + n) else Char.ofNat (87 + n)

def escapeChar (c : Char) : List Char :=
  if c = '"' then ['\\', '"']
  else if c = '\\' then ['\\', '\\']
  else if c = '\n' then ['\\', 'n']
  else if c = '\r' then ['\\', 'r']
  else if c = '\t' then ['\\', 't']
  else if c.toNat < 32 then
    ['\\', 'u', '0', '0', hexDigit (c.toNat / 16), hexDigit (c.toNat % 16)]
  else [c]

def quote (s : String) : String :=
  String.ofList ('"' :: (s.toList.flatMap escapeChar ++ ['"']))

mutual
/-- The JSON text of a value, with no spaces. -/
def render : Json → String
  | .null => "null"
  | .bool true => "true"
  | .bool false => "false"
  | .num n => n.render
  | .str s => quote s
  | .arr items => "[" ++ renderItems items ++ "]"
  | .obj fields => "{" ++ renderFields fields ++ "}"
def renderItems : List Json → String
  | [] => ""
  | [x] => render x
  | x :: xs => render x ++ "," ++ renderItems xs
def renderFields : List (String × Json) → String
  | [] => ""
  | [(k, x)] => quote k ++ ":" ++ render x
  | (k, x) :: xs => quote k ++ ":" ++ render x ++ "," ++ renderFields xs
end

/-! ### Parsing JSON text -/

def isSpace (c : Char) : Bool :=
  c = ' ' || c = '\n' || c = '\r' || c = '\t'

def skipSpace : List Char → List Char
  | c :: cs => if isSpace c then skipSpace cs else c :: cs
  | [] => []

def hexValue (c : Char) : Option Nat :=
  if '0' ≤ c ∧ c ≤ '9' then some (c.toNat - 48)
  else if 'a' ≤ c ∧ c ≤ 'f' then some (c.toNat - 87)
  else if 'A' ≤ c ∧ c ≤ 'F' then some (c.toNat - 55)
  else none

def hex4 : List Char → Option (Nat × List Char)
  | a :: b :: c :: d :: rest => do
    let a ← hexValue a
    let b ← hexValue b
    let c ← hexValue c
    let d ← hexValue d
    pure (((a * 16 + b) * 16 + c) * 16 + d, rest)
  | _ => none

/-- The characters of a string literal after its opening quote, and the text after the closing
quote. -/
def parseStringGo : Nat → List Char → List Char → Option (String × List Char)
  | 0, _, _ => none
  | _ + 1, [], _ => none
  | _ + 1, '"' :: rest, acc => some (String.ofList acc.reverse, rest)
  | fuel + 1, '\\' :: c :: rest, acc =>
    if c = 'u' then
      match hex4 rest with
      | none => none
      | some (hi, rest) =>
        if 0xD800 ≤ hi ∧ hi < 0xDC00 then
          match rest with
          | '\\' :: 'u' :: rest' =>
            match hex4 rest' with
            | some (lo, rest'') =>
              if 0xDC00 ≤ lo ∧ lo < 0xE000 then
                parseStringGo fuel rest'' (Char.ofNat (0x10000 + (hi - 0xD800) * 0x400 + (lo - 0xDC00)) :: acc)
              else none
            | none => none
          | _ => none
        else if 0xDC00 ≤ hi ∧ hi < 0xE000 then none
        else parseStringGo fuel rest (Char.ofNat hi :: acc)
    else
      let e :=
        if c = '"' then some '"' else if c = '\\' then some '\\' else if c = '/' then some '/'
        else if c = 'b' then some (Char.ofNat 8) else if c = 'f' then some (Char.ofNat 12)
        else if c = 'n' then some '\n' else if c = 'r' then some '\r'
        else if c = 't' then some '\t' else none
      match e with
      | some e => parseStringGo fuel rest (e :: acc)
      | none => none
  | fuel + 1, c :: rest, acc => if c.toNat < 32 then none else parseStringGo fuel rest (c :: acc)

def parseString (cs : List Char) : Option (String × List Char) :=
  parseStringGo (cs.length + 1) cs []

def isDigit (c : Char) : Bool := '0' ≤ c && c ≤ '9'

def digitsValue (ds : List Char) : Nat :=
  ds.foldl (fun n c => n * 10 + (c.toNat - 48)) 0

/-- A JSON number: an optional `-`, the integer part, an optional fraction and exponent. -/
def parseNumber (cs : List Char) : Option (Num × List Char) :=
  let (neg, cs) := match cs with
    | '-' :: rest => (true, rest)
    | _ => (false, cs)
  let whole := cs.takeWhile isDigit
  let cs := cs.drop whole.length
  if whole.isEmpty || (whole.length > 1 && whole.head? = some '0') then none
  else
    let (frac, cs, ok) := match cs with
      | '.' :: rest =>
        let f := rest.takeWhile isDigit
        (f, rest.drop f.length, !f.isEmpty)
      | _ => ([], cs, true)
    if !ok then none
    else
      let (exp, cs, ok) : Int × List Char × Bool := match cs with
        | c :: rest =>
          if c = 'e' || c = 'E' then
            let (eneg, rest) := match rest with
              | '-' :: r => (true, r)
              | '+' :: r => (false, r)
              | _ => (false, rest)
            let ds := rest.takeWhile isDigit
            let v : Int := digitsValue ds
            (if eneg then -v else v, rest.drop ds.length, !ds.isEmpty)
          else (0, cs, true)
        | [] => (0, cs, true)
      if !ok then none
      else
        let m : Int := digitsValue (whole ++ frac)
        some (Num.normalize ⟨if neg then -m else m, exp - frac.length⟩, cs)

def literal : List Char → List Char → Option (List Char)
  | [], rest => some rest
  | c :: cs, d :: rest => if c = d then literal cs rest else none
  | _ :: _, [] => none

mutual
/-- A JSON value at the start of the text (after spaces), and the text after it. `fuel` bounds
the depth of nesting. -/
def parseValue : Nat → List Char → Option (Json × List Char)
  | 0, _ => none
  | fuel + 1, cs =>
    match skipSpace cs with
    | '{' :: rest => parseFields fuel (skipSpace rest) []
    | '[' :: rest => parseItems fuel (skipSpace rest) []
    | '"' :: rest => (parseString rest).map fun (s, r) => (.str s, r)
    | 't' :: rest => (literal "rue".toList rest).map fun r => (.bool true, r)
    | 'f' :: rest => (literal "alse".toList rest).map fun r => (.bool false, r)
    | 'n' :: rest => (literal "ull".toList rest).map fun r => (.null, r)
    | cs => (parseNumber cs).map fun (n, r) => (.num n, r)
def parseItems : Nat → List Char → List Json → Option (Json × List Char)
  | 0, _, _ => none
  | fuel + 1, cs, acc =>
    match cs, acc with
    | ']' :: rest, [] => some (.arr [], rest)
    | cs, acc =>
      match parseValue fuel cs with
      | none => none
      | some (v, rest) =>
        match skipSpace rest with
        | ',' :: rest => parseItems fuel rest (v :: acc)
        | ']' :: rest => some (.arr (v :: acc).reverse, rest)
        | _ => none
def parseFields : Nat → List Char → List (String × Json) → Option (Json × List Char)
  | 0, _, _ => none
  | fuel + 1, cs, acc =>
    match cs, acc with
    | '}' :: rest, [] => some (.obj [], rest)
    | '"' :: rest, acc =>
      match parseString rest with
      | none => none
      | some (k, rest) =>
        match skipSpace rest with
        | ':' :: rest =>
          match parseValue fuel rest with
          | none => none
          | some (v, rest) =>
            let acc := (k, v) :: acc.filter (·.1 != k)
            match skipSpace rest with
            | ',' :: rest => parseFields fuel (skipSpace rest) acc
            | '}' :: rest => some (.obj acc.reverse, rest)
            | _ => none
        | _ => none
    | _, _ => none
end

/-- The value of a JSON text, which may have spaces around it. -/
def parse (s : String) : Option Json :=
  let cs := s.toList
  match parseValue (cs.length + 1) cs with
  | some (v, rest) => if (skipSpace rest).isEmpty then some v else none
  | none => none

end Json

end Sanchoris.Engine
