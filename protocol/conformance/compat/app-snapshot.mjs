// Compile the released Codable declarations with a headless driver. No application,
// Keychain, defaults, Xcode project or login session is used.
import { readFileSync } from "node:fs";
import path from "node:path";
import { isolatedEnv, write } from "../adapter.mjs";
import { runSync, tags } from "./tagged.mjs";

function declaration(source, name) {
  const at = source.indexOf(`struct ${name}:`);
  const end = source.indexOf("\n    }", at);
  if (at < 0 || end < 0) throw new Error(`The release has no ${name} declaration`);
  return source.slice(at, end + "\n    }".length);
}

export function saveAppSnapshot({ root, tree }, tag, machine, input, output, vectors) {
  const source = readFileSync(path.join(tree, "KuvoCode/Stores/PhoneLink.swift"), "utf8");
  const beta = tag === tags[1];
  const model = beta
    ? readFileSync(path.join(tree, "Packages/KuvoLink/Sources/KuvoLink/PairedPhones.swift"), "utf8").split("    public var phones:")[0] +
      "}\ntypealias Phone = PairedPhones.Phone"
    : declaration(source, "Phone");
  const letters = beta ? declaration(readFileSync(path.join(tree, "KuvoCode/Stores/AgentMail.swift"), "utf8"), "Mailing") : "";
  const linkSources = path.join(tree, "Packages/KuvoLink/Sources/KuvoLink");
  const wire = ["Keys", "Frames", "Noise", "Notice"].map(name => readFileSync(path.join(linkSources, name + ".swift"), "utf8")).join("\n");
  const link = readFileSync(path.join(linkSources, "Link.swift"), "utf8");
  const prologue = link.split("\n").find(line => line.includes("static let mailPrologue"));
  const mail = link.slice(link.indexOf("public enum Mail {")).split("\n}")[0] + "\n}";
  const program = path.join(root, "snapshot.swift");
  write(program, `
import Foundation
${wire}
enum Link {
${prologue}
}
${mail}
${model}
${declaration(source, "MachineHost")}
${declaration(source, "Saved")}
${letters}
func hex(_ text: String) -> Data {
    Data(stride(from: 0, to: text.count, by: 2).map { offset in
        let start = text.index(text.startIndex, offsetBy: offset)
        return UInt8(text[start..<text.index(start, offsetBy: 2)], radix: 16)!
    })
}
let vectors = try JSONSerialization.jsonObject(with: Data(contentsOf: URL(fileURLWithPath: CommandLine.arguments[5]))) as! [String: Any]
let expectedMail = vectors["mail"] as! [String: String]
let from = try LinkKey(rawRepresentation: hex(expectedMail["sender"]!))
let to = try LinkKey(rawRepresentation: hex(expectedMail["recipient"]!))
let ephemeral = try LinkKey(rawRepresentation: hex(expectedMail["ephemeral"]!))
let packet = hex(expectedMail["frame"]!)
let plaintext = try Mail.open(packet, to: to, from: from.publicBytes)
precondition(String(decoding: plaintext, as: UTF8.self) == expectedMail["plaintext"])
let sealed = try OneWay.seal(plaintext, from: from, to: to.publicBytes, prologue: Link.mailPrologue, ephemeral: ephemeral)
precondition(LinkFrame(.mail, session: 0, body: sealed.message).data == packet)
${beta ? "" : `let notice = try JSONDecoder().decode(Notice.self, from: plaintext)
precondition(notice.title == "Fixture Agent" && notice.body == "Ready.")`}
let phone = Phone(key: CommandLine.arguments[1], name: "Fixture Phone",
                  added: Date(timeIntervalSince1970: 1760000000), address: nil${beta ? ', pairing: "fixture"' : ""})
let host = MachineHost(name: "Fixture Machine", address: CommandLine.arguments[2],
                       key: CommandLine.arguments[3], anotherMac: false${beta ? ", peers: true" : ""})
let encoder = JSONEncoder()
encoder.outputFormatting = [.sortedKeys]
try encoder.encode(Saved(phones: [phone], machines: ["fixture-machine": host]))
    .write(to: URL(fileURLWithPath: CommandLine.arguments[4]), options: .atomic)
${beta ? `
let letter = Mailing(id: "ltr_fixture", from: "ses_source", to: "ses_target", toName: "Fixture Agent",
                     machine: "fixture-machine", text: "Keep this letter.", wait: false, at: 1760000000000, answers: nil)
try encoder.encode([letter])
    .write(to: URL(fileURLWithPath: CommandLine.arguments[4]).deletingLastPathComponent().appendingPathComponent("agent-letters.json"))
` : ""}
`);
  const env = isolatedEnv(root);
  runSync("/usr/bin/swift", ["-module-cache-path", path.join(root, "swift-cache"), program,
    input.phones[0].key, machine.address, machine.key, output, vectors], env, root, "", 90_000);
}
