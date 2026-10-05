import Lake.CLI.Main
import Lake.CLI.Serve

-- Retain Lake's native server options and invalid-configuration fallback.
def main (args : List String) : IO UInt32 := do
  let proxy :: rest := args | throw <| IO.userError "missing MathDoc Lake proxy"
  let (elanInstall?, leanInstall?, lakeInstall?) ← Lake.findInstall?
  let opts : Lake.LakeOptions := { elanInstall?, leanInstall?, lakeInstall? }
  let config ← opts.mkLoadConfig |>.toIO (IO.userError ∘ toString)
  let lake := { config.lakeEnv.lake with lake := proxy : Lake.LakeInstall }
  Lake.serve { config with lakeEnv := { config.lakeEnv with lake } } rest.toArray
