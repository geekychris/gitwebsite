import Foundation

typealias Args = [String: Any]

/// What a one-shot op returns.
enum Output {
    /// JSON (application/json).
    case json(Any)
    /// Bytes with a content type, and small metadata sent in the header line.
    case data(Data, type: String, meta: [String: Any] = [:])
    /// JSON lines written as they are produced (application/x-ndjson): call `emit` per line.
    case lines((_ emit: @escaping ([String: Any]) -> Void) throws -> Void)
}

struct HelperError: Error, CustomStringConvertible {
    let description: String
    init(_ d: String) { description = d }
}

/// A session: JSON lines in (`next()`), JSON lines out (`send`).
final class Session {
    private let lock = NSLock()
    func send(_ obj: [String: Any]) {
        lock.lock(); defer { lock.unlock() }
        Helper.writeLine(obj)
    }
    /// The next line from the page, or nil when it has gone.
    func next() -> [String: Any]? {
        guard let line = readLine(strippingNewline: true) else { return nil }
        return (try? JSONSerialization.jsonObject(with: Data(line.utf8))) as? [String: Any] ?? [:]
    }
}

struct Op {
    let about: String
    var run: ((Args, Data) throws -> Output)? = nil
    var session: ((Args, Session) throws -> Void)? = nil
}

protocol HelperModule {
    static func register()
}

enum Helper {
    static var ops: [String: Op] = [:]

    static func add(_ name: String, _ op: Op) { ops[name] = op }

    static func writeLine(_ obj: Any) {
        var d = (try? JSONSerialization.data(withJSONObject: obj, options: [.withoutEscapingSlashes])) ?? Data("{}".utf8)
        d.append(0x0A)
        FileHandle.standardOutput.write(d)
    }

    static func fail(_ msg: String) -> Never {
        var d = (try? JSONSerialization.data(withJSONObject: ["error": msg])) ?? Data()
        d.append(0x0A)
        FileHandle.standardError.write(d)
        exit(1)
    }

    static func main(_ argv: [String]) {
        guard argv.count >= 2 else { fail("usage: gw-mac-helper --list | <op> <args-json> [--session]") }
        if argv[1] == "--list" {
            let list = ops.keys.sorted().map { k -> [String: Any] in
                ["op": k, "about": ops[k]!.about, "session": ops[k]!.session != nil]
            }
            writeLine(["ops": list])
            return
        }
        let name = argv[1]
        guard let op = ops[name] else { fail("no op \(name)") }
        var args: Args = [:]
        if argv.count >= 3 {
            guard let a = (try? JSONSerialization.jsonObject(with: Data(argv[2].utf8))) as? Args else { fail("the arguments are not a JSON object") }
            args = a
        }
        do {
            if argv.count >= 4 && argv[3] == "--session" {
                guard let s = op.session else { fail("\(name) is not a session") }
                try s(args, Session())
                return
            }
            guard let run = op.run else { fail("\(name) is a session: use --session") }
            let input = FileHandle.standardInput.readDataToEndOfFile()
            switch try run(args, input) {
            case .json(let v):
                writeLine(["type": "application/json"])
                let d = (try? JSONSerialization.data(withJSONObject: v, options: [.withoutEscapingSlashes, .fragmentsAllowed])) ?? Data("null".utf8)
                FileHandle.standardOutput.write(d)
            case .data(let d, let type, let meta):
                writeLine(["type": type, "meta": meta])
                FileHandle.standardOutput.write(d)
            case .lines(let produce):
                writeLine(["type": "application/x-ndjson"])
                try produce { writeLine($0) }
            }
        } catch {
            fail("\(error)")
        }
    }
}

extension Dictionary where Key == String, Value == Any {
    func str(_ k: String, _ def: String = "") -> String { self[k] as? String ?? def }
    func num(_ k: String, _ def: Double = 0) -> Double { (self[k] as? NSNumber)?.doubleValue ?? def }
    func int(_ k: String, _ def: Int = 0) -> Int { (self[k] as? NSNumber)?.intValue ?? def }
    func bool(_ k: String, _ def: Bool = false) -> Bool { (self[k] as? NSNumber)?.boolValue ?? def }
    func strs(_ k: String) -> [String] { self[k] as? [String] ?? [] }
}
