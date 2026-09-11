import Lean.Elab.ParseImportsFast

-- Parse headers with the project's compiler, including public/meta imports and comments.
def main (args : List String) : IO Unit := do
  let [listFile] := args | throw <| IO.userError "expected a newline-delimited source-file list"
  for path in (← IO.FS.readFile listFile).splitOn "\n" do
    unless path.isEmpty do
      let header ← Lean.parseImports' (← IO.FS.readFile path) path
      IO.println <| Lean.Json.compress <| Lean.toJson
        (path, header.imports.map (·.module.toString))
  (← IO.getStdout).flush
  -- Lean 4.34.0-rc2 on macOS 27 can crash in pthread cleanup after --run returns.
  -- Exit only after every header parsed and output was flushed; errors still fail.
  IO.Process.exit 0
