import Foundation

/// Ops for checking the helper itself.
enum CoreOps: HelperModule {
    static func register() {
        Helper.add("ping", Op(about: "Answers with the helper's macOS version and the input size", run: { args, input in
            .json(["pong": true, "macos": ProcessInfo.processInfo.operatingSystemVersionString, "bytes": input.count, "args": args])
        }))
        Helper.add("echo", Op(about: "Session: sends every line back", session: { _, s in
            while let m = s.next() { s.send(["echo": m]) }
        }))
    }
}
