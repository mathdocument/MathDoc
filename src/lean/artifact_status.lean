import Lean.Environment
import Lean.Util.Sorry

open Lean

-- Read proof bodies without importing or elaborating the managed modules.
-- The private part contains the actual bodies in Lean's module system.
@[noinline] def inspect (paths : Array System.FilePath) : IO (Bool × Array CompactedRegion) := do
  let parts ← readModuleDataParts paths
  let some (data, _) := parts.back? | throw (IO.userError "missing module data")
  if data.isModule && parts.size != 3 then
    throw (IO.userError "missing private module data")
  let hasSorry := data.constants.any fun c =>
    c.type.hasSorry || (c.value? (allowOpaque := true)).any Expr.hasSorry
  return (hasSorry, parts.map (·.2))

unsafe def main : IO Unit := do
  let input ← (← IO.getStdin).getLine
  let paths : Array (Array String) ← IO.ofExcept (Json.parse input >>= fromJson?)
  let mut results : Array (Option Bool) := #[]
  for path in paths do
    try
      let (hasSorry, regions) ← inspect (path.map System.FilePath.mk)
      -- inspect returns no references into the regions; release each module
      -- before reading the next to bound memory for large dependency closures.
      regions.reverse.forM CompactedRegion.free
      results := results.push (some hasSorry)
    catch e =>
      (← IO.getStderr).putStrLn s!"Lean artifact status: {path.toList}: {e}"
      results := results.push none
  (← IO.getStdout).putStrLn (toJson results).compress
