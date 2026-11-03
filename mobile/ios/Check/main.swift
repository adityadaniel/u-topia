// Runs the generated Swift bindings against the real Rust core (no UI): the same calls
// the app makes. Built and run by `cargo run -p xtask -- mobile`.

import Foundation

func check(_ cond: Bool, _ what: String) {
    if !cond {
        FileHandle.standardError.write("FAIL: \(what)\n".data(using: .utf8)!)
        exit(1)
    }
    print("ok: \(what)")
}

let dir = FileManager.default.temporaryDirectory.appendingPathComponent("trellis-check-\(getpid())")
try FileManager.default.createDirectory(at: dir, withIntermediateDirectories: true)
let path = dir.appendingPathComponent("m.trellis").path
let g = try Graph.open(path: path, passphrase: nil)
try g.setToday(date: "2026-11-02")
let day = try g.today()
check(day.title == "November 2nd, 2026", "today's page from Swift")
_ = try g.appendBlock(pageId: day.id, text: "Call [[Dana]] from the phone")
_ = try g.appendBlock(pageId: day.id, text: "Dinner with [[Dana]]")
let dana = try g.findPage(title: "dana")
check(dana != nil, "a link created its page")
check(try g.backlinks(pageId: dana!.id).count == 2, "backlinks over FFI")
check(try g.searchTitles(query: "Da", limit: 5) == ["Dana"], "title search for [[")
do {
    _ = try Graph.open(path: dir.appendingPathComponent("x").path, passphrase: "secret")
    // A new file with a key is created encrypted; reopening without one must fail.
    _ = try Graph.open(path: dir.appendingPathComponent("x").path, passphrase: nil)
    check(false, "an encrypted graph refuses to open without its passphrase")
} catch FfiError.Locked {
    check(true, "an encrypted graph refuses to open without its passphrase")
}
do {
    try g.setBlockText(blockId: "missing", text: "x")
    check(false, "errors cross the boundary")
} catch FfiError.NotFound {
    check(true, "errors cross the boundary as Swift errors")
}
// Reopen: everything was persisted.
let again = try Graph.open(path: path, passphrase: nil)
try again.setToday(date: "2026-11-02")
check(try again.today().blocks.count == 2, "durable across reopen")
print("all checks passed")
