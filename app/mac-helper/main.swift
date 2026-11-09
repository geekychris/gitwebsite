// gw-mac-helper: Apple frameworks for the site's tools, run by the gitwebsite companion
// (crates/gw/src/companion/native/mac.rs has the protocol). One process per call:
//
//   gw-mac-helper --list                      → {"ops":[{"op","about","session"}…]}
//   gw-mac-helper <op> <args-json>            stdin: the input bytes
//                                             stdout: {"type":"<mime>","meta":{…}}\n then the body
//   gw-mac-helper <op> <args-json> --session  JSON lines on stdin and stdout until stdin closes
//
// A failure exits non-zero with {"error":"…"} as the last stderr line. Each area registers its
// ops from its own file (Core.swift, Vision.swift, …): add it to `modules` below.

import Foundation

let modules: [HelperModule.Type] = [
    CoreOps.self,
]

for m in modules { m.register() }
Helper.main(CommandLine.arguments)
